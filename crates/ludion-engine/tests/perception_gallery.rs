//! `Plan/perception.md` Wave 4b: the gallery's stealth corridor (`examples/gallery/perception.yaml`)
//! stepped for real. Under a plain `Engine::builder()` the scene is a static render; this file's
//! builder installs `PerceptionConfig`, walks the intruder through standing, crouching and prone
//! stances, and emits its footsteps, then traces the per-tick `WorldHash` against a committed
//! reference (`examples/gallery/perception.trace.txt`) so a second pinned machine has something to
//! reproduce bit-exactly. The `perception` trace channel is exercised over the same run.

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Name, Noise, PendingNoise, PerceptionConfig, SightTarget,
    StimulusRecord, TraceChannel, Transform, emit_noise, perception_update, trace, write_jsonl,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::InputScript;
use serde_json::{Value, json};

const TICKS: usize = 300;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
/// The tick the snapshot is taken after: the footstep emitted on tick 149 is still pending.
const SNAPSHOT_AT: usize = 150;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

#[derive(Clone, Copy, PartialEq)]
enum Stance {
    Stand,
    Crouch,
    Prone,
}

/// The stance and the feet at gameplay tick `t`: a closed form, so a restored run needs no state.
fn route(t: u64) -> (Stance, [f32; 3]) {
    let t = t as f32;
    if t < 120.0 {
        (Stance::Stand, [2.0 + 0.1 * t, 0.0, 0.0])
    } else if t < 200.0 {
        (Stance::Crouch, [14.0 + 0.05 * (t - 120.0), 0.0, 0.0])
    } else {
        (Stance::Prone, [18.0 + 0.02 * (t - 200.0), 0.0, 0.0])
    }
}

/// The sample points of a stance; the lower the point, the more cover classes hide it.
fn points(stance: Stance) -> Value {
    let high = ["wall", "cover_high", "cover_overhead"];
    let all = ["wall", "cover_low", "cover_high", "cover_overhead"];
    let point =
        |y: f32, blocked: &[&str]| json!({ "offset": [0.0, y, 0.0], "blocked_by": blocked });
    match stance {
        Stance::Stand => json!([point(1.6, &high), point(1.0, &high), point(0.4, &all)]),
        Stance::Crouch => json!([point(0.9, &high), point(0.4, &all)]),
        Stance::Prone => json!([point(0.2, &all)]),
    }
}

