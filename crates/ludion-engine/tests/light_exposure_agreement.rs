//! "Looks lit = is lit": the `irradiance` debug view of `examples/gallery/light-exposure.yaml` against
//! `light_exposure` with `Facing::Normal([0, 1, 0])` over a 32x32 grid of floor points
//! (`Plan/light-exposure.md` decision 19). The tolerances are the request's, fixed — not a per-OS floor:
//! both sides run one formula, so a calibrated floor would only hide drift.

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::math::glam::Vec3;
use ludion_engine::{
    AntiAliasing, Combine, DebugCategories, DebugView, Engine, ExposureQuery, FrameSize, Name,
    QualitySetting, RenderOutcome, SpotLight, Transform, builtin_registry, light_exposure,
    render_scene_with_registry,
};
use ludion_physics::QueryFilter;
use ludion_renderer::IRRADIANCE_VIEW_SCALE;

/// Square, so the orthographic mapping between pixel and floor position is exact.
const SIZE: u32 = 512;
/// The floor's extent in metres: the scene camera's orthographic `height`.
const FLOOR: f32 = 16.0;
/// Samples per side.
const GRID: u32 = 32;
/// Pixels between samples (half a metre of floor); each sample is the pixel at the middle of its cell.
const STEP: u32 = SIZE / GRID;
/// Radius at which a sample's neighbours are probed for a change of lighting status.
const EDGE: f32 = 0.25;
/// The request's lit/unlit threshold.
const LIT: f32 = 0.25;

fn scene_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/light-exposure.yaml")
}

/// Linear value of an sRGB byte (IEC 61966-2-1).
fn decode(byte: u8) -> f32 {
    let v = f32::from(byte) / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ludion_core::math::powf((v + 0.055) / 1.055, 2.4)
    }
}

/// The top-down camera looks down -Y with screen-up along -Z: pixel x grows with world x, pixel y
/// with world z. `pixel` is a pixel centre.
fn floor_point(px: f32, py: f32) -> [f32; 3] {
    let scale = FLOOR / SIZE as f32;
    [px * scale - FLOOR / 2.0, 0.0, py * scale - FLOOR / 2.0]
}

fn pixel_of(p: [f32; 3]) -> (u32, u32) {
    let scale = SIZE as f32 / FLOOR;
    (
        ((p[0] + FLOOR / 2.0) * scale) as u32,
        ((p[2] + FLOOR / 2.0) * scale) as u32,
    )
}

struct Cpu {
    engine: Engine,
    filter: QueryFilter,
    /// The spots' `(position, unit axis, inner, outer)`.
    spots: Vec<(Vec3, Vec3, f32, f32)>,
    /// The wall's footprint `[x0, x1, z0, z1]`: the cube mesh is its scale across.
    wall: [f32; 4],
}

impl Cpu {
    fn load() -> Cpu {
        let mut engine = Engine::builder().build().expect("boots");
        let text = std::fs::read_to_string(scene_path()).expect("the gallery scene is readable");
        engine.load_inline_scene(&text).expect("the scene loads");
        // The solver sees the colliders from the tick after they load.
        engine.advance_live();
        engine.advance_live();
        let world = engine.world_mut();
        let spot_ids: Vec<_> = world
            .query_ids::<(&SpotLight,)>()
            .map(|(id, (spot,))| (id, *spot))
            .collect();
        let spots = spot_ids
            .into_iter()
            .map(|(id, spot)| {
                let at = world
                    .get::<Transform>(id)
                    .unwrap()
                    .expect("a spot has a Transform")
                    .translation;
                (
                    Vec3::from(at),
                    Vec3::from(spot.direction).normalize(),
                    spot.inner_angle,
                    spot.outer_angle,
                )
            })
            .collect();
        let (wall_id, _) = world
            .query_ids::<(&Name,)>()
            .find(|(_, (name,))| name.0 == "wall")
            .expect("the scene has a wall");
        let t = world
            .get::<Transform>(wall_id)
            .unwrap()
            .expect("a Transform");
        let (c, s) = (t.translation, t.scale);
        let wall = [
            c[0] - s[0] / 2.0,
            c[0] + s[0] / 2.0,
            c[2] - s[2] / 2.0,
            c[2] + s[2] / 2.0,
        ];
        Cpu {
            engine,
            filter: QueryFilter::new().only_fixed(),
            spots,
            wall,
        }
    }

    fn read(&self, p: [f32; 3]) -> ludion_engine::Exposure {
        light_exposure(
            self.engine.world(),
            &ExposureQuery {
                points: &[p],
                facing: ludion_engine::Facing::Normal([0.0, 1.0, 0.0]),
                occluders: &self.filter,
                combine: Combine::Max,
            },
        )
        .expect("the query is well formed")
    }

    /// Each reaching light and whether its shadow ray was blocked.
    fn statuses(&self, p: [f32; 3]) -> Vec<(EntityId, bool)> {
        self.read(p)
            .lights
            .iter()
            .map(|l| (l.light, l.occluded))
            .collect()
    }
}

const RING: [(f32, f32); 8] = [
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (1.0, 1.0),
    (1.0, -1.0),
    (-1.0, 1.0),
    (-1.0, -1.0),
];

fn ring(p: [f32; 3]) -> impl Iterator<Item = [f32; 3]> {
    RING.into_iter()
        .map(move |(dx, dz)| [p[0] + dx * EDGE, 0.0, p[2] + dz * EDGE])
}

