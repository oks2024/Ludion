//! `ludion render --paired`: a contact sheet of a `kind: paired` document, one cell per phase, the
//! roles standing at their aligned pose (`Plan/paired-animations.md` decisions 33 and 34).
//!
//! A phase is a clip instant, drawn statically: each clip role carries an `Animation` started at
//! tick 0 and the cell is extracted at the phase's tick, so the posing is the one every other
//! sheet and the game use. Nothing is simulated; the sheet is an authoring look, like the report
//! [`check_paired_files`] prints, and never reaches a hash.
//!
//! The framing is the union of each role's assembly box under its `T · R_y · S` placement, padded
//! for posed limbs and normalized through the model sheet's own [`Subject`], so the lights, camera
//! and cell layout are the model sheet's.
//!
//! # Example
//! ```no_run
//! use ludion_engine::{ModelView, PairedSheetRequest, render_paired_sheet};
//! let request = PairedSheetRequest {
//!     document: std::path::Path::new("examples/gallery/voxels/choke.paired.yaml"),
//!     view: ModelView::Iso,
//!     time: None,
//!     strip: Some(6),
//!     width: 1920,
//!     height: 1080,
//! };
//! let outcome = render_paired_sheet(&request).expect("renders");
//! for phase in &outcome.phases {
//!     println!("t {} s {:?}", phase.time, phase.events);
//! }
//! ```

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math;
use ludion_core::time::{FrameIndex, SimTime, TickRate};
use ludion_ecs::Schedule;
use ludion_renderer::{FrameSize, QualityParams, RenderedImage, Renderer};
use ludion_sim::{SimConfig, SimDriver};

use crate::builtin_registry;
use crate::extract::extract_at;
use crate::model_sheet::{
    FAR, FILL_INTENSITY, FOV_Y_DEG, Grid, KEY_INTENSITY, LABEL_OFFSET, LABEL_SIZE, NEAR, Placement,
    SheetLayout, Subject, UNUSED_CELL, ViewRig, assembly_aabb, blit_cell, emit_entities, entity,
    fill, light_entity, phase_times, quantize, typed,
};
use crate::paired_doc::{PairedReport, check_paired_files};
use crate::{
    Anchor, Animation, Camera, Mesh, MeshSource, ModelView, Projection, Transform, UiText,
};

/// Fewest phases `--strip` draws.
pub const MIN_PAIRED_STRIP: u32 = 2;
/// Most phases `--strip` draws.
pub const MAX_PAIRED_STRIP: u32 = 16;
/// How much the framing box grows beyond the rest-pose boxes, as a fraction of each extent, so a
/// posed limb stays inside the cell.
const FRAMING_PAD: f32 = 0.15;

/// What to draw: a document, the view, and which phases.
#[derive(Clone, Copy, Debug)]
pub struct PairedSheetRequest<'a> {
    /// The `*.paired.yaml` file; its roles' assemblies resolve beside it.
    pub document: &'a Path,
    /// The camera angle every cell shares.
    pub view: ModelView,
    /// One instant, in seconds, to draw instead of the events.
    pub time: Option<f64>,
    /// `N` evenly spaced phases over `[0, end]` instead of the events.
    pub strip: Option<u32>,
    /// The sheet's requested width in pixels.
    pub width: u32,
    /// The sheet's requested height in pixels.
    pub height: u32,
}

/// One drawn phase.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedPhase {
    /// The instant in seconds.
    pub time: f64,
    /// The sim tick the instant quantizes to.
    pub tick: u64,
    /// The document events that land on that tick.
    pub events: Vec<String>,
}

/// A rendered paired sheet.
#[derive(Debug)]
pub struct PairedSheetOutcome {
    /// The sheet, one cell per phase.
    pub image: RenderedImage,
    /// The GPU adapter that drew it.
    pub adapter: String,
    /// The same report `check-scene` prints.
    pub report: PairedReport,
    /// The phases, in cell order.
    pub phases: Vec<PairedPhase>,
}

