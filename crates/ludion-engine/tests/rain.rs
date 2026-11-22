//! `Rain`: extract turns the lowest-id `Rain` into `RenderScene::rain` with game time and the streak
//! budget; a second one and an over-budget one are warnings; check-scene reports all three codes at
//! their pointers; a presentation `Rain` never moves the `WorldHash`. None of it needs a GPU.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::DiagnosticCode;
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Diagnostics, Engine, Rain};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_renderer::RenderScene;

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
