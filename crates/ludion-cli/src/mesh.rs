//! Implementation of `ludion mesh pick` and `ludion mesh apply [--check]` — the two verbs that edit
//! a mesh's texture maps (`Plan/mesh-texture-editing.md`).
//!
//! Both are batch producers over committed text, like `generate-map` and `generate-atlas`: no engine
//! process, no device, no wire method. `ludion-mesh-edit` owns every computation here — the region
//! algebra, the UV rasteriser, the derive and the ray test — and this module supplies the one thing
//! that crate deliberately does not know about: the camera `ludion render --model` draws a cell with.
//!
//! **Why `pick` rebuilds that camera rather than owning one.** The author reads a pixel off a
//! rendered sheet, so a pick is only meaningful against the exact camera that drew it. That camera is
//! already a pure function of `(view, model bounds, cell size)` — [`ludion::math::model_camera`] —
//! and [`ludion::math::camera_ray`] already turns a pixel into a ray, so the two compose into a pick
//! with nothing new in between. A second camera definition would agree today and diverge silently,
//! and every pick after that would be a few pixels off with the author blaming their region.
//!
//! **The two spaces.** A sheet cell draws the model *normalized* — scaled into a unit sphere and sat
//! on the ground — so the ray the camera produces lives in that space. The triangles are therefore
//! transformed forward before the cast, and the hit is reported back in the model's own coordinates,
//! which is the space a region in a `kind: mesh-edit` document is written in.

use std::path::Path;
use std::process::ExitCode;

use crate::diagnostic;
use ludion::math::{ModelCamera, Projection, camera_ray, model_camera, world_to_pixel};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::ModelView;
use ludion_mesh_edit::{Aabb, Derived, ray};
use ludion_scene::{MeshDocument, MeshTextures, TextureRef};
use serde::Serialize;
use serde_json::{Value, json};

/// The default cell extent, matching `ludion render`'s own `--width`/`--height` defaults: a pick
/// with no extent given answers for the sheet a bare `render --model … --views front` wrote.
pub const DEFAULT_WIDTH: u32 = 1920;
pub const DEFAULT_HEIGHT: u32 = 1080;

/// One [`SLOTS`] row: a texture slot's `map:` name and the accessor that reads it off a document.
type Slot = (&'static str, fn(&MeshTextures) -> Option<&TextureRef>);

/// The five texture slots in canonical order, each with the accessor that reads it off a document.
const SLOTS: [Slot; 5] = [
    ("base_color", |t| t.base_color.as_ref()),
    ("metallic_roughness", |t| t.metallic_roughness.as_ref()),
    ("normal", |t| t.normal.as_ref()),
    ("occlusion", |t| t.occlusion.as_ref()),
    ("emissive", |t| t.emissive.as_ref()),
];

// --- mesh pick ----------------------------------------------------------------------------------

/// Where one pick landed: the picture it was read off, the point it found, and what each referenced
/// map holds there.
#[derive(Debug, Serialize)]
struct PickReport {
    mesh: String,
    view: &'static str,
    label: &'static str,
    pixel: [f64; 2],
    extent: [u32; 2],
    position: [f64; 3],
    normalized: [f64; 3],
    distance: f64,
    triangle: u32,
    barycentric: [f64; 3],
    uv: Option<[f64; 2]>,
    maps: Vec<MapHit>,
    base_color_srgb8: Option<[u8; 4]>,
}

/// One referenced map and the texel the hit's UV lands on in it.
#[derive(Debug, Serialize)]
struct MapHit {
    map: &'static str,
    path: String,
    width: u32,
    height: u32,
    texel: Option<[u32; 2]>,
}

/// Parses a `--pixel X,Y` value. A clap value parser, so a malformed pair is a usage error rather
/// than a diagnostic — nothing has been picked yet.
pub fn parse_pixel(text: &str) -> Result<[f64; 2], String> {
    let (x, y) = text
        .split_once(',')
        .ok_or_else(|| format!("`{text}` is not a `X,Y` pixel pair"))?;
    let parse = |part: &str, axis: &str| -> Result<f64, String> {
        let value: f64 = part
            .trim()
            .parse()
            .map_err(|_| format!("`{}` is not a number ({axis} of `{text}`)", part.trim()))?;
        if !value.is_finite() || value < 0.0 {
            return Err(format!("`{part}` is not a pixel coordinate ({axis})"));
        }
        Ok(value)
    };
    Ok([parse(x, "the x")?, parse(y, "the y")?])
}

/// Runs `ludion mesh pick <mesh.yaml> --view <name> --pixel X,Y [--width] [--height] [--json]`.
pub fn run_pick(
    mesh_path: &Path,
    view: &str,
    pixel: [f64; 2],
    width: Option<u32>,
    height: Option<u32>,
    json: bool,
) -> ExitCode {
    let (width, height) = (
        width.unwrap_or(DEFAULT_WIDTH),
        height.unwrap_or(DEFAULT_HEIGHT),
    );
    let mesh = match read_mesh(mesh_path, json) {
        Ok(mesh) => mesh,
        Err(exit) => return exit,
    };
    let report = match pick(&mesh, mesh_path, view, pixel, width, height) {
        Ok(report) => report,
        Err(diagnostic) => return fail(mesh_path, std::slice::from_ref(&diagnostic), json),
    };

    if json {
        let mut value = serde_json::to_value(&report).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("ok".to_string(), json!(true));
        }
        print_json(&value);
    } else {
        print_pick(&report);
    }
    ExitCode::SUCCESS
}

