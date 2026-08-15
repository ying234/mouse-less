//! Virtual-key code translation.

use mouseless_core::{Key, Mods};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, MapVirtualKeyExW, MAP_VIRTUAL_KEY_TYPE, VK_BACK, VK_DOWN,
    VK_LEFT, VK_RIGHT, VK_UP,
    VK_CONTROL, VK_ESCAPE, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RETURN, VK_RMENU, VK_RSHIFT,
    VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB,
};

const MAPVK_VK_TO_CHAR: MAP_VIRTUAL_KEY_TYPE = MAP_VIRTUAL_KEY_TYPE(2);

/// Modifier keys held right now.
///
/// `GetAsyncKeyState` reads the physical key state, which is what we want:
/// the hook runs before the foreground application's queue and we cannot rely
/// on that queue's synchronous view.
pub fn current_mods() -> Mods {
    let down = |vk: u16| -> bool { (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0 };

    let mut mods = Mods::NONE;
    if down(VK_SHIFT.0) {
        mods |= Mods::SHIFT;
    }
    if down(VK_CONTROL.0) {
        mods |= Mods::CTRL;
    }
    if down(VK_MENU.0) {
        mods |= Mods::ALT;
    }
    if down(VK_LWIN.0) || down(VK_RWIN.0) {
        mods |= Mods::WIN;
    }
    mods
}

/// True for keys that only ever act as modifiers.
///
/// These are passed through even while we are capturing. If we swallowed a
/// Ctrl key-up, the foreground application would be left believing Ctrl is
/// still held — a stuck modifier is far more disruptive than the keystroke we
/// were trying to hide.
pub fn is_modifier(vk: u32) -> bool {
    let vk = vk as u16;
    [
        VK_SHIFT, VK_LSHIFT, VK_RSHIFT, VK_CONTROL, VK_MENU, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN,
    ]
    .iter()
    .any(|k| k.0 == vk)
        // VK_LCONTROL / VK_RCONTROL are not re-exported consistently; match raw.
        || vk == 0xA2
        || vk == 0xA3
}

/// Translate a virtual-key code into a layout-resolved [`Key`].
///
/// Uses `MapVirtualKeyExW` rather than `ToUnicodeEx` on purpose: `ToUnicodeEx`
/// mutates the keyboard's dead-key composition state, and doing that from
/// inside a hook corrupts accented input in the foreground application.
pub fn translate(vk: u32) -> Key {
    let vk16 = vk as u16;
    match vk16 {
        v if v == VK_ESCAPE.0 => return Key::Escape,
        v if v == VK_BACK.0 => return Key::Backspace,
        v if v == VK_RETURN.0 => return Key::Enter,
        v if v == VK_SPACE.0 => return Key::Space,
        v if v == VK_TAB.0 => return Key::Tab,
        v if v == VK_LEFT.0 => return Key::Left,
        v if v == VK_UP.0 => return Key::Up,
        v if v == VK_RIGHT.0 => return Key::Right,
        v if v == VK_DOWN.0 => return Key::Down,
        _ => {}
    }

    // Layout of the calling (hook) thread. On a machine where the foreground
    // application uses a different layout this can disagree; resolving the
    // foreground layout per keystroke is the follow-up fix.
    let layout = unsafe { GetKeyboardLayout(0) };
    let mapped = unsafe { MapVirtualKeyExW(vk, MAPVK_VK_TO_CHAR, Some(layout)) };

    // The high bit flags a dead key; the character itself is in the low bits.
    match char::from_u32(mapped & 0x7FFF) {
        Some(c) if !c.is_control() && c != '\0' => {
            Key::Char(c.to_lowercase().next().unwrap_or(c))
        }
        _ => Key::Other(vk),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_translate() {
        assert_eq!(translate(VK_ESCAPE.0 as u32), Key::Escape);
        assert_eq!(translate(VK_BACK.0 as u32), Key::Backspace);
        assert_eq!(translate(VK_SPACE.0 as u32), Key::Space);
    }

    #[test]
    fn arrow_keys_translate() {
        // Without these the cursor-mode nudge silently does nothing.
        assert_eq!(translate(VK_LEFT.0 as u32), Key::Left);
        assert_eq!(translate(VK_UP.0 as u32), Key::Up);
        assert_eq!(translate(VK_RIGHT.0 as u32), Key::Right);
        assert_eq!(translate(VK_DOWN.0 as u32), Key::Down);
    }

    #[test]
    fn letters_translate_lowercase() {
        // 0x41 is VK_A on every layout that has a Latin 'a'.
        assert_eq!(translate(0x41), Key::Char('a'));
    }

    #[test]
    fn modifiers_are_recognised() {
        assert!(is_modifier(VK_CONTROL.0 as u32));
        assert!(is_modifier(VK_LWIN.0 as u32));
        assert!(is_modifier(0xA2)); // VK_LCONTROL
        assert!(!is_modifier(0x41));
    }
}
