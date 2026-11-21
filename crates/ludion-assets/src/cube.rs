//! Adobe/Resolve `.cube` 3D LUT parser (no GPU types).
//!
//! Accepted grammar: blank lines and `#` comments; header keywords before the first data row
//! (`TITLE "name"`, `LUT_3D_SIZE n` with n in 17/33/65, `DOMAIN_MIN r g b`, `DOMAIN_MAX r g b`,
//! `LUT_3D_INPUT_RANGE lo hi`); then exactly n³ rows of three finite floats, red varying fastest.
//! `LUT_1D_SIZE` and other sizes are `L2391`; every other defect is `L2390` with a `label:line`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::{AssetRoot, load_bytes};

/// The 3D sizes the engine accepts.
const SIZES: [u32; 3] = [17, 33, 65];

/// A parsed 3D LUT: `rgb` holds `size³` rows, red fastest (`i = r + g·size + b·size²`).
#[derive(Clone, PartialEq, Debug)]
pub struct CubeLut {
    /// The `TITLE`, if the file has one.
    pub title: Option<String>,
    /// Lattice points per axis: 17, 33 or 65.
    pub size: u32,
    /// Input domain lower bound per channel (default 0).
    pub domain_min: [f32; 3],
    /// Input domain upper bound per channel (default 1).
    pub domain_max: [f32; 3],
    /// The table rows in file order.
    pub rgb: Vec<[f32; 3]>,
    /// Content hash of the source bytes (0 for [`identity`]).
    pub hash: u64,
}

fn malformed(label: &str, line: usize, why: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::new(code::CUBE_LUT_MALFORMED, format!("{label}:{line}: {why}")).with_help(
        "a `.cube` file is header keywords (`LUT_3D_SIZE n`, optional `TITLE`, `DOMAIN_MIN`, `DOMAIN_MAX`, \
         `LUT_3D_INPUT_RANGE`) then n³ rows of three floats; `ludion explain L2390` lists the grammar",
    )
}

fn unsupported(label: &str, line: usize, why: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::new(code::CUBE_LUT_UNSUPPORTED, format!("{label}:{line}: {why}"))
        .with_help("re-export the LUT as a 3D table of size 17, 33 or 65")
}

fn floats<const N: usize>(
    args: &[&str],
    label: &str,
    line: usize,
    keyword: &str,
) -> Result<[f32; N], Diagnostic> {
    let bad = || malformed(label, line, format!("`{keyword}` takes {N} finite numbers"));
    if args.len() != N {
        return Err(bad());
    }
    let mut out = [0.0; N];
    for (slot, arg) in out.iter_mut().zip(args) {
        *slot = arg
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(bad)?;
    }
    Ok(out)
}

