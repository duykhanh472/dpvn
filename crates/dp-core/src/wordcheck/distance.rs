//! String edit distance and spelling suggestion generator (§9.4).
//!
//! Generates corrections with Levenshtein edit distance <= 2 against good word lists.

/// Compute Unicode scalar value Levenshtein distance between two strings.
#[must_use]
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    // Single-row dynamic programming buffer for low memory footprint
    let mut dp: Vec<usize> = (0..=n).collect();

    for (i, &ca) in a_chars.iter().enumerate() {
        let mut prev = dp[0];
        dp[0] = i + 1;

        for (j, &cb) in b_chars.iter().enumerate() {
            let temp = dp[j + 1];
            let cost = if ca == cb { 0 } else { 1 };

            dp[j + 1] = std::cmp::min(
                std::cmp::min(dp[j] + 1, dp[j + 1] + 1), // insertion, deletion
                prev + cost,                             // substitution
            );
            prev = temp;
        }
    }

    dp[n]
}

/// Find the closest match for `word` among `candidates` within `max_distance`.
pub fn find_best_suggestion<'a, I>(word: &str, candidates: I, max_distance: usize) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut best_word = None;
    let mut min_dist = max_distance + 1;

    let word_lower = word.to_lowercase();

    for cand in candidates {
        // Fast skip based on length difference
        let len_diff =
            (cand.chars().count() as isize - word.chars().count() as isize).unsigned_abs();
        if len_diff > max_distance {
            continue;
        }

        let cand_lower = cand.to_lowercase();
        let dist = levenshtein_distance(&word_lower, &cand_lower);

        if dist < min_dist && dist <= max_distance {
            min_dist = dist;
            best_word = Some(cand.to_string());
            if dist == 1 {
                // Return immediate close match if distance is 1
                break;
            }
        }
    }

    best_word
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("sách", "sách"), 0);
        assert_eq!(levenshtein_distance("nghỉ", "nghĩ"), 1); // Tone difference
        assert_eq!(levenshtein_distance("hoà", "hòa"), 2);
        assert_eq!(levenshtein_distance("việt", "nam"), 4);
    }

    #[test]
    fn test_find_best_suggestion() {
        let candidates = ["nghiên", "nghiêm", "nghiêng", "khoảng"];
        // Typo: "nghiêngg"
        let sug = find_best_suggestion("nghiêngg", candidates, 2);
        assert_eq!(sug.as_deref(), Some("nghiêng"));
    }
}