/// The pick itself: rebuild the cell's camera, cast one ray at the normalized triangles, and report
/// the hit in the model's own coordinates.
fn pick(
    mesh: &MeshDocument,
    mesh_path: &Path,
    view: &str,
    pixel: [f64; 2],
    width: u32,
    height: u32,
) -> Result<PickReport, Diagnostic> {
    let view = resolve_view(view)?;
    let label = view.label();
    if pixel[0] >= f64::from(width) || pixel[1] >= f64::from(height) {
        return Err(outside_extent(pixel, width, height, label));
    }
    let camera = sheet_camera(mesh, mesh_path, view, width, height)?;
    // A pixel argument names a pixel, and a ray is cast through that pixel's centre — the half-texel
    // offset `camera_ray`'s edge-relative screen space does not add for you.
    let screen = [(pixel[0] + 0.5) as f32, (pixel[1] + 0.5) as f32];
    let Some(ray) = camera_ray(
        camera.translation,
        camera.rotation_deg,
        Projection::Perspective {
            fov_y_deg: camera.fov_y_deg,
        },
        camera.near,
        camera.far,
        screen,
        (width, height),
    ) else {
        return Err(unresolved(format!(
            "the {label} cell's camera produced no ray at pixel {}",
            pixel_text(pixel)
        ))
        .with_help("check the model's bounds and that `--width`/`--height` are both non-zero"));
    };

    let scale = f64::from(camera.normalize_scale);
    let offset = camera.normalize_translation.map(f64::from);
    let normalized: Vec<f64> = mesh
        .positions
        .iter()
        .enumerate()
        .map(|(slot, axis)| axis * scale + offset[slot % 3])
        .collect();

    let hit = ray::cast(
        ray.origin.map(f64::from),
        ray.dir.map(f64::from),
        &normalized,
        &mesh.indices,
        mesh.uvs.as_deref(),
    )
    .ok_or_else(|| missed(mesh_path, pixel, width, height, view))?;

    // Interpolated over the model-space corners rather than divided back out of the hit, so the
    // reported point is on the authored surface exactly as the region algebra will read it.
    let position = interpolate(
        &mesh.positions,
        &mesh.indices,
        hit.triangle,
        hit.barycentric,
    );
    let maps = map_hits(mesh, mesh_path, hit.uv)?;
    Ok(PickReport {
        mesh: mesh_path.display().to_string(),
        view: view.name(),
        label,
        pixel,
        extent: [width, height],
        position,
        normalized: hit.position,
        distance: hit.t,
        triangle: hit.triangle,
        barycentric: hit.barycentric,
        uv: hit.uv,
        base_color_srgb8: base_color_texel(mesh_path, &maps)?,
        maps,
    })
}

