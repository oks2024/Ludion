//! Implementation of `ludion lut <identity|bake|apply|stats>` — authoring tools for the `.cube`
//! 3D LUT that `PostProcess.lut` applies (`Plan/post-film-and-screen-effects.md` decision 14).
//!
//! All four are pure CPU batch verbs over committed files (no GPU, no sim). Each takes `--json`,
//! which prints exactly one compact JSON object on one stdout line: `{ok, …the verb's fields,
//! diagnostics}`. Diagnostics always render to stderr through [`crate::diagnostic`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_assets::cube::{CubeLut, identity, load_cube};
use ludion_assets::{AssetRoot, encode_png, load_png};
use ludion_core::atomic::write_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_renderer::grade_model::{grade, is_identity, linear_to_srgb, srgb_to_linear};
use serde_json::{Value, json};

use crate::diagnostic;

/// The lattice sizes a `.cube` may have.
const SIZES: [u32; 3] = [17, 33, 65];

/// The six colour bands `stats` measures, as sRGB-encoded colours.
const BANDS: [(&str, [f32; 3]); 6] = [
    ("amber", [1.0, 0.69, 0.0]),
    ("cyan", [0.0, 0.72, 0.92]),
    ("green", [0.2, 0.75, 0.25]),
    ("magenta", [0.85, 0.2, 0.75]),
    ("skin", [0.88, 0.67, 0.55]),
    ("neutral", [0.5, 0.5, 0.5]),
];

