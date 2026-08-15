//! User configuration, read from `%APPDATA%\my-mouseless\config.toml`.

use std::path::PathBuf;

use mouseless_core::{GridConfig, Mods};
use mouseless_os_kit::Hotkey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    /// What toggles grid mode: a chord ("ctrl+alt+space"), or a modifier tap
    /// ("tap:rctrl", "doubletap:ctrl").
    pub hotkey: String,
    pub coarse_cols: u32,
    pub coarse_rows: u32,
    pub refine_cols: u32,
    pub refine_rows: u32,
    /// Refinement passes after the coarse selection. 0 commits immediately.
    pub refine_levels: u32,
    /// Symbols used to build cell labels, in order.
    pub alphabet: String,
    /// Left-click as soon as the final cell is chosen, skipping cursor mode.
    pub click_on_select: bool,
    /// Pixels moved by one arrow / hjkl press in cursor mode.
    pub nudge_step: i32,
    /// Pixels moved when the same key is pressed with Shift.
    pub nudge_step_fast: i32,
    /// Longest press still counted as a tap rather than a hold, in ms.
    pub tap_timeout_ms: u32,
    /// Longest gap between the two taps of a double tap, in ms.
    pub double_tap_ms: u32,
    /// Largest label text in pixels. In practice this sets the coarse grid's
    /// size; refined cells compute smaller than it and are unaffected.
    pub label_font_max_px: f32,
}

