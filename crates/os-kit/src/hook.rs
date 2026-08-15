//! The low-level keyboard hook.
//!
//! # The one rule
//!
//! `keyboard_proc` runs on the OS input path, ahead of the foreground
//! application. If it takes longer than `LowLevelHooksTimeout` (300 ms by
//! default) Windows silently discards the hook and the tool goes dead with no
//! error. So the callback only reads a few atomics, decides swallow-or-pass,
//! and `try_send`s a small `Copy` struct. No allocation, no locks, no blocking,
//! no logging. All real work happens on the worker thread that owns the
//! receiver.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::OnceLock;

use crossbeam_channel::{bounded, Receiver, Sender};
use mouseless_core::{KeyPress, Mods};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_MOUSEMOVE, WM_QUIT, WM_SYSKEYDOWN,
};

use crate::keycode;

/// What the hook observed, already translated off the raw platform types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// The activation hotkey fired.
    Hotkey,
    /// A key pressed while capturing (and swallowed).
    Key(KeyPress),
}

/// How grid mode is triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    /// A modifier chord plus an ordinary key, e.g. Ctrl+Alt+Space. The final
    /// key is swallowed so the foreground application never sees it.
    Chord { vk: u32, mods: Mods },
    /// Tapping a lone modifier: pressed and released inside `tap_ms` with no
    /// other key or mouse button in between.
    ///
    /// The modifier itself is never swallowed — doing so would break every
    /// shortcut on the machine and risk a stuck modifier — so the foreground
    /// application still sees a harmless bare Ctrl press.
    Tap {
        /// Virtual-key code, generic (`VK_CONTROL`) or side-specific
        /// (`VK_LCONTROL`).
        vk: u32,
        /// 1 for a single tap, 2 for a double tap.
        count: u8,
        /// Longest press that still counts as a tap rather than a hold.
        tap_ms: u32,
        /// Longest gap between taps of a double tap.
        gap_ms: u32,
    },
}

impl Default for Hotkey {
    /// Ctrl+Alt+Space: unclaimed by Windows itself and rare in applications.
    fn default() -> Self {
        Hotkey::Chord {
            vk: 0x20, // VK_SPACE
            mods: Mods(Mods::CTRL.0 | Mods::ALT.0),
        }
    }
}

static EVENT_TX: OnceLock<Sender<HookEvent>> = OnceLock::new();
static CAPTURING: AtomicBool = AtomicBool::new(false);

// Chord configuration.
static HOTKEY_VK: AtomicU32 = AtomicU32::new(0);
static HOTKEY_MODS: AtomicU32 = AtomicU32::new(0);

// Tap configuration. `TAP_COUNT == 0` means chord mode.
static TAP_COUNT: AtomicU8 = AtomicU8::new(0);
static TAP_VK: AtomicU32 = AtomicU32::new(0);
static TAP_MS: AtomicU32 = AtomicU32::new(0);
static TAP_GAP_MS: AtomicU32 = AtomicU32::new(0);

// Tap tracking. A "candidate" is a target modifier currently held with nothing
// else pressed since; anything else happening clears it.
static TAP_CANDIDATE: AtomicBool = AtomicBool::new(false);
static TAP_DOWN_MS: AtomicU32 = AtomicU32::new(0);
static PENDING_TAP: AtomicBool = AtomicBool::new(false);
static PENDING_TAP_MS: AtomicU32 = AtomicU32::new(0);

/// Switch key swallowing on and off.
///
/// Called from the worker thread; the hook callback observes it on the next
/// keystroke. `Relaxed` is sufficient — this is a single flag with no other
/// state ordered against it, and being one keystroke late is harmless.
pub fn set_capturing(on: bool) {
    CAPTURING.store(on, Ordering::Relaxed);
}

pub fn is_capturing() -> bool {
    CAPTURING.load(Ordering::Relaxed)
}