#[test]
fn needs_gpu_the_irradiance_view_agrees_with_light_exposure() {
    let outcome = match render_scene_with_registry(
        &builtin_registry(),
        &scene_path(),
        FrameSize::native(SIZE, SIZE),
        &QualitySetting::full(),
        false,
        &[],
        0,
        &DebugCategories::None,
        Some(DebugView::Irradiance),
        AntiAliasing::Off,
        None,
    ) {
        Ok(outcome) => outcome,
        Err(errors) if ludion_core::require_gpu::is_no_adapter(&errors) => {
            eprintln!("[light_exposure_agreement] skipped: no GPU adapter");
            return;
        }
        Err(errors) => panic!("[light_exposure_agreement] render failed: {errors:?}"),
    };
    let RenderOutcome {
        image, debug_view, ..
    } = outcome;
    assert_eq!(debug_view, Some(DebugView::Irradiance), "the view drew");
    assert_eq!((image.width, image.height), (SIZE, SIZE));

    // `Rec.709 luminance of E`, `E` decoded from the stored `sRGB(E / scale)`.
    let gpu_at = |x: u32, y: u32| -> (f32, bool) {
        let i = ((y * SIZE + x) * 4) as usize;
        let px = &image.rgba[i..i + 3];
        let e = [
            decode(px[0]) * IRRADIANCE_VIEW_SCALE,
            decode(px[1]) * IRRADIANCE_VIEW_SCALE,
            decode(px[2]) * IRRADIANCE_VIEW_SCALE,
        ];
        let luminance = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
        (luminance, px.contains(&255))
    };
    let cpu = Cpu::load();

    // The axis orientation, proved rather than assumed: the hard-edged spot's axis point is lit and a
    // point one metre outside its outer cone is dark.
    let (hard_at, _, _, outer) = cpu
        .spots
        .iter()
        .copied()
        .find(|(_, _, inner, outer)| inner == outer)
        .expect("the scene has a hard-edged spot");
    let axis = [hard_at.x, 0.0, hard_at.z];
    let rim = hard_at.y * ludion_core::math::tan(outer.to_radians());
    let outside = [axis[0], 0.0, axis[2] + rim + 1.0];
    let (ax, ay) = pixel_of(axis);
    let (ox, oy) = pixel_of(outside);
    assert!(
        gpu_at(ax, ay).0 >= LIT,
        "the hard spot's axis renders lit: the pixel mapping is wrong"
    );
    assert!(
        gpu_at(ox, oy).0 < LIT,
        "a point outside the hard spot's cone renders dark: the pixel mapping is wrong"
    );

    let (spots, wall) = (&cpu.spots, cpu.wall);
    let near_wall = |p: [f32; 3]| {
        p[0] > wall[0] - EDGE
            && p[0] < wall[1] + EDGE
            && p[2] > wall[2] - EDGE
            && p[2] < wall[3] + EDGE
    };
    let in_ramp = |p: [f32; 3]| {
        spots.iter().any(|(at, axis, inner, outer)| {
            let v = Vec3::from(p) - *at;
            let cos = v.normalize().dot(*axis).clamp(-1.0, 1.0);
            let angle = ludion_core::math::acos(cos).to_degrees();
            angle >= inner - 1.0 && angle <= outer + 1.0
        })
    };

    let (mut clean, mut lit, mut unlit, mut agree, mut steady, mut steady_agree) =
        (0, 0, 0, 0, 0, 0);
    let (mut max_rel, mut max_abs) = (0.0_f32, 0.0_f32);
    for j in 0..GRID {
        for i in 0..GRID {
            let (px, py) = (i * STEP + STEP / 2, j * STEP + STEP / 2);
            let (gpu, saturated) = gpu_at(px, py);
            assert!(!saturated, "sample ({i}, {j}) saturates the view");
            let p = floor_point(px as f32 + 0.5, py as f32 + 0.5);
            let exposure = cpu.read(p);
            let cpu_total = exposure.total;

            let verdict = cpu_total >= LIT;
            let same = verdict == (gpu >= LIT);
            agree += i32::from(same);
            if verdict {
                lit += 1;
            } else {
                unlit += 1;
            }
            if ring(p).all(|q| (cpu.read(q).total >= LIT) == verdict) {
                steady += 1;
                steady_agree += i32::from(same);
            }

            let status = cpu.statuses(p);
            if near_wall(p) || in_ramp(p) || ring(p).any(|q| cpu.statuses(q) != status) {
                continue;
            }
            clean += 1;
            let abs = (gpu - cpu_total).abs();
            max_abs = max_abs.max(abs);
            max_rel = max_rel.max(abs / cpu_total.max(1e-6));
            assert!(
                abs <= (0.05 * cpu_total).max(0.01),
                "clean sample ({i}, {j}) at {p:?}: gpu {gpu} vs light_exposure {cpu_total}"
            );
        }
    }
    let total = GRID * GRID;
    let agreement = 100.0 * agree as f32 / total as f32;
    eprintln!(
        "[light_exposure_agreement] clean {clean}/{total}, max relative error {max_rel:.4}, max absolute error {max_abs:.4}, binary agreement {agreement:.2}% ({steady_agree}/{steady} steady), lit {lit}, unlit {unlit}"
    );
    assert!(
        clean >= 400,
        "only {clean} clean samples: the gate measures too little"
    );
    assert!(lit >= 50 && unlit >= 50, "lit {lit}, unlit {unlit}");
    assert!(agreement >= 99.0, "binary agreement {agreement:.2}%");
    assert_eq!(
        steady_agree, steady,
        "a steady sample disagrees on lit/unlit"
    );
}