/// One role as the sheet places it.
struct RolePose {
    name: String,
    /// The assembly path relative to the document's directory.
    assembly: String,
    clip: Option<String>,
    offset: [f64; 3],
    yaw_deg: f64,
    scale: f64,
}

/// Everything the CPU decides before a scene is written.
struct Plan {
    report: PairedReport,
    roles: Vec<RolePose>,
    phases: Vec<PairedPhase>,
    subject: Subject,
}

fn refuse(message: String, help: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::new(code::PAIRED_RENDER_INPUT, message).with_help(help.to_string())]
}

/// Draws the document at `request.document`, one cell per phase.
///
/// # Errors
/// `L1756` for a `strip` outside 2..=16, a `time` that is not finite or lies outside `[0, end]`, or
/// both flags together; the document's own `L1752`–`L1754` and loader diagnostics; and
/// `L2003`–`L2005` from the GPU.
pub fn render_paired_sheet(
    request: &PairedSheetRequest,
) -> Result<PairedSheetOutcome, Vec<Diagnostic>> {
    let plan = plan(request)?;
    let grid = Grid::near_square(plan.phases.len() as u32);
    let layout = SheetLayout::divide(grid, request.width, request.height);
    let rig = ViewRig::compute(request.view, &plan.subject, layout.cell_w, layout.cell_h);

    let mut render_scenes = Vec::with_capacity(plan.phases.len());
    let dir = request.document.parent().unwrap_or(Path::new("."));
    for phase in &plan.phases {
        let yaml = phase_scene(&plan, phase, &rig);
        let scene = ludion_scene::check(&yaml, &builtin_registry())?;
        let world = ludion_scene::instantiate(&scene, &builtin_registry())?;
        let mut driver = SimDriver::new(world, Schedule::new(), SimConfig::default());
        driver.run_ticks(0);
        render_scenes.push(extract_at(
            driver.world(),
            (layout.cell_w, layout.cell_h),
            ludion_assets::AssetRoot::local_document_unconfined(dir),
            SimTime::from_ticks(phase.tick, TickRate::DEFAULT),
            QualityParams::FULL.particle_density,
        )?);
    }

    let renderer = Renderer::new().map_err(|d| vec![d])?;
    let adapter = format!(
        "{} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );
    let mut sheet = vec![0u8; layout.sheet_w as usize * layout.sheet_h as usize * 4];
    fill(&mut sheet, UNUSED_CELL);
    for (index, render_scene) in render_scenes.iter().enumerate() {
        let cell = renderer
            .render(
                render_scene,
                FrameSize::native(layout.cell_w, layout.cell_h),
            )
            .map_err(|d| vec![d])?;
        let row = index as u32 / layout.cols;
        let col = index as u32 % layout.cols;
        blit_cell(&mut sheet, layout.sheet_w, &cell, row, col, &layout);
    }

    Ok(PairedSheetOutcome {
        image: RenderedImage {
            width: layout.sheet_w,
            height: layout.sheet_h,
            rgba: sheet,
        },
        adapter,
        report: plan.report,
        phases: plan.phases,
    })
}

/// The document, its phases and its framing: every decision that needs no GPU.
fn plan(request: &PairedSheetRequest) -> Result<Plan, Vec<Diagnostic>> {
    let dir = request.document.parent().unwrap_or(Path::new("."));
    let rel = request
        .document
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.document.display().to_string());
    let root = ludion_assets::AssetRoot::local_document_unconfined(dir);
    let report = check_paired_files(root, &rel)?;
    let doc = &report.doc;

    let end = report
        .roles
        .iter()
        .filter_map(|role| role.duration)
        .fold(None, |longest: Option<f64>, d| {
            Some(longest.map_or(d, |l| l.max(d)))
        })
        .unwrap_or(doc.align.ticks as f64 / f64::from(TickRate::DEFAULT.hz()));
    let event_times: Vec<f64> = doc.events.iter().map(|event| event.t).collect();
    let times = phase_instants(request, end, &event_times)?;
    let phases = times
        .into_iter()
        .map(|time| {
            let tick = quantize(time, TickRate::DEFAULT);
            let events = doc
                .events
                .iter()
                .filter(|event| quantize(event.t, TickRate::DEFAULT) == tick)
                .map(|event| event.name.clone())
                .collect();
            PairedPhase { time, tick, events }
        })
        .collect();

    let mut roles = Vec::with_capacity(report.roles.len());
    let (mut low, mut high) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for role in &report.roles {
        let spec = doc
            .roles
            .iter()
            .find(|candidate| candidate.name == role.name)
            .expect("the report lists the document's roles");
        let assembly = ludion_core::paths::sibling(&rel, &role.assembly);
        let (_, document, _) = crate::assembly_mesh::load_meshed_documents(root, &assembly)?;
        let pose = RolePose {
            name: role.name.clone(),
            assembly,
            clip: role.clip.clone(),
            offset: role.offset,
            yaw_deg: role.yaw_deg,
            scale: spec.scale,
        };
        let (min, max) = assembly_aabb(&document);
        for corner in corners(min, max) {
            let world = place(&pose, corner);
            for axis in 0..3 {
                low[axis] = low[axis].min(world[axis]);
                high[axis] = high[axis].max(world[axis]);
            }
        }
        roles.push(pose);
    }
    let (min, max) = padded(low, high);
    let subject = Subject::from_box(min, max, 1.0);
    Ok(Plan {
        report,
        roles,
        phases,
        subject,
    })
}