/// Parses `.cube` text. `label` names the file in errors; `hash` is stored as [`CubeLut::hash`].
///
/// # Errors
/// - `L2390` for a malformed header or row, a missing size, a bad domain or a wrong row count.
/// - `L2391` for `LUT_1D_SIZE` or a `LUT_3D_SIZE` other than 17, 33 or 65.
///
/// # Example
/// ```
/// use ludion_assets::cube::{identity, parse_cube};
/// let lut = identity(17);
/// let mut text = String::from("LUT_3D_SIZE 17\n");
/// for row in &lut.rgb {
///     text.push_str(&format!("{} {} {}\n", row[0], row[1], row[2]));
/// }
/// assert_eq!(parse_cube(&text, "id.cube", 7).unwrap().rgb, lut.rgb);
/// assert_eq!(parse_cube("LUT_1D_SIZE 16\n", "x.cube", 0).unwrap_err().code.as_str(), "L2391");
/// ```
pub fn parse_cube(text: &str, label: &str, hash: u64) -> Result<CubeLut, Diagnostic> {
    let mut title = None;
    let mut size: Option<u32> = None;
    let mut domain_min = [0.0f32; 3];
    let mut domain_max = [1.0f32; 3];
    let mut rgb: Vec<[f32; 3]> = Vec::new();
    let mut first_row_line = None;
    let mut expected = 0usize;
    let mut last_line = 0usize;

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        last_line = line;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut words = trimmed.split_whitespace();
        let head = words.next().unwrap_or_default();
        let args: Vec<&str> = words.collect();
        if head.parse::<f32>().is_ok() {
            if first_row_line.is_none() {
                first_row_line = Some(line);
                let n =
                    size.ok_or_else(|| malformed(label, line, "data row before `LUT_3D_SIZE`"))?;
                if (0..3).any(|c| domain_max[c] <= domain_min[c]) {
                    return Err(malformed(
                        label,
                        line,
                        "a domain maximum is not above its minimum",
                    ));
                }
                expected = (n as usize).pow(3);
                rgb.reserve(expected);
            }
            if rgb.len() == expected {
                return Err(malformed(
                    label,
                    line,
                    format!("more than {expected} data rows"),
                ));
            }
            let all: Vec<&str> = std::iter::once(head).chain(args.iter().copied()).collect();
            rgb.push(floats::<3>(&all, label, line, "a data row")?);
            continue;
        }
        if let Some(first) = first_row_line {
            return Err(malformed(
                label,
                line,
                format!(
                    "keyword `{head}` after the first data row (line {first}); header keywords come first"
                ),
            ));
        }
        match head {
            "TITLE" => {
                let name = trimmed["TITLE".len()..].trim().trim_matches('"');
                title = Some(name.to_string());
            }
            "LUT_3D_SIZE" => {
                let n: u32 = match args.as_slice() {
                    [one] => one.parse().map_err(|_| {
                        malformed(label, line, "`LUT_3D_SIZE` takes one whole number")
                    })?,
                    _ => {
                        return Err(malformed(
                            label,
                            line,
                            "`LUT_3D_SIZE` takes one whole number",
                        ));
                    }
                };
                if !SIZES.contains(&n) {
                    return Err(unsupported(
                        label,
                        line,
                        format!("`LUT_3D_SIZE {n}` is not supported (17, 33 or 65)"),
                    ));
                }
                size = Some(n);
            }
            "LUT_1D_SIZE" => {
                return Err(unsupported(
                    label,
                    line,
                    "1D LUTs are not supported; use a 3D table",
                ));
            }
            "DOMAIN_MIN" => domain_min = floats::<3>(&args, label, line, "DOMAIN_MIN")?,
            "DOMAIN_MAX" => domain_max = floats::<3>(&args, label, line, "DOMAIN_MAX")?,
            "LUT_3D_INPUT_RANGE" => {
                let [lo, hi] = floats::<2>(&args, label, line, "LUT_3D_INPUT_RANGE")?;
                domain_min = [lo; 3];
                domain_max = [hi; 3];
            }
            other => return Err(malformed(label, line, format!("unknown keyword `{other}`"))),
        }
    }

    let Some(n) = size else {
        return Err(malformed(label, last_line.max(1), "no `LUT_3D_SIZE`"));
    };
    if first_row_line.is_none() {
        if (0..3).any(|c| domain_max[c] <= domain_min[c]) {
            return Err(malformed(
                label,
                last_line.max(1),
                "a domain maximum is not above its minimum",
            ));
        }
        expected = (n as usize).pow(3);
    }
    if rgb.len() != expected {
        return Err(malformed(
            label,
            last_line.max(1),
            format!(
                "`LUT_3D_SIZE {n}` needs {expected} data rows, found {}",
                rgb.len()
            ),
        ));
    }
    Ok(CubeLut {
        title,
        size: n,
        domain_min,
        domain_max,
        rgb,
        hash,
    })
}

/// Reads and parses the `.cube` at `root`/`relative`; the hash is the file's content hash.
///
/// # Errors
/// - `L1101` if the file cannot be read; `L2390` if it is not UTF-8 or not a valid `.cube`;
///   `L2391` for a 1D table or an unsupported size.
///
/// # Example
/// ```
/// use ludion_assets::{AssetRoot, cube::load_cube};
/// let dir = std::path::Path::new(".");
/// assert_eq!(load_cube(AssetRoot::content(dir), "no-such.cube").unwrap_err().code.as_str(), "L1101");
/// ```
pub fn load_cube(root: AssetRoot<'_>, relative: &str) -> Result<CubeLut, Diagnostic> {
    let asset = load_bytes(root, relative)?;
    let text = std::str::from_utf8(&asset.bytes)
        .map_err(|_| malformed(relative, 1, "the file is not UTF-8 text"))?;
    parse_cube(text, relative, asset.hash)
}

