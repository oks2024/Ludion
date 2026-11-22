//! `Rain`: extract turns the lowest-id `Rain` into `RenderScene::rain` with game time and the streak
//! budget; a second one and an over-budget one are warnings; check-scene reports all three codes at
//! their pointers; a presentation `Rain` never moves the `WorldHash`. None of it needs a GPU.

use std::path::Path;

use ludion_assets::AssetRoot;
use ludion_core::codes::code;
use ludion_core::diagnostics::DiagnosticCode;
use ludion_core::math::glam::{IVec3, Vec3};
use ludion_core::time::{GameClock, SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Diagnostics, Engine, FrameSize, Name, QualityParams, Rain, Transform};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_renderer::{RenderScene, Renderer, drop_position, rain_layout};

fn document(entities: &str) -> String {
    format!(
        "format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n      Transform: {{}}\n      Camera: {{}}\n{entities}"
    )
}

/// A `Rain` entity; `fields` are block-style `key: value` lines, as a scene file writes them.
fn rain_entity(name: &str, presentation: bool, fields: &[&str]) -> String {
    let presentation = if presentation {
        "    presentation: true\n"
    } else {
        ""
    };
    let body = if fields.is_empty() {
        " {}\n".to_string()
    } else {
        let lines: String = fields.iter().map(|f| format!("        {f}\n")).collect();
        format!("\n{lines}")
    };
    format!("  - name: {name}\n{presentation}    components:\n      Rain:{body}")
}

fn engine_for(text: &str) -> Engine {
    let mut engine = Engine::builder().build().expect("the game builds");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn extract(engine: &mut Engine) -> RenderScene {
    engine.advance_live();
    engine.extract_present_scene().expect("extracts").0
}

fn reported(engine: &Engine, code: DiagnosticCode) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .map_or(0, |d| d.reported(code).count())
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("the sim steps");
}

#[test]
fn extract_fills_rain_from_the_component() {
    let mut engine = engine_for(&document(&rain_entity(
        "weather",
        true,
        &[
            "wind:",
            "  - 1.5",
            "  - 0.0",
            "  - -2.0",
            "seed: 7",
            "lit: false",
        ],
    )));
    let rain = extract(&mut engine).rain.expect("the rain extracts");
    assert_eq!(rain.wind, [1.5, 0.0, -2.0]);
    assert_eq!(rain.seed, 7);
    assert!(!rain.lit);
    assert_eq!(rain.per_tile, 96, "density 1.5 x cell 4 cubed");
    assert!(rain.time_s < 0.05, "one tick in, got {}", rain.time_s);
    assert!(extract(&mut engine_for(&document(""))).rain.is_none());
}

/// `wetness` defaults to 1 and the authored value reaches the frame's rain.
#[test]
fn extract_carries_wetness() {
    let default = engine_for(&document(&rain_entity("weather", true, &[])));
    assert_eq!(extract(&mut { default }).rain.expect("rain").wetness, 1.0);
    let dry = engine_for(&document(&rain_entity("weather", true, &["wetness: 0.25"])));
    assert_eq!(extract(&mut { dry }).rain.expect("rain").wetness, 0.25);
}

#[test]
fn intensity_zero_extracts_no_rain() {
    let mut engine = engine_for(&document(&rain_entity(
        "weather",
        true,
        &["intensity: 0.0"],
    )));
    assert!(extract(&mut engine).rain.is_none());
    let half = engine_for(&document(&rain_entity(
        "weather",
        true,
        &["intensity: 0.5"],
    )));
    let rain = extract(&mut { half }).rain.expect("half rain");
    assert_eq!(rain.per_tile, 48);
}

/// A running clock advances `time_s`; a game that holds the clock every frame keeps it fixed.
#[test]
fn time_follows_the_game_clock() {
    let scene = document(&rain_entity("weather", true, &[]));
    let mut running = engine_for(&scene);
    step(&mut running, 60);
    let moved = extract(&mut running).rain.expect("rain").time_s;
    assert!(
        (moved - 1.0).abs() < 0.05,
        "one second of ticks, got {moved}"
    );

    fn hold(world: &mut World, _ctx: &SystemCtx) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
    let mut held = Engine::builder()
        .system("hold", hold)
        .build()
        .expect("the game builds");
    held.load_inline_scene(&scene).expect("the scene loads");
    step(&mut held, 60);
    assert_eq!(extract(&mut held).rain.expect("rain").time_s, 0.0);
}

