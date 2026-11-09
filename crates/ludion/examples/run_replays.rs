//! **Run recordings in a game**: record one run to a `.lrp`, list it, play it back in the live engine
//! and verify it — over a small arena of rigid-body enemies, a turret that fires pooled projectiles
//! and a scripted input bot.
//!
//! The `director` system starts a recording a few ticks into the scene and stops it when the run is
//! `Run::length` ticks long. `RunRecording::start` restarts the scene at frame 0, so the director
//! guards on `active()` / `last_finished()`. A real game calls `start` from a "new run" menu and
//! `ReplayPlayback::play` from a "watch replay" menu; `playback_controls` maps keys to pause, speed
//! and stop while a replay plays. The sim never reads the live input that arrives during a playback.
//!
//! Everything above the `main` divider is copyable. Run it headless (writes only under a temp dir):
//!
//! ```text
//! cargo run -p ludion --example run_replays
//! ```

use ludion::prelude::*;
use ludion_engine::replay_hashes_with;
use ludion_protocol::Command;
use projectiles::{ProjectileDesc, Response};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use spatial3d::Handle;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// An enemy: hit points and the phase of its wander.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Enemy {
    pub hp: u32,
    pub phase: f32,
}

/// Authored on one scene entity: how many ticks the recorded run lasts.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Run {
    pub length: u64,
}

/// The name of the recording the director makes.
pub const RUN_NAME: &str = "run";
/// The tick at which the director starts recording; the run restarts at frame 0 there.
pub const TITLE_TICKS: u64 = 30;
/// Hit points an enemy spawns with.
const HP: u32 = 3;
/// Half the side of the square the enemies live in, in metres.
const ARENA: f32 = 40.0;

/// Each enemy wanders on a slow circle and is pulled toward the arena centre.
fn steer(world: &mut World, ctx: &SystemCtx) {
    let t = ctx.frame.as_u64() as f32 * 0.02;
    let enemies: Vec<(EntityId, f32)> = world
        .query_ids::<(&Enemy,)>()
        .map(|(id, (enemy,))| (id, enemy.phase))
        .collect();
    for (id, phase) in enemies {
        let Ok(Some(at)) = world.get::<Transform>(id).map(|t| t.copied()) else {
            continue;
        };
        let (sin, cos) = math::sin_cos(phase + t);
        let pull = [-at.translation[0] * 0.05, -at.translation[2] * 0.05];
        if let Ok(Some(velocity)) = world.get_mut::<Velocity>(id) {
            velocity.linear = [cos * 2.0 + pull[0], 0.0, sin * 2.0 + pull[1]];
        }
    }
}

/// Every 10 ticks the turret at the centre shoots at a random enemy.
fn fire(world: &mut World, ctx: &SystemCtx) {
    if !ctx.frame.as_u64().is_multiple_of(10) {
        return;
    }
    let targets: Vec<EntityId> = world.query_ids::<(&Enemy,)>().map(|(id, _)| id).collect();
    if targets.is_empty() {
        return;
    }
    let roll = world
        .resource_mut::<DeterministicRng>()
        .map_or(0, DeterministicRng::next_u32);
    let target = targets[roll as usize % targets.len()];
    let Ok(Some(at)) = world.get::<Transform>(target).map(|t| t.copied()) else {
        return;
    };
    let origin = [0.0, 1.0, 0.0];
    let to = [
        at.translation[0] - origin[0],
        at.translation[1] - origin[1],
        at.translation[2] - origin[2],
    ];
    let length = math::sqrt(to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).max(0.001);
    let speed = 80.0 / length;
    let mut shot = ProjectileDesc::new(origin, [to[0] * speed, to[1] * speed, to[2] * speed]);
    shot.response = Response::Despawn;
    shot.lifetime = 1.0;
    spawn_projectile(world, shot);
}