impl Default for FileConfig {
    fn default() -> Self {
        let g = GridConfig::default();
        Self {
            hotkey: "ctrl+alt+space".into(),
            coarse_cols: g.coarse_cols,
            coarse_rows: g.coarse_rows,
            refine_cols: g.refine_cols,
            refine_rows: g.refine_rows,
            refine_levels: g.refine_levels,
            alphabet: g.alphabet.into_iter().collect(),
            click_on_select: g.click_on_select,
            nudge_step: g.nudge_step,
            nudge_step_fast: g.nudge_step_fast,
            tap_timeout_ms: 250,
            double_tap_ms: 350,
            label_font_max_px: mouseless_overlay::DEFAULT_LABEL_FONT_MAX_PX,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Parse(String),
    Hotkey(String),
    Grid(mouseless_core::ConfigError),
    Render(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Parse(e) => write!(f, "could not parse config: {e}"),
            ConfigError::Hotkey(e) => write!(f, "invalid hotkey: {e}"),
            ConfigError::Grid(e) => write!(f, "invalid grid settings: {e}"),
            ConfigError::Render(e) => write!(f, "invalid display settings: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl FileConfig {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Convert into the validated types the rest of the program uses.
    pub fn resolve(&self) -> Result<(Hotkey, GridConfig), ConfigError> {
        let hotkey = parse_hotkey(&self.hotkey, self.tap_timeout_ms, self.double_tap_ms)
            .map_err(ConfigError::Hotkey)?;
        let grid = GridConfig {
            coarse_cols: self.coarse_cols,
            coarse_rows: self.coarse_rows,
            refine_cols: self.refine_cols,
            refine_rows: self.refine_rows,
            refine_levels: self.refine_levels,
            alphabet: self.alphabet.chars().collect(),
            click_on_select: self.click_on_select,
            nudge_step: self.nudge_step,
            nudge_step_fast: self.nudge_step_fast,
        };
        grid.validate().map_err(ConfigError::Grid)?;

        // The renderer clamps this to a readable floor anyway, so a bad value
        // would silently do nothing. Say so instead.
        let font = self.label_font_max_px;
        if !font.is_finite() || !(9.0..=200.0).contains(&font) {
            return Err(ConfigError::Render(format!(
                "label_font_max_px must be between 9 and 200, got {font}"
            )));
        }

        Ok((hotkey, grid))
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("my-mouseless").join("config.toml")
}

/// Read the config file, creating it with defaults if it does not exist.
///
/// A missing file is normal (first run) and is not an error; a malformed file
/// is an error, because silently ignoring it would leave the user staring at a
/// setting that appears to do nothing.
pub fn load() -> Result<(FileConfig, PathBuf), ConfigError> {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((FileConfig::parse(&text)?, path)),
        Err(_) => {
            let cfg = FileConfig::default();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(text) = toml::to_string_pretty(&cfg) {
                let _ = std::fs::write(&path, text);
            }
            Ok((cfg, path))
        }
    }
}

/// Parse a hotkey spec.
///
/// Three forms:
///   * `ctrl+alt+space`  — a chord
///   * `tap:ctrl`        — tap a lone modifier
///   * `doubletap:ctrl`  — tap it twice
///
/// A bare modifier name (`ctrl`) is accepted as shorthand for `tap:ctrl`,
/// since that is the only thing it could reasonably mean.
fn parse_hotkey(spec: &str, tap_ms: u32, gap_ms: u32) -> Result<Hotkey, String> {
    let trimmed = spec.trim().to_ascii_lowercase();

    if let Some(rest) = trimmed.strip_prefix("doubletap:") {
        return tap_hotkey(rest, 2, tap_ms, gap_ms);
    }
    if let Some(rest) = trimmed.strip_prefix("tap:") {
        return tap_hotkey(rest, 1, tap_ms, gap_ms);
    }
    // A lone modifier can only mean a tap; a chord needs a real key.
    if tap_vk_from_name(&trimmed).is_some() {
        return tap_hotkey(&trimmed, 1, tap_ms, gap_ms);
    }

    let mut mods = Mods::NONE;
    let mut key: Option<u32> = None;

    for part in trimmed.split('+').map(|p| p.trim().to_string()) {
        if part.is_empty() {
            return Err(format!("empty component in {spec:?}"));
        }
        match part.as_str() {
            "ctrl" | "control" => mods |= Mods::CTRL,
            "alt" => mods |= Mods::ALT,
            "shift" => mods |= Mods::SHIFT,
            "win" | "super" | "meta" => mods |= Mods::WIN,
            other => {
                if key.is_some() {
                    return Err(format!("{spec:?} names more than one non-modifier key"));
                }
                key = Some(vk_from_name(other).ok_or_else(|| format!("unknown key {other:?}"))?);
            }
        }
    }

    match key {
        Some(vk) => Ok(Hotkey::Chord { vk, mods }),
        None => Err(format!(
            "{spec:?} has no non-modifier key; for a lone modifier use \
             \"tap:{trimmed}\" or \"doubletap:{trimmed}\""
        )),
    }
}

fn tap_hotkey(name: &str, count: u8, tap_ms: u32, gap_ms: u32) -> Result<Hotkey, String> {
    let vk = tap_vk_from_name(name).ok_or_else(|| {
        format!("{name:?} is not a modifier; tap triggers only work on ctrl, alt, shift or win")
    })?;
    Ok(Hotkey::Tap {
        vk,
        count,
        tap_ms,
        gap_ms,
    })
}

/// Modifier names usable as tap triggers.
///
/// The generic forms match either side; the `l`/`r` forms are exact. Right Ctrl
/// is the standout choice — almost nothing binds it, so a single tap is safe.
fn tap_vk_from_name(name: &str) -> Option<u32> {
    Some(match name {
        "ctrl" | "control" => 0x11,
        "lctrl" => 0xA2,
        "rctrl" => 0xA3,
        "shift" => 0x10,
        "lshift" => 0xA0,
        "rshift" => 0xA1,
        "alt" => 0x12,
        "lalt" => 0xA4,
        "ralt" => 0xA5,
        "win" | "super" | "meta" | "lwin" => 0x5B,
        "rwin" => 0x5C,
        _ => return None,
    })
}

fn vk_from_name(name: &str) -> Option<u32> {
    // Single characters map directly onto the ASCII-aligned VK range.
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphabetic() {
            return Some(c.to_ascii_uppercase() as u32);
        }
        if c.is_ascii_digit() {
            return Some(c as u32);
        }
    }

    if let Some(n) = name.strip_prefix('f') {
        if let Ok(n) = n.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0x70 + n - 1); // VK_F1
            }
        }
    }