#[test]
fn a_second_rain_reports_l2381_and_the_lowest_id_wins() {
    let mut engine = engine_for(&document(&format!(
        "{}{}",
        rain_entity("first", true, &["seed: 1"]),
        rain_entity("second", true, &["seed: 2"])
    )));
    let rain = extract(&mut engine).rain.expect("rain");
    assert_eq!(rain.seed, 1);
    extract(&mut engine);
    assert_eq!(reported(&engine, code::RAIN_DUPLICATE), 1, "once per load");
}

#[test]
fn an_over_budget_rain_clamps_per_tile_and_reports_l2382() {
    let mut engine = engine_for(&document(&rain_entity(
        "storm",
        true,
        &["density: 16.0", "radius: 64.0", "cell: 8.0"],
    )));
    let rain = extract(&mut engine).rain.expect("rain");
    let tiles = 17 * 17 * 4;
    assert_eq!(rain.per_tile, 65_536 / tiles);
    assert!(rain.per_tile * tiles <= ludion_renderer::MAX_RAIN_STREAKS);
    extract(&mut engine);
    assert_eq!(
        reported(&engine, code::RAIN_STREAK_BUDGET),
        1,
        "once per load"
    );
}

#[test]
fn check_scene_reports_the_three_codes_at_their_pointers() {
    let registry = ludion_engine::builtin_registry();

    let invalid = document(&rain_entity("weather", true, &["density: 99.0"]));
    let errors = ludion_scene::check(&invalid, &registry).expect_err("an out-of-range density");
    let hit = errors
        .iter()
        .find(|d| d.code == code::RAIN_INVALID)
        .unwrap_or_else(|| panic!("L2380 in {errors:?}"));
    assert_eq!(
        hit.path.as_deref(),
        Some("/entities/1/components/Rain/density")
    );

    let busy = document(&format!(
        "{}{}",
        rain_entity(
            "first",
            true,
            &["density: 16.0", "radius: 64.0", "cell: 8.0"]
        ),
        rain_entity("second", true, &[])
    ));
    let scene = ludion_scene::check(&busy, &registry).expect("valid");
    let found = ludion_engine::scene_engine_diagnostics(Path::new("."), &scene);
    let pointer = |wanted: DiagnosticCode| {
        found
            .iter()
            .find(|d| d.code == wanted)
            .unwrap_or_else(|| panic!("{wanted:?} in {found:?}"))
            .path
            .clone()
    };
    assert_eq!(
        pointer(code::RAIN_DUPLICATE).as_deref(),
        Some("/entities/2/components/Rain")
    );
    assert_eq!(
        pointer(code::RAIN_STREAK_BUDGET).as_deref(),
        Some("/entities/1/components/Rain/density")
    );
}

/// A `Rain` on a presentation entity is outside the hash: the world hashes the same with and without.
#[test]
fn a_presentation_rain_does_not_move_the_world_hash() {
    let bare = document("");
    let mut without = engine_for(&bare);
    let mut with = engine_for(&document(&rain_entity("weather", true, &[])));
    step(&mut without, 5);
    step(&mut with, 5);
    assert_eq!(
        without.world_hash().expect("hash"),
        with.world_hash().expect("hash")
    );
    let _ = Rain::default();
}

/// A cube entity with extra component blocks (each already indented to six spaces).
fn cube(name: &str, extra: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform: {{}}\n      Mesh:\n        shape: cube\n{extra}"
    )
}

/// `Parent` of the entity at list position `index` (`cam` is 0).
fn parent_block(index: u32) -> String {
    format!("      Parent:\n        index: {index}\n        generation: 0\n")
}

const SHADOWLESS: &str = "      Visibility:\n        cast_shadows: false\n";

fn occluders(engine: &mut Engine) -> Vec<bool> {
    extract(engine)
        .meshes
        .iter()
        .map(|mesh| mesh.rain_occluder)
        .collect()
}

/// Without a `RainOccluder` a mesh occludes exactly when it casts a shadow.
#[test]
fn an_absent_rain_occluder_follows_cast_shadows() {
    let mut engine = engine_for(&document(&format!(
        "{}{}",
        cube("caster", ""),
        cube("ghost", SHADOWLESS)
    )));
    assert_eq!(occluders(&mut engine), [true, false]);
}

