//! Playing a paired action (Neon Shroud request 0008 W5a, `Plan/paired-animations.md` decisions
//! 27–32): the warp, the events, the end, the lockstep guard, abort, a ragdoll mid-action, chaining,
//! postures, held frames and determinism.
//!
//! The game's system order is the test engine's: a driver (start, then `paired_motion` →
//! `move_character` + `turn_character` each tick), `animation_events`, `paired_update`, a recorder.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_components::paired_action::PairedAction;
use ludion_components::{Animation, Name, crossfade};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::{GameClock, SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::components::{Mesh, Transform};
use ludion_engine::{
    ClipEvent, Engine, abort_paired, animation_events, load_assembly, move_character,
    paired_motion, paired_update, start_paired, switch_to_ragdoll, turn_character,
};
use ludion_physics::IgnoreCollisions;
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};

use crate::paired_fit::{copy_dir, gallery, named, scene};

const RUN: &str = "voxels/run.paired.yaml";
const START: u64 = 5;
/// The longest clip (`idle`, 1.2 s) at 60 Hz.
const CLIP_TICKS: u64 = 72;

fn doc(posture: &str) -> String {
    format!(
        "format: 1
kind: paired
name: choke
roles:
  instigator:
    assembly: humanoid.assembly.yaml
    clip: carry
{posture}    scale: 0.125
  target:
    assembly: humanoid.assembly.yaml
    clip: idle
    scale: 0.125
align:
  reference: instigator
  moves: instigator
  ticks: 12
  offsets:
    target:
      translation:
        - 0.0
        - 0.0
        - -0.5
      yaw_deg: 180.0
events:
  - name: start
    t: 0.0
  - name: grab
    t: 0.25
  - name: release
    t: 0.9
"
    )
}

const POSTURES: &str = "        postures:
          low:
            shape:
              upright:
                height: 1.0
          tall:
            shape:
              upright:
                height: 3.0
";

#[derive(Clone)]
struct Cfg {
    start_at: u64,
    doc: &'static str,
}

type Log = Arc<Mutex<Vec<(u64, ClipEvent)>>>;

struct Run {
    engine: Engine,
    root: PathBuf,
    log: Log,
    instigator: EntityId,
    target: EntityId,
}

fn find(world: &World, name: &str) -> Option<EntityId> {
    world.entities().find(|id| {
        world
            .get_unrecorded::<Name>(*id)
            .ok()
            .flatten()
            .is_some_and(|n| n.0 == name)
    })
}

fn driver(cfg: Cfg) -> impl Fn(&mut World, &SystemCtx) + Send + Sync + 'static {
    move |world, ctx| {
        let (Some(i), Some(t)) = (find(world, "instigator"), find(world, "target")) else {
            return;
        };
        if ctx.frame.as_u64() == cfg.start_at {
            start_paired(
                world,
                cfg.doc,
                &[("instigator", i), ("target", t)],
                0.0,
                ctx.game_instant,
            )
            .expect("the action starts");
        }
        for e in [i, t] {
            if let Some(step) = paired_motion(world, e, ctx) {
                let _ = move_character(world, e, step.displacement);
                let _ = turn_character(world, e, step.yaw);
            }
        }
    }
}