/// Collapse side-specific modifier codes onto their generic form, so a config
/// of "ctrl" matches both the left and right keys.
fn generic_modifier(vk: u32) -> u32 {
    match vk {
        0xA0 | 0xA1 => 0x10, // L/R shift   -> VK_SHIFT
        0xA2 | 0xA3 => 0x11, // L/R control -> VK_CONTROL
        0xA4 | 0xA5 => 0x12, // L/R menu    -> VK_MENU
        0x5C => 0x5B,        // right win   -> left win, used as "either win"
        other => other,
    }
}

/// Does this key event concern the configured tap modifier?
fn matches_tap_target(vk: u32, target: u32) -> bool {
    vk == target || generic_modifier(vk) == target
}

/// Was a press this long a tap rather than a hold?
///
/// `wrapping_sub` is deliberate: the timestamps come from the tick counter in
/// `KBDLLHOOKSTRUCT`, which wraps roughly every 49 days.
fn is_tap(down_ms: u32, up_ms: u32, tap_ms: u32) -> bool {
    up_ms.wrapping_sub(down_ms) <= tap_ms
}

/// Decide whether a completed tap fires the hotkey.
///
/// Returns `(fire, pending)` — `pending` is the timestamp to remember for the
/// next tap, or `None` to forget.
fn tap_completes(
    count_needed: u8,
    now_ms: u32,
    pending: Option<u32>,
    gap_ms: u32,
) -> (bool, Option<u32>) {
    if count_needed <= 1 {
        return (true, None);
    }
    match pending {
        Some(prev) if now_ms.wrapping_sub(prev) <= gap_ms => (true, None),
        // Too slow, or the first of a pair: this tap becomes the new anchor.
        _ => (false, Some(now_ms)),
    }
}

/// A running hook. Dropping this does *not* stop it; call [`Hook::stop`].
pub struct Hook {
    thread_id: u32,
}

impl Hook {
    /// Ask the hook thread to unhook and exit.
    pub fn stop(&self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

#[derive(Debug)]
pub enum HookError {
    AlreadyStarted,
    SetHookFailed(windows::core::Error),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::AlreadyStarted => write!(f, "the keyboard hook is already running"),
            HookError::SetHookFailed(e) => write!(f, "SetWindowsHookExW failed: {e}"),
        }
    }
}

impl std::error::Error for HookError {}

/// Install the input hooks on a dedicated thread and return their event stream.
///
/// The thread runs its own message pump because a low-level hook is dispatched
/// to the installing thread's queue: without a pump the callback never runs.
///
/// In tap mode a second, passive mouse hook is installed. It swallows nothing;
/// it exists only so that Ctrl+click cannot be mistaken for a Ctrl tap.
pub fn start(hotkey: Hotkey) -> Result<(Hook, Receiver<HookEvent>), HookError> {
    // Bounded so a stalled worker can never grow this without limit. Overflow
    // drops keystrokes, which is strictly better than unbounded memory growth
    // in a process wired into the input path.
    let (tx, rx) = bounded(256);
    EVENT_TX.set(tx).map_err(|_| HookError::AlreadyStarted)?;

    let tap_mode = match hotkey {
        Hotkey::Chord { vk, mods } => {
            HOTKEY_VK.store(vk, Ordering::Relaxed);
            HOTKEY_MODS.store(mods.0 as u32, Ordering::Relaxed);
            TAP_COUNT.store(0, Ordering::Relaxed);
            false
        }
        Hotkey::Tap {
            vk,
            count,
            tap_ms,
            gap_ms,
        } => {
            TAP_VK.store(vk, Ordering::Relaxed);
            TAP_COUNT.store(count.max(1), Ordering::Relaxed);
            TAP_MS.store(tap_ms, Ordering::Relaxed);
            TAP_GAP_MS.store(gap_ms, Ordering::Relaxed);
            true
        }
    };

    let (ready_tx, ready_rx) = bounded::<Result<u32, windows::core::Error>>(1);

    std::thread::Builder::new()
        .name("mouseless-hook".into())
        .spawn(move || unsafe {
            let kb = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) {
                Ok(h) => h,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let mouse = if tap_mode {
                SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0).ok()
            } else {
                None
            };

            let _ = ready_tx.send(Ok(windows::Win32::System::Threading::GetCurrentThreadId()));

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            if let Some(m) = mouse {
                let _ = UnhookWindowsHookEx(m);
            }
            let _ = UnhookWindowsHookEx(kb);
        })
        .expect("failed to spawn hook thread");

    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok((Hook { thread_id }, rx)),
        Ok(Err(e)) => Err(HookError::SetHookFailed(e)),
        Err(_) => Err(HookError::AlreadyStarted),
    }
}

