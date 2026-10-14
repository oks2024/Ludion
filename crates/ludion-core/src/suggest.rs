//! Nearest-match suggestions for typo diagnostics ("nearest match is `Transform`").

/// Picks the candidate closest to `target` by case-insensitive edit distance, if any is close
/// enough to plausibly be a typo (distance ≤ max(1, len/3)).
///
/// Candidates are scanned in the order given; ties keep the earliest, so callers that pass a
/// deterministic order (registry order, schema property order) get deterministic suggestions.
///
/// # Example
///
/// ```
/// use ludion_core::suggest::nearest;
///
/// let candidates = ["Transform", "Sprite", "Parent"];
/// assert_eq!(nearest("Trasform", candidates), Some("Transform"));
/// assert_eq!(nearest("sprite", candidates), Some("Sprite"));
/// // Nothing plausibly a typo of "Camera" — no suggestion rather than a misleading one.
/// assert_eq!(nearest("Camera", candidates), None);
/// ```
pub fn nearest<'a, I>(target: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let target_lower = target.to_lowercase();
    let budget = (target.chars().count() / 3).max(1);
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in candidates {
        let distance = edit_distance(&target_lower, &candidate.to_lowercase());
        if distance <= budget && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Optimal-string-alignment (restricted Damerau-Levenshtein) distance over chars: insert,
/// delete, substitute, and — because it is the single most common typo — adjacent transposition,
/// each costing 1.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut two_ago: Vec<usize> = vec![0; b.len() + 1];
    let mut one_ago: Vec<usize> = (0..=b.len()).collect();
    for i in 0..a.len() {
        let mut current = vec![0; b.len() + 1];
        current[0] = i + 1;
        for j in 0..b.len() {
            let substitution = one_ago[j] + usize::from(a[i] != b[j]);
            current[j + 1] = substitution.min(one_ago[j + 1] + 1).min(current[j] + 1);
            if i > 0 && j > 0 && a[i] == b[j - 1] && a[i - 1] == b[j] {
                current[j + 1] = current[j + 1].min(two_ago[j - 1] + 1);
            }
        }
        two_ago = std::mem::replace(&mut one_ago, current);
    }
    one_ago[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_basics() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        // Adjacent transposition costs 1, not 2.
        assert_eq!(edit_distance("naem", "name"), 1);
    }

    #[test]
    fn suggests_close_names_only() {
        let candidates = ["Transform", "Sprite", "Parent"];
        assert_eq!(nearest("Trasform", candidates), Some("Transform"));
        assert_eq!(nearest("sprite", candidates), Some("Sprite"));
        assert_eq!(nearest("Sprtie", candidates), Some("Sprite"));
        assert_eq!(nearest("Camera", candidates), None);
        assert_eq!(nearest("x", candidates), None);
    }
}
