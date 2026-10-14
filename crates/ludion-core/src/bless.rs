//! The bless switches: one meaning for "is this run rewriting baselines".
//!
//! `LUDION_BLESS` (goldens, physics baselines), `LUDION_BLESS_ANY_ADAPTER` (bless off the pinned
//! GPU) and `BLESS_SCHEMA` (the protocol and pattern schema snapshots) are all read here: a switch
//! is set when it is non-empty and not `"0"`, so `LUDION_BLESS=0` never blesses in one test and
//! does in another.
//! `scripts/lists-in-step.mjs` rejects a direct read of these names anywhere else.

/// Rewrite goldens and committed baselines.
pub const BLESS_ENV: &str = "LUDION_BLESS";
/// Allow `LUDION_BLESS` to write goldens from an adapter other than the pinned one.
pub const BLESS_ANY_ADAPTER_ENV: &str = "LUDION_BLESS_ANY_ADAPTER";
/// Rewrite the protocol and pattern schema snapshots.
pub const BLESS_SCHEMA_ENV: &str = "BLESS_SCHEMA";

/// Parses a switch value: set is non-empty and not `"0"`.
pub fn switch_setting(value: Option<&str>) -> bool {
    matches!(value, Some(v) if !v.is_empty() && v != "0")
}

/// Whether the environment variable `name` is set. A non-UTF-8 value counts as unset.
pub fn switch_set(name: &str) -> bool {
    switch_setting(std::env::var(name).ok().as_deref())
}

/// Whether this run blesses goldens and baselines (`LUDION_BLESS`).
pub fn bless_requested() -> bool {
    switch_set(BLESS_ENV)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_empty_and_zero_are_off() {
        assert!(!switch_setting(None));
        assert!(!switch_setting(Some("")));
        assert!(!switch_setting(Some("0")));
    }

    #[test]
    fn anything_else_is_on() {
        assert!(switch_setting(Some("1")));
        assert!(switch_setting(Some("true")));
    }
}