/// An own `RainOccluder` overrides `cast_shadows` both ways.
#[test]
fn an_own_rain_occluder_overrides_cast_shadows_both_ways() {
    let mut engine = engine_for(&document(&format!(
        "{}{}",
        cube("forced", &format!("{SHADOWLESS}      RainOccluder: {{}}\n")),
        cube("exempt", "      RainOccluder:\n        occludes: false\n"),
    )));
    assert_eq!(occluders(&mut engine), [true, false]);
}

/// A parent's tag reaches an untagged child, and the nearest tag wins down the chain.
#[test]
fn a_parents_rain_occluder_reaches_its_children_and_the_nearest_wins() {
    let mut engine = engine_for(&document(&format!(
        "{}{}{}{}",
        cube("root", "      RainOccluder:\n        occludes: false\n"),
        cube("child", &parent_block(1)),
        cube(
            "retagged",
            &format!("{}      RainOccluder: {{}}\n", parent_block(1))
        ),
        cube("grandchild", &parent_block(3)),
    )));
    assert_eq!(occluders(&mut engine), [false, false, true, true]);
}

/// `occludes: true` on a blended mesh is inert and reported once as `L2383`; `occludes: false` is not.
#[test]
fn a_rain_occluder_on_a_blended_mesh_reports_l2383() {
    let blended = "        blend: alpha\n";
    let mut engine = engine_for(&document(&format!(
        "{}{}",
        cube("glass", &format!("{blended}      RainOccluder: {{}}\n")),
        cube(
            "net",
            &format!("{blended}      RainOccluder:\n        occludes: false\n")
        ),
    )));
    let scene = extract(&mut engine);
    assert!(scene.meshes.is_empty(), "both are blended");
    assert_eq!(reported(&engine, code::RAIN_OCCLUDER_BLENDED), 1);
}

/// What the camera-move proof needs from one render: the rain, the camera, and the pixels.
struct Frame {
    scene: RenderScene,
    rgba: Vec<u8>,
}

const FRAME: (u32, u32) = (960, 540);

/// The committed gallery scene loaded into an engine rooted at its own directory.
fn gallery_engine(stem: &str) -> Engine {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
    let source = std::fs::read_to_string(dir.join(format!("{stem}.yaml")))
        .expect("the committed gallery scene reads");
    let mut engine = Engine::with_project_root(&dir);
    engine
        .load_inline_scene(&source)
        .expect("the committed gallery scene validates");
    engine
}