/// Non-blocking send. A full channel means the worker is wedged; dropping the
/// event keeps the input path moving rather than stalling every keystroke on
/// the machine.
#[inline]
fn emit(event: HookEvent) {
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.try_send(event);
    }
}

/// Advance the tap state machine. Returns true if the hotkey fired.
#[inline]
fn track_tap(vk: u32, is_down: bool, is_mod: bool, time_ms: u32) -> bool {
    let count = TAP_COUNT.load(Ordering::Relaxed);
    if count == 0 {
        return false;
    }
    let target = TAP_VK.load(Ordering::Relaxed);

    if is_mod && matches_tap_target(vk, target) {
        if is_down {
            // Ignore auto-repeat: only the first press starts the clock, so
            // holding the key never counts as a tap.
            if !TAP_CANDIDATE.load(Ordering::Relaxed) {
                TAP_DOWN_MS.store(time_ms, Ordering::Relaxed);
                TAP_CANDIDATE.store(true, Ordering::Relaxed);
            }
            return false;
        }

        let was_candidate = TAP_CANDIDATE.swap(false, Ordering::Relaxed);
        if !was_candidate {
            return false;
        }
        if !is_tap(
            TAP_DOWN_MS.load(Ordering::Relaxed),
            time_ms,
            TAP_MS.load(Ordering::Relaxed),
        ) {
            PENDING_TAP.store(false, Ordering::Relaxed);
            return false;
        }

        let pending = PENDING_TAP
            .load(Ordering::Relaxed)
            .then(|| PENDING_TAP_MS.load(Ordering::Relaxed));
        let (fire, next) = tap_completes(
            count,
            time_ms,
            pending,
            TAP_GAP_MS.load(Ordering::Relaxed),
        );
        match next {
            Some(ms) => {
                PENDING_TAP_MS.store(ms, Ordering::Relaxed);
                PENDING_TAP.store(true, Ordering::Relaxed);
            }
            None => PENDING_TAP.store(false, Ordering::Relaxed),
        }
        return fire;
    }

    // Any other key press means the modifier is being used as a modifier.
    if is_down {
        TAP_CANDIDATE.store(false, Ordering::Relaxed);
    }
    false
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Negative codes must be forwarded untouched, per the hook contract.
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    let vk = info.vkCode;

    // Our own SendInput calls come back through here. Passing them through
    // unconditionally avoids a feedback loop.
    if info.flags.0 & LLKHF_INJECTED.0 != 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let msg = wparam.0 as u32;
    let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let is_mod = keycode::is_modifier(vk);

    if track_tap(vk, is_down, is_mod, info.time) {
        emit(HookEvent::Hotkey);
        // Deliberately fall through rather than swallowing: the modifier must
        // still reach the foreground application.
    }

    // Modifiers always pass through so the foreground app's view of Ctrl/Alt/
    // Shift/Win never diverges from the physical keyboard.
    if is_mod {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // The chord is tested before the capturing branch so it toggles from any
    // mode. Checking capture first would make the hotkey un-pressable once the
    // grid was up, leaving Esc as the only way out.
    if TAP_COUNT.load(Ordering::Relaxed) == 0
        && vk == HOTKEY_VK.load(Ordering::Relaxed)
        && keycode::current_mods().0 as u32 == HOTKEY_MODS.load(Ordering::Relaxed)
    {
        if is_down {
            emit(HookEvent::Hotkey);
        }
        return LRESULT(1);
    }

    if is_capturing() {
        if is_down {
            emit(HookEvent::Key(KeyPress::new(
                keycode::translate(vk),
                keycode::current_mods(),
            )));
        }
        // Swallow the key-up too, so the application never sees half a press.
        return LRESULT(1);
    }

    CallNextHookEx(None, code, wparam, lparam)
}

/// Passive mouse hook: cancels a tap candidate, swallows nothing.
///
/// Without this, Ctrl+click would register as a Ctrl tap on release and pop the
/// grid open every time the user ctrl-clicked a link.
unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 != WM_MOUSEMOVE {
        // Buttons and the wheel count as "the modifier is in use". Plain moves
        // do not, so the cursor drifting never cancels a tap.
        TAP_CANDIDATE.store(false, Ordering::Relaxed);
    }
    CallNextHookEx(None, code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_core::Key;

    #[test]
    fn capturing_flag_round_trips() {
        set_capturing(true);
        assert!(is_capturing());
        set_capturing(false);
        assert!(!is_capturing());
    }

    #[test]
    fn default_hotkey_is_ctrl_alt_space() {
        let Hotkey::Chord { vk, mods } = Hotkey::default() else {
            panic!("default should be a chord");
        };
        assert_eq!(vk, 0x20);
        assert!(mods.contains(Mods::CTRL));
        assert!(mods.contains(Mods::ALT));
        assert!(!mods.contains(Mods::SHIFT));
    }

    #[test]
    fn hook_event_is_copy_and_small() {
        // The callback must never allocate; keeping the event Copy and tiny is
        // what makes `try_send` from the input path safe.
        fn assert_copy<T: Copy>() {}
        assert_copy::<HookEvent>();
        assert!(std::mem::size_of::<HookEvent>() <= 16);
    }

    #[test]
    fn key_events_carry_translated_keys() {
        let e = HookEvent::Key(KeyPress::plain(Key::Escape));
        assert_eq!(e, HookEvent::Key(KeyPress::plain(Key::Escape)));
    }

    #[test]
    fn generic_ctrl_matches_either_side() {
        const VK_CONTROL: u32 = 0x11;
        const VK_LCONTROL: u32 = 0xA2;
        const VK_RCONTROL: u32 = 0xA3;

        assert!(matches_tap_target(VK_LCONTROL, VK_CONTROL));
        assert!(matches_tap_target(VK_RCONTROL, VK_CONTROL));
        // A side-specific config must not accept the other side.
        assert!(matches_tap_target(VK_RCONTROL, VK_RCONTROL));
        assert!(!matches_tap_target(VK_LCONTROL, VK_RCONTROL));
        // Shift is not Ctrl.
        assert!(!matches_tap_target(0xA0, VK_CONTROL));
    }

    #[test]
    fn holding_the_key_is_not_a_tap() {
        assert!(is_tap(1000, 1200, 250));
        assert!(is_tap(1000, 1250, 250), "boundary counts as a tap");
        assert!(!is_tap(1000, 1251, 250));
        assert!(!is_tap(1000, 5000, 250), "a long hold is never a tap");
    }

    #[test]
    fn tap_timing_survives_tick_counter_wraparound() {
        // The tick counter wraps every ~49 days; a press across the wrap must
        // not read as a 49-day hold.
        let down = u32::MAX - 100;
        let up = 50u32; // 151 ms later in wrapped arithmetic
        assert!(is_tap(down, up, 250));
    }

    #[test]
    fn single_tap_fires_immediately() {
        let (fire, pending) = tap_completes(1, 5_000, None, 350);
        assert!(fire);
        assert_eq!(pending, None);
    }

    #[test]
    fn double_tap_needs_two_taps_inside_the_gap() {
        // First tap: arms, does not fire.
        let (fire, pending) = tap_completes(2, 1_000, None, 350);
        assert!(!fire);
        assert_eq!(pending, Some(1_000));

        // Second tap inside the gap: fires and disarms.
        let (fire, pending) = tap_completes(2, 1_300, Some(1_000), 350);
        assert!(fire);
        assert_eq!(pending, None, "must not fire again on a third tap");
    }

    #[test]
    fn a_slow_second_tap_re_arms_instead_of_firing() {
        let (fire, pending) = tap_completes(2, 2_000, Some(1_000), 350);
        assert!(!fire, "1 s apart is two separate taps, not a double tap");
        assert_eq!(pending, Some(2_000), "it becomes the new first tap");
    }

    /// Serialises tests that drive the shared tap atomics.
    static TAP_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const VK_LCONTROL: u32 = 0xA2;
    const VK_CONTROL: u32 = 0x11;

    fn arm_tap_mode(count: u8) {
        TAP_VK.store(VK_CONTROL, Ordering::Relaxed);
        TAP_COUNT.store(count, Ordering::Relaxed);
        TAP_MS.store(250, Ordering::Relaxed);
        TAP_GAP_MS.store(350, Ordering::Relaxed);
        TAP_CANDIDATE.store(false, Ordering::Relaxed);
        PENDING_TAP.store(false, Ordering::Relaxed);
    }

    fn disarm_tap_mode() {
        TAP_COUNT.store(0, Ordering::Relaxed);
        TAP_CANDIDATE.store(false, Ordering::Relaxed);
        PENDING_TAP.store(false, Ordering::Relaxed);
    }

    /// End-to-end through the real atomic state machine, not the pure helpers.
    #[test]
    fn tapping_ctrl_fires_the_hotkey() {
        let _guard = TAP_TEST_LOCK.lock().unwrap();
        arm_tap_mode(1);

        assert!(!track_tap(VK_LCONTROL, true, true, 1_000), "press alone");
        assert!(
            track_tap(VK_LCONTROL, false, true, 1_100),
            "release within the timeout should fire"
        );
        disarm_tap_mode();
    }

    #[test]
    fn using_ctrl_as_a_modifier_does_not_fire() {
        let _guard = TAP_TEST_LOCK.lock().unwrap();
        arm_tap_mode(1);

        track_tap(VK_LCONTROL, true, true, 1_000);
        track_tap(0x43, true, false, 1_050); // 'C' -> Ctrl+C
        assert!(
            !track_tap(VK_LCONTROL, false, true, 1_100),
            "Ctrl+C must not open the grid"
        );
        disarm_tap_mode();
    }

    #[test]
    fn holding_ctrl_does_not_fire() {
        let _guard = TAP_TEST_LOCK.lock().unwrap();
        arm_tap_mode(1);

        track_tap(VK_LCONTROL, true, true, 1_000);
        track_tap(VK_LCONTROL, true, true, 1_100); // auto-repeat
        track_tap(VK_LCONTROL, true, true, 1_200);
        assert!(
            !track_tap(VK_LCONTROL, false, true, 2_000),
            "a 1 s hold is not a tap"
        );
        disarm_tap_mode();
    }

    #[test]
    fn double_tap_through_the_state_machine() {
        let _guard = TAP_TEST_LOCK.lock().unwrap();
        arm_tap_mode(2);

        track_tap(VK_LCONTROL, true, true, 1_000);
        assert!(!track_tap(VK_LCONTROL, false, true, 1_080), "first tap arms");

        track_tap(VK_LCONTROL, true, true, 1_200);
        assert!(
            track_tap(VK_LCONTROL, false, true, 1_280),
            "second tap inside the gap fires"
        );
        disarm_tap_mode();
    }

    #[test]
    fn chord_mode_ignores_taps_entirely() {
        let _guard = TAP_TEST_LOCK.lock().unwrap();
        disarm_tap_mode();
        track_tap(VK_LCONTROL, true, true, 1_000);
        assert!(!track_tap(VK_LCONTROL, false, true, 1_050));
    }

    #[test]
    fn double_tap_gap_survives_wraparound() {
        let first = u32::MAX - 50;
        let second = 100u32; // 151 ms later
        let (fire, _) = tap_completes(2, second, Some(first), 350);
        assert!(fire);
    }
}