fn find(world: &mut World, wanted: &str) -> EntityId {
    world
        .query_ids::<(&Name,)>()
        .find(|(_, (name,))| name.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

/// Moves the intruder and rewrites its sample points in place when the stance changes.
fn intruder_route(world: &mut World, ctx: &SystemCtx) {
    let intruder = find(world, "intruder");
    let (stance, at) = route(ctx.game_frame.as_u64());
    world
        .get_mut::<Transform>(intruder)
        .unwrap()
        .unwrap()
        .translation = at;
    let wanted: SightTarget = serde_json::from_value(json!({
        "class": "player",
        "points": points(stance),
    }))
    .unwrap();
    let target = world.get_mut::<SightTarget>(intruder).unwrap().unwrap();
    if target.points != wanted.points {
        target.points = wanted.points;
    }
}

/// A footstep every 15 ticks, after `perception_update`, so it pends across the tick boundary.
fn footsteps(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.game_frame.as_u64();
    if frame % 15 != 14 {
        return;
    }
    let intruder = find(world, "intruder");
    let (stance, at) = route(frame);
    let radius = match stance {
        Stance::Stand => 6.0,
        Stance::Crouch => 3.0,
        Stance::Prone => 1.5,
    };
    let noise = Noise {
        at,
        radius,
        loudness: 0.8,
        kind: "footstep".into(),
        source: Some(intruder),
        surface: None,
        occlusion: false,
    };
    emit_noise(world, ctx, noise);
}

fn builder() -> EngineBuilder {
    Engine::builder()
        .perception(PerceptionConfig {
            classes: vec!["player".into()],
            noise_kinds: vec!["footstep".into()],
            ..Default::default()
        })
        .system("intruder_route", intruder_route)
        .system("perception_update", perception_update)
        .system("footsteps", footsteps)
}

fn build() -> Engine {
    let mut engine = builder().build().expect("the engine builds");
    engine
        .load_inline_scene(
            &std::fs::read_to_string(gallery_root().join("perception.yaml"))
                .expect("the committed scene"),
        )
        .expect("the committed scene loads");
    engine
}

/// One `<tick:04> <hash:016x>` line per tick, from tick `from`.
fn trace_from(engine: &mut Engine, from: usize, to: usize) -> Vec<String> {
    (from..to)
        .map(|tick| {
            engine.advance_live();
            let hash = engine.world_hash().unwrap().as_u64();
            format!("{tick:04} {hash:016x}")
        })
        .collect()
}

fn run(n: usize) -> Vec<String> {
    trace_from(&mut build(), 0, n)
}

fn committed_trace() -> Vec<String> {
    std::fs::read_to_string(gallery_root().join("perception.trace.txt"))
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_corridor_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/perception.yaml, stepped under the perception_gallery builder\n\
# (PerceptionConfig, the intruder route and its footsteps; crates/ludion-engine/tests/perception_gallery.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(/perception_gallery/)'\n\
# The camera's period_ticks puts carried sight samples, and the footsteps put pending noises and\n\
# stimulus records, into the hash. The M4 must reproduce this file byte-identically\n\
# (Plan/perception.md, cross-arch) - never fabricated by copying the Windows trace.\n";
        std::fs::write(
            gallery_root().join("perception.trace.txt"),
            format!("{header}{}\n", trace.join("\n")),
        )
        .expect("the gallery directory is committed");
        return;
    }
    let committed = committed_trace();
    assert_eq!(
        committed.len(),
        TICKS,
        "the committed trace covers every tick"
    );
    for (tick, (actual, expected)) in trace.iter().zip(&committed).enumerate() {
        assert_eq!(actual, expected, "the trace diverges at tick {tick}");
    }
    let mut distinct: Vec<&String> = trace.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(distinct.len() > TICKS / 2, "the run barely changes state");
}

#[test]
fn the_trace_is_identical_at_every_thread_count() {
    let reference = par::with_threads(THREAD_COUNTS[0], || run(TICKS));
    for threads in &THREAD_COUNTS[1..] {
        assert_eq!(
            par::with_threads(*threads, || run(TICKS)),
            reference,
            "{threads} threads"
        );
    }
}

#[test]
fn a_snapshot_holding_pending_noise_and_memory_rejoins_the_committed_trace() {
    let committed = committed_trace();
    let mut first = build();
    let head = trace_from(&mut first, 0, SNAPSHOT_AT);
    assert_eq!(head, committed[..SNAPSHOT_AT]);
    assert!(
        first.world_mut().query_ids::<(&PendingNoise,)>().count() > 0,
        "a footstep is pending at the snapshot"
    );
    assert!(
        first.world_mut().query_ids::<(&StimulusRecord,)>().count() > 0,
        "a guard remembers the intruder at the snapshot"
    );
    let snapshot = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    let mut resumed = build();
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(
        trace_from(&mut resumed, SNAPSHOT_AT, TICKS),
        committed[SNAPSHOT_AT..]
    );
}

fn channel_lines() -> (Vec<Value>, Vec<u8>) {
    let mut engine = build();
    let records = trace(
        &mut engine,
        &InputScript::new(),
        TICKS as u64,
        &[TraceChannel::Perception],
    )
    .expect("the trace runs");
    let mut bytes = Vec::new();
    write_jsonl(&records, &mut bytes).unwrap();
    let lines = records
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    (lines, bytes)
}

#[test]
fn the_perception_channel_tells_the_run_and_is_byte_stable() {
    let (lines, bytes) = channel_lines();
    let count = |event: &str, field: &str, value: Value| {
        lines
            .iter()
            .filter(|l| l["event"] == event && l[field] == value)
            .count()
    };
    for guard in ["guard-a", "guard-b", "camera"] {
        assert!(
            count("sight", "perceiver", json!(guard)) > 0
                && lines.iter().any(|l| l["event"] == "sight"
                    && l["perceiver"] == guard
                    && l["visible"] == true),
            "{guard} sees the intruder"
        );
    }
    assert!(
        lines
            .iter()
            .any(|l| l["event"] == "sight" && l["visible"] == false),
        "somebody loses sight of the intruder"
    );
    for event in ["noise", "remember", "forget"] {
        assert!(
            lines.iter().any(|l| l["event"] == event),
            "no {event} line in the run"
        );
    }
    assert_eq!(bytes, channel_lines().1, "two runs print the same bytes");
}
