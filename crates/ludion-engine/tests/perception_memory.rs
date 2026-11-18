//! Runtime proof for stimulus memory (`Plan/perception.md` Wave 2): records are refreshed by sight,
//! kept as a ghost marker, forgotten by gameplay time, trimmed to `max_records`, reported into by a
//! game, hashed, snapshotted and thread-count invariant.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, EngineBuilder, Name, PerceptionConfig, PerceptionResults, Sense, Stimulus,
    StimulusRecord, Transform, forget, forget_all, perception_update, report_stimulus, stimuli,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

const FORGET_TICKS: u64 = 60;

/// An entity named `name` at `at` with the extra component YAML (indented to 6).
fn entity(name: &str, at: [f32; 3], components: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n{components}",
        at[0], at[1], at[2]
    )
}

/// A guard at the origin with the given zones (block YAML at 10) and `memory` YAML (at 8, may be empty).
fn guard(zones: &str, memory: &str) -> String {
    let memory = if memory.is_empty() {
        String::new()
    } else {
        format!("        memory:\n{memory}")
    };
    entity(
        "guard",
        [0.0; 3],
        &format!("      Perceiver:\n        sight:\n{zones}{memory}"),
    )
}

const WIDE: &str =
    "          - name: wide\n            range: 40.0\n            half_angle_deg: 80.0\n";

/// A player-class target with one unblockable point at its origin.
fn target(name: &str, at: [f32; 3]) -> String {
    let class = "player";
    entity(
        name,
        at,
        &format!(
            "      SightTarget:\n        class: {class}\n        points:\n          - offset:\n              - 0.0\n              - 0.0\n              - 0.0\n"
        ),
    )
}

/// A wall on `cover_high` in front of the guard: half width 1 at z = -5.
fn wall() -> String {
    entity(
        "wall",
        [0.0, 1.5, -5.0],
        "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 1.0\n              - 1.5\n              - 0.1\n        body: static\n        layer: cover_high\n",
    )
}

fn scene(entities: &[String]) -> String {
    let mut out = String::from(
        "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: cover_high\nentities:\n",
    );
    for e in entities {
        out += e;
    }
    out
}

/// A target whose single point is blocked by `cover_high`.
fn walker(at: [f32; 3]) -> String {
    entity(
        "walker",
        at,
        "      SightTarget:\n        class: player\n        points:\n          - offset:\n              - 0.0\n              - 1.0\n              - 0.0\n            blocked_by:\n              - cover_high\n",
    )
}

fn builder() -> EngineBuilder {
    Engine::builder().perception(PerceptionConfig {
        classes: vec!["player".into(), "decoy".into()],
    })
}

