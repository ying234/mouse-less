//! Platform-neutral key representation.
//!
//! `os-kit` is responsible for turning a Windows virtual-key code plus the
//! active keyboard layout into one of these. Keeping the engine free of VK
//! codes is what lets the whole state machine be tested without Windows.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A layout-resolved printable character, already lowercased.
    Char(char),
    Escape,
    Backspace,
    Enter,
    Space,
    Tab,
    Left,
    Up,
    Right,
    Down,
    /// Anything we do not model yet, carrying the raw platform code.
    Other(u32),
}

impl Key {
    pub fn as_char(self) -> Option<char> {
        match self {
            Key::Char(c) => Some(c),
            _ => None,
        }
    }
}

/// Modifier keys held at the time of a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods(pub u8);

impl Mods {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1 << 0);
    pub const CTRL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const WIN: Self = Self(1 << 3);

    /// True when every bit in `other` is set in `self`.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOr for Mods {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for Mods {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// A key press as delivered to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub key: Key,
    pub mods: Mods,
}

impl KeyPress {
    pub const fn new(key: Key, mods: Mods) -> Self {
        Self { key, mods }
    }

    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            mods: Mods::NONE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mods_combine_and_test() {
        let m = Mods::CTRL | Mods::SHIFT;
        assert!(m.contains(Mods::CTRL));
        assert!(m.contains(Mods::SHIFT));
        assert!(!m.contains(Mods::ALT));
        assert!(Mods::NONE.is_empty());
    }

    #[test]
    fn contains_requires_all_bits() {
        assert!(!Mods::CTRL.contains(Mods::CTRL | Mods::ALT));
    }
}
