//! Synthetic cursor movement, clicks, and press-and-hold dragging.

use std::sync::atomic::{AtomicU8, Ordering};

use mouseless_core::{Button, Point};
use windows::core::BOOL;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

/// Warp the cursor to a virtual-screen coordinate.
///
/// `SetCursorPos` takes virtual-screen pixels directly, which avoids the
/// normalise-to-65535 rounding that `SendInput`'s absolute move requires — and
/// that rounding is visible as an off-by-a-pixel landing on wide desktops.
pub fn move_cursor(p: Point) -> bool {
    unsafe { SetCursorPos(p.x, p.y).is_ok() }
}

/// Buttons currently held down by us, as a bitmask.
///
/// A synthetic button-down that never gets its matching up leaves the button
/// stuck for the whole session — the user cannot release it, because they never
/// physically pressed it. Tracking what we hold is what makes
/// [`release_all_held`] possible.
static HELD: AtomicU8 = AtomicU8::new(0);

const fn bit(button: Button) -> u8 {
    match button {
        Button::Left => 1,
        Button::Right => 2,
        Button::Middle => 4,
    }
}

const fn flags(button: Button) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
    match button {
        Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    }
}

fn send(events: &[INPUT]) -> bool {
    let sent = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) };
    sent == events.len() as u32
}

/// Press and release a mouse button at the cursor's current position.
pub fn click(button: Button) -> bool {
    let (down, up) = flags(button);
    send(&[mouse_event(down), mouse_event(up)])
}

/// Press and hold a button. Pair with [`mouse_up`].
pub fn mouse_down(button: Button) -> bool {
    let (down, _) = flags(button);
    // Record before sending: if the send succeeds but we are killed an instant
    // later, the exit guard still knows to release it.
    HELD.fetch_or(bit(button), Ordering::SeqCst);
    send(&[mouse_event(down)])
}

pub fn mouse_up(button: Button) -> bool {
    let (_, up) = flags(button);
    let ok = send(&[mouse_event(up)]);
    HELD.fetch_and(!bit(button), Ordering::SeqCst);
    ok
}

/// Release every button we are holding. Safe to call when holding none.
pub fn release_all_held() {
    let held = HELD.swap(0, Ordering::SeqCst);
    for button in [Button::Left, Button::Right, Button::Middle] {
        if held & bit(button) != 0 {
            let (_, up) = flags(button);
            send(&[mouse_event(up)]);
        }
    }
}

/// Release held buttons if the process is killed from the console.
///
/// Ctrl+C terminates without unwinding, so `Drop` cannot be relied on. Without
/// this, quitting mid-drag would leave the left button down system-wide.
pub fn install_exit_guard() -> bool {
    unsafe { SetConsoleCtrlHandler(Some(on_console_ctrl), true).is_ok() }
}

unsafe extern "system" fn on_console_ctrl(_ctrl_type: u32) -> BOOL {
    release_all_held();
    // FALSE: we only clean up, then let the default handler terminate us.
    BOOL(0)
}

fn mouse_event(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