/// The camera `ludion render --model` draws `view`'s cell with, over this document's own vertex
/// bounds — the one construction `pick` and the `apply --view` projection share.
fn sheet_camera(
    mesh: &MeshDocument,
    mesh_path: &Path,
    view: ModelView,
    width: u32,
    height: u32,
) -> Result<ModelCamera, Diagnostic> {
    let Some(bounds) = Aabb::of_positions(&mesh.positions) else {
        return Err(unresolved(format!(
            "`{}` has no vertices, so there is no sheet to draw it on",
            mesh_path.display()
        ))
        .with_help("check the document's `positions:` list"));
    };
    let min = bounds.min.map(|axis| axis as f32);
    let max = bounds.max.map(|axis| axis as f32);
    Ok(model_camera(view, min, max, width, height))
}

fn resolve_view(name: &str) -> Result<ModelView, Diagnostic> {
    ModelView::from_name(name).ok_or_else(|| unknown_view(name))
}

/// A triangle's corners weighted by the hit's barycentrics, over any 3-floats-per-vertex list.
fn interpolate(positions: &[f64], indices: &[u32], triangle: u32, weights: [f64; 3]) -> [f64; 3] {
    let mut point = [0.0; 3];
    for (corner, weight) in indices
        .chunks_exact(3)
        .nth(triangle as usize)
        .unwrap_or(&[0, 0, 0])
        .iter()
        .zip(weights)
    {
        let base = (*corner as usize) * 3;
        for (axis, value) in point.iter_mut().enumerate() {
            *value += positions.get(base + axis).copied().unwrap_or(0.0) * weight;
        }
    }
    point
}

/// Every map the document references, with the texel the hit's UV lands on. A map that cannot be
/// read is an error: a document naming a file that is not there is broken whatever the pick found.
fn map_hits(
    mesh: &MeshDocument,
    mesh_path: &Path,
    uv: Option<[f64; 2]>,
) -> Result<Vec<MapHit>, Diagnostic> {
    let dir = mesh_path.parent().unwrap_or_else(|| Path::new("."));
    let mut hits = Vec::new();
    for (map, read) in SLOTS {
        let Some(reference) = read(&mesh.textures) else {
            continue;
        };
        let image = ludion_assets::load_png(
            ludion_assets::AssetRoot::local_document_unconfined(dir),
            &reference.path,
        )
        .map_err(|mut error| {
            error.message = format!("the mesh's `{map}` map: {}", error.message);
            error
        })?;
        hits.push(MapHit {
            map,
            path: reference.path.clone(),
            width: image.width,
            height: image.height,
            texel: uv.map(|uv| texel(uv, image.width, image.height)),
        });
    }
    Ok(hits)
}

/// The texel a UV samples: `v` runs downward from the top row (glTF's convention and `Image.rgba`'s
/// row order), and a UV outside `[0, 1]` clamps to the edge rather than reporting a texel the map
/// does not have.
fn texel(uv: [f64; 2], width: u32, height: u32) -> [u32; 2] {
    let axis = |value: f64, extent: u32| -> u32 {
        let scaled = (value * f64::from(extent)).floor();
        if !scaled.is_finite() || scaled < 0.0 {
            return 0;
        }
        (scaled as u32).min(extent.saturating_sub(1))
    };
    [axis(uv[0], width), axis(uv[1], height)]
}

/// The four sRGB bytes under the hit in the base-colour map, when the document has one.
fn base_color_texel(mesh_path: &Path, maps: &[MapHit]) -> Result<Option<[u8; 4]>, Diagnostic> {
    let Some(hit) = maps.iter().find(|hit| hit.map == "base_color") else {
        return Ok(None);
    };
    let Some(texel) = hit.texel else {
        return Ok(None);
    };
    let dir = mesh_path.parent().unwrap_or_else(|| Path::new("."));
    let image = ludion_assets::load_png(
        ludion_assets::AssetRoot::local_document_unconfined(dir),
        &hit.path,
    )?;
    let base = ((texel[1] as usize) * (image.width as usize) + texel[0] as usize) * 4;
    Ok(image
        .rgba
        .get(base..base + 4)
        .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]]))
}