/// The instants to draw, in seconds: the flags' choice, else one per event, else `[0]`.
fn phase_instants(
    request: &PairedSheetRequest,
    end: f64,
    events: &[f64],
) -> Result<Vec<f64>, Vec<Diagnostic>> {
    let working = "use `--strip 6` for evenly spaced phases, `--time 0.5` for one instant, or \
                   neither for the document's events";
    match (request.strip, request.time) {
        (Some(_), Some(_)) => Err(refuse(
            "`--strip` and `--time` both choose the phases".to_string(),
            working,
        )),
        (Some(n), None) if !(MIN_PAIRED_STRIP..=MAX_PAIRED_STRIP).contains(&n) => Err(refuse(
            format!("`--strip {n}` is outside {MIN_PAIRED_STRIP} to {MAX_PAIRED_STRIP} phases"),
            working,
        )),
        (Some(n), None) => Ok(phase_times(end, false, Some(n), None)),
        (None, Some(t)) if !t.is_finite() || t < 0.0 || t > end => Err(refuse(
            format!("`--time {t}` is outside the document's [0, {end}] s"),
            working,
        )),
        (None, Some(t)) => Ok(vec![t]),
        (None, None) if events.is_empty() => Ok(vec![0.0]),
        (None, None) => Ok(events.to_vec()),
    }
}

/// The eight corners of a box.
fn corners(min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 8] {
    let pick = |bit: usize, axis: usize| if bit == 0 { min[axis] } else { max[axis] };
    std::array::from_fn(|i| [pick(i & 1, 0), pick((i >> 1) & 1, 1), pick((i >> 2) & 1, 2)])
}

/// A model-space point in the reference frame: `offset + R_y(yaw) · (scale · p)`.
fn place(role: &RolePose, p: [f32; 3]) -> [f32; 3] {
    let (sin, cos) = math::sin_cos((role.yaw_deg as f32).to_radians());
    let scale = role.scale as f32;
    let (x, y, z) = (p[0] * scale, p[1] * scale, p[2] * scale);
    [
        role.offset[0] as f32 + cos * x + sin * z,
        role.offset[1] as f32 + y,
        role.offset[2] as f32 + -sin * x + cos * z,
    ]
}