/// A hit costs one hit point; at zero the enemy respawns on the arena's edge with full health, so
/// the field stays full and the state keeps changing.
fn score(world: &mut World, _ctx: &SystemCtx) {
    let hit: Vec<Handle> = world
        .resource::<Projectiles>()
        .map(|p| p.events().hits.iter().map(|h| h.handle).collect())
        .unwrap_or_default();
    for handle in hit {
        let Some(id) = handle.as_entity() else {
            continue;
        };
        let Ok(Some(enemy)) = world.get_mut::<Enemy>(id) else {
            continue;
        };
        enemy.hp = enemy.hp.saturating_sub(1);
        if enemy.hp > 0 {
            continue;
        }
        enemy.hp = HP;
        let roll = world
            .resource_mut::<DeterministicRng>()
            .map_or(0, DeterministicRng::next_u32);
        let side = (roll % 2) as usize * 2;
        let along = ((roll >> 1) % 1000) as f32 / 1000.0 * 2.0 * ARENA - ARENA;
        let mut spot = [along, 1.0, along];
        spot[side] = if roll & 0x8000 == 0 { ARENA } else { -ARENA };
        if let Ok(Some(transform)) = world.get_mut::<Transform>(id) {
            transform.translation = spot;
        }
    }
}

/// Starts the recording once and stops it when the run is `Run::length` ticks long.
fn director(world: &mut World, ctx: &SystemCtx) {
    let Some(length) = world.single_get::<Run>().map(|run| run.length) else {
        return;
    };
    let frame = ctx.frame.as_u64();
    let Some(recording) = world.resource_mut::<RunRecording>() else {
        return;
    };
    if frame == TITLE_TICKS && recording.active().is_none() && recording.last_finished().is_none() {
        recording.start(RUN_NAME);
    } else if frame == length && recording.active().is_some() {
        recording.stop();
    }
}

/// The game, recording into `<root>/save/replays` every `keyframe_every` ticks. A real game's `main`
/// is `ludion::run(game())`; the caller adds `persist_profile()`.
pub fn game(root: &Path, keyframe_every: u64) -> EngineBuilder {
    Engine::builder()
        .project_root(root)
        .components(|registry| {
            registry.register::<Enemy>("Enemy")?;
            registry.register::<Run>("Run")
        })
        .projectiles(ProjectileConfig::default())
        .system("steer", steer)
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("fire", fire)
        .system("projectiles_step", projectiles_step)
        .system("score", score)
        .system("director", director)
        .run_recordings(RunRecordingConfig {
            keyframe_every,
            ..RunRecordingConfig::default()
        })
        .replay_playback(ReplayPlaybackConfig::default())
        .playback_controls(|input, controls| {
            if input.was_pressed(Key::Space) {
                if controls.paused() {
                    controls.resume();
                } else {
                    controls.pause();
                }
            }
            if input.was_pressed(Key::Digit1) {
                controls.set_speed(PlaybackSpeed::X1);
            }
            if input.was_pressed(Key::Digit2) {
                controls.set_speed(PlaybackSpeed::X2);
            }
            if input.was_pressed(Key::Digit4) {
                controls.set_speed(PlaybackSpeed::X4);
            }
            if input.was_pressed(Key::Escape) {
                controls.stop();
            }
        })
        .game_version("1")
}

/// The arena: a floor, the run's length, and `enemies` rigid spheres on a grid with no overlap.
pub fn scene(enemies: usize, length: u64) -> String {
    let mut text = format!(
        "format: 1\nseed: 11\nphysics:\n  space: free\n  gravity:\n    - 0.0\n    - 0.0\n    - 0.0\n\
         entities:\n  - name: run\n    components:\n      Run:\n        length: {length}\n\
         \x20 - name: floor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - -0.5\n          - 0.0\n\
         \x20     Collider:\n        shape:\n          box:\n            half_extents:\n              - {ARENA}\n              - 0.5\n              - {ARENA}\n        body: static\n"
    );
    let columns = 25;
    for index in 0..enemies {
        let (column, row) = ((index % columns) as f32, (index / columns) as f32);
        let phase = (index % 63) as f32 * 0.1;
        let _ = write!(
            text,
            "  - name: enemy{index}\n    components:\n      Transform:\n        translation:\n          - {x}\n          - 1.0\n          - {z}\n\
             \x20     Collider:\n        shape:\n          sphere:\n            radius: 0.5\n        body:\n          rigid:\n            mass: 1.0\n            gravity_scale: 0.0\n\
             \x20     Enemy:\n        hp: {HP}\n        phase: {phase}\n      SpatialVolume: {{}}\n",
            x = (column - 12.0) * 3.0,
            z = (row - 10.0) * 3.0,
        );
    }
    text
}