/// The identity LUT of `size` points per axis (hash 0, domain 0..1).
///
/// # Example
/// ```
/// let lut = ludion_assets::cube::identity(17);
/// assert_eq!(lut.rgb.len(), 17 * 17 * 17);
/// assert_eq!(lut.rgb[1], [1.0 / 16.0, 0.0, 0.0]);
/// ```
#[must_use]
pub fn identity(size: u32) -> CubeLut {
    let step = 1.0 / (size - 1) as f32;
    let mut rgb = Vec::with_capacity((size as usize).pow(3));
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                rgb.push([r as f32 * step, g as f32 * step, b as f32 * step]);
            }
        }
    }
    CubeLut {
        title: None,
        size,
        domain_min: [0.0; 3],
        domain_max: [1.0; 3],
        rgb,
        hash: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(n: u32) -> String {
        format!("LUT_3D_SIZE {n}\n")
    }

    fn body(lut: &CubeLut) -> String {
        lut.rgb
            .iter()
            .map(|r| format!("{} {} {}\n", r[0], r[1], r[2]))
            .collect()
    }

    fn err(text: &str) -> Diagnostic {
        parse_cube(text, "t.cube", 0).unwrap_err()
    }

    #[test]
    fn identity_17_round_trips() {
        let lut = identity(17);
        let parsed = parse_cube(&format!("{}{}", header(17), body(&lut)), "t.cube", 5).unwrap();
        assert_eq!(parsed.rgb, lut.rgb);
        assert_eq!(parsed.hash, 5);
    }

    #[test]
    fn crlf_comments_title_and_domain() {
        let lut = identity(17);
        let text = format!(
            "# made by hand\r\nTITLE \"warm\"\r\n\r\nDOMAIN_MIN 0 0 0\r\nDOMAIN_MAX 2 2 2\r\n{}{}",
            header(17).replace('\n', "\r\n"),
            body(&lut).replace('\n', "\r\n")
        );
        let parsed = parse_cube(&text, "t.cube", 0).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("warm"));
        assert_eq!(parsed.domain_max, [2.0; 3]);
    }

    #[test]
    fn input_range_sets_the_domain() {
        let text = format!(
            "LUT_3D_INPUT_RANGE -1 3\n{}{}",
            header(17),
            body(&identity(17))
        );
        let parsed = parse_cube(&text, "t.cube", 0).unwrap();
        assert_eq!(
            (parsed.domain_min, parsed.domain_max),
            ([-1.0; 3], [3.0; 3])
        );
    }

    #[test]
    fn one_d_and_odd_sizes_are_unsupported() {
        let e = err("TITLE x\nLUT_1D_SIZE 16\n");
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "L2391",
                "t.cube:2: 1D LUTs are not supported; use a 3D table"
            )
        );
        let e = err("\nLUT_3D_SIZE 16\n");
        assert_eq!(e.code.as_str(), "L2391");
        assert!(e.message.starts_with("t.cube:2:"));
    }

    #[test]
    fn malformed_cases_carry_their_line() {
        let cases = [
            ("LUT_3D_SIZE 17\nBOGUS 1\n", "t.cube:2:"),
            ("LUT_3D_SIZE 17\nDOMAIN_MIN 0 0\n", "t.cube:2:"),
            ("LUT_3D_SIZE 17\n0 0 x\n", "t.cube:2:"),
            ("LUT_3D_SIZE 17\n0 0 nan\n", "t.cube:2:"),
            ("LUT_3D_SIZE 17\n0 0\n", "t.cube:2:"),
            ("0 0 0\n", "t.cube:1:"),
            ("LUT_3D_SIZE 17\nDOMAIN_MAX 0 1 1\n0 0 0\n", "t.cube:3:"),
            ("TITLE x\n", "t.cube:1:"),
            ("LUT_3D_SIZE 17\n0 0 0\n", "t.cube:2:"),
            ("LUT_3D_SIZE 17\n0 0 0\nTITLE late\n", "t.cube:3:"),
        ];
        for (text, prefix) in cases {
            let e = err(text);
            assert_eq!(e.code.as_str(), "L2390", "{text:?}");
            assert!(e.message.starts_with(prefix), "{text:?}: {}", e.message);
            assert!(e.help.is_some());
        }
    }

    #[test]
    fn too_many_rows_is_malformed() {
        let text = format!("{}{}0 0 0\n", header(17), body(&identity(17)));
        let e = err(&text);
        assert_eq!(e.code.as_str(), "L2390");
        assert!(e.message.starts_with("t.cube:4915:"), "{}", e.message);
    }
}