fn print_pick(report: &PickReport) {
    println!(
        "{}  pixel {} of {}x{}",
        report.label,
        pixel_text(report.pixel),
        report.extent[0],
        report.extent[1]
    );
    println!("  position   {}  (model space)", vec_text(report.position));
    println!(
        "  normalized {}  (what the sheet drew)",
        vec_text(report.normalized)
    );
    let uv = report.uv.map_or_else(
        || "none".to_string(),
        |uv| format!("[{}, {}]", uv[0], uv[1]),
    );
    println!(
        "  triangle   {}  barycentric {}  uv {uv}",
        report.triangle,
        vec_text(report.barycentric)
    );
    for hit in &report.maps {
        let texel = hit.texel.map_or_else(
            || "no uvs".to_string(),
            |t| format!("texel {},{}", t[0], t[1]),
        );
        let colour = match (hit.map, report.base_color_srgb8) {
            ("base_color", Some([r, g, b, a])) => format!("  srgb8 {r},{g},{b},{a}"),
            _ => String::new(),
        };
        println!(
            "  {:<18} {}  {}x{}  {texel}{colour}",
            hit.map, hit.path, hit.width, hit.height
        );
    }
    // The point, already spelled as the `at:` of a region, so placing one is a copy and a radius.
    println!("  at:");
    for axis in report.position {
        println!("    - {axis}");
    }
}

fn pixel_text(pixel: [f64; 2]) -> String {
    format!("{},{}", pixel[0], pixel[1])
}

fn vec_text(value: [f64; 3]) -> String {
    format!("[{}, {}, {}]", value[0], value[1], value[2])
}

// --- mesh apply ---------------------------------------------------------------------------------