/// One scripted input per call: a movement key toggled every 20–40 ticks and a mouse move every 10.
/// The recording carries whatever a player's devices would produce; this bot stands in for them.
pub fn bot_input(engine: &mut Engine, tick: u64) {
    const KEYS: [&str; 4] = ["W", "A", "S", "D"];
    let mut inject = |event: serde_json::Value| {
        engine
            .execute(Command::InputInject(command::InputInjectParams {
                frame: None,
                event,
            }))
            .expect("the engine takes the injected event");
    };
    if tick.is_multiple_of(30) {
        let key = KEYS[(tick / 30) as usize % 4];
        let kind = if (tick / 120).is_multiple_of(2) {
            "key_down"
        } else {
            "key_up"
        };
        inject(json!({ "kind": kind, "key": key }));
    }
    if tick.is_multiple_of(10) {
        let (x, y) = ((tick % 1920) as f32, (tick % 1080) as f32);
        inject(json!({ "kind": "mouse_move", "position_bits": [x.to_bits(), y.to_bits()] }));
    }
}

// --- Headless demo `main` ------------------------------------------------------------------------

fn main() -> ExitCode {
    let root: PathBuf =
        std::env::temp_dir().join(format!("ludion-run-replays-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ok = demo(&root);
    let _ = std::fs::remove_dir_all(&root);
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn demo(root: &Path) -> bool {
    std::fs::create_dir_all(root).expect("temp root");
    // `play` returns to a scene *file*, so the arena is on disk as well as loaded.
    let arena = scene(50, 600);
    std::fs::write(root.join("arena.yaml"), &arena).expect("scene file");
    let mut engine = game(root, 60)
        .persist_profile()
        .build()
        .expect("the game boots");
    engine.load_inline_scene(&arena).expect("the arena loads");

    // Play until the director has stopped the recording.
    let mut tick = 0;
    loop {
        bot_input(&mut engine, tick);
        engine.advance_live();
        tick += 1;
        let finished = engine
            .world()
            .resource::<RunRecording>()
            .and_then(|recording| recording.last_finished().map(str::to_owned));
        if finished.as_deref() == Some(RUN_NAME) {
            break;
        }
        if tick > 2000 {
            eprintln!("the run never finished");
            return false;
        }
    }

    // List it.
    let playback = engine
        .world_mut()
        .resource_mut::<ReplayPlayback>()
        .expect("replay_playback is installed");
    playback.refresh();
    engine.advance_live();
    let entries = engine
        .world()
        .resource::<ReplayPlayback>()
        .map(|playback| playback.entries().to_vec())
        .unwrap_or_default();
    for entry in &entries {
        println!(
            "{}: {} bytes, compatible: {}",
            entry.name,
            entry.bytes,
            entry.refusal.is_none()
        );
    }
    if entries.len() != 1 || entries[0].refusal.is_some() {
        eprintln!("expected one compatible recording");
        return false;
    }

    // Play it back in place and compare the end state with a standalone replay of the file.
    engine
        .world_mut()
        .resource_mut::<ReplayPlayback>()
        .expect("installed")
        .play(RUN_NAME, "arena.yaml");
    engine.advance_live();
    let mut last = (engine.frame(), engine.world_hash().expect("hash"));
    while engine.advance_live() {
        last = (engine.frame(), engine.world_hash().expect("hash"));
    }
    let outcome = engine
        .world()
        .resource::<ReplayPlayback>()
        .and_then(|playback| playback.last_outcome().cloned());
    println!("playback: {outcome:?}");
    let bytes = std::fs::read(root.join("save/replays").join(format!("{RUN_NAME}.lrp")))
        .expect("the recording");
    let trace = replay_hashes_with(game(root, 60), &bytes, root).expect("the file replays");
    let same = trace.last() == Some(&last);
    println!("played end state matches the standalone replay: {same}");
    matches!(outcome, Some(PlaybackOutcome::Finished { .. })) && same
}