/// Ends a verb: renders `diagnostics` to stderr and, under `--json`, prints the one object.
fn finish(json: bool, ok: bool, fields: Value, diagnostics: &[Diagnostic]) -> ExitCode {
    for d in diagnostics {
        diagnostic::print(d);
    }
    if json {
        let object = diagnostic::json_envelope(ok, fields, diagnostics);
        println!("{}", Value::Object(object));
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn fail(json: bool, diagnostics: &[Diagnostic]) -> ExitCode {
    finish(json, false, Value::Null, diagnostics)
}

fn bad_size(size: u32) -> Diagnostic {
    Diagnostic::new(
        code::CUBE_LUT_UNSUPPORTED,
        format!("`--size {size}` is not supported (17, 33 or 65)"),
    )
    .with_help("pass `--size 17`, `--size 33` or `--size 65`")
}

fn write_failure(path: &Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::OUTPUT_IO,
        format!("could not write `{}`: {error}", path.display()),
    )
    .with_help(
        "check that the destination directory is writable and the file is not open in another process",
    )
}

/// Splits `path` into the directory it is read relative to and its file name.
fn split(path: &Path) -> (PathBuf, String) {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (dir, name)
}

fn load_lut(path: &Path) -> Result<CubeLut, Diagnostic> {
    let (dir, name) = split(path);
    load_cube(AssetRoot::content(&dir), &name)
}

fn write_lut(lut: &CubeLut, out: &Path) -> Result<(), Diagnostic> {
    write_atomic(out, lut.to_text()).map_err(|e| write_failure(out, &e))
}

/// Runs `ludion lut identity`.
pub fn run_identity(size: u32, out: &Path, json: bool) -> ExitCode {
    if !SIZES.contains(&size) {
        return fail(json, &[bad_size(size)]);
    }
    if let Err(d) = write_lut(&identity(size), out) {
        return fail(json, &[d]);
    }
    if !json {
        println!("wrote {} ({size}³ identity)", out.display());
    }
    finish(
        json,
        true,
        json!({ "out": out.display().to_string(), "size": size }),
        &[],
    )
}

/// Runs `ludion lut bake`.
pub fn run_bake(scene: &Path, out: &Path, size: u32, json: bool) -> ExitCode {
    if !SIZES.contains(&size) {
        return fail(json, &[bad_size(size)]);
    }
    let post = match scene_post(scene) {
        Ok(post) => post,
        Err(diagnostics) => return fail(json, &diagnostics),
    };
    let mut lut = bake(&post, size);
    lut.title = out.file_stem().map(|s| s.to_string_lossy().into_owned());
    if let Err(d) = write_lut(&lut, out) {
        return fail(json, &[d]);
    }
    if !json {
        println!(
            "wrote {} ({size}³, baked from {})",
            out.display(),
            scene.display()
        );
        eprintln!(
            "note: the LUT runs after the scene's own grade, so reset `PostProcess`'s temperature, \
             tint, lift, gamma, gain, contrast and saturation to identity and point `lut` at this \
             file to reproduce the grade"
        );
    }
    finish(
        json,
        true,
        json!({
            "scene": scene.display().to_string(),
            "out": out.display().to_string(),
            "size": size,
        }),
        &[],
    )
}

/// The scene's post settings (the default imaging when it authors no `PostProcess`).
fn scene_post(scene: &Path) -> Result<ludion_renderer::PostData, Vec<Diagnostic>> {
    let engine = ludion_engine::Engine::builder()
        .patterns()
        .load_scene_file(scene)
        .map_err(|d| vec![d])?;
    let root = scene.parent().unwrap_or(Path::new("."));
    let extracted = ludion_engine::extract(engine.world(), (1920, 1080), AssetRoot::content(root))?;
    Ok(extracted.post.unwrap_or_default())
}

/// The scene grade sampled on the lattice: each point is an sRGB-encoded colour, taken to linear,
/// graded, and re-encoded (clamped, because the output store clamps). An identity grade bakes the
/// exact identity table.
fn bake(post: &ludion_renderer::PostData, size: u32) -> CubeLut {
    let mut lut = identity(size);
    if is_identity(post) {
        return lut;
    }
    for row in &mut lut.rgb {
        let linear = row.map(srgb_to_linear);
        *row = grade(linear, post).map(|c| linear_to_srgb(c).clamp(0.0, 1.0));
    }
    lut
}

/// Runs `ludion lut apply`.
pub fn run_apply(lut: &Path, image: &Path, out: &Path, strength: f32, json: bool) -> ExitCode {
    if !(0.0..=1.0).contains(&strength) {
        return diagnostic::usage_error(format!(
            "`--strength {strength}` is outside [0, 1]: pass 0 for the image unchanged up to 1 for the LUT's colours"
        ));
    }
    let table = match load_lut(lut) {
        Ok(t) => t,
        Err(d) => return fail(json, &[d]),
    };
    let (dir, name) = split(image);
    let mut img = match load_png(AssetRoot::content(&dir), &name) {
        Ok(i) => i,
        Err(d) => return fail(json, &[d]),
    };
    for px in img.rgba.chunks_exact_mut(4) {
        let encoded = [px[0], px[1], px[2]].map(|b| f32::from(b) / 255.0);
        let sampled = table.sample(encoded);
        for c in 0..3 {
            let mixed = encoded[c] * (1.0 - strength) + sampled[c] * strength;
            px[c] = (mixed.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    let bytes = match encode_png(img.width, img.height, &img.rgba) {
        Ok(b) => b,
        Err(e) => {
            return fail(
                json,
                &[Diagnostic::new(
                    code::OUTPUT_IO,
                    format!("could not encode `{}`: {e}", out.display()),
                )],
            );
        }
    };
    if let Err(error) = write_atomic(out, bytes) {
        return fail(json, &[write_failure(out, &error)]);
    }
    if !json {
        println!(
            "wrote {} ({} through {})",
            out.display(),
            image.display(),
            lut.display()
        );
    }
    finish(
        json,
        true,
        json!({
            "in": image.display().to_string(),
            "out": out.display().to_string(),
            "lut": lut.display().to_string(),
            "size": table.size,
            "strength": strength,
        }),
        &[],
    )
}

/// Hue (degrees, `[0, 360)`), saturation and lightness of an encoded colour.
fn hsl(c: [f32; 3]) -> (f32, f32, f32) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    (h * 60.0, s, l)
}

struct BandShift {
    band: &'static str,
    input: [f32; 3],
    output: [f32; 3],
    d_hue: Option<f32>,
    d_sat: f32,
    d_light: f32,
}

fn band_shift(band: &'static str, input: [f32; 3], lut: &CubeLut) -> BandShift {
    let output = lut.sample(input);
    let (h0, s0, l0) = hsl(input);
    let (h1, s1, l1) = hsl(output);
    let d_hue = (s0 >= 0.05 && s1 >= 0.05).then(|| {
        let mut d = (h1 - h0).rem_euclid(360.0);
        if d > 180.0 {
            d -= 360.0;
        }
        d
    });
    BandShift {
        band,
        input,
        output,
        d_hue,
        d_sat: s1 - s0,
        d_light: l1 - l0,
    }
}

/// Runs `ludion lut stats`.
pub fn run_stats(lut: &Path, json: bool) -> ExitCode {
    let table = match load_lut(lut) {
        Ok(t) => t,
        Err(d) => return fail(json, &[d]),
    };
    let shifts: Vec<BandShift> = BANDS
        .iter()
        .map(|&(band, input)| band_shift(band, input, &table))
        .collect();
    if json {
        let bands: Vec<Value> = shifts
            .iter()
            .map(|s| {
                json!({
                    "band": s.band,
                    "in": s.input,
                    "out": s.output,
                    "d_hue": s.d_hue,
                    "d_sat": s.d_sat,
                    "d_light": s.d_light,
                })
            })
            .collect();
        return finish(
            true,
            true,
            json!({ "lut": lut.display().to_string(), "size": table.size, "bands": bands }),
            &[],
        );
    }
    println!(
        "{} ({}³): shifts of six bands, HSL in the encoded sRGB domain",
        lut.display(),
        table.size
    );
    println!(
        "{:<8} {:>9} {:>8} {:>8}",
        "band", "d_hue", "d_sat", "d_light"
    );
    for s in &shifts {
        let hue = s
            .d_hue
            .map_or_else(|| "-".to_string(), |h| format!("{h:+.1}°"));
        println!(
            "{:<8} {:>9} {:>+8.3} {:>+8.3}",
            s.band, hue, s.d_sat, s.d_light
        );
    }
    ExitCode::SUCCESS
}