/// `--view`: the sheet cell each op's painted box is projected into.
pub struct ApplyView {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// An op's painted box in a cell's pixels: inclusive spans, `clipped` when the box left the cell.
#[derive(Debug, PartialEq, Serialize)]
struct ViewBox {
    x: [u32; 2],
    y: [u32; 2],
    clipped: bool,
}

/// The resolved `--view`: the cell it names and one projected box per op, in the report's order.
struct ViewProjection {
    name: &'static str,
    label: &'static str,
    extent: [u32; 2],
    boxes: Vec<Option<ViewBox>>,
}

/// Projects `bounds` (model space) into the cell `camera` draws: its eight corners are normalized
/// exactly as the sheet normalizes the model, sent through [`world_to_pixel`], and hulled. `None`
/// when a corner cannot be projected.
fn view_box(bounds: &Aabb, camera: &ModelCamera, width: u32, height: u32) -> Option<ViewBox> {
    let scale = f64::from(camera.normalize_scale);
    let offset = camera.normalize_translation.map(f64::from);
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for corner in 0..8u8 {
        let point: [f32; 3] = std::array::from_fn(|axis| {
            let edge = if corner >> axis & 1 == 0 {
                bounds.min[axis]
            } else {
                bounds.max[axis]
            };
            (edge * scale + offset[axis]) as f32
        });
        let pixel = world_to_pixel(
            camera.translation,
            camera.rotation_deg,
            Projection::Perspective {
                fov_y_deg: camera.fov_y_deg,
            },
            camera.near,
            camera.far,
            point,
            (width, height),
        )?;
        for axis in 0..2 {
            lo[axis] = lo[axis].min(pixel[axis]);
            hi[axis] = hi[axis].max(pixel[axis]);
        }
    }

    // Inclusive pixel span: every pixel the box overlaps, `floor(lo)..=ceil(hi) - 1`. A box wholly
    // outside the cell reports the nearest edge pixel, with `clipped` set.
    let mut clipped = false;
    let mut span = |lo: f32, hi: f32, extent: u32| -> [u32; 2] {
        let first = lo.floor() as i64;
        let last = (hi.ceil() as i64 - 1).max(first);
        let end = i64::from(extent) - 1;
        clipped |= first < 0 || last > end;
        [first.clamp(0, end) as u32, last.clamp(0, end) as u32]
    };
    Some(ViewBox {
        x: span(lo[0], hi[0], width),
        y: span(lo[1], hi[1], height),
        clipped,
    })
}

/// Resolves `--view` and projects every op's painted box, before anything is written.
fn project_ops(
    edit_path: &Path,
    derived: &Derived,
    view: &ApplyView,
) -> Result<ViewProjection, Vec<Diagnostic>> {
    let model_view = resolve_view(&view.name).map_err(|d| vec![d])?;
    let label = model_view.label();
    if view.width == 0 || view.height == 0 {
        return Err(vec![
            unresolved(format!(
                "the {label} cell cannot be {}x{}",
                view.width, view.height
            ))
            .with_help("pass non-zero `--width`/`--height`"),
        ]);
    }
    let mesh = ludion_scene::check_mesh(derived.document())?;
    let camera =
        sheet_camera(&mesh, edit_path, model_view, view.width, view.height).map_err(|d| vec![d])?;
    let boxes = derived
        .report()
        .ops
        .iter()
        .map(|op| {
            op.bounds
                .as_ref()
                .and_then(|bounds| view_box(bounds, &camera, view.width, view.height))
        })
        .collect();
    Ok(ViewProjection {
        name: model_view.name(),
        label,
        extent: [view.width, view.height],
        boxes,
    })
}

/// One text line per op for `--view`, indexed like the report's ops.
fn view_line(projection: &ViewProjection, report_op: &ludion_mesh_edit::OpReport) -> String {
    let [width, height] = projection.extent;
    match projection.boxes.get(report_op.index) {
        Some(Some(b)) => format!(
            "  op {} covers x {}..{}, y {}..{} of a {width}x{height} {} cell{}",
            report_op.index,
            b.x[0],
            b.x[1],
            b.y[0],
            b.y[1],
            projection.label,
            if b.clipped {
                " (clipped to the cell)"
            } else {
                ""
            }
        ),
        _ if report_op.bounds.is_none() => {
            format!("  op {} has no painted surface to project", report_op.index)
        }
        _ => format!(
            "  op {} could not be projected into the {} cell",
            report_op.index, projection.label
        ),
    }
}

/// Adds the `--view` keys to a serialized report: a top-level `view` and each op's `view_box`.
fn annotate_view(value: &mut Value, projection: Option<&ViewProjection>) {
    let (Some(projection), Some(object)) = (projection, value.as_object_mut()) else {
        return;
    };
    object.insert(
        "view".to_string(),
        json!({
            "name": projection.name,
            "label": projection.label,
            "extent": projection.extent,
        }),
    );
    if let Some(ops) = object.get_mut("ops").and_then(Value::as_array_mut) {
        for (op, view_box) in ops.iter_mut().zip(&projection.boxes) {
            if let Some(op) = op.as_object_mut() {
                op.insert(
                    "view_box".to_string(),
                    serde_json::to_value(view_box).unwrap_or(Value::Null),
                );
            }
        }
    }
}

/// Runs `ludion mesh apply <edit.yaml> [--check] [--json] [--view NAME]`: derive the edit's whole
/// tree and either write it or byte-compare it, the shape `generate-atlas` established.
pub fn run_apply(edit_path: &Path, check: bool, json: bool, view: Option<ApplyView>) -> ExitCode {
    let derived = match ludion_mesh_edit::derive(edit_path) {
        Ok(derived) => derived,
        Err(diagnostics) => return fail(edit_path, &diagnostics, json),
    };
    let projection = match view {
        Some(view) => match project_ops(edit_path, &derived, &view) {
            Ok(projection) => Some(projection),
            Err(diagnostics) => return fail(edit_path, &diagnostics, json),
        },
        None => None,
    };
    if check {
        check_tree(edit_path, &derived, json, projection.as_ref())
    } else {
        write_tree(edit_path, &derived, json, projection.as_ref())
    }
}

fn write_tree(
    edit_path: &Path,
    derived: &Derived,
    json: bool,
    projection: Option<&ViewProjection>,
) -> ExitCode {
    let written = match derived.write() {
        Ok(written) => written,
        Err(diagnostics) => return fail(edit_path, &diagnostics, json),
    };
    if json {
        let mut value = serde_json::to_value(derived.report()).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("ok".to_string(), json!(true));
            object.insert("out_dir".to_string(), json!(path_text(derived.out_dir())));
            object.insert(
                "written".to_string(),
                json!(written.iter().map(|p| path_text(p)).collect::<Vec<_>>()),
            );
        }
        annotate_view(&mut value, projection);
        print_json(&value);
        return ExitCode::SUCCESS;
    }

