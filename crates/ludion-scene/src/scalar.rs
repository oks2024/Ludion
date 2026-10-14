//! Plain-scalar resolution (YAML 1.2 core schema) and the canonical writer's quoting rule.
//!
//! These two halves are deliberately one module because they must agree: the writer may emit a
//! string as a *plain* (unquoted) scalar only if resolving that text as a plain scalar yields the
//! same string back. Any text that resolves to `null`, a boolean, or a number — or that could be
//! misread structurally — is double-quoted. This is what kills the "Norway problem" (`no` parsed
//! as `false`) by construction: `no` is a string in YAML 1.2 core anyway, and anything genuinely
//! ambiguous never leaves the writer unquoted.

use serde_json::Value;

/// The result of resolving a plain (unquoted) YAML scalar under the YAML 1.2 core schema.
pub(crate) enum Resolved {
    /// The scalar is a `null`, boolean, or number, resolved to its JSON value.
    Value(Value),
    /// The scalar is a plain string (no special resolution applies).
    Str,
    /// The scalar names a value JSON cannot represent (non-finite float, out-of-range integer).
    /// The payload is a human-readable reason.
    Unrepresentable(String),
}

/// Resolves a plain scalar per the YAML 1.2 core schema.
///
/// Quoted scalars must *not* go through this function — they are always strings.
pub(crate) fn resolve_plain(text: &str) -> Resolved {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Resolved::Value(Value::Null),
        "true" | "True" | "TRUE" => return Resolved::Value(Value::Bool(true)),
        "false" | "False" | "FALSE" => return Resolved::Value(Value::Bool(false)),
        _ => {}
    }
    if is_core_int(text) {
        return resolve_int(text);
    }
    if let Some(radix_value) = resolve_radix_int(text) {
        return radix_value;
    }
    if is_non_finite_float(text) {
        return Resolved::Unrepresentable(format!(
            "the non-finite float `{text}` cannot be represented (scene values must be JSON-compatible)"
        ));
    }
    if is_core_float(text) {
        return resolve_float(text);
    }
    Resolved::Str
}

/// Whether the canonical writer may emit `text` as a plain (unquoted) scalar.
///
/// Deliberately conservative: plain output is limited to identifier-ish strings (letters, digits,
/// `_ - . /` and single interior spaces) that still resolve to themselves as strings. Everything
/// else — empty strings, strings with YAML indicator characters, lookalikes of other scalar types
/// — is double-quoted by [`quote_double`].
pub(crate) fn plain_safe(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false; // empty string must be quoted
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '/') {
        return false;
    }
    if !text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ' '))
    {
        return false;
    }
    // Trailing spaces would be stripped on re-parse (data loss); runs of spaces would be folded.
    if text.ends_with(' ') || text.contains("  ") {
        return false;
    }
    // YAML 1.1 boolean lookalikes stay quoted even though YAML 1.2 reads them as strings — this
    // is the Norway problem (`no` = false in 1.1), and canonical files must be unambiguous to
    // *every* YAML tool a human might open them with, not just our parser. (`y`/`n` are exempt:
    // no mainstream parser resolves them, and `y` is a coordinate name everywhere.)
    if matches!(
        text.to_ascii_lowercase().as_str(),
        "yes" | "no" | "on" | "off"
    ) {
        return false;
    }
    // The text must survive plain resolution as itself (e.g. `True` must be quoted).
    matches!(resolve_plain(text), Resolved::Str)
}

/// Double-quotes `text` with YAML double-quote escaping.
///
/// C0/C1 control characters, `DEL`, and the BOM are `\u00XX`-escaped; everything printable is
/// emitted raw (UTF-8). The escape set is a strict subset of YAML 1.2's double-quote escapes, so
/// any YAML 1.2 parser reads the exact original string back.
pub(crate) fn quote_double(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || ('\u{7f}'..='\u{9f}').contains(&c) || c == '\u{feff}' => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A string as the canonical writer spells it: plain when unambiguous, double-quoted otherwise. The
/// one shared entry point for the scene/mesh/voxel emitters — the Norway-problem primitive (`no`
/// resolving to boolean `false` is the canonical example of why a plain-looking string still needs
/// this check) they all need.
pub(crate) fn scalar_string(text: &str) -> String {
    if plain_safe(text) {
        text.to_string()
    } else {
        quote_double(text)
    }
}

/// Formats an `f64` in ryu shortest-round-trip form (the canonical float representation).
///
/// Never fixed precision — fixed precision is a data-loss bug (see `CLAUDE.md`). Every output of
/// this function re-resolves as a YAML 1.2 core float and parses back to the identical bits.
pub(crate) fn format_float(value: f64) -> String {
    let mut buffer = ryu::Buffer::new();
    buffer.format(value).to_string()
}

/// Formats an `f64` that carries an `f32` value (an engine component widened by
/// `serde_json::to_value`) as ryu's shortest *f32* decimal, so `0.3_f32` reads `0.3` and not
/// `0.30000001192092896`. A value that is not f32-exact is a genuine `f64` and keeps
/// [`format_float`]. The text reads back to the same `f32` either way.
///
/// A value that *is* the f64 reading of its own shortest f32 decimal (what that decimal parses to)
/// keeps that decimal too, so a written file is a fixed point: ryu spells large and small
/// magnitudes differently for `f32` and `f64`.
pub(crate) fn format_f32_float(value: f64) -> String {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the narrowing is checked bit-for-bit against the original below"
    )]
    let narrowed = value as f32;
    let short = ryu::Buffer::new().format(narrowed).to_string();
    // Bits, not `==`: keeps `-0.0`; an out-of-range value narrows to infinity and falls through.
    let is_f32_value = f64::from(narrowed).to_bits() == value.to_bits();
    let is_its_decimal = short
        .parse::<f64>()
        .is_ok_and(|read| read.to_bits() == value.to_bits());
    if is_f32_value || is_its_decimal {
        short
    } else {
        format_float(value)
    }
}