fn engine_with(builder: EngineBuilder, text: &str) -> Engine {
    let mut engine = builder
        .system("perception_update", perception_update)
        .build()
        .expect("boots");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn engine(text: &str) -> Engine {
    engine_with(builder(), text)
}

fn run(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

fn find(engine: &mut Engine, wanted: &str) -> EntityId {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .find(|(_, (name,))| name.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

fn now(engine: &Engine) -> u64 {
    engine
        .world()
        .resource::<PerceptionResults>()
        .unwrap()
        .tick()
}

fn records(engine: &mut Engine, perceiver: &str) -> Vec<StimulusRecord> {
    let id = find(engine, perceiver);
    stimuli(engine.world_mut(), id)
        .into_iter()
        .map(|(_, r)| r)
        .collect()
}

fn sources(engine: &mut Engine, perceiver: &str) -> Vec<Option<EntityId>> {
    records(engine, perceiver)
        .iter()
        .map(|r| r.source)
        .collect()
}

/// Places `walker` at x = -5 + 0.1 * frame until frame 20, then behind the wall at the origin line.
fn walker_path(world: &mut World, ctx: &SystemCtx) {
    let at = if ctx.frame.as_u64() < 20 {
        [-5.0 + 0.1 * ctx.frame.as_u64() as f32, 0.0, -10.0]
    } else {
        [0.0, 0.0, -10.0]
    };
    let ids: Vec<EntityId> = world
        .query_ids::<(&Name,)>()
        .filter(|(_, (n,))| n.0 == "walker")
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        world.get_mut::<Transform>(id).unwrap().unwrap().translation = at;
    }
}

fn walker_scene(memory: &str) -> String {
    scene(&[guard(WIDE, memory), walker([-5.0, 0.0, -10.0]), wall()])
}

fn walking(text: &str, hold: bool) -> Engine {
    let mut b = builder().system("walker_path", walker_path);
    if hold {
        b = b.system("hold_window", hold_window);
    }
    engine_with(b, text)
}

fn hold_window(world: &mut World, ctx: &SystemCtx) {
    if (40..=69).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

const ONE_SECOND: &str = "          forget_sight_s: 1.0\n";

#[test]
fn the_ghost_keeps_its_position_after_line_of_sight_breaks() {
    let mut engine = walking(&walker_scene(ONE_SECOND), false);
    let walker = find(&mut engine, "walker");
    let mut last_seen = None;
    for _ in 0..40 {
        engine.advance_live();
        let all = records(&mut engine, "guard");
        let Some(record) = all.first() else { continue };
        assert_eq!(record.source, Some(walker));
        assert_eq!(record.sense, Sense::Sight);
        if record.sensed_now {
            last_seen = Some((record.last_position, record.last_sensed_tick));
        }
    }
    let (position, tick) = last_seen.expect("the walker was seen first");
    assert!(
        position[0] < -2.0,
        "seen only before the wall: {position:?}"
    );
    let ghost = &records(&mut engine, "guard")[0];
    assert!(!ghost.sensed_now);
    assert_eq!(ghost.last_position, position, "the ghost marker holds");
    assert_eq!(ghost.last_sensed_tick, tick);
    assert!(ghost.first_sensed_tick <= tick);
    assert!(
        ghost.last_direction[0] > 0.9,
        "it was walking +x: {ghost:?}"
    );
    assert_eq!(ghost.strength, 1.0);
}

/// The engine frame the walker's record disappears on, and how many frames the clock held.
fn forgotten_at(hold: bool) -> (usize, usize, u64) {
    let mut engine = walking(&walker_scene(ONE_SECOND), hold);
    let mut last_tick = None;
    let mut ticks = Vec::new();
    for frame in 0..400 {
        engine.advance_live();
        ticks.push(now(&engine));
        let all = records(&mut engine, "guard");
        if let Some(record) = all.first() {
            last_tick = Some(record.last_sensed_tick);
        } else if let Some(seen) = last_tick {
            let held = ticks.windows(2).filter(|w| w[0] == w[1]).count();
            return (frame, held, seen);
        }
    }
    panic!("never forgotten");
}

#[test]
fn a_record_is_forgotten_exactly_at_the_forget_time_after_the_last_sight() {
    let mut engine = walking(&walker_scene(ONE_SECOND), false);
    let mut seen = None;
    for _ in 0..200 {
        engine.advance_live();
        let all = records(&mut engine, "guard");
        match (all.first(), seen) {
            (Some(r), _) => {
                seen = Some(r.last_sensed_tick);
                assert!(
                    now(&engine) - r.last_sensed_tick < FORGET_TICKS,
                    "one tick early"
                );
            }
            (None, Some(seen)) => {
                assert_eq!(
                    now(&engine),
                    seen + FORGET_TICKS,
                    "forgotten on the deadline"
                );
                return;
            }
            (None, None) => {}
        }
    }
    panic!("never forgotten");
}

#[test]
fn a_game_clock_hold_spanning_the_deadline_delays_the_forgetting_by_its_length() {
    let (plain, plain_held, seen) = forgotten_at(false);
    assert_eq!(plain_held, 0);
    let (held_frame, held, held_seen) = forgotten_at(true);
    assert_eq!(held_seen, seen, "the hold starts after the last sighting");
    assert!(held >= 30, "the hold lasted: {held}");
    assert_eq!(held_frame - plain, held, "offset by the hold length");
}

#[test]
fn max_records_evicts_the_least_recently_sensed_then_the_lowest_source() {
    // a and b are seen; b then leaves the wedge; c arrives: b is the stale one, though a has the
    // lowest source id.
    fn script(world: &mut World, ctx: &SystemCtx) {
        let frame = ctx.frame.as_u64();
        let behind = [0.0, 0.0, 10.0];
        let ahead = |x: f32| [x, 0.0, -10.0];
        let spots = [
            ("a", ahead(-3.0)),
            ("b", if frame < 10 { ahead(3.0) } else { behind }),
            ("c", if frame < 20 { behind } else { ahead(0.0) }),
        ];
        for (name, at) in spots {
            let id = world
                .query_ids::<(&Name,)>()
                .find(|(_, (n,))| n.0 == name)
                .map(|(id, _)| id)
                .unwrap();
            world.get_mut::<Transform>(id).unwrap().unwrap().translation = at;
        }
    }
    let text = scene(&[
        guard(WIDE, "          max_records: 2\n"),
        target("a", [-3.0, 0.0, -10.0]),
        target("b", [3.0, 0.0, -10.0]),
        target("c", [0.0, 0.0, 10.0]),
    ]);
    let mut engine = engine_with(builder().system("script", script), &text);
    let (a, b, c) = (
        find(&mut engine, "a"),
        find(&mut engine, "b"),
        find(&mut engine, "c"),
    );
    run(&mut engine, 15);
    assert_eq!(sources(&mut engine, "guard"), [Some(a), Some(b)]);
    run(&mut engine, 15);
    assert_eq!(
        sources(&mut engine, "guard"),
        [Some(a), Some(c)],
        "b was stalest"
    );
}

#[test]
fn a_tie_evicts_the_lowest_source() {
    let text = scene(&[
        guard(WIDE, "          max_records: 2\n"),
        target("a", [-3.0, 0.0, -10.0]),
        target("b", [0.0, 0.0, -10.0]),
        target("c", [3.0, 0.0, -10.0]),
    ]);
    let mut engine = engine(&text);
    let (b, c) = (find(&mut engine, "b"), find(&mut engine, "c"));
    run(&mut engine, 10);
    assert_eq!(sources(&mut engine, "guard"), [Some(b), Some(c)]);
}

#[test]
fn two_zones_seeing_one_target_make_one_record_with_the_lowest_zone() {
    let both = format!(
        "{WIDE}          - name: narrow\n            range: 40.0\n            half_angle_deg: 20.0\n"
    );
    let mut engine = engine(&scene(&[
        guard(&both, "          max_records: 8\n"),
        target("t", [0.0, 0.0, -10.0]),
    ]));
    run(&mut engine, 5);
    let all = records(&mut engine, "guard");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].zone, 0);

    // Only the second zone detects a `player`: the record carries zone 1.
    let split = "          - name: decoys\n            range: 40.0\n            half_angle_deg: 80.0\n            detects:\n              - decoy\n          - name: players\n            range: 40.0\n            half_angle_deg: 80.0\n            detects:\n              - player\n";
    let mut engine = engine_with(
        builder(),
        &scene(&[
            guard(split, "          max_records: 8\n"),
            target("t", [0.0, 0.0, -10.0]),
        ]),
    );
    run(&mut engine, 5);
    assert_eq!(records(&mut engine, "guard")[0].zone, 1);
}

fn hearing(at: [f32; 3]) -> Stimulus {
    Stimulus {
        sense: Sense::Hearing,
        source: None,
        at,
        strength: 1.0,
    }
}

/// Reports from inside a tick, the way a game system does.
fn reporter(world: &mut World, ctx: &SystemCtx) {
    let find = |world: &mut World, wanted: &str| {
        world
            .query_ids::<(&Name,)>()
            .find(|(_, (n,))| n.0 == wanted)
            .map(|(id, _)| id)
            .unwrap()
    };
    let guard = find(world, "guard");
    match ctx.frame.as_u64() {
        5 => report_stimulus(world, ctx, guard, hearing([1.0, 0.0, 0.0])),
        6 => {
            report_stimulus(world, ctx, guard, hearing([2.0, 0.0, 0.0]));
            let custom = Stimulus {
                sense: Sense::Custom(7),
                strength: 0.5,
                ..hearing([0.0, 0.0, 3.0])
            };
            report_stimulus(world, ctx, guard, custom);
        }
        7 => {
            let deaf = find(world, "deaf");
            report_stimulus(world, ctx, deaf, hearing([0.0; 3]));
            report_stimulus(world, ctx, guard, hearing([f32::NAN, 0.0, 0.0]));
        }
        _ => {}
    }
}

fn reported(engine: &Engine) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::STIMULUS_REPORT_INVALID)
        .map(String::from)
        .collect()
}

