//! Wayland keyboard events to platform-neutral [`Key`] values.
//!
//! The compositor hands us an xkb keysym plus the UTF-8 the keymap resolved it
//! to. Using the UTF-8 rather than the keysym for printable keys is what makes
//! non-US layouts work: on a Dvorak or AZERTY keymap the physical key labelled
//! `a` produces a different symbol, and the label the user reads on screen has
//! to be the one their keyboard actually types.

use mouseless_core::{Key, Mods};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, Keysym, Modifiers};

/// Translate a key event into the engine's key representation.
pub fn translate(event: &KeyEvent) -> Key {
    match event.keysym {
        Keysym::Escape => return Key::Escape,
        Keysym::BackSpace => return Key::Backspace,
        Keysym::Return | Keysym::KP_Enter => return Key::Enter,
        Keysym::space => return Key::Space,
        Keysym::Tab | Keysym::ISO_Left_Tab => return Key::Tab,
        Keysym::Left | Keysym::KP_Left => return Key::Left,
        Keysym::Up | Keysym::KP_Up => return Key::Up,
        Keysym::Right | Keysym::KP_Right => return Key::Right,
        Keysym::Down | Keysym::KP_Down => return Key::Down,
        _ => {}
    }

    // Lowercased, because the engine matches labels case-insensitively and
    // reports Shift separately — `Shift+j` has to stay a fast nudge, not
    // become an unknown key.
    let printable = event
        .utf8
        .as_deref()
        .and_then(|s| s.chars().next())
        .filter(|c| !c.is_control());

    match printable {
        Some(c) => Key::Char(c.to_lowercase().next().unwrap_or(c)),
        None => Key::Other(event.raw_code),
    }
}

/// Translate the seat's modifier state into the engine's bitmask.
pub fn mods(m: Modifiers) -> Mods {
    let mut out = Mods::NONE;
    if m.shift {
        out |= Mods::SHIFT;
    }
    if m.ctrl {
        out |= Mods::CTRL;
    }
    if m.alt {
        out |= Mods::ALT;
    }
    if m.logo {
        out |= Mods::WIN;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(keysym: Keysym, utf8: Option<&str>) -> KeyEvent {
        KeyEvent {
            time: 0,
            raw_code: 0,
            keysym,
            utf8: utf8.map(str::to_string),
        }
    }

    #[test]
    fn named_keys_translate() {
        assert_eq!(translate(&event(Keysym::Escape, None)), Key::Escape);
        assert_eq!(translate(&event(Keysym::BackSpace, None)), Key::Backspace);
        // Space carries a UTF-8 payload too; the named match must win, or the
        // engine sees Char(' ') and the click key stops working.
        assert_eq!(translate(&event(Keysym::space, Some(" "))), Key::Space);
    }

    #[test]
    fn arrow_keys_translate() {
        assert_eq!(translate(&event(Keysym::Left, None)), Key::Left);
        assert_eq!(translate(&event(Keysym::Up, None)), Key::Up);
        assert_eq!(translate(&event(Keysym::Right, None)), Key::Right);
        assert_eq!(translate(&event(Keysym::Down, None)), Key::Down);
    }

    #[test]
    fn letters_come_from_the_layout_not_the_keysym() {
        assert_eq!(translate(&event(Keysym::a, Some("a"))), Key::Char('a'));
        // Shift is reported separately, so the label match must not see 'J'.
        assert_eq!(translate(&event(Keysym::J, Some("J"))), Key::Char('j'));
    }

    #[test]
    fn unprintable_keys_keep_their_raw_code() {
        let mut e = event(Keysym::F1, None);
        e.raw_code = 59;
        assert_eq!(translate(&e), Key::Other(59));
    }

    #[test]
    fn control_characters_are_not_treated_as_printable() {
        // Ctrl+C resolves to U+0003; typing it must not match a cell label.
        let mut e = event(Keysym::c, Some("\u{3}"));
        e.raw_code = 46;
        assert_eq!(translate(&e), Key::Other(46));
    }

    #[test]
    fn modifiers_map_across() {
        let m = Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let out = mods(m);
        assert!(out.contains(Mods::CTRL));
        assert!(out.contains(Mods::SHIFT));
        assert!(!out.contains(Mods::ALT));
    }
}
