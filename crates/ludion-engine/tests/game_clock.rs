//! Runtime proof for the held gameplay clock: a game that calls [`GameClock::hold`] every frame it
//! is frozen stops every engine-evaluated closed form, while the sim keeps ticking.
//!
//! Three claims, over the ordinary public surface (a scene document, `Engine`, `extract_at`, the
//! `engine.snapshot`/`engine.restore` commands):
//!
//! - **A hold costs gameplay time nothing.** A run of 120 unheld frames and a run of 150 frames
//!   holding 30 of them stand up the same volleys — same fire frames, same seeds, same census — and
//!   place every live bullet at the same point. Gameplay frame *N* is fired exactly once however
//!   many frames the game was frozen around it.
//! - **The picture freezes.** The same world extracted at the first held frame and at the last one
//!   is equal instance for instance: the particle discs, the flipbook's atlas region, and the
//!   bullets on the field all read the held clock.
//! - **The clock survives a snapshot.** A capture taken mid-freeze restores a session that is still
//!   frozen at the same gameplay time; a clock that holds nothing is omitted from the document, so
//!   every snapshot that existed before the field is byte-identical.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::time::{GameClock, SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    BulletKind, DocStore, Engine, EngineBuilder, PatternDoc, QualityParams, Transform, Volley,
    advance_volleys, bullet_is_alive, bullet_position, extract_at, play_patterns,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_renderer::RenderScene;
use ludion_scene::{AtlasDocument, AtlasFilter, AtlasRegion, Flipbook};
use ludion_spatial::Plane;
use serde_json::Value;

const TICK: TickRate = TickRate::DEFAULT;

/// The frames the held run freezes — the window a bomb flash or a pause overlay would hold.
const HOLD_FROM: u64 = 60;
const HOLD_TO: u64 = 89;
/// How many frames that window removes from gameplay time.
const HELD: u64 = HOLD_TO - HOLD_FROM + 1;

/// A ring every six frames for the whole run, each bullet outliving it — so the census at the end
/// is every volley ever fired, and a volley fired one frame early or late is visible in it.
const STORM: &str = "\
format: 1
kind: pattern
bullets:
  pellet:
    life: 600
    hit_radius: 0.1
timeline:
  - at: 0
    repeat: 30
    every: 6
    shape: ring
    bullet: pellet
    count: 6
    speed: 2.0
";

/// A turret playing [`STORM`], an endless particle plume, and a looping flipbook — one entity per
/// closed form the clock has to freeze.
const SCENE: &str = "\
format: 1
seed: 5
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 8.0
      Camera: {}
  - name: turret
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      PatternPlayer:
        pattern: patterns/storm.yaml
        started: 0
  - name: plume
    components:
      Transform:
        translation:
          - -3.0
          - 0.0
          - 0.0
      ParticleEmitter:
        rate: 24.0
        life:
          - 1.5
          - 2.5
        speed:
          - 0.5
          - 1.5
        spread_deg: 20.0
        gravity: -0.5
        start_size: 0.3
        end_size: 0.8
        start_color:
          - 0.6
          - 0.6
          - 0.7
          - 0.5
        end_color:
          - 0.3
          - 0.3
          - 0.4
          - 0.0
        seed: 11
  - name: banner
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: wide
      Animation:
        clip: fly
";

// --- Harness ---------------------------------------------------------------------------------

/// The fixture atlas the flipbook plays: three regions of three different shapes, so a wrong frame
/// changes the quad's extent as well as its UVs.
fn atlas() -> AtlasDocument {
    let mut regions = BTreeMap::new();
    for (name, rect, pivot) in [
        ("wide", [0, 0, 4, 2], [2.0, 1.0]),
        ("tall", [4, 0, 2, 3], [1.0, 1.5]),
        ("dot", [0, 4, 1, 1], [0.5, 0.5]),
    ] {
        regions.insert(name.to_string(), AtlasRegion { rect, pivot });
    }
    AtlasDocument {
        generated_from: "fixture.atlas-source.yaml".to_string(),
        pixels_per_unit: 4.0,
        color_map: "fixture.png".to_string(),
        filter: AtlasFilter::Nearest,
        regions,
        // 4 fps at the fixed tick is one clip frame every 15 ticks, so the held window spans
        // several frame changes — a clock that leaked would show a different region.
        flipbooks: vec![Flipbook {
            name: "fly".to_string(),
            frames: ["wide", "tall", "dot"].map(str::to_string).to_vec(),
            fps: 4.0,
            looped: true,
            hold: None,
        }],
    }
}

/// A content root unique to this process + `tag`, carrying the pattern document and the atlas pair.
fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-game-clock-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("patterns")).expect("a temp content root");
    std::fs::create_dir_all(dir.join("art")).expect("a temp art directory");
    std::fs::write(dir.join("patterns/storm.yaml"), STORM).unwrap();

    let rgba: Vec<u8> = (0..8u32 * 8 * 4).map(|index| (index % 251) as u8).collect();
    let png = ludion_assets::encode_png(8, 8, &rgba).unwrap();
    std::fs::write(dir.join("art/fixture.png"), &png).unwrap();
    std::fs::write(
        dir.join("art/fixture.atlas.yaml"),
        ludion_scene::emit_atlas(&atlas()),
    )
    .unwrap();
    dir
}