#[test]
fn reported_stimuli_merge_per_sense_and_bad_ones_report_l1574() {
    let deaf = entity(
        "deaf",
        [0.0; 3],
        &format!("      Perceiver:\n        sight:\n{WIDE}"),
    );
    let text = scene(&[guard(WIDE, "          max_records: 8\n"), deaf]);
    let mut engine = engine_with(builder().system("reporter", reporter), &text);
    run(&mut engine, 6);
    let heard = records(&mut engine, "guard");
    assert_eq!(heard.len(), 1, "the hearing record exists after frame 5");
    assert!(heard[0].sensed_now || now(&engine) > heard[0].last_sensed_tick);
    run(&mut engine, 1);
    let all = records(&mut engine, "guard");
    assert_eq!(
        all.iter().map(|r| r.sense).collect::<Vec<_>>(),
        [Sense::Hearing, Sense::Custom(7)]
    );
    assert_eq!(all[0].last_position, [2.0, 0.0, 0.0], "merged, refreshed");
    assert_eq!(all[0].first_sensed_tick, 5);
    assert_eq!(all[0].last_direction, [0.0; 3], "no direction for hearing");
    assert_eq!(all[1].strength, 0.5);
    run(&mut engine, 2);
    assert_eq!(
        records(&mut engine, "guard").len(),
        2,
        "the NaN stimulus was dropped"
    );
    assert!(records(&mut engine, "deaf").is_empty());
    let keys = reported(&engine);
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys.iter().any(|k| k.contains("no_memory")));
    assert!(keys.iter().any(|k| k.contains("not_finite")));
}