    let report = derived.report();
    println!(
        "{}: wrote {} ({} file{})",
        path_text(edit_path),
        path_text(derived.out_dir()),
        written.len(),
        if written.len() == 1 { "" } else { "s" }
    );
    for op in &report.ops {
        let bounds = op.bounds.map_or_else(
            || "no bounds".to_string(),
            |b| format!("{}..{}", vec_text(b.min), vec_text(b.max)),
        );
        println!(
            "  op {} {} {}: {} texel{} ({:.3}% of the map), {bounds}, {} triangle{}",
            op.index,
            op.op,
            op.map,
            op.texels,
            if op.texels == 1 { "" } else { "s" },
            op.share * 100.0,
            op.triangles,
            if op.triangles == 1 { "" } else { "s" }
        );
        if let Some(projection) = projection {
            println!("{}", view_line(projection, op));
        }
    }
    for map in &report.maps {
        println!(
            "  map {} -> {} ({}x{}, {} non-black texel{}, {})",
            map.map,
            map.path,
            map.width,
            map.height,
            map.non_black_texels,
            if map.non_black_texels == 1 { "" } else { "s" },
            if map.created { "created" } else { "repainted" }
        );
    }
    println!("  next: {}", report.next);
    ExitCode::SUCCESS
}

fn check_tree(
    edit_path: &Path,
    derived: &Derived,
    json: bool,
    projection: Option<&ViewProjection>,
) -> ExitCode {
    let drifted = derived.check();
    if json {
        let mut value = serde_json::to_value(derived.report()).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("ok".to_string(), json!(drifted.is_empty()));
            object.insert("out_dir".to_string(), json!(path_text(derived.out_dir())));
            object.insert(
                "drift".to_string(),
                json!(
                    drifted
                        .iter()
                        .map(|d| d.message.clone())
                        .collect::<Vec<_>>()
                ),
            );
        }
        annotate_view(&mut value, projection);
        print_json(&value);
        return if drifted.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    if drifted.is_empty() {
        println!(
            "{}: up to date -> {} ({} file{})",
            path_text(edit_path),
            path_text(derived.out_dir()),
            derived.files().len(),
            if derived.files().len() == 1 { "" } else { "s" }
        );
        if let Some(projection) = projection {
            for op in &derived.report().ops {
                println!("{}", view_line(projection, op));
            }
        }
        return ExitCode::SUCCESS;
    }
    diagnostic::report(edit_path, &drifted)
}

// --- shared -------------------------------------------------------------------------------------

fn read_mesh(path: &Path, json: bool) -> Result<MeshDocument, ExitCode> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        let diagnostic = Diagnostic::new(
            code::SCENE_IO,
            format!("could not read `{}`: {error}", path.display()),
        )
        .with_help("check the path (relative to the current directory) and permissions");
        fail(path, std::slice::from_ref(&diagnostic), json)
    })?;
    ludion_scene::check_mesh(&text).map_err(|diagnostics| fail(path, &diagnostics, json))
}

/// A runtime failure: the diagnostics to stderr as always and, under `--json`, one
/// `{ok: false, path, diagnostics}` object on stdout, so a consumer parsing stdout always gets
/// one object. A usage error never reaches here.
fn fail(path: &Path, diagnostics: &[Diagnostic], json: bool) -> ExitCode {
    diagnostic::report(path, diagnostics);
    if json {
        let fields = json!({ "path": path_text(path) });
        print_json(&Value::Object(diagnostic::json_envelope(
            false,
            fields,
            diagnostics,
        )));
    }
    ExitCode::FAILURE
}