/// The framing box grown for posed limbs: symmetric on the ground plane, upward only, so the feet
/// stay on the floor.
fn padded(low: [f32; 3], high: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let grow = |axis: usize| (high[axis] - low[axis]) * FRAMING_PAD;
    (
        [low[0] - grow(0) * 0.5, low[1], low[2] - grow(2) * 0.5],
        [
            high[0] + grow(0) * 0.5,
            high[1] + grow(1),
            high[2] + grow(2) * 0.5,
        ],
    )
}

/// A role's `Transform` in the sheet's normalized space (`p_norm = p · k + t`).
fn role_transform(role: &RolePose, placement: &Placement) -> Transform {
    let k = placement.scale;
    let t = placement.translation;
    Transform {
        translation: [
            role.offset[0] as f32 * k + t[0],
            role.offset[1] as f32 * k + t[1],
            role.offset[2] as f32 * k + t[2],
        ],
        rotation: [0.0, role.yaw_deg as f32, 0.0],
        scale: [role.scale as f32 * k; 3],
    }
}

/// One phase's scene text: the roles, the ground, the camera, the key and fill lights and the label.
fn phase_scene(plan: &Plan, phase: &PairedPhase, rig: &ViewRig) -> String {
    let placement = &plan.subject.placements[0];
    let mut entities = Vec::new();
    for role in &plan.roles {
        let mut components = vec![
            typed("Transform", &role_transform(role, placement)),
            typed(
                "Mesh",
                &Mesh {
                    shape: MeshSource::Assembly(role.assembly.clone()),
                    ..Mesh::default()
                },
            ),
        ];
        if let Some(clip) = &role.clip {
            components.push(typed(
                "Animation",
                &Animation {
                    clip: clip.clone(),
                    started: FrameIndex::ZERO,
                    ..Animation::default()
                },
            ));
        }
        let mut named = entity(components);
        named.name = Some(role.name.clone());
        entities.push(named);
    }
    entities.push(entity(vec![
        typed(
            "Transform",
            &Transform {
                scale: [8.0, 1.0, 8.0],
                ..Transform::default()
            },
        ),
        typed(
            "Mesh",
            &Mesh {
                shape: MeshSource::Plane,
                color: [0.5, 0.55, 0.6, 1.0],
                ..Mesh::default()
            },
        ),
    ]));
    entities.push(entity(vec![
        typed(
            "Transform",
            &Transform {
                translation: [rig.position.x, rig.position.y, rig.position.z],
                rotation: rig.rotation_deg,
                ..Transform::default()
            },
        ),
        typed(
            "Camera",
            &Camera {
                projection: Projection::Perspective {
                    fov_y_deg: FOV_Y_DEG,
                },
                near: NEAR,
                far: FAR,
                ..Camera::default()
            },
        ),
    ]));
    entities.push(light_entity(
        rig.key_position,
        [1.0, 0.97, 0.9],
        KEY_INTENSITY,
        rig.range,
    ));
    entities.push(light_entity(
        rig.fill_position,
        [0.7, 0.78, 0.95],
        FILL_INTENSITY,
        rig.range,
    ));
    let mut label = format!("t {:.2}", phase.time);
    for event in &phase.events {
        label.push(' ');
        label.push_str(event);
    }
    entities.push(entity(vec![typed(
        "UiText",
        &UiText {
            anchor: Anchor::TopLeft,
            offset: [LABEL_OFFSET, LABEL_OFFSET],
            text: label,
            size: LABEL_SIZE,
            color: [1.0, 0.9, 0.25, 1.0],
            ..UiText::default()
        },
    )]));
    emit_entities(entities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::math::glam::Vec3;

    fn choke() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gallery/voxels/choke.paired.yaml")
    }

    fn request<'a>(
        document: &'a Path,
        strip: Option<u32>,
        time: Option<f64>,
    ) -> PairedSheetRequest<'a> {
        PairedSheetRequest {
            document,
            view: ModelView::Iso,
            time,
            strip,
            width: 960,
            height: 540,
        }
    }

    fn code_of(result: Result<Plan, Vec<Diagnostic>>) -> String {
        match result {
            Ok(_) => "ok".to_string(),
            Err(found) => found[0].code.to_string(),
        }
    }

    #[test]
    fn events_are_the_default_phases() {
        let path = choke();
        let plan = plan(&request(&path, None, None)).expect("plans");
        let times: Vec<f64> = plan.phases.iter().map(|p| p.time).collect();
        assert_eq!(times, vec![0.25, 0.5, 0.9]);
        assert_eq!(plan.phases[0].events, vec!["grab".to_string()]);
        assert_eq!(plan.phases[0].tick, 15);
    }

    #[test]
    fn a_strip_ends_on_the_longest_clip() {
        let path = choke();
        let plan = plan(&request(&path, Some(4), None)).expect("plans");
        assert_eq!(plan.phases.len(), 4);
        let end = plan
            .report
            .roles
            .iter()
            .filter_map(|role| role.duration)
            .fold(0.0, f64::max);
        assert_eq!(plan.phases[3].time, end);
        assert_eq!(plan.phases[0].time, 0.0);
    }

    #[test]
    fn a_time_is_one_phase_with_its_event() {
        let path = choke();
        let plan = plan(&request(&path, None, Some(0.5))).expect("plans");
        assert_eq!(plan.phases.len(), 1);
        assert_eq!(plan.phases[0].events, vec!["hold".to_string()]);
    }

    #[test]
    fn unusable_phase_flags_are_refused_with_l1756() {
        let path = choke();
        for (strip, time) in [
            (Some(1), None),
            (Some(17), None),
            (None, Some(9.0)),
            (None, Some(-0.1)),
            (None, Some(f64::NAN)),
            (Some(4), Some(0.5)),
        ] {
            assert_eq!(
                code_of(plan(&request(&path, strip, time))),
                "L1756",
                "strip {strip:?} time {time:?}"
            );
        }
    }

    #[test]
    fn the_roles_stand_where_the_document_aligns_them() {
        let path = choke();
        let plan = plan(&request(&path, None, None)).expect("plans");
        let placement = &plan.subject.placements[0];
        let (k, t) = (placement.scale, placement.translation);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        let instigator = role_transform(&plan.roles[0], placement);
        assert_eq!(plan.roles[0].name, "instigator");
        assert!(near(instigator.scale[0], 0.125 * k));
        assert!((0..3).all(|i| near(instigator.translation[i], t[i])));
        let target = role_transform(&plan.roles[1], placement);
        assert!(near(target.translation[2], -0.5 * k + t[2]));
        assert!(near(target.translation[0], t[0]));
        assert_eq!(target.rotation, [0.0, 180.0, 0.0]);
    }

    #[test]
    fn the_sheet_and_the_report_place_a_cell_identically() {
        let path = choke();
        let plan = plan(&request(&path, None, None)).expect("plans");
        let placement = &plan.subject.placements[0];
        let target = plan
            .roles
            .iter()
            .find(|role| role.name == "target")
            .expect("target");
        let dir = path.parent().unwrap();
        let (local, world) = crate::paired_doc::first_rest_cell(
            ludion_assets::AssetRoot::local_document_unconfined(dir),
            "choke.paired.yaml",
            "target",
            "head",
        );
        let transform = role_transform(target, placement);
        let matrix = ludion_core::math::glam::Mat4::from_scale_rotation_translation(
            Vec3::from(transform.scale),
            ludion_core::math::glam::Quat::from_euler(
                ludion_core::math::glam::EulerRot::XYZ,
                transform.rotation[0].to_radians(),
                transform.rotation[1].to_radians(),
                transform.rotation[2].to_radians(),
            ),
            Vec3::from(transform.translation),
        );
        let normalized = matrix.transform_point3(local);
        let back = (normalized - Vec3::from(placement.translation)) / placement.scale;
        assert!(
            back.distance(world) < 1e-4,
            "sheet {back:?} vs report {world:?}"
        );
    }
}
