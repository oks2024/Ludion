//! The host-side render-extent chain: how a machine's stream resolution is chosen, and where that
//! choice is resolved.
//!
//! The extent itself is one field on the [`Engine`](crate::Engine) — the default screenshot
//! resolution, and the size every live stream frame is produced at. This module is the half that
//! reads a machine's sources and turns them into one resolved `(width, height)`: a host's
//! `--render-size` flag, else [`RENDER_SIZE_ENV`], else nothing at all, which keeps the engine's own
//! default.
//!
//! **The extent is a per-machine knob, exactly as `LUDION_THREADS` and `LUDION_QUALITY` are**
//! (`ludion devdocs determinism`): every stage of the frame path — GPU readback, the unpad copy, the
//! per-format encode, the socket write, the viewer's decode and blit — costs in proportion to the
//! pixel count, so how many pixels a box is willing to move is a property of the box, not of the
//! project, and it lives in the environment rather than in any committed file. A host's flag beats
//! the variable outright, without validating it, so a flag still works on a machine whose
//! `LUDION_RENDER_SIZE` is malformed. The extent is sim input (`RenderExtent`), so a replay carries it
//! (the header, and every applied `render.size` record) and runs at the recorded extent; a game that
//! never reads it hashes identically at every extent.
//!
//! **A malformed extent refuses to start** ([`code::RENDER_SIZE_INVALID`], `L0017`), rather than
//! falling back to the default. A silently-ignored setting means streaming at a resolution nobody
//! chose, which is the one outcome the knob exists to prevent.
//!
//! A value is `WIDTHxHEIGHT` in whole pixels — `960x540` — and there is no second form. Both axes
//! are bounded from one pixel up to [`MAX_WIDTH`]×[`MAX_HEIGHT`] (8K), which is the ceiling past
//! which a readback ring stops being a stream and starts being an allocation failure.
//!
//! **The recording rule this knob carries.** A game's mouse input arrives as *render-target pixel*
//! positions (`ludion_sim::InputEvent::MouseMove`), so a session played at one extent is a different
//! input trace from the same session played at another — bit-identical on replay either way, but not
//! comparable with each other. Committed recordings (every `.lrp` under `games/` and `examples/`) are
//! therefore made at the engine's default extent: **never record one from a resized host.**
//!
//! [`resolve_startup_render_size`] is the only function here that reads the process environment;
//! everything under it is pure over its inputs, so the precedence and the parsing are testable
//! without mutating the environment (which edition 2024 makes `unsafe`).

use std::fmt;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

/// The environment variable naming a machine's render extent, as `WIDTHxHEIGHT` (`960x540`). Unset
/// (or empty) keeps the engine's own default extent.
///
/// The per-machine knob — see the module docs. A host's `--render-size` flag beats it.
pub const RENDER_SIZE_ENV: &str = "LUDION_RENDER_SIZE";

/// The widest extent a host may ask for: 8K's width.
///
/// A bound rather than a taste: the readback ring holds three staging buffers of `width * height * 4`
/// bytes plus the frame graph's whole physical cache, so an unbounded extent turns a typo into an
/// allocation failure several layers below the place that could explain it.
pub const MAX_WIDTH: u32 = 7680;

/// The tallest extent a host may ask for: 8K's height. See [`MAX_WIDTH`] for why there is a ceiling.
pub const MAX_HEIGHT: u32 = 4320;

/// Resolves a host process's render extent once at startup.
///
/// Precedence is `explicit` (a `--render-size` flag) → [`RENDER_SIZE_ENV`] → `None`, where `None`
/// means "nothing was chosen" and leaves the engine's own default extent in place. An explicit value
/// wins outright and the environment is not even read for validity, so a flag still works on a box
/// whose `LUDION_RENDER_SIZE` is malformed. A value that is absent, empty, or all whitespace counts
/// as not chosen at all, from either source.
///
/// # Errors
/// One `L0017` [`Diagnostic`] when the winning value is not a `WIDTHxHEIGHT` extent inside
/// `1..=7680` × `1..=4320`. Hosts print it and refuse to start; the alternative is a process
/// streaming at a resolution nobody chose.
///
/// # Example
/// ```
/// use ludion_engine::resolve_startup_render_size;
///
/// // A host's flag wins outright, and names the extent every stream frame is produced at.
/// assert_eq!(resolve_startup_render_size(Some("960x540")).unwrap(), Some((960, 540)));
///
/// // Anything that is not a `WIDTHxHEIGHT` extent refuses to start — the unicode `×` included.
/// let rejected = resolve_startup_render_size(Some("960×540")).unwrap_err();
/// assert_eq!(rejected.code.as_str(), "L0017");
/// ```
pub fn resolve_startup_render_size(
    explicit: Option<&str>,
) -> Result<Option<(u32, u32)>, Diagnostic> {
    resolve_extent(explicit, std::env::var(RENDER_SIZE_ENV).ok().as_deref())
}