fn run(name: &str, extra_doc: &str, scene_text: &str, start_at: u64) -> Run {
    let root =
        std::env::temp_dir().join(format!("ludion-paired-run-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&gallery().join("voxels"), &root.join("voxels"));
    std::fs::write(root.join(RUN), doc(extra_doc)).unwrap();
    let log: Log = Arc::default();
    let sink = Arc::clone(&log);
    let mut engine = Engine::builder()
        .project_root(&root)
        .system("driver", driver(Cfg { start_at, doc: RUN }))
        .system("animation_events", animation_events)
        .system("paired_update", paired_update)
        .system("recorder", move |world: &mut World, ctx: &SystemCtx| {
            if let Some(events) = world.resource::<ludion_engine::AnimationEvents>() {
                let mut log = sink.lock().unwrap();
                for event in events.iter().filter(|e| e.clip.starts_with("paired:")) {
                    log.push((ctx.frame.as_u64(), event.clone()));
                }
            }
        })
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(scene_text)
        .expect("the scene loads");
    let instigator = named(&mut engine, "instigator");
    let target = named(&mut engine, "target");
    Run {
        engine,
        root,
        log,
        instigator,
        target,
    }
}

fn posture_scene() -> String {
    scene("").replace(
        "        height: 1.7\n",
        &format!("        height: 1.7\n{POSTURES}"),
    )
}

impl Run {
    fn tick(&mut self, n: u64) {
        for _ in 0..n {
            self.engine.advance_live();
        }
    }
    /// Advances until the sim has run frame `frame` (inclusive).
    fn until(&mut self, frame: u64) {
        while self.engine.frame().as_u64() <= frame {
            self.engine.advance_live();
        }
    }
    fn action(&self, id: EntityId) -> Option<PairedAction> {
        self.engine
            .world()
            .get::<PairedAction>(id)
            .ok()
            .flatten()
            .cloned()
    }
    fn ignores(&self, id: EntityId) -> bool {
        self.engine.world().has::<IgnoreCollisions>(id)
    }
    fn events(&self) -> Vec<(u64, EntityId, String)> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|(t, e)| (*t, e.entity, e.name.clone()))
            .collect()
    }
    /// The heading of the entity's -Z axis in radians, as `paired::heading` reads it (an Euler Y
    /// angle alone cannot say it: a yaw of pi is stored with x and z near 180).
    fn heading(&self, id: EntityId) -> f32 {
        let rotation = self
            .engine
            .world()
            .get::<Transform>(id)
            .unwrap()
            .unwrap()
            .rotation;
        let [x, y, z, w] = ludion_core::math::euler_xyz_degrees_to_quat(rotation);
        ludion_core::math::atan2(2.0 * (x * z + w * y), 1.0 - 2.0 * (x * x + y * y))
    }
    fn done(self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn at(tick: u64) -> SimTime {
    SimTime::from_ticks(tick, TickRate::DEFAULT)
}

#[test]
fn the_warp_lands_on_the_authored_relative_pose_and_both_clips_start_together() {
    let mut r = run("warp", "", &scene(""), START);
    r.until(START);
    let (i, t) = (r.instigator, r.target);
    let animation = |r: &Run, id| {
        r.engine
            .world()
            .get::<Animation>(id)
            .unwrap()
            .unwrap()
            .clone()
    };
    let (a, b) = (animation(&r, i), animation(&r, t));
    assert_eq!((a.clip.as_str(), b.clip.as_str()), ("carry", "idle"));
    assert_eq!(a.started, b.started);
    assert_eq!((a.speed, b.speed), (1.0, 1.0));
    assert!(r.ignores(i) && r.ignores(t), "both look through each other");
    r.until(START + 13);
    let feet = r
        .engine
        .world()
        .get::<Transform>(i)
        .unwrap()
        .unwrap()
        .translation;
    assert!(
        (feet[0] - 3.0).abs() < 1e-4 && (feet[2] - 1.5).abs() < 1e-4,
        "{feet:?}"
    );
    let off = (r.heading(i) - std::f32::consts::PI).rem_euclid(std::f32::consts::TAU);
    assert!(
        off < 1e-4 || std::f32::consts::TAU - off < 1e-4,
        "yaw off by {off}"
    );
    r.done();
}

#[test]
fn every_event_fires_once_per_participant_and_the_end_clears_the_state() {
    let mut r = run("events", "", &scene(""), START);
    r.until(START + CLIP_TICKS + 4);
    let (i, t) = (r.instigator, r.target);
    let (lo, hi) = if i < t { (i, t) } else { (t, i) };
    let events = r.events();
    for name in ["start", "grab", "release", "end"] {
        let hits: Vec<_> = events.iter().filter(|(_, _, n)| n == name).collect();
        assert_eq!(hits.len(), 2, "{name}: {events:?}");
        assert_eq!(hits[0].0, hits[1].0, "{name} fires on one tick");
        assert_eq!((hits[0].1, hits[1].1), (lo, hi), "{name}: ascending ids");
    }
    let tick_of = |n: &str| events.iter().find(|(_, _, e)| e == n).unwrap().0;
    assert_eq!(
        tick_of("start"),
        START,
        "a t:0 event fires on the start tick"
    );
    assert_eq!(tick_of("end"), START + CLIP_TICKS);
    assert!(r.action(i).is_none() && r.action(t).is_none());
    assert!(!r.ignores(i) && !r.ignores(t));
    r.done();
}

#[test]
fn a_game_write_to_a_clip_aborts_both_and_warns_once() {
    let mut r = run("lockstep", "", &scene(""), START);
    r.until(START + 3);
    let t = r.target;
    let frame = r.engine.frame();
    crossfade(
        r.engine
            .world_mut()
            .get_mut::<Animation>(t)
            .unwrap()
            .unwrap(),
        "idle",
        0.0,
        ludion_core::time::FrameIndex::new(frame.as_u64()),
    );
    r.tick(3);
    assert!(r.action(r.instigator).is_none() && r.action(t).is_none());
    assert!(!r.ignores(r.instigator) && !r.ignores(t));
    let diagnostics = r
        .engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .unwrap();
    assert_eq!(
        diagnostics.reported(code::PAIRED_LOCKSTEP_BROKEN).count(),
        1
    );
    r.done();
}

#[test]
fn abort_ends_every_participant_and_refuses_a_non_participant() {
    let mut r = run("abort", "", &scene(""), START);
    r.until(START + 2);
    let (i, t) = (r.instigator, r.target);
    let cast = abort_paired(r.engine.world_mut(), t).expect("in an action");
    assert_eq!(
        cast,
        vec![("instigator".to_owned(), i), ("target".to_owned(), t)]
    );
    assert!(r.action(i).is_none() && r.action(t).is_none());
    assert!(!r.ignores(i) && !r.ignores(t));
    let refused = abort_paired(r.engine.world_mut(), t).unwrap_err();
    assert_eq!(refused.code, code::PAIRED_NOT_PLAYING);
    r.done();
}

#[test]
fn a_ragdoll_switch_ends_only_that_role() {
    let mut r = run("ragdoll", "", &scene(""), START);
    r.until(START + 10);
    let (i, t) = (r.instigator, r.target);
    let data = load_assembly(
        ludion_assets::AssetRoot::content(&r.root),
        "voxels/humanoid.assembly.yaml",
    )
    .unwrap();
    let frame = r.engine.frame().as_u64();
    switch_to_ragdoll(r.engine.world_mut(), t, &data, at(frame)).expect("a valid switch");
    assert!(r.engine.world().get::<Mesh>(t).unwrap().is_none());
    r.tick(1);
    assert!(r.action(t).is_none(), "the ragdolled role ended");
    assert!(r.action(i).is_some(), "the instigator plays on");
    assert!(!r.ignores(i), "its partner is no longer looked through");
    r.until(START + CLIP_TICKS + 4);
    let events = r.events();
    assert!(
        events.iter().any(|(_, id, n)| *id == i && n == "end"),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|(_, id, n)| *id == t && n == "end"),
        "{events:?}"
    );
    assert!(r.action(i).is_none());
    r.done();
}

#[test]
fn a_new_action_mid_action_leaves_the_old_partner_playing_to_its_own_end() {
    let extra = "  - name: third\n    components:\n      Transform:\n        translation:\n          - 6.0\n          - 0.0\n          - 2.0\n        scale:\n          - 0.125\n          - 0.125\n          - 0.125\n      Mesh:\n        shape:\n          assembly: voxels/humanoid.assembly.yaml\n      CharacterMotor:\n        radius: 0.35\n        height: 1.7\n";
    let mut r = run("chain", "", &scene(extra), u64::MAX);
    let (i, t) = (r.instigator, r.target);
    let third = named(&mut r.engine, "third");
    r.tick(3);
    let f = r.engine.frame().as_u64();
    start_paired(
        r.engine.world_mut(),
        RUN,
        &[("instigator", i), ("target", t)],
        0.0,
        at(f),
    )
    .unwrap();
    r.tick(10);
    let f = r.engine.frame().as_u64();
    start_paired(
        r.engine.world_mut(),
        RUN,
        &[("instigator", i), ("target", third)],
        0.0,
        at(f),
    )
    .expect("chains");
    assert_eq!(
        r.action(t).map(|a| a.role),
        Some("target".to_owned()),
        "the old target plays on"
    );
    assert!(!r.ignores(t), "but no longer looks through the instigator");
    assert_eq!(r.action(third).map(|a| a.role), Some("target".to_owned()));
    r.tick(CLIP_TICKS + 4);
    let events = r.events();
    for who in [t, third] {
        assert!(
            events.iter().any(|(_, id, n)| *id == who && n == "end"),
            "{who:?}: {events:?}"
        );
    }
    r.done();
}

#[test]
fn a_posture_is_set_at_start_and_a_blocked_one_rolls_back() {
    let mut r = run("posture", "    posture: low\n", &posture_scene(), u64::MAX);
    let (i, t) = (r.instigator, r.target);
    r.tick(3);
    let f = r.engine.frame().as_u64();
    start_paired(
        r.engine.world_mut(),
        RUN,
        &[("instigator", i), ("target", t)],
        0.0,
        at(f),
    )
    .unwrap();
    let state = r
        .engine
        .world()
        .get::<ludion_physics::CharacterMotorState>(i)
        .unwrap()
        .unwrap();
    assert_eq!(state.posture.as_deref(), Some("low"));
    r.done();

    let ceiling = "  - name: ceiling\n    components:\n      Transform:\n        translation:\n          - 3.4\n          - 2.5\n          - 1.5\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 1.0\n              - 0.2\n              - 1.0\n        body: static\n";
    let text = scene(ceiling).replace(
        "        height: 1.7\n",
        &format!("        height: 1.7\n{POSTURES}"),
    );
    let mut r = run("blocked", "    posture: tall\n", &text, u64::MAX);
    let (i, t) = (r.instigator, r.target);
    r.tick(3);
    let f = r.engine.frame().as_u64();
    let errors = start_paired(
        r.engine.world_mut(),
        RUN,
        &[("instigator", i), ("target", t)],
        0.0,
        at(f),
    )
    .unwrap_err();
    assert_eq!(errors[0].code, code::PAIRED_CAST_INVALID, "{errors:?}");
    assert!(r.action(i).is_none() && !r.ignores(i), "nothing started");
    r.done();
}

#[test]
fn a_held_frame_fires_nothing_and_steps_nothing() {
    let mut r = run("held", "", &scene(""), START);
    r.until(START);
    let (i, before) = (r.instigator, r.events().len());
    let ctx = SystemCtx::new(r.engine.frame(), TickRate::DEFAULT)
        .with_clock(&GameClock::default(), false);
    let step = paired_motion(r.engine.world(), i, &ctx).expect("on the action");
    assert_eq!(step.displacement, [0.0; 3]);
    paired_update(r.engine.world_mut(), &ctx);
    assert_eq!(r.events().len(), before);
    r.done();
}

fn trace(name: &str, split: Option<u64>) -> Vec<u64> {
    let mut r = run(name, "", &scene(""), START);
    let mut hashes = Vec::new();
    let total = START + CLIP_TICKS + 4;
    while r.engine.frame().as_u64() < total {
        r.engine.advance_live();
        hashes.push(r.engine.world_hash().unwrap().as_u64());
        if Some(r.engine.frame().as_u64()) == split {
            let snapshot = match r.engine.execute(Command::EngineSnapshot).unwrap().body {
                ResponseBody::Snapshot { snapshot } => snapshot,
                other => panic!("expected a snapshot, got {other:?}"),
            };
            r.engine
                .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
                .unwrap();
        }
    }
    r.done();
    hashes
}

#[test]
fn a_restore_mid_warp_and_mid_clip_continues_the_same_trace_at_every_thread_count() {
    let whole = trace("whole", None);
    for split in [START + 6, START + 40] {
        assert_eq!(
            trace(&format!("split-{split}"), Some(split)),
            whole,
            "split at {split}"
        );
    }
    for n in [2, 8] {
        let got = ludion_core::par::with_threads(n, || trace(&format!("threads-{n}"), None));
        assert_eq!(got, whole, "{n} threads");
    }
}

#[test]
fn a_game_that_never_starts_an_action_hashes_like_one_without_the_system() {
    let hashes = |with: bool| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
        let mut builder = Engine::builder().project_root(root);
        if with {
            builder = builder.system("paired_update", paired_update);
        }
        let mut engine = builder.build().unwrap();
        engine.load_inline_scene(&scene("")).unwrap();
        (0..10)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(true), hashes(false));
}