// --- YAML 1.2 core schema shape checks -----------------------------------------------------

/// `[-+]?[0-9]+`
fn is_core_int(text: &str) -> bool {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text).as_bytes();
    !body.is_empty() && body.iter().all(u8::is_ascii_digit)
}

/// `[-+]? ( \.[0-9]+ | [0-9]+ (\.[0-9]*)? ) ( [eE] [-+]? [0-9]+ )?`, excluding pure integers.
fn is_core_float(text: &str) -> bool {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    if let Some(exponent) = exponent {
        let digits = exponent
            .strip_prefix(['-', '+'])
            .unwrap_or(exponent)
            .as_bytes();
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return false;
        }
    }
    match mantissa.split_once('.') {
        Some((int_part, frac_part)) => {
            let int_ok = int_part.bytes().all(|b| b.is_ascii_digit());
            let frac_ok = frac_part.bytes().all(|b| b.is_ascii_digit());
            int_ok && frac_ok && (!int_part.is_empty() || !frac_part.is_empty())
        }
        // No dot: only a float if an exponent is present (otherwise it is an int).
        None => {
            exponent.is_some()
                && !mantissa.is_empty()
                && mantissa.bytes().all(|b| b.is_ascii_digit())
        }
    }
}

/// `[-+]?\.(inf|Inf|INF)` or `\.(nan|NaN|NAN)`.
fn is_non_finite_float(text: &str) -> bool {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    matches!(body, ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN")
}

fn resolve_int(text: &str) -> Resolved {
    if let Ok(v) = text.parse::<i64>() {
        return Resolved::Value(Value::from(v));
    }
    // Large positive values may still fit u64.
    if !text.starts_with('-')
        && let Ok(v) = text.trim_start_matches('+').parse::<u64>()
    {
        return Resolved::Value(Value::from(v));
    }
    Resolved::Unrepresentable(format!(
        "the integer `{text}` does not fit in 64 bits (scene integers must fit i64 or u64)"
    ))
}

/// `0x[0-9a-fA-F]+` / `0o[0-7]+` (YAML 1.2 core hexadecimal and octal integers).
fn resolve_radix_int(text: &str) -> Option<Resolved> {
    let (digits, radix) = if let Some(hex) = text.strip_prefix("0x") {
        (hex, 16)
    } else if let Some(oct) = text.strip_prefix("0o") {
        (oct, 8)
    } else {
        return None;
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None; // not a radix int; falls through to plain string
    }
    Some(match u64::from_str_radix(digits, radix) {
        Ok(v) => Resolved::Value(Value::from(v)),
        Err(_) => Resolved::Unrepresentable(format!(
            "the integer `{text}` does not fit in 64 bits (scene integers must fit i64 or u64)"
        )),
    })
}

fn resolve_float(text: &str) -> Resolved {
    match text.parse::<f64>() {
        Ok(v) if v.is_finite() => Resolved::Value(Value::from(v)),
        _ => {
            Resolved::Unrepresentable(format!("the float `{text}` overflows the finite f64 range"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_value(text: &str) -> Value {
        match resolve_plain(text) {
            Resolved::Value(v) => v,
            Resolved::Str => Value::String(text.to_string()),
            Resolved::Unrepresentable(reason) => panic!("unrepresentable: {reason}"),
        }
    }

    #[test]
    fn core_schema_resolution() {
        assert_eq!(resolved_value(""), Value::Null);
        assert_eq!(resolved_value("~"), Value::Null);
        assert_eq!(resolved_value("NULL"), Value::Null);
        assert_eq!(resolved_value("true"), Value::Bool(true));
        assert_eq!(resolved_value("False"), Value::Bool(false));
        assert_eq!(resolved_value("42"), Value::from(42));
        assert_eq!(resolved_value("-7"), Value::from(-7));
        assert_eq!(resolved_value("+5"), Value::from(5));
        assert_eq!(resolved_value("0x1F"), Value::from(31));
        assert_eq!(resolved_value("0o17"), Value::from(15));
        assert_eq!(resolved_value("1.5"), Value::from(1.5));
        assert_eq!(resolved_value("-0.0"), Value::from(-0.0));
        assert_eq!(resolved_value(".5"), Value::from(0.5));
        assert_eq!(resolved_value("1."), Value::from(1.0));
        assert_eq!(resolved_value("1e3"), Value::from(1000.0));
        assert_eq!(resolved_value("2.5E-2"), Value::from(0.025));
    }

    #[test]
    fn norway_problem_is_dead() {
        // YAML 1.2 core: only true/false (with case variants) are booleans.
        for s in ["no", "No", "NO", "yes", "on", "off", "y", "n"] {
            assert!(
                matches!(resolve_plain(s), Resolved::Str),
                "{s} must be a string"
            );
        }
    }

    #[test]
    fn non_finite_and_overflow_are_rejected() {
        assert!(matches!(
            resolve_plain(".inf"),
            Resolved::Unrepresentable(_)
        ));
        assert!(matches!(
            resolve_plain("-.INF"),
            Resolved::Unrepresentable(_)
        ));
        assert!(matches!(
            resolve_plain(".nan"),
            Resolved::Unrepresentable(_)
        ));
        assert!(matches!(
            resolve_plain("1e999"),
            Resolved::Unrepresentable(_)
        ));
        // 2^64 exactly one too large:
        assert!(matches!(
            resolve_plain("18446744073709551616"),
            Resolved::Unrepresentable(_)
        ));
        // i64::MIN..u64::MAX still fit:
        assert_eq!(
            resolved_value("18446744073709551615"),
            Value::from(u64::MAX)
        );
        assert_eq!(
            resolved_value("-9223372036854775808"),
            Value::from(i64::MIN)
        );
    }

    #[test]
    fn plain_safety_quotes_lookalikes() {
        assert!(plain_safe("paddle"));
        assert!(plain_safe("brick_diff.png"));
        assert!(plain_safe("a b"));
        assert!(plain_safe("/assets/x"));
        assert!(!plain_safe(""));
        assert!(!plain_safe("true"));
        assert!(!plain_safe("Null"));
        assert!(!plain_safe("no")); // a string in YAML 1.2, but `false` to any 1.1 tool
        assert!(!plain_safe("3.14"));
        assert!(!plain_safe("-x"));
        assert!(!plain_safe("a:b"));
        assert!(!plain_safe("a #b"));
        assert!(!plain_safe("x "));
        assert!(!plain_safe("héllo")); // non-ASCII goes quoted
    }

    #[test]
    fn quote_double_escapes() {
        assert_eq!(quote_double("a\"b"), r#""a\"b""#);
        assert_eq!(quote_double("a\\b"), r#""a\\b""#);
        assert_eq!(quote_double("a\nb"), r#""a\nb""#);
        assert_eq!(quote_double("\u{1}"), "\"\\u0001\"");
        assert_eq!(quote_double("\u{9f}"), "\"\\u009F\"");
        assert_eq!(quote_double("héllo"), "\"héllo\"");
    }

    #[test]
    fn float_format_is_shortest_round_trip() {
        assert_eq!(format_float(1.0), "1.0");
        assert_eq!(format_float(0.3), "0.3");
        assert_eq!(format_float(-0.0), "-0.0");
        assert_eq!(format_float(f64::from(0.3_f32)), "0.30000001192092896");
        // Everything format_float emits must re-resolve as a float, byte-exactly.
        for v in [1.0, 0.3, -0.0, 1e30, 5e-324, f64::MAX] {
            let text = format_float(v);
            match resolve_plain(&text) {
                Resolved::Value(Value::Number(n)) => {
                    let back = n.as_f64().unwrap();
                    assert_eq!(back.to_bits(), v.to_bits(), "{text}");
                }
                _ => panic!("{text} did not resolve as a float"),
            }
        }
    }

    #[test]
    fn f32_float_format_is_shortest_f32() {
        assert_eq!(format_f32_float(f64::from(0.3_f32)), "0.3");
        assert_eq!(format_f32_float(f64::from(1e-7_f32)), "1e-7");
        assert_eq!(format_f32_float(0.3), "0.3"); // not f32-exact: the f64 path
        assert_eq!(format_f32_float(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(format_f32_float(-0.0), "-0.0");
        assert_eq!(format_f32_float(1e300), format_float(1e300));
        // The f64 reading of a shortest f32 decimal keeps that decimal: a fixed point, though ryu
        // spells this magnitude differently for f64 (`-52829826000000.0`).
        assert_eq!(format_f32_float(-5.2829826e13), "-5.2829826e13");
        for v in [
            f64::from(0.3_f32),
            f64::from(1e-7_f32),
            f64::from(f32::MAX),
            f64::from(f32::MIN_POSITIVE),
            -0.0,
            0.3,
            0.1 + 0.2,
            1e300,
        ] {
            let text = format_f32_float(v);
            match resolve_plain(&text) {
                Resolved::Value(Value::Number(n)) => {
                    let back = n.as_f64().unwrap();
                    if f64::from(v as f32).to_bits() == v.to_bits() {
                        assert_eq!((back as f32).to_bits(), (v as f32).to_bits(), "{text}");
                    } else {
                        assert_eq!(back.to_bits(), v.to_bits(), "{text}");
                    }
                }
                _ => panic!("{text} did not resolve as a float"),
            }
        }
    }
}
