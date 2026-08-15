//! Engine configuration.
//!
//! Kept plain so the `app` crate can deserialize it from YAML without `core`
//! taking on a serde dependency.

/// The default coarse grid. 24x14 = 336 cells, which fits in two characters of
/// a 26-symbol alphabet and gives roughly 80x77 px cells on a 1920x1080 screen.
pub const DEFAULT_COARSE: (u32, u32) = (24, 14);

/// One refinement step of 5x5, a single keystroke because 25 cells fit in one
/// symbol. Three keys total.
///
/// Deeper grids were tried and reverted: a third level puts cells at roughly
/// 6x8 px, where the labels are too small to read, which defeats the point.
/// Precision past this level comes from nudging in cursor mode instead.
pub const DEFAULT_REFINE: (u32, u32) = (5, 5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridConfig {
    pub coarse_cols: u32,
    pub coarse_rows: u32,
    pub refine_cols: u32,
    pub refine_rows: u32,
    /// How many refinement steps follow the coarse selection.
    pub refine_levels: u32,
    pub alphabet: Vec<char>,
    /// Click immediately when the final cell is chosen, instead of handing
    /// over to cursor mode. Fast, but commits to wherever the cell center
    /// happens to land.
    pub click_on_select: bool,
    /// Pixels moved by one arrow / hjkl press in cursor mode.
    pub nudge_step: i32,
    /// Pixels moved when the same key is pressed with Shift.
    pub nudge_step_fast: i32,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            coarse_cols: DEFAULT_COARSE.0,
            coarse_rows: DEFAULT_COARSE.1,
            refine_cols: DEFAULT_REFINE.0,
            refine_rows: DEFAULT_REFINE.1,
            refine_levels: 1,
            alphabet: ('a'..='z').collect(),
            // Off by default: after two levels the cell center can still be
            // ~18 px from the target, which auto-clicking would often miss.
            click_on_select: false,
            nudge_step: 1,
            nudge_step_fast: 16,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    AlphabetTooSmall(usize),
    DuplicateAlphabetSymbol(char),
    ZeroGridDimension(&'static str),
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConfigError::AlphabetTooSmall(n) => {
                write!(f, "alphabet needs at least 2 symbols, got {n}")
            }
            ConfigError::DuplicateAlphabetSymbol(c) => {
                write!(f, "alphabet contains {c:?} more than once")
            }
            ConfigError::ZeroGridDimension(which) => {
                write!(f, "grid dimension {which} must be at least 1")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl GridConfig {
    /// Reject configurations that would produce an unusable or ambiguous grid.
    ///
    /// A duplicated alphabet symbol is worth failing on rather than silently
    /// deduplicating: it makes two cells answer to the same keystroke, which
    /// reads to the user as the tool ignoring their input.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.alphabet.len() < 2 {
            return Err(ConfigError::AlphabetTooSmall(self.alphabet.len()));
        }
        let mut seen = self.alphabet.clone();
        seen.sort_unstable();
        if let Some(dup) = seen.windows(2).find(|w| w[0] == w[1]).map(|w| w[0]) {
            return Err(ConfigError::DuplicateAlphabetSymbol(dup));
        }
        for (n, which) in [
            (self.coarse_cols, "coarse_cols"),
            (self.coarse_rows, "coarse_rows"),
        ] {
            if n == 0 {
                return Err(ConfigError::ZeroGridDimension(which));
            }
        }
        if self.refine_levels > 0 {
            for (n, which) in [
                (self.refine_cols, "refine_cols"),
                (self.refine_rows, "refine_rows"),
            ] {
                if n == 0 {
                    return Err(ConfigError::ZeroGridDimension(which));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        assert_eq!(GridConfig::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_tiny_alphabet() {
        let cfg = GridConfig {
            alphabet: vec!['a'],
            ..Default::default()
        };
        assert_eq!(cfg.validate(), Err(ConfigError::AlphabetTooSmall(1)));
    }

    #[test]
    fn rejects_duplicate_symbols() {
        let cfg = GridConfig {
            alphabet: vec!['a', 'b', 'a'],
            ..Default::default()
        };
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::DuplicateAlphabetSymbol('a'))
        );
    }

    #[test]
    fn rejects_zero_dimensions() {
        let cfg = GridConfig {
            coarse_rows: 0,
            ..Default::default()
        };
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::ZeroGridDimension("coarse_rows"))
        );
    }

    #[test]
    fn ignores_refine_dimensions_when_refinement_is_off() {
        let cfg = GridConfig {
            refine_levels: 0,
            refine_cols: 0,
            ..Default::default()
        };
        assert_eq!(cfg.validate(), Ok(()));
    }
}
