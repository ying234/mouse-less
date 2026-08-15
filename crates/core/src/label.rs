//! Cell label generation.
//!
//! Labels are fixed-width base-N numerals over the configured alphabet. Fixed
//! width buys us a property the matching logic leans on hard: the label set is
//! prefix-free, so a typed string can never be both a complete label and a
//! prefix of a longer one. That removes any need for a disambiguation timeout.

/// Number of characters needed to give `count` distinct labels.
pub fn width_for(alphabet_len: usize, count: usize) -> usize {
    if count <= 1 || alphabet_len < 2 {
        return 1;
    }
    let mut width = 1usize;
    let mut capacity = alphabet_len;
    while capacity < count {
        capacity = capacity.saturating_mul(alphabet_len);
        width += 1;
    }
    width
}

/// Generate `count` fixed-width labels over `alphabet`.
///
/// Returns an empty vector for a degenerate alphabet (fewer than 2 symbols),
/// which the caller should treat as a configuration error.
pub fn generate(alphabet: &[char], count: usize) -> Vec<String> {
    if alphabet.len() < 2 || count == 0 {
        return Vec::new();
    }
    let base = alphabet.len();
    let width = width_for(base, count);

    (0..count)
        .map(|i| {
            let mut buf = vec![alphabet[0]; width];
            let mut n = i;
            // Least-significant digit last, so labels read in a natural order.
            for slot in buf.iter_mut().rev() {
                *slot = alphabet[n % base];
                n /= base;
            }
            buf.into_iter().collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn width_grows_with_count() {
        assert_eq!(width_for(26, 1), 1);
        assert_eq!(width_for(26, 26), 1);
        assert_eq!(width_for(26, 27), 2);
        assert_eq!(width_for(26, 676), 2);
        assert_eq!(width_for(26, 677), 3);
    }

    #[test]
    fn labels_are_unique_and_fixed_width() {
        let a = alpha("asdfghjkl");
        let labels = generate(&a, 336);
        assert_eq!(labels.len(), 336);

        let width = labels[0].chars().count();
        assert!(labels.iter().all(|l| l.chars().count() == width));

        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "labels must be unique");
    }

    #[test]
    fn labels_use_only_alphabet_symbols() {
        let a = alpha("asdfghjkl");
        for label in generate(&a, 100) {
            assert!(label.chars().all(|c| a.contains(&c)));
        }
    }

    #[test]
    fn single_row_of_labels_is_one_char() {
        let labels = generate(&alpha("abcdef"), 6);
        assert_eq!(labels, ["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn degenerate_alphabet_yields_nothing() {
        assert!(generate(&alpha("a"), 10).is_empty());
        assert!(generate(&alpha("abc"), 0).is_empty());
    }
}
