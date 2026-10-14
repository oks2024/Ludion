//! Small string helpers shared across crates.
//!
//! Nothing here is clever; each entry exists because two or more crates were about to spell the same
//! two-line predicate differently.

/// The single [`char`] of a one-character string, or `None` if `s` is empty or longer.
///
/// "One character" means one Unicode scalar value, not one byte: `"é"` and `"火"` are single chars,
/// `"ab"` and `""` are not.
///
/// # Example
///
/// ```
/// use ludion_core::text::single_char;
///
/// assert_eq!(single_char("#"), Some('#'));
/// assert_eq!(single_char("火"), Some('火'));
/// assert_eq!(single_char("##"), None);
/// assert_eq!(single_char(""), None);
/// ```
pub fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::single_char;

    #[test]
    fn multi_byte_scalars_count_as_one_char() {
        // A byte-length check would get all three of these wrong.
        assert_eq!(single_char("é"), Some('é'));
        assert_eq!(single_char("🜁"), Some('🜁'));
        assert_eq!(single_char("ab"), None);
    }
}