/// Extracts at a fixed tick, never ticking the sim, so two calls see the same game time.
fn extract_at_tick_12(engine: &Engine) -> RenderScene {
    ludion_engine::extract_at(
        engine.world(),
        FRAME,
        AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(12, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the scene extracts")
}

/// A drop's pixel in `frame`, or `None` when it is behind the camera.
fn project(scene: &RenderScene, world: Vec3) -> Option<(f32, f32)> {
    let clip = scene.camera.projection * scene.camera.view * world.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some((
        (ndc.x * 0.5 + 0.5) * FRAME.0 as f32,
        (0.5 - ndc.y * 0.5) * FRAME.1 as f32,
    ))
}

/// Every drop well inside the rain's fade, in front of the camera and at least 6 px inside the frame:
/// `(tile, k, world position, pixel)`.
fn kept_drops(scene: &RenderScene) -> Vec<(IVec3, u32, Vec3, (f32, f32))> {
    let rain = scene.rain.expect("the scene has rain");
    let cam = scene.camera.position;
    let layout = rain_layout(&rain, cam);
    let mut kept = Vec::new();
    for ix in 0..layout.n_xz as i32 {
        for iy in 0..layout.n_y as i32 {
            for iz in 0..layout.n_xz as i32 {
                let tile = layout.base + IVec3::new(ix, iy, iz);
                for k in 0..layout.per_tile {
                    let pos = drop_position(&rain, tile, k);
                    let dxz = ludion_core::math::hypot(pos.x - cam.x, pos.z - cam.z);
                    let rel_y =
                        (pos.y - cam.y - rain.height[0]) / (rain.height[1] - rain.height[0]);
                    if dxz >= 0.8 * rain.radius || !(0.15..=0.85).contains(&rel_y) {
                        continue;
                    }
                    let Some(pixel) = project(scene, pos) else {
                        continue;
                    };
                    let inside = |v: f32, extent: u32| v >= 6.0 && v <= extent as f32 - 6.0;
                    if inside(pixel.0, FRAME.0) && inside(pixel.1, FRAME.1) {
                        kept.push((tile, k, pos, pixel));
                    }
                }
            }
        }
    }
    kept
}

/// The brightest channel anywhere in the 5x5 block around `pixel`.
fn max_near(frame: &Frame, pixel: (f32, f32)) -> u8 {
    let (cx, cy) = (pixel.0.round() as i32, pixel.1.round() as i32);
    let mut best = 0u8;
    for y in cy - 2..=cy + 2 {
        for x in cx - 2..=cx + 2 {
            if x < 0 || y < 0 || x >= FRAME.0 as i32 || y >= FRAME.1 as i32 {
                continue;
            }
            let at = ((y as u32 * FRAME.0 + x as u32) * 4) as usize;
            best = best.max(
                frame.rgba[at]
                    .max(frame.rgba[at + 1])
                    .max(frame.rgba[at + 2]),
            );
        }
    }
    best
}

/// Two renders at one tick from cameras 2 m apart put every drop at the pixel its fixed world position
/// projects to: the field is world-anchored, not carried by the camera (`Plan/rain-and-wet.md` D35).
#[test]
fn needs_gpu_drops_stay_world_anchored_when_the_camera_moves() {
    let renderer = match Renderer::new() {
        Ok(renderer) => renderer,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("[rain] SKIPPED: no GPU adapter");
            return;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };
    let mut engine = gallery_engine("rain-camera-move");
    let render = |scene: RenderScene| {
        let image = renderer
            .render(&scene, FrameSize::native(FRAME.0, FRAME.1))
            .expect("the frame renders");
        Frame {
            scene,
            rgba: image.rgba,
        }
    };

    let a = render(extract_at_tick_12(&engine));
    let camera = {
        let world = engine.world();
        world
            .entities()
            .find(|&id| {
                world
                    .get::<Name>(id)
                    .ok()
                    .flatten()
                    .is_some_and(|name| name.0 == "camera")
            })
            .expect("the scene has a camera")
    };
    engine
        .world_mut()
        .get_mut::<Transform>(camera)
        .expect("the camera is alive")
        .expect("the camera has a Transform")
        .translation[0] += 2.0;
    let b = render(extract_at_tick_12(&engine));
    assert_eq!(a.scene.rain, b.scene.rain, "the same rain at the same tick");

    let kept_a = kept_drops(&a.scene);
    let kept_b = kept_drops(&b.scene);
    assert!(kept_a.len() >= 6, "frame A keeps {} drops", kept_a.len());
    assert!(kept_b.len() >= 6, "frame B keeps {} drops", kept_b.len());
    for (name, frame, kept) in [("A", &a, &kept_a), ("B", &b, &kept_b)] {
        for &(tile, k, pos, pixel) in kept {
            let seen = max_near(frame, pixel);
            assert!(
                seen > 64,
                "frame {name}: drop {tile:?}/{k} at {pos:?} projects to {pixel:?} but the brightest nearby channel is {seen}"
            );
        }
    }

    let shared: Vec<_> = kept_a
        .iter()
        .filter(|(tile, k, ..)| kept_b.iter().any(|(t, j, ..)| t == tile && j == k))
        .collect();
    assert!(
        shared.len() >= 3,
        "only {} drops kept in both",
        shared.len()
    );

    // Swim control: a camera-carried field would light the pixel a drop had in A even where B's drops are not.
    let mut checked = 0;
    for &&(_, _, _, pixel) in &shared {
        let far_from_b = kept_b
            .iter()
            .all(|&(.., (bx, by))| ludion_core::math::hypot(bx - pixel.0, by - pixel.1) >= 8.0);
        if far_from_b {
            checked += 1;
            let seen = max_near(&b, pixel);
            assert!(
                seen <= 64,
                "B is lit at {pixel:?} where only A has a drop: {seen}"
            );
        }
    }
    eprintln!("[rain] swim control checked {checked} pixels");
}