/// Freezes the game for [`HOLD_FROM`]`..=`[`HOLD_TO`] — the one line a game writes.
fn freeze_window(world: &mut World, ctx: &SystemCtx) {
    if (HOLD_FROM..=HOLD_TO).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

/// The two pattern systems in the order a game registers them, optionally behind the freeze.
fn game(dir: &Path, freezes: bool) -> EngineBuilder {
    let mut builder = Engine::builder().patterns();
    if freezes {
        builder = builder.system("freeze_window", freeze_window);
    }
    builder
        .system("play_patterns", play_patterns)
        .system("advance_volleys", advance_volleys)
        .project_root(dir)
}

fn playing(dir: &Path, freezes: bool) -> Engine {
    let mut engine = game(dir, freezes).build().expect("the documents validate");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn advance(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("the sim steps");
}

/// The session's gameplay time — what every closed-form evaluator reads.
fn game_now(engine: &Engine) -> SimTime {
    let now = SimTime::from_ticks(engine.frame().as_u64(), TICK);
    engine
        .world()
        .resource::<GameClock>()
        .expect("the engine installs the clock")
        .game_time(now)
}

/// Every live volley with its captured origin, in ascending entity order.
fn volleys(engine: &Engine) -> Vec<(Volley, Transform)> {
    let world = engine.world();
    world
        .entities()
        .filter_map(|id| {
            let volley = world.get::<Volley>(id).ok().flatten()?.clone();
            let origin = world.get::<Transform>(id).ok().flatten().copied()?;
            Some((volley, origin))
        })
        .collect()
}

fn bullet_kind(engine: &Engine, volley: &Volley) -> BulletKind {
    engine
        .world()
        .resource::<DocStore<PatternDoc>>()
        .expect("the pattern store is registered")
        .get(&volley.pattern)
        .expect("the document is loaded")
        .bullets
        .get(&volley.bullet)
        .expect("the bullet kind is defined")
        .clone()
}

/// One volley's identity — what it is, when it fired in *gameplay* frames, and the stream it draws
/// from — with every live bullet's position at gameplay time.
#[derive(Clone, PartialEq, Eq, Debug)]
struct VolleyField {
    bullet: String,
    started: u64,
    seed: u64,
    count: u32,
    /// Live bullet positions, quantized to micro-units for a readable failure.
    live: Vec<[i64; 3]>,
}

/// The whole observable field: two runs agreeing on it agree on what the player sees and on what a
/// hit test would score.
fn field(engine: &Engine) -> Vec<VolleyField> {
    let now = game_now(engine);
    let plane = Plane::of(engine.world());
    volleys(engine)
        .iter()
        .map(|(volley, origin)| {
            let kind = bullet_kind(engine, volley);
            let live = (0..volley.count)
                .filter(|&index| bullet_is_alive(volley, &kind, index, now))
                .map(|index| {
                    let at = bullet_position(volley, origin, &kind, index, now, plane);
                    // Quantized: the two runs evaluate the identical closed form, so this is an
                    // exactness claim with a readable failure, not a tolerance.
                    at.map(|value| (f64::from(value) * 1e6) as i64)
                })
                .collect();
            VolleyField {
                bullet: volley.bullet.clone(),
                started: volley.started.as_u64(),
                seed: volley.seed,
                count: volley.count,
                live,
            }
        })
        .collect()
}

fn drawn(engine: &Engine) -> RenderScene {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(engine.frame().as_u64(), TICK),
        QualityParams::FULL.particle_density,
    )
    .expect("the scene extracts")
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

// --- The claims ------------------------------------------------------------------------------

/// 120 unheld frames and 150 frames holding 30 of them leave the identical field: the same volleys
/// at the same gameplay frames with the same seeds, and every live bullet at the same point.
///
/// The held run ticks 30 frames more than the unheld one and ends at the same gameplay time — which
/// is the whole claim: a freeze costs the game nothing and skips nothing.
#[test]
fn a_held_run_stands_up_exactly_the_field_an_unheld_run_does() {
    let dir = root("ab");

    let mut unheld = playing(&dir, false);
    advance(&mut unheld, 120);

    let mut held = playing(&dir, true);
    advance(&mut held, 150);

    assert_eq!(
        held.world().resource::<GameClock>().unwrap().held(),
        HELD,
        "the freeze window removed exactly its own frames"
    );
    assert_eq!(
        game_now(&held),
        game_now(&unheld),
        "150 frames with 30 held is 120 frames of gameplay time"
    );

    let expected = field(&unheld);
    assert!(
        expected.len() >= 20,
        "the fixture actually fires a field: {} volleys",
        expected.len()
    );
    assert_eq!(field(&held), expected);

    let _ = std::fs::remove_dir_all(&dir);
}

/// The picture freezes: the first held frame and the last one extract to the identical
/// `RenderScene`, particle discs and flipbook region included.
///
/// Not vacuous — the same window on the unheld run draws a visibly different frame, which the
/// second half asserts.
#[test]
fn a_held_run_draws_the_same_frame_for_the_whole_window() {
    let dir = root("frozen");

    let mut held = playing(&dir, true);
    advance(&mut held, HOLD_FROM + 1); // through frame 60: the first frame the game holds
    let first = drawn(&held);
    advance(&mut held, HOLD_TO - HOLD_FROM); // through frame 89: the last
    let last = drawn(&held);

    assert!(
        !first.particles.is_empty() && !first.sprites.is_empty(),
        "the fixture draws particles and sprites, so the comparison has something to hold still"
    );
    assert_eq!(first, last, "a held run redraws the same frame");

    let mut unheld = playing(&dir, false);
    advance(&mut unheld, HOLD_FROM + 1);
    let before = drawn(&unheld);
    advance(&mut unheld, HOLD_TO - HOLD_FROM);
    assert_ne!(
        before,
        drawn(&unheld),
        "the same 29 frames move the picture when nothing holds"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A snapshot taken mid-freeze restores a session that is still frozen, at the same gameplay time —
/// and one taken by a game that never freezes carries no clock field at all.
#[test]
fn a_snapshot_carries_the_clock_and_omits_an_unheld_one() {
    let dir = root("snapshot");

    let mut held = playing(&dir, true);
    advance(&mut held, HOLD_TO); // mid-window: frames 60..=88 held, and 88 is still holding
    let captured = *held.world().resource::<GameClock>().unwrap();
    assert!(captured.held() > 0 && captured.is_holding());

    let document = snapshot_of(&mut held);
    assert_eq!(
        document["game_clock"]["held"].as_u64(),
        Some(captured.held()),
        "a held clock rides the snapshot"
    );

    let mut restored = playing(&dir, true);
    restored
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        }))
        .expect("the snapshot restores");
    assert_eq!(
        *restored.world().resource::<GameClock>().unwrap(),
        captured,
        "the restored session resumes frozen where the capture stopped"
    );
    assert_eq!(game_now(&restored), game_now(&held));

    // The other half: a game that never freezes writes exactly the document it always did.
    let mut unheld = playing(&dir, false);
    advance(&mut unheld, 30);
    assert!(
        snapshot_of(&mut unheld).get("game_clock").is_none(),
        "an unheld clock is omitted, so older snapshot documents are byte-identical"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A system scaling by NaN is reported once as `L4260` and leaves the clock untouched.
#[test]
fn an_invalid_scale_reports_once_and_changes_nothing() {
    fn bad_scale(world: &mut World, _ctx: &SystemCtx) {
        world.resource_mut::<GameClock>().unwrap().scale(f32::NAN);
    }
    let dir = root("invalid_scale");
    let mut engine = Engine::builder()
        .system("bad_scale", bad_scale)
        .project_root(&dir)
        .build()
        .expect("the documents validate");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    advance(&mut engine, 10);

    let code = ludion_core::codes::code::GAME_SCALE_INVALID;
    let reported = engine
        .world()
        .resource::<ludion_engine::diagnostics::Diagnostics>()
        .unwrap()
        .reported(code)
        .count();
    assert_eq!(reported, 1, "ten bad frames report once per session");
    assert!(
        engine.world().resource::<GameClock>().unwrap().is_zero(),
        "an ignored request leaves nothing behind"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A snapshot taken mid-scale restores the same clock, and an unscaled run still omits the field.
#[test]
fn a_scaled_snapshot_round_trips() {
    fn slow(world: &mut World, _ctx: &SystemCtx) {
        world.resource_mut::<GameClock>().unwrap().scale(0.25);
    }
    let dir = root("scaled_snapshot");
    let build = || {
        let mut engine = Engine::builder()
            .system("slow", slow)
            .project_root(&dir)
            .build()
            .expect("the documents validate");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine
    };
    let mut scaled = build();
    advance(&mut scaled, 21);
    let captured = *scaled.world().resource::<GameClock>().unwrap();
    assert!(captured.slowed_q16() > 0 && captured.is_scaling());

    let document = snapshot_of(&mut scaled);
    assert!(
        document["game_clock"].get("slowed_q16").is_some(),
        "the schema-skipped field still travels in the snapshot"
    );
    let mut restored = build();
    restored
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        }))
        .expect("the snapshot restores");
    assert_eq!(*restored.world().resource::<GameClock>().unwrap(), captured);
    assert_eq!(game_now(&restored), game_now(&scaled));

    let mut unscaled = playing(&dir, false);
    advance(&mut unscaled, 30);
    assert!(snapshot_of(&mut unscaled).get("game_clock").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}