/// The pure half of [`resolve_startup_render_size`]: the same precedence over an explicitly supplied
/// environment value, so the chain is testable without mutating the process environment.
fn resolve_extent(
    explicit: Option<&str>,
    env: Option<&str>,
) -> Result<Option<(u32, u32)>, Diagnostic> {
    if let Some(value) = chosen(explicit) {
        return parse_extent(value, Origin::Flag).map(Some);
    }
    match chosen(env) {
        Some(value) => parse_extent(value, Origin::Env).map(Some),
        None => Ok(None),
    }
}

/// The trimmed value a source actually chose: `None` for absent, empty, or all-whitespace.
fn chosen(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|value| !value.is_empty())
}

/// Parses one value into the extent it names. Pure over the text; `origin` names the source in
/// diagnostics.
fn parse_extent(value: &str, origin: Origin) -> Result<(u32, u32), Diagnostic> {
    // A lowercase ASCII `x` is the whole grammar. The unicode `×` reads identically to a human and
    // is refused rather than accepted, because a knob with two spellings is a knob whose value a
    // reader cannot predict — and it is the separator a copy-paste from a spec sheet brings.
    let Some((width, height)) = value.split_once('x') else {
        return Err(invalid(format!(
            "{origin} is `{value}`, which is not a `WIDTHxHEIGHT` extent: the two pixel counts are \
             separated by a lowercase `x`, as in `960x540`"
        )));
    };
    Ok((
        axis(value, width, "width", MAX_WIDTH, origin)?,
        axis(value, height, "height", MAX_HEIGHT, origin)?,
    ))
}

/// One axis of an extent: a whole pixel count inside `1..=max`.
fn axis(value: &str, text: &str, name: &str, max: u32, origin: Origin) -> Result<u32, Diagnostic> {
    // A zero, a negative, and a count past the ceiling are one message on purpose: each is a value
    // that cannot be an extent, and naming the range says everything the reader needs either way.
    match text.trim().parse::<u32>() {
        Ok(pixels) if (1..=max).contains(&pixels) => Ok(pixels),
        _ => Err(invalid(format!(
            "{origin} is `{value}`, whose {name} `{}` is not a whole number of pixels in 1..={max}",
            text.trim(),
        ))),
    }
}

/// Which source a value came from, so the diagnostic says where to go and fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Flag,
    Env,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Flag => write!(f, "the `--render-size` setting"),
            Origin::Env => write!(f, "`{RENDER_SIZE_ENV}`"),
        }
    }
}