#[test]
fn reporting_to_a_dead_perceiver_is_l1574_and_creates_nothing() {
    let mut engine = engine(&scene(&[guard(WIDE, "          max_records: 8\n")]));
    let id = find(&mut engine, "guard");
    let ctx = SystemCtx::new(
        ludion_core::time::FrameIndex::new(0),
        ludion_core::time::TickRate::DEFAULT,
    );
    engine.world_mut().despawn(id).unwrap();
    report_stimulus(engine.world_mut(), &ctx, id, hearing([0.0; 3]));
    assert!(reported(&engine).iter().any(|k| k.contains("dead")));
    let mut count = 0;
    count += engine.world_mut().query_ids::<(&StimulusRecord,)>().count();
    assert_eq!(count, 0);
}

#[test]
fn forget_and_forget_all_drop_records() {
    let text = scene(&[
        guard(WIDE, "          max_records: 8\n"),
        target("a", [-3.0, 0.0, -10.0]),
        target("b", [3.0, 0.0, -10.0]),
    ]);
    let mut engine = engine(&text);
    run(&mut engine, 3);
    let (guard, a, b) = (
        find(&mut engine, "guard"),
        find(&mut engine, "a"),
        find(&mut engine, "b"),
    );
    let ctx = SystemCtx::new(
        ludion_core::time::FrameIndex::new(2),
        ludion_core::time::TickRate::DEFAULT,
    );
    report_stimulus(engine.world_mut(), &ctx, guard, hearing([0.0; 3]));
    assert_eq!(stimuli(engine.world_mut(), guard).len(), 3);
    forget(engine.world_mut(), guard, Some(a));
    let left: Vec<_> = stimuli(engine.world_mut(), guard)
        .iter()
        .map(|(_, r)| r.source)
        .collect();
    assert_eq!(left, [Some(b), None], "sight sorts before hearing");
    forget(engine.world_mut(), guard, None);
    assert_eq!(stimuli(engine.world_mut(), guard).len(), 1);
    forget_all(engine.world_mut(), guard);
    assert!(stimuli(engine.world_mut(), guard).is_empty());
}

