//! The gallery scenes' game halves, shared by their CPU proofs (`character_cycle.rs`,
//! `camera_rig.rs`), `gallery.rs` (the driven goldens) and `examples/` (the runnable hosts).
//!
//! **character-cycle** — the engine plays clips; this system chooses them: the mannequin idles, runs,
//! jumps, runs, spins, runs and returns to idle, crossfading each switch and leaving the one-shots on
//! their `end` events (`ludion docs character-animation`).
//!
//! **camera-rig** — [`drive_rig`] swaps the rail camera to a side-on rig at [`SWITCH_TICK`] (a timed
//! blend), cuts back with `snap_rig` at [`SNAP_TICK`] and pulses trauma at [`SHAKE_TICK`]
//! (`ludion docs camera`).

#![allow(dead_code)]

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Animation, AnimationEvents, Engine, EngineBuilder, Mesh, Sprite};
use ludion_engine::{
    BlendIn, CameraRig, RigConfig, RigMode, TrackRig, add_trauma, animation_events, crossfade,
    snap_rig,
};

/// Ticks the mannequin idles before its first run.
pub const IDLE_TICKS: u64 = 60;
/// Ticks each run lasts: two strides at 60 Hz (the run clip is 0.8 s at overlay speed 1.2).
pub const RUN_TICKS: u64 = 80;
/// Seconds of the idle → run crossfade.
pub const RUN_IN_FADE: f32 = 0.25;

/// What ends a clip's turn.
enum Leave {
    /// This many ticks after the clip started.
    After(u64),
    /// The clip's implicit `end` event (a one-shot finishing).
    OnEnd,
}

/// `(clip, entered from, leaves on, next clip, fade seconds)`; a `None` origin matches any.
/// Every entry into `run` is a crossfade, and `crossfade` keeps `Animation.from` after the fade
/// ends, so `run` reads its predecessor there and the cycle needs no state of its own. A fade of
/// `0.0` is a hard cut.
const CYCLE: [(&str, Option<&str>, Leave, &str, f32); 6] = [
    ("idle", None, Leave::After(IDLE_TICKS), "run", RUN_IN_FADE),
    ("run", Some("idle"), Leave::After(RUN_TICKS), "jump", 0.1),
    ("run", Some("jump"), Leave::After(RUN_TICKS), "spin", 0.0),
    ("run", Some("spin"), Leave::After(RUN_TICKS), "idle", 0.3),
    ("jump", None, Leave::OnEnd, "run", 0.15),
    ("spin", None, Leave::OnEnd, "run", 0.15),
];

/// Advances every `Animation` along [`CYCLE`], writing only on a switch and stamping with
/// `ctx.game_frame`, so the component stays quiet in the hash between switches.
pub fn cycle_character(world: &mut World, ctx: &SystemCtx) {
    let events = world
        .resource::<AnimationEvents>()
        .cloned()
        .unwrap_or_default();
    for (entity, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        let elapsed = ctx
            .game_frame
            .as_u64()
            .saturating_sub(animation.started.as_u64());
        let ended = events.fired(entity, &animation.clip, "end");
        let next = CYCLE
            .iter()
            .find(|(clip, from, _, _, _)| {
                *clip == animation.clip && from.is_none_or(|from| from == animation.from)
            })
            .and_then(|(_, _, leave, next, fade)| {
                let due = match leave {
                    Leave::After(ticks) => elapsed >= *ticks,
                    Leave::OnEnd => ended,
                };
                due.then_some((*next, *fade))
            });
        if let Some((clip, fade)) = next {
            crossfade(animation, clip, fade, ctx.game_frame);
        }
    }
}

/// The scene's engine: `animation_events` first, so `cycle_character` reads this tick's events.
pub fn cycle_builder() -> EngineBuilder {
    Engine::builder()
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&Animation, &Sprite)>()
        .system("cycle_character", cycle_character)
        .query::<(&mut Animation,)>()
}

/// The tick the rail camera blends to the side-on rig.
pub const SWITCH_TICK: u64 = 60;
/// The tick the camera cuts back to the behind rig.
pub const SNAP_TICK: u64 = 180;
/// The tick of the trauma pulse.
pub const SHAKE_TICK: u64 = 220;

/// The track rig with the eye `offset` and a one-second blend in.
fn rig_at(offset: [f32; 3]) -> RigConfig {
    RigConfig {
        mode: RigMode::Track(TrackRig {
            path: "paths/camera-track.path.yaml".into(),
            offset,
            ..TrackRig::default()
        }),
        blend_in: BlendIn {
            seconds: 1.0,
            ..BlendIn::default()
        },
        ..RigConfig::default()
    }
}

/// The side-on rig: the eye five metres to the rail's right.
pub fn side_rig() -> RigConfig {
    rig_at([5.0, 1.5, 0.0])
}

/// The scene's authored rig: behind and above the hero.
pub fn behind_rig() -> RigConfig {
    rig_at([0.0, 2.2, 4.5])
}

/// Writes the rig on `SWITCH_TICK` (a blend), `SNAP_TICK` (a cut) and `SHAKE_TICK` (trauma), keyed
/// on `ctx.game_frame`. Game systems run before `camera_rig`, so a write at tick N is seen at N.
pub fn drive_rig(world: &mut World, ctx: &SystemCtx) {
    let tick = ctx.game_frame.as_u64();
    if ![SWITCH_TICK, SNAP_TICK, SHAKE_TICK].contains(&tick) {
        return;
    }
    for (_, (rig,)) in world.query_ids::<(&mut CameraRig,)>() {
        match tick {
            SWITCH_TICK => rig.rig = side_rig(),
            SNAP_TICK => {
                rig.rig = behind_rig();
                snap_rig(rig);
            }
            _ => add_trauma(rig, 0.8),
        }
    }
}

/// The camera-rig scene's engine.
pub fn rig_builder() -> EngineBuilder {
    Engine::builder()
        .system("drive_rig", drive_rig)
        .query::<(&mut CameraRig,)>()
}