/// An `L0017`, carrying the help every member of the family needs: the form, and the precedence.
fn invalid(message: String) -> Diagnostic {
    Diagnostic::new(code::RENDER_SIZE_INVALID, message).with_help(format!(
        "use `WIDTHxHEIGHT` in whole pixels (`960x540`), with each axis inside 1..={MAX_WIDTH} × \
         1..={MAX_HEIGHT}; a host's `--render-size` flag beats {RENDER_SIZE_ENV} either way, and \
         setting neither keeps the engine's default extent"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(explicit: Option<&str>, env: Option<&str>) -> Option<(u32, u32)> {
        resolve_extent(explicit, env).expect("the extent resolves")
    }

    fn rejected(explicit: Option<&str>, env: Option<&str>) -> Diagnostic {
        resolve_extent(explicit, env).expect_err("the extent must be refused")
    }

    /// A well-formed extent resolves from either source, and surrounding whitespace is not part of
    /// the value — a shell profile's stray space must not be a startup failure.
    #[test]
    fn a_well_formed_extent_resolves_from_either_source() {
        for spelling in ["960x540", "  960x540  ", "960 x 540"] {
            assert_eq!(resolved(Some(spelling), None), Some((960, 540)));
            assert_eq!(resolved(None, Some(spelling)), Some((960, 540)));
        }
        // Both ends of both ranges are legal, so the bound guards nothing a host could want.
        assert_eq!(resolved(Some("1x1"), None), Some((1, 1)));
        assert_eq!(
            resolved(Some(&format!("{MAX_WIDTH}x{MAX_HEIGHT}")), None),
            Some((MAX_WIDTH, MAX_HEIGHT)),
        );
    }

    /// Nothing set anywhere is `None` — the machine that was never told anything keeps the engine's
    /// own default extent, which is what makes this knob invisible to every existing host.
    #[test]
    fn an_unset_extent_keeps_the_engines_default() {
        for env in [None, Some(""), Some("   ")] {
            assert_eq!(resolved(None, env), None);
        }
        assert_eq!(resolved(Some("  "), None), None);
    }

    /// A flag must work on a box whose environment is malformed, so the explicit path never consults
    /// (or validates) the environment at all.
    #[test]
    fn an_explicit_value_beats_the_environment() {
        assert_eq!(
            resolved(Some("854x480"), Some("not-an-extent")),
            Some((854, 480)),
        );
        assert_eq!(
            resolved(Some("854x480"), Some("640x360")),
            Some((854, 480)),
            "the flag decides even when the environment is perfectly valid",
        );
    }

    /// A value with no separator cannot name two numbers, and the message says what the form is
    /// rather than only that the value is wrong.
    #[test]
    fn a_separatorless_value_is_l0017_naming_the_form() {
        for source in [(Some("960"), None), (None, Some("960"))] {
            let diagnostic = rejected(source.0, source.1);
            assert_eq!(diagnostic.code, code::RENDER_SIZE_INVALID);
            let message = diagnostic.message.clone();
            assert!(message.contains("960x540"), "{message}");
            assert!(message.contains("lowercase `x`"), "{message}");
        }
        assert!(
            rejected(None, Some("960"))
                .message
                .contains(RENDER_SIZE_ENV),
            "an environment value's diagnostic must name the variable",
        );
        assert!(
            rejected(Some("960"), None)
                .message
                .contains("--render-size"),
            "and a flag's diagnostic must name the flag",
        );
    }

    /// **The unicode `×` is not the separator.** It reads identically to a human and arrives with
    /// every copy-paste from a spec sheet, so it is the one wrong value worth its own test: it must
    /// refuse rather than resolve, because a knob with two spellings has a value a reader cannot
    /// predict.
    #[test]
    fn a_unicode_multiplication_sign_is_not_the_separator() {
        let diagnostic = rejected(Some("960×540"), None);
        assert_eq!(diagnostic.code, code::RENDER_SIZE_INVALID);
        assert!(
            diagnostic.message.contains("lowercase `x`"),
            "{}",
            diagnostic.message,
        );
        assert_eq!(
            rejected(Some("960X540"), None).code,
            code::RENDER_SIZE_INVALID,
            "and neither is a capital `X` — one spelling, exactly",
        );
    }

    /// A zero axis is refused on both axes: an extent with no pixels is not a smaller stream, it is
    /// a readback ring that cannot be built.
    #[test]
    fn a_zero_axis_is_refused_on_either_side() {
        for value in ["0x0", "0x540", "960x0"] {
            let diagnostic = rejected(Some(value), None);
            assert_eq!(diagnostic.code, code::RENDER_SIZE_INVALID);
            assert!(
                diagnostic.message.contains("1..="),
                "the message names the range: {}",
                diagnostic.message,
            );
        }
    }

    /// Past the ceiling — including a count too large for the type at all — is the same refusal, and
    /// each axis is bounded by its own maximum rather than by a shared one.
    #[test]
    fn an_axis_past_its_ceiling_is_refused() {
        for (value, bound) in [
            (format!("{}x540", MAX_WIDTH + 1), MAX_WIDTH),
            (format!("960x{}", MAX_HEIGHT + 1), MAX_HEIGHT),
            ("99999999999999x540".to_string(), MAX_WIDTH),
            ("-960x540".to_string(), MAX_WIDTH),
        ] {
            let diagnostic = rejected(Some(&value), None);
            assert_eq!(diagnostic.code, code::RENDER_SIZE_INVALID);
            assert!(
                diagnostic.message.contains(&format!("1..={bound}")),
                "the message names the axis's own bound: {}",
                diagnostic.message,
            );
        }
        assert_eq!(
            resolved(Some(&format!("{MAX_WIDTH}x540")), None),
            Some((MAX_WIDTH, 540)),
            "the taller axis's lower ceiling never bounds the wider one",
        );
    }

    /// Every refusal carries the same help — the form, the ranges, and the precedence — so a reader
    /// who typed any of the wrong values learns the right one from the diagnostic alone.
    #[test]
    fn every_refusal_carries_the_form_and_the_precedence_as_help() {
        for value in ["960", "960×540", "0x0", "wide x tall"] {
            let help = rejected(Some(value), None)
                .help
                .expect("the family carries help");
            assert!(help.contains("WIDTHxHEIGHT"), "{help}");
            assert!(help.contains(RENDER_SIZE_ENV), "{help}");
            assert!(help.contains("default extent"), "{help}");
        }
    }
}