    Some(match name {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "insert" => 0x2D,
        "delete" | "del" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "semicolon" => 0xBA,
        "equals" => 0xBB,
        "comma" => 0xBC,
        "minus" => 0xBD,
        "period" => 0xBE,
        "slash" => 0xBF,
        "backtick" | "grave" => 0xC0,
        "lbracket" => 0xDB,
        "backslash" => 0xDC,
        "rbracket" => 0xDD,
        "quote" => 0xDE,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_resolves() {
        let (hotkey, grid) = FileConfig::default().resolve().expect("defaults valid");
        let Hotkey::Chord { vk, mods } = hotkey else {
            panic!("default should be a chord");
        };
        assert_eq!(vk, 0x20);
        assert!(mods.contains(Mods::CTRL));
        assert!(mods.contains(Mods::ALT));
        assert_eq!(grid.alphabet.len(), 26);
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let text = toml::to_string_pretty(&FileConfig::default()).unwrap();
        let parsed = FileConfig::parse(&text).expect("written config must re-parse");
        assert_eq!(parsed.hotkey, FileConfig::default().hotkey);
    }

    #[test]
    fn partial_config_fills_in_defaults() {
        let cfg = FileConfig::parse("refine_levels = 0").unwrap();
        assert_eq!(cfg.refine_levels, 0);
        assert_eq!(cfg.coarse_cols, FileConfig::default().coarse_cols);
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_ignored() {
        // A typo'd setting that silently does nothing is a bad afternoon.
        let err = FileConfig::parse("refine_lvls = 2").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn hotkey_parses_modifiers_and_key() {
        let Hotkey::Chord { vk, mods } = parse_hotkey("ctrl+shift+k", 250, 350).unwrap() else {
            panic!("expected a chord");
        };
        assert_eq!(vk, 'K' as u32);
        assert!(mods.contains(Mods::CTRL));
        assert!(mods.contains(Mods::SHIFT));
        assert!(!mods.contains(Mods::ALT));
    }

    #[test]
    fn hotkey_is_case_and_space_insensitive() {
        assert_eq!(
            parse_hotkey("Ctrl + Alt + Space", 250, 350).unwrap(),
            parse_hotkey("ctrl+alt+space", 250, 350).unwrap()
        );
    }

    #[test]
    fn hotkey_supports_function_and_named_keys() {
        assert!(matches!(
            parse_hotkey("f9", 250, 350).unwrap(),
            Hotkey::Chord { vk: 0x78, .. }
        ));
        assert!(matches!(
            parse_hotkey("win+semicolon", 250, 350).unwrap(),
            Hotkey::Chord { vk: 0xBA, .. }
        ));
    }

    #[test]
    fn bare_modifier_means_a_single_tap() {
        // The obvious reading of hotkey = "ctrl"; anything else would be a
        // surprise.
        assert_eq!(
            parse_hotkey("ctrl", 250, 350).unwrap(),
            Hotkey::Tap { vk: 0x11, count: 1, tap_ms: 250, gap_ms: 350 }
        );
    }

    #[test]
    fn tap_prefixes_select_the_count() {
        assert_eq!(
            parse_hotkey("tap:rctrl", 250, 350).unwrap(),
            Hotkey::Tap { vk: 0xA3, count: 1, tap_ms: 250, gap_ms: 350 }
        );
        assert_eq!(
            parse_hotkey("doubletap:ctrl", 200, 400).unwrap(),
            Hotkey::Tap { vk: 0x11, count: 2, tap_ms: 200, gap_ms: 400 }
        );
    }

    #[test]
    fn side_specific_modifiers_are_distinct() {
        let left = parse_hotkey("tap:lctrl", 250, 350).unwrap();
        let right = parse_hotkey("tap:rctrl", 250, 350).unwrap();
        assert_ne!(left, right, "lctrl and rctrl must not collapse together");
    }

    #[test]
    fn tap_rejects_non_modifiers() {
        // "tap:a" cannot work: we would have to swallow the key to tell a tap
        // from typing, and then the user could never type an 'a'.
        let err = parse_hotkey("tap:a", 250, 350).unwrap_err();
        assert!(err.contains("not a modifier"), "unhelpful error: {err}");
        assert!(parse_hotkey("doubletap:f1", 250, 350).is_err());
    }

    #[test]
    fn chord_without_a_key_suggests_the_tap_syntax() {
        let err = parse_hotkey("ctrl+alt", 250, 350).unwrap_err();
        assert!(
            err.contains("tap:"),
            "error should point at the tap syntax: {err}"
        );
    }

    #[test]
    fn hotkey_rejects_nonsense() {
        assert!(parse_hotkey("ctrl+alt", 250, 350).is_err(), "no real key");
        assert!(parse_hotkey("ctrl+a+b", 250, 350).is_err(), "two real keys");
        assert!(parse_hotkey("ctrl+nope", 250, 350).is_err(), "unknown key name");
        assert!(parse_hotkey("ctrl++a", 250, 350).is_err(), "empty component");
    }

    #[test]
    fn an_unreadable_font_size_is_rejected_not_silently_clamped() {
        for bad in [0.0, 2.0, -5.0, 1e6, f32::NAN] {
            let cfg = FileConfig {
                label_font_max_px: bad,
                ..Default::default()
            };
            assert!(
                matches!(cfg.resolve(), Err(ConfigError::Render(_))),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn a_reasonable_font_size_is_accepted() {
        for good in [9.0, 22.0, 48.0] {
            let cfg = FileConfig {
                label_font_max_px: good,
                ..Default::default()
            };
            assert!(cfg.resolve().is_ok(), "{good} should be accepted");
        }
    }

    #[test]
    fn invalid_grid_settings_are_reported() {
        let cfg = FileConfig {
            alphabet: "a".into(),
            ..Default::default()
        };
        assert!(matches!(cfg.resolve(), Err(ConfigError::Grid(_))));
    }
}