#[test]
fn records_despawn_with_their_perceiver_on_the_next_update() {
    let text = scene(&[
        guard(WIDE, "          max_records: 8\n"),
        target("a", [0.0, 0.0, -10.0]),
    ]);
    let mut engine = engine(&text);
    run(&mut engine, 3);
    let id = find(&mut engine, "guard");
    assert_eq!(stimuli(engine.world_mut(), id).len(), 1);
    engine.world_mut().despawn(id).unwrap();
    run(&mut engine, 1);
    let left = engine.world_mut().query_ids::<(&StimulusRecord,)>().count();
    assert_eq!(left, 0);
}

#[test]
fn a_dead_source_keeps_its_record_until_it_ages_out() {
    let text = scene(&[guard(WIDE, ONE_SECOND), target("a", [0.0, 0.0, -10.0])]);
    let mut engine = engine(&text);
    run(&mut engine, 3);
    let a = find(&mut engine, "a");
    engine.world_mut().despawn(a).unwrap();
    run(&mut engine, 5);
    let kept = records(&mut engine, "guard");
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].sensed_now);
    run(&mut engine, 70);
    assert!(records(&mut engine, "guard").is_empty());
}

#[test]
fn a_perceiver_without_memory_spawns_nothing() {
    let text = scene(&[
        entity(
            "guard",
            [0.0; 3],
            &format!("      Perceiver:\n        sight:\n{WIDE}"),
        ),
        target("a", [0.0, 0.0, -10.0]),
    ]);
    let mut engine = engine(&text);
    run(&mut engine, 20);
    let count = engine.world_mut().query_ids::<(&StimulusRecord,)>().count();
    assert_eq!(count, 0);
}

fn trace(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn the_registry_lists_the_two_entity_refs_of_a_record() {
    let registry = ludion_engine::builtin_registry();
    let info = registry
        .iter()
        .find(|i| i.name() == "StimulusRecord")
        .unwrap();
    let mut refs: Vec<String> = info
        .entity_refs()
        .iter()
        .map(|r| r.suffix.clone())
        .collect();
    refs.sort();
    assert_eq!(refs, ["/perceiver", "/source"]);
}

#[test]
fn records_are_hashed_and_a_restore_rejoins_the_uninterrupted_run() {
    let text = walker_scene(ONE_SECOND);
    let game = || builder().system("walker_path", walker_path);
    let mut whole = engine_with(game(), &text);
    let reference = trace(&mut whole, 120);

    let mut first = engine_with(game(), &text);
    let head = trace(&mut first, 40);
    assert_eq!(head, reference[..40]);
    let snapshot = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    assert!(
        snapshot.to_string().contains("StimulusRecord"),
        "the records ride the snapshot"
    );
    let mut resumed = engine_with(game(), &text);
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: snapshot as Value,
        }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(trace(&mut resumed, 80), reference[40..]);

    let mut plain = engine_with(game(), &walker_scene(""));
    let without = trace(&mut plain, 120);
    assert_ne!(
        without, reference,
        "memory moves the hash: records are entities"
    );
}

#[test]
fn record_sets_are_identical_at_any_thread_count() {
    let text = scene(&[
        guard(WIDE, "          max_records: 3\n"),
        target("a", [-3.0, 0.0, -10.0]),
        target("b", [0.0, 0.0, -10.0]),
        target("c", [3.0, 0.0, -10.0]),
        target("d", [6.0, 0.0, -10.0]),
    ]);
    let at = |threads: usize| {
        par::with_threads(threads, || {
            let mut engine = engine(&text);
            let hashes = trace(&mut engine, 30);
            (hashes, records(&mut engine, "guard"))
        })
    };
    assert_eq!(at(1), at(8));
}

#[test]
fn a_memoryless_world_hashes_like_one_that_never_ran_the_update() {
    let text = scene(&[
        entity(
            "guard",
            [0.0; 3],
            &format!("      Perceiver:\n        sight:\n{WIDE}"),
        ),
        target("a", [0.0, 0.0, -10.0]),
    ]);
    let mut with = engine(&text);
    let mut without = builder().build().expect("boots");
    without.load_inline_scene(&text).unwrap();
    assert_eq!(trace(&mut with, 20), trace(&mut without, 20));
}