/// Every path a `mesh` verb prints, in forward-slash spelling on every OS — a path built from a
/// document-relative string joined onto a platform path mixes separators on Windows, and neither a
/// `--json` consumer nor a reader of the text report should have to notice which.
fn path_text(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Prints one JSON object on stdout, compact on one line with a trailing newline.
fn print_json(value: &Value) {
    let mut text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    text.push('\n');
    print!("{text}");
}

fn unresolved(message: String) -> Diagnostic {
    Diagnostic::new(code::MESH_EDIT_PICK_UNRESOLVED, message)
}

fn unknown_view(view: &str) -> Diagnostic {
    let names: Vec<String> = ModelView::ALL
        .iter()
        .map(|view| format!("{} ({})", view.name(), view.label()))
        .collect();
    unresolved(format!(
        "`--view {view}` names no view the model sheet draws"
    ))
    .with_help(format!("the views are {}", names.join(", ")))
}

fn outside_extent(pixel: [f64; 2], width: u32, height: u32, label: &str) -> Diagnostic {
    unresolved(format!(
        "pixel {} is outside the {width}x{height} {label} cell this pick assumed",
        pixel_text(pixel)
    ))
    .with_help(
        "pass the `--width`/`--height` the sheet was rendered at; a pick reads a single-cell image, \
         so render it with `--views <one view>`",
    )
}

fn missed(
    mesh_path: &Path,
    pixel: [f64; 2],
    width: u32,
    height: u32,
    view: ModelView,
) -> Diagnostic {
    unresolved(format!(
        "no triangle of `{}` lies under pixel {} of the {width}x{height} {} cell",
        mesh_path.display(),
        pixel_text(pixel),
        view.label()
    ))
    .with_help(format!(
        "the ray passed beside the model or through a hole in it; re-read the pixel off \
         `ludion render --model {} --views {} --width {width} --height {height}`",
        mesh_path.display(),
        view.name()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_pair_parses_and_rejects() {
        assert_eq!(parse_pixel("960,540"), Ok([960.0, 540.0]));
        assert_eq!(parse_pixel(" 12 , 7.5 "), Ok([12.0, 7.5]));
        assert!(parse_pixel("960").is_err());
        assert!(parse_pixel("960,x").is_err());
        assert!(parse_pixel("-1,0").is_err());
    }

    /// `v` runs downward and a UV past the edge clamps rather than naming a texel off the map.
    #[test]
    fn a_uv_resolves_to_a_texel() {
        assert_eq!(texel([0.0, 0.0], 64, 64), [0, 0]);
        assert_eq!(texel([0.5, 0.5], 64, 64), [32, 32]);
        assert_eq!(texel([1.0, 1.0], 64, 64), [63, 63]);
        assert_eq!(texel([2.5, -0.5], 64, 32), [63, 0]);
    }

    /// A box round the model's centre projects to a span holding the cell's centre pixel.
    #[test]
    fn a_box_at_the_origin_projects_round_the_cell_centre() {
        let camera = model_camera(
            ModelView::Front,
            [-1.0, 0.0, -1.0],
            [1.0, 2.0, 1.0],
            400,
            300,
        );
        let target = Aabb {
            min: [-0.1, 0.9, -0.1],
            max: [0.1, 1.1, 0.1],
        };
        let boxed = view_box(&target, &camera, 400, 300).expect("in front of the camera");
        assert!(!boxed.clipped);
        assert!(boxed.x[0] <= 200 && 200 <= boxed.x[1], "{boxed:?}");
        assert!(boxed.y[0] <= 150 && 150 <= boxed.y[1], "{boxed:?}");

        let huge = Aabb {
            min: [-9.0, -9.0, -1.0],
            max: [9.0, 9.0, 1.0],
        };
        assert!(view_box(&huge, &camera, 400, 300).unwrap().clipped);
    }

    #[test]
    fn barycentrics_interpolate_over_the_named_triangle() {
        let positions = [0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0];
        let indices = [0, 1, 2];
        assert_eq!(
            interpolate(&positions, &indices, 0, [0.5, 0.25, 0.25]),
            [0.5, 0.5, 0.0]
        );
    }
}
