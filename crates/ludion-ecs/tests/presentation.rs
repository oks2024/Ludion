//! The presentation id range, the phases, and the guard between them (`L0028`).
//!
//! Systems run through a real [`Schedule`] — the only thing that sets the world's phase — and report
//! what they saw into a [`Log`] resource the world allowlists as an outbox.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{
    PRESENTATION_BASE, Phase, Schedule, SystemCtx, SystemPanic, World, is_presentation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const COUNTS: [usize; 3] = [1, 2, 8];

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Pos(f32);

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Vel(f32);

/// The label component the messages name entities by.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Label(String);

impl AsRef<str> for Label {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// What a system saw, by label.
#[derive(Default)]
struct Log(Vec<(&'static str, String)>);

impl Log {
    fn note(world: &mut World, label: &'static str, outcome: impl ToString) {
        world
            .resource_mut::<Log>()
            .unwrap()
            .0
            .push((label, outcome.to_string()));
    }
}

/// The ids the systems act on.
struct Targets {
    sim: EntityId,
    pres: EntityId,
}

/// A sim resource no presentation system may write.
struct SimOnly(u32);

fn ctx() -> SystemCtx {
    SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT)
}

/// `Ok` or the diagnostic's code, so one row of the guard table is one comparable string.
fn outcome<T>(result: Result<T, ludion_core::diagnostics::Diagnostic>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(diagnostic) => diagnostic.code.to_string(),
    }
}

/// One sim entity and one presentation entity, both carrying `Pos`, plus the outbox and `SimOnly`.
fn world_with_pair() -> (World, Targets) {
    let mut world = World::new();
    let sim = world.spawn();
    let pres = world.spawn_presentation();
    world.insert(sim, Pos(1.0)).unwrap();
    world.insert(pres, Pos(2.0)).unwrap();
    world.set_entity_label::<Label>();
    world.insert(sim, Label("ball".into())).unwrap();
    world.insert(pres, Label("popup/3".into())).unwrap();
    world.insert_resource(Log::default());
    world.insert_resource(SimOnly(0));
    world.allow_presentation_writes::<Log>();
    world.insert_resource(Targets { sim, pres });
    (world, Targets { sim, pres })
}

fn log(world: &World) -> Vec<(&'static str, String)> {
    world.resource::<Log>().unwrap().0.clone()
}

/// The panic message the schedule's boundary caught, if a system panicked.
fn caught(world: &mut World) -> Option<String> {
    world.remove_resource::<SystemPanic>().map(|p| p.message)
}

// --- Ids ----------------------------------------------------------------------------------------

#[test]
fn presentation_ids_start_at_the_base_and_are_recognised() {
    let mut world = World::new();
    let sim = world.spawn();
    let pres = world.spawn_presentation();
    assert_eq!(sim.index(), 0);
    assert_eq!(pres.index(), PRESENTATION_BASE);
    assert!(!is_presentation(sim));
    assert!(is_presentation(pres));
    assert!(world.is_alive(pres));
    assert_eq!(world.entities().collect::<Vec<_>>(), vec![sim]);
    assert_eq!(
        world.presentation_entities().collect::<Vec<_>>(),
        vec![pres]
    );
}

#[test]
fn presentation_spawns_and_despawns_never_move_a_sim_id() {
    let mut with_popups = World::new();
    let mut without = World::new();
    let mut expected = Vec::new();
    let mut seen = Vec::new();
    let mut popups = Vec::new();
    for round in 0..6 {
        popups.push(with_popups.spawn_presentation());
        seen.push(with_popups.spawn());
        expected.push(without.spawn());
        if round % 2 == 1 {
            let popup = popups.remove(0);
            with_popups.despawn(popup).unwrap();
            // A sim despawn interleaved with it: both worlds recycle the same sim slot.
            with_popups.despawn(seen[round - 1]).unwrap();
            without.despawn(expected[round - 1]).unwrap();
        }
    }
    assert_eq!(seen, expected);
    assert_eq!(with_popups.entities().count(), without.entities().count());
}

#[test]
fn presentation_slots_reuse_lifo_with_a_generation_bump() {
    let mut world = World::new();
    let p0 = world.spawn_presentation();
    let p1 = world.spawn_presentation();
    let p2 = world.spawn_presentation();
    assert_eq!(
        [p0, p1, p2].map(|p| p.index() - PRESENTATION_BASE),
        [0, 1, 2]
    );
    world.despawn(p1).unwrap();
    world.despawn(p2).unwrap();
    // LIFO: the most recently freed slot comes back first, one generation up.
    let back2 = world.spawn_presentation();
    let back1 = world.spawn_presentation();
    assert_eq!((back2.index(), back2.generation()), (p2.index(), 1));
    assert_eq!((back1.index(), back1.generation()), (p1.index(), 1));
    // The stale handles are errors, not hits on the new occupants.
    assert!(!world.is_alive(p1));
    assert_eq!(
        world.get::<Pos>(p2).unwrap_err().code,
        code::DEAD_ENTITY,
        "a stale presentation handle is L0002"
    );
}

#[test]
fn despawn_many_frees_each_range_in_its_own_allocator() {
    let mut world = World::new();
    let s = world.spawn();
    let p = world.spawn_presentation();
    assert!(world.despawn_many(&[p, s]).is_empty());
    assert_eq!(world.spawn(), EntityId::new(0, 1));
    assert_eq!(
        world.spawn_presentation(),
        EntityId::new(PRESENTATION_BASE, 1)
    );
}

// --- The guard table: Sim phase -----------------------------------------------------------------

fn sim_probe(world: &mut World, _: &SystemCtx) {
    let Targets { pres, sim } = {
        let t = world.resource::<Targets>().unwrap();
        Targets {
            sim: t.sim,
            pres: t.pres,
        }
    };
    let rows = [
        ("has", world.has::<Pos>(pres).to_string()),
        ("get", outcome(world.get::<Pos>(pres))),
        ("get_mut", outcome(world.get_mut::<Pos>(pres))),
        ("insert", outcome(world.insert(pres, Pos(9.0)))),
        ("remove", outcome(world.remove::<Pos>(pres))),
        ("despawn", outcome(world.despawn(pres))),
        (
            "despawn_many",
            world
                .despawn_many(&[pres, pres])
                .iter()
                .map(|d| d.code.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
        (
            "presentation_entities",
            world.presentation_entities().count().to_string(),
        ),
        (
            "entity_indices_with",
            world
                .entity_indices_with(std::any::TypeId::of::<Pos>())
                .count()
                .to_string(),
        ),
        ("sim get", outcome(world.get::<Pos>(sim))),
        ("sim get_mut", outcome(world.get_mut::<Pos>(sim))),
        (
            "sim resource",
            world.resource_mut::<SimOnly>().is_some().to_string(),
        ),
    ];
    for (label, value) in rows {
        Log::note(world, label, value);
    }
}

#[test]
fn the_sim_phase_cannot_see_presentation_entities() {
    let (mut world, _) = world_with_pair();
    let mut schedule = Schedule::new();
    schedule.add_system("sim_probe", sim_probe);
    schedule.run(&mut world, &ctx());
    assert!(caught(&mut world).is_none());
    let l0028 = code::PRESENTATION_BOUNDARY.to_string();
    let expected = [
        ("has", "false".to_string()),
        ("get", l0028.clone()),
        ("get_mut", l0028.clone()),
        ("insert", l0028.clone()),
        ("remove", l0028.clone()),
        ("despawn", l0028.clone()),
        ("despawn_many", format!("{l0028},{l0028}")),
        ("presentation_entities", "0".to_string()),
        ("entity_indices_with", "1".to_string()),
        ("sim get", "ok".to_string()),
        ("sim get_mut", "ok".to_string()),
        ("sim resource", "true".to_string()),
    ];
    assert_eq!(log(&world), expected);
    // The refused calls changed nothing.
    assert_eq!(
        world
            .get::<Pos>(world.presentation_entities().next().unwrap())
            .unwrap(),
        Some(&Pos(2.0))
    );
}

fn spawn_presentation_from_sim(world: &mut World, _: &SystemCtx) {
    world.spawn_presentation();
}

#[test]
fn a_sim_system_spawning_presentation_panics_with_l0028() {
    let (mut world, _) = world_with_pair();
    let mut schedule = Schedule::new();
    schedule.add_system("spawn_presentation", spawn_presentation_from_sim);
    schedule.run(&mut world, &ctx());
    assert!(caught(&mut world).unwrap().contains("L0028"));
}

// --- The guard table: Presentation phase --------------------------------------------------------

fn presentation_probe(world: &mut World, _: &SystemCtx) {
    let (sim, pres) = {
        let t = world.resource::<Targets>().unwrap();
        (t.sim, t.pres)
    };
    let rows = [
        ("sim get", outcome(world.get::<Pos>(sim))),
        ("sim has", world.has::<Pos>(sim).to_string()),
        ("sim get_mut", outcome(world.get_mut::<Pos>(sim))),
        ("sim insert", outcome(world.insert(sim, Pos(9.0)))),
        ("sim remove", outcome(world.remove::<Pos>(sim))),
        ("sim despawn", outcome(world.despawn(sim))),
        (
            "sim despawn_many",
            world
                .despawn_many(&[sim])
                .iter()
                .map(|d| d.code.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
        ("pres get_mut", outcome(world.get_mut::<Pos>(pres))),
        ("pres insert", outcome(world.insert(pres, Vel(1.0)))),
        (
            "sim resource read",
            world.resource::<SimOnly>().is_some().to_string(),
        ),
        (
            "outbox write",
            world.resource_mut::<Log>().is_some().to_string(),
        ),
    ];
    for (label, value) in rows {
        Log::note(world, label, value);
    }
}

#[test]
fn the_presentation_phase_reads_sim_state_but_cannot_write_it() {
    let (mut world, _) = world_with_pair();
    let mut schedule = Schedule::new();
    schedule.add_presentation_system("presentation_probe", presentation_probe);
    schedule.run(&mut world, &ctx());
    assert!(caught(&mut world).is_none());
    let l0028 = code::PRESENTATION_BOUNDARY.to_string();
    let expected = [
        ("sim get", "ok".to_string()),
        ("sim has", "true".to_string()),
        ("sim get_mut", l0028.clone()),
        ("sim insert", l0028.clone()),
        ("sim remove", l0028.clone()),
        ("sim despawn", l0028.clone()),
        ("sim despawn_many", l0028),
        ("pres get_mut", "ok".to_string()),
        ("pres insert", "ok".to_string()),
        ("sim resource read", "true".to_string()),
        ("outbox write", "true".to_string()),
    ];
    assert_eq!(log(&world), expected);
    assert_eq!(
        world.get::<Pos>(world.entities().next().unwrap()).unwrap(),
        Some(&Pos(1.0))
    );
}

fn spawn_sim_from_presentation(world: &mut World, _: &SystemCtx) {
    world.spawn();
}
fn resource_mut_from_presentation(world: &mut World, _: &SystemCtx) {
    world.resource_mut::<SimOnly>();
}
fn insert_resource_from_presentation(world: &mut World, _: &SystemCtx) {
    world.insert_resource(SimOnly(1));
}
fn remove_resource_from_presentation(world: &mut World, _: &SystemCtx) {
    world.remove_resource::<SimOnly>();
}

#[test]
fn presentation_panics_carry_l0028() {
    type System = fn(&mut World, &SystemCtx);
    let systems: [(&'static str, System); 4] = [
        ("spawn", spawn_sim_from_presentation),
        ("resource_mut", resource_mut_from_presentation),
        ("insert_resource", insert_resource_from_presentation),
        ("remove_resource", remove_resource_from_presentation),
    ];
    for (name, system) in systems {
        let (mut world, _) = world_with_pair();
        let mut schedule = Schedule::new();
        schedule.add_presentation_system(name, system);
        schedule.run(&mut world, &ctx());
        let message = caught(&mut world).unwrap_or_else(|| panic!("{name} did not panic"));
        assert!(message.contains("L0028"), "{name}: {message}");
        assert_eq!(world.phase(), Phase::Host);
    }
}

#[test]
fn a_sim_system_may_write_the_resource_a_presentation_system_may_not() {
    let (mut world, _) = world_with_pair();
    let mut schedule = Schedule::new();
    schedule.add_system("sim", |world: &mut World, _: &SystemCtx| {
        world.resource_mut::<SimOnly>().unwrap().0 += 1;
    });
    schedule.run(&mut world, &ctx());
    assert!(caught(&mut world).is_none());
    assert_eq!(world.resource::<SimOnly>().unwrap().0, 1);
}

// --- Query windowing ----------------------------------------------------------------------------

const SIM_ROWS: usize = 40;
const PRES_ROWS: usize = 30;

/// `SIM_ROWS` sim and `PRES_ROWS` presentation entities with `Pos` (and `Vel` on every other), the
/// presentation range recycled once so its generations are not all zero.
fn world_with_rows() -> (World, Vec<EntityId>, Vec<EntityId>) {
    let mut world = World::new();
    let mut sim = Vec::new();
    let mut pres = Vec::new();
    for i in 0..SIM_ROWS {
        let e = world.spawn();
        world.insert(e, Pos(i as f32)).unwrap();
        if i % 2 == 0 {
            world.insert(e, Vel(1.0)).unwrap();
        }
        sim.push(e);
    }
    let churn: Vec<_> = (0..4).map(|_| world.spawn_presentation()).collect();
    for p in churn {
        world.despawn(p).unwrap();
    }
    // The recycled slots come back last-freed first; rows are ascending by index, so sort.
    pres.extend((0..PRES_ROWS).map(|_| world.spawn_presentation()));
    pres.sort_by_key(|e| e.index());
    for (i, &e) in pres.iter().enumerate() {
        world.insert(e, Pos(1000.0 + i as f32)).unwrap();
        if i % 2 == 0 {
            world.insert(e, Vel(1.0)).unwrap();
        }
    }
    assert!(pres.iter().any(|p| p.generation() > 0));
    world.insert_resource(Seen::default());
    world.allow_presentation_writes::<Seen>();
    (world, sim, pres)
}

#[derive(Default)]
struct Seen(Vec<(&'static str, Vec<EntityId>)>);

fn see(world: &mut World, label: &'static str, ids: Vec<EntityId>) {
    world.resource_mut::<Seen>().unwrap().0.push((label, ids));
}

fn seen(world: &World, label: &str) -> Vec<EntityId> {
    world
        .resource::<Seen>()
        .unwrap()
        .0
        .iter()
        .find(|(l, _)| *l == label)
        .unwrap_or_else(|| panic!("no `{label}`"))
        .1
        .clone()
}

fn sim_queries(world: &mut World, _: &SystemCtx) {
    let plain = world.query::<(&Pos,)>().count();
    let ids: Vec<_> = world.query_ids::<(&Pos,)>().map(|(id, _)| id).collect();
    let joined: Vec<_> = world
        .query_ids::<(&mut Pos, &Vel)>()
        .map(|(id, (pos, vel))| {
            pos.0 += vel.0;
            id
        })
        .collect();
    let par = world.par_for_each::<(&mut Pos, &Vel), _>(4, |_, id, (pos, vel)| {
        pos.0 += vel.0;
        id
    });
    let fold = world.par_join_fold::<(&Pos,), Vec<EntityId>, Vec<EntityId>>(
        4,
        |rows| rows.map(|(id, _)| id).collect(),
        Vec::new(),
        |mut all, part| {
            all.extend(part);
            all
        },
    );
    see(world, "plain_count", vec![EntityId::new(plain as u32, 0)]);
    see(world, "query_ids", ids);
    see(world, "joined", joined);
    see(world, "par", par);
    see(world, "fold", fold);
}

fn presentation_queries(world: &mut World, _: &SystemCtx) {
    let all: Vec<_> = world.query_ids::<(&Pos,)>().map(|(id, _)| id).collect();
    let writable: Vec<_> = world
        .query_ids::<(&mut Pos,)>()
        .map(|(id, (pos,))| {
            pos.0 += 0.5;
            id
        })
        .collect();
    let par = world.par_for_each::<(&mut Pos, &Vel), _>(4, |_, id, (pos, vel)| {
        pos.0 += vel.0;
        id
    });
    let fold = world.par_join_fold::<(&Pos, &Vel), Vec<EntityId>, Vec<EntityId>>(
        4,
        |rows| rows.map(|(id, _)| id).collect(),
        Vec::new(),
        |mut all, part| {
            all.extend(part);
            all
        },
    );
    see(world, "all", all);
    see(world, "writable", writable);
    see(world, "par", par);
    see(world, "fold", fold);
}

#[test]
fn sim_queries_stop_before_the_presentation_tail() {
    for count in COUNTS {
        let (mut world, sim, pres) = world_with_rows();
        let mut schedule = Schedule::new();
        schedule.add_system("sim_queries", sim_queries);
        par::with_threads(count, || schedule.run(&mut world, &ctx()));
        assert!(caught(&mut world).is_none(), "at {count} threads");

        let even: Vec<_> = sim.iter().copied().step_by(2).collect();
        assert_eq!(seen(&world, "plain_count")[0].index() as usize, SIM_ROWS);
        assert_eq!(seen(&world, "query_ids"), sim, "at {count} threads");
        assert_eq!(seen(&world, "joined"), even, "at {count} threads");
        assert_eq!(seen(&world, "par"), even, "at {count} threads");
        assert_eq!(seen(&world, "fold"), sim, "at {count} threads");
        // Every even sim row took two `+1`s; the presentation rows were never touched.
        for (i, &e) in sim.iter().enumerate() {
            let bump = if i % 2 == 0 { 2.0 } else { 0.0 };
            assert_eq!(world.get::<Pos>(e).unwrap(), Some(&Pos(i as f32 + bump)));
        }
        for (i, &e) in pres.iter().enumerate() {
            assert_eq!(world.get::<Pos>(e).unwrap(), Some(&Pos(1000.0 + i as f32)));
        }
    }
}

#[test]
fn presentation_queries_with_a_mut_column_join_presentation_rows_only() {
    for count in COUNTS {
        let (mut world, sim, pres) = world_with_rows();
        let mut schedule = Schedule::new();
        schedule.add_presentation_system("presentation_queries", presentation_queries);
        par::with_threads(count, || schedule.run(&mut world, &ctx()));
        assert!(caught(&mut world).is_none(), "at {count} threads");

        let all: Vec<_> = sim.iter().chain(pres.iter()).copied().collect();
        let even: Vec<_> = pres.iter().copied().step_by(2).collect();
        // A read-only query sees both ranges, with correct generations on presentation rows.
        assert_eq!(seen(&world, "all"), all, "at {count} threads");
        assert_eq!(seen(&world, "writable"), pres, "at {count} threads");
        assert_eq!(seen(&world, "par"), even, "at {count} threads");
        let read_join: Vec<_> = sim
            .iter()
            .copied()
            .step_by(2)
            .chain(pres.iter().copied().step_by(2))
            .collect();
        assert_eq!(seen(&world, "fold"), read_join, "at {count} threads");
        for (i, &e) in sim.iter().enumerate() {
            assert_eq!(world.get::<Pos>(e).unwrap(), Some(&Pos(i as f32)));
        }
        for (i, &e) in pres.iter().enumerate() {
            let bump = 0.5 + if i % 2 == 0 { 1.0 } else { 0.0 };
            assert_eq!(
                world.get::<Pos>(e).unwrap(),
                Some(&Pos(1000.0 + i as f32 + bump))
            );
        }
    }
}

#[test]
fn host_queries_see_every_row() {
    let (mut world, sim, pres) = world_with_rows();
    let ids: Vec<_> = world.query_ids::<(&mut Pos,)>().map(|(id, _)| id).collect();
    let all: Vec<_> = sim.iter().chain(pres.iter()).copied().collect();
    assert_eq!(ids, all);
}

// --- The phase is always put back ---------------------------------------------------------------

fn record_phase(world: &mut World, _: &SystemCtx) {
    let phase = format!("{:?}", world.phase());
    Log::note(world, "phase", phase);
}

fn boom(_: &mut World, _: &SystemCtx) {
    panic!("boom");
}

#[test]
fn systems_see_their_phase_and_the_host_gets_it_back() {
    let (mut world, _) = world_with_pair();
    let mut schedule = Schedule::new();
    schedule.add_system("sim", record_phase);
    schedule.add_presentation_system("presentation", |world: &mut World, ctx: &SystemCtx| {
        record_phase(world, ctx);
    });
    assert_eq!(
        schedule.system_phases().collect::<Vec<_>>(),
        vec![("sim", Phase::Sim), ("presentation", Phase::Presentation)]
    );
    assert_eq!(world.phase(), Phase::Host);
    schedule.run(&mut world, &ctx());
    assert_eq!(
        log(&world),
        vec![
            ("phase", "Sim".to_string()),
            ("phase", "Presentation".to_string())
        ]
    );
    assert_eq!(world.phase(), Phase::Host);
}

#[test]
fn a_panicking_system_of_either_phase_leaves_the_host_phase() {
    for presentation in [false, true] {
        let (mut world, _) = world_with_pair();
        let mut schedule = Schedule::new();
        if presentation {
            schedule.add_presentation_system("boom", boom);
        } else {
            schedule.add_system("boom", boom);
        }
        schedule.run(&mut world, &ctx());
        assert_eq!(caught(&mut world).as_deref(), Some("boom"));
        assert_eq!(world.phase(), Phase::Host);
        // The host may touch both ranges again.
        assert!(
            world
                .get::<Pos>(world.presentation_entities().next().unwrap())
                .is_ok()
        );
    }
}

#[test]
fn a_probe_reads_in_the_host_phase() {
    let (mut world, _) = world_with_pair();
    let mut schedule = Schedule::new();
    schedule.add_system("sim", record_phase);
    schedule.add_presentation_system("presentation", |world: &mut World, ctx: &SystemCtx| {
        record_phase(world, ctx);
    });
    let mut probed = Vec::new();
    schedule.run_probed(&mut world, &ctx(), &mut |name, world| {
        probed.push((name, world.phase(), world.presentation_entities().count()));
    });
    assert_eq!(
        probed,
        vec![("sim", Phase::Host, 1), ("presentation", Phase::Host, 1)]
    );
}

#[test]
#[should_panic(expected = "L4018")]
fn the_duplicate_name_check_spans_phases() {
    let mut schedule = Schedule::new();
    schedule.add_system("tick", |_: &mut World, _: &SystemCtx| {});
    schedule.add_presentation_system("tick", |_: &mut World, _: &SystemCtx| {});
}

// --- Restore ------------------------------------------------------------------------------------

#[test]
fn restore_presentation_round_trips_the_allocator() {
    let mut world = World::new();
    let sim = world.spawn();
    let p0 = world.spawn_presentation();
    let p1 = world.spawn_presentation();
    let p2 = world.spawn_presentation();
    world.despawn(p1).unwrap();
    for p in [p0, p2] {
        world.insert(p, Pos(5.0)).unwrap();
    }
    let live: Vec<_> = world.presentation_entities().collect();
    let state = world.presentation_allocator_state();
    assert!(
        state.is_some(),
        "a recycled slot makes the live set insufficient"
    );

    let mut restored = World::new();
    restored.restore_entities(&[sim]).unwrap();
    restored
        .restore_presentation(&live, state.as_ref())
        .unwrap();
    assert_eq!(restored.presentation_entities().collect::<Vec<_>>(), live);
    assert!(
        restored.get::<Pos>(p0).unwrap().is_none(),
        "no storage was touched"
    );
    // The next spawn mints what an uninterrupted run would: the freed slot at its bumped generation.
    assert_eq!(restored.spawn_presentation(), world.spawn_presentation());
}

#[test]
fn restore_presentation_without_state_frees_the_gaps() {
    let mut world = World::new();
    let ids = [
        EntityId::new(PRESENTATION_BASE, 0),
        EntityId::new(PRESENTATION_BASE + 2, 3),
    ];
    world.restore_presentation(&ids, None).unwrap();
    assert_eq!(world.presentation_entities().collect::<Vec<_>>(), ids);
    assert_eq!(
        world.spawn_presentation(),
        EntityId::new(PRESENTATION_BASE + 1, 0)
    );
}

#[test]
fn restore_presentation_refuses_bad_input_and_leaves_the_world_intact() {
    let mut world = World::new();
    let p = world.spawn_presentation();
    world.insert(p, Pos(3.0)).unwrap();

    let sim_range = [EntityId::new(PRESENTATION_BASE, 0), EntityId::new(7, 0)];
    let duplicate = [
        EntityId::new(PRESENTATION_BASE + 1, 0),
        EntityId::new(PRESENTATION_BASE + 1, 2),
    ];
    for (bad, path) in [(&sim_range[..], "/1"), (&duplicate[..], "/1")] {
        let error = world.restore_presentation(bad, None).unwrap_err();
        assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
        assert_eq!(error.path.as_deref(), Some(path));
        assert_eq!(world.presentation_entities().collect::<Vec<_>>(), vec![p]);
        assert_eq!(world.get::<Pos>(p).unwrap(), Some(&Pos(3.0)));
    }
}

#[test]
fn the_sim_restores_clear_the_presentation_range() {
    let mut world = World::new();
    world.spawn_presentation();
    world.restore_entities(&[]).unwrap();
    assert_eq!(world.presentation_entities().count(), 0);
    world.spawn_presentation();
    world
        .restore_allocator(&world.allocator_state().unwrap_or_else(|| {
            // A clean world has no state to carry: build an empty one by round-tripping a spawn.
            let mut scratch = World::new();
            let e = scratch.spawn();
            scratch.despawn(e).unwrap();
            scratch.allocator_state().unwrap()
        }))
        .unwrap();
    assert_eq!(world.presentation_entities().count(), 0);
}

// --- L0028 message text -------------------------------------------------------------------------

fn grab(world: &mut World, _: &SystemCtx) {
    let pres = world.resource::<Targets>().unwrap().pres;
    let message = world.get::<Pos>(pres).unwrap_err().to_string();
    Log::note(world, "message", message);
}

fn popups(world: &mut World, _: &SystemCtx) {
    let sim = world.resource::<Targets>().unwrap().sim;
    let message = world.get_mut::<Pos>(sim).unwrap_err().to_string();
    Log::note(world, "message", message);
}

fn popups_spawn(world: &mut World, _: &SystemCtx) {
    world.spawn();
}

fn score(world: &mut World, _: &SystemCtx) {
    world.spawn_presentation();
}

fn popups_resource(world: &mut World, _: &SystemCtx) {
    world.resource_mut::<SimOnly>();
}

/// The text of the one message a single-system schedule produced, whether returned or panicked.
fn message_of(
    world: &mut World,
    presentation: bool,
    name: &'static str,
    system: fn(&mut World, &SystemCtx),
) -> String {
    let mut schedule = Schedule::new();
    if presentation {
        schedule.add_presentation_system(name, system);
    } else {
        schedule.add_system(name, system);
    }
    schedule.run(world, &ctx());
    match caught(world) {
        Some(panic) => panic,
        None => log(world).pop().expect("the system logged a message").1,
    }
}

#[test]
fn every_l0028_names_system_entity_call_and_fix() {
    let (probe, _) = world_with_pair();
    let (sim, pres) = {
        let t = probe.resource::<Targets>().unwrap();
        (t.sim, t.pres)
    };
    type System = fn(&mut World, &SystemCtx);
    let cases: [(bool, &'static str, System, Vec<String>); 5] = [
        (
            false,
            "grab",
            grab,
            vec![
                "sim system `grab`".into(),
                pres.to_string(),
                "`popup/3`".into(),
                "`get::<Pos>`".into(),
                "the sim never sees presentation state".into(),
                "make it a sim entity".into(),
                "unprojects through".into(),
            ],
        ),
        (
            true,
            "popups",
            popups,
            vec![
                "presentation system `popups`".into(),
                sim.to_string(),
                "`ball`".into(),
                "`get_mut::<Pos>`".into(),
                "read-only to presentation".into(),
                "write the derived value onto a presentation entity".into(),
            ],
        ),
        (
            true,
            "popups_spawn",
            popups_spawn,
            vec![
                "presentation system `popups_spawn`".into(),
                "`World::spawn`".into(),
                "sim ids belong to the sim".into(),
                "`World::spawn_presentation`".into(),
            ],
        ),
        (
            false,
            "score",
            score,
            vec![
                "sim system `score`".into(),
                "`World::spawn_presentation`".into(),
                "never holds presentation ids".into(),
                "move the spawn into a presentation system".into(),
            ],
        ),
        (
            true,
            "popups_resource",
            popups_resource,
            vec![
                "presentation system `popups_resource`".into(),
                "`SimOnly`".into(),
                "`resource_mut`".into(),
                "allow_presentation_writes".into(),
                "DeterministicRng::derived".into(),
            ],
        ),
    ];
    for (presentation, name, system, needles) in cases {
        let (mut world, _) = world_with_pair();
        let message = message_of(&mut world, presentation, name, system);
        assert!(message.contains("L0028"), "{name}: {message}");
        for needle in &needles {
            assert!(
                message.contains(needle.as_str()),
                "{name} lacks `{needle}`: {message}"
            );
        }
    }
}

#[test]
fn an_unlabelled_entity_is_named_by_its_bare_id() {
    let (mut world, _) = world_with_pair();
    let bare = world.spawn_presentation();
    world.insert(bare, Pos(0.0)).unwrap();
    world.resource_mut::<Targets>().unwrap().pres = bare;
    let message = message_of(&mut world, false, "grab", grab);
    assert!(message.contains(&bare.to_string()), "{message}");
    assert!(
        !message.contains("(``)") && !message.contains("``"),
        "{message}"
    );
    assert!(!message.contains(&format!("{bare} (")), "{message}");
}

#[test]
fn despawn_many_names_each_refused_id() {
    fn doomed(world: &mut World, _: &SystemCtx) {
        let pres = world.resource::<Targets>().unwrap().pres;
        let messages: Vec<_> = world
            .despawn_many(&[pres])
            .iter()
            .map(ToString::to_string)
            .collect();
        Log::note(
            world,
            "message",
            messages.join(
                "
",
            ),
        );
    }
    let (mut world, targets) = world_with_pair();
    let message = message_of(&mut world, false, "doomed", doomed);
    assert!(message.contains(&targets.pres.to_string()), "{message}");
    assert!(
        message.contains("`despawn_many`") && message.contains("sim system `doomed`"),
        "{message}"
    );
}

// --- Dead ids stay L0002 ------------------------------------------------------------------------

struct Dead(EntityId);

fn dead_probe(world: &mut World, _: &SystemCtx) {
    let dead = world.resource::<Dead>().unwrap().0;
    let rows = [
        ("get", outcome(world.get::<Pos>(dead))),
        ("get_mut", outcome(world.get_mut::<Pos>(dead))),
        ("despawn", outcome(world.despawn(dead))),
        ("has", world.has::<Pos>(dead).to_string()),
    ];
    for (label, value) in rows {
        Log::note(world, label, value);
    }
}

#[test]
fn a_dead_sim_id_is_l0002_in_every_phase() {
    let expected = [
        ("get", code::DEAD_ENTITY.to_string()),
        ("get_mut", code::DEAD_ENTITY.to_string()),
        ("despawn", code::DEAD_ENTITY.to_string()),
        ("has", "false".to_string()),
    ];
    for presentation in [false, true] {
        let (mut world, _) = world_with_pair();
        let dead = world.spawn();
        world.despawn(dead).unwrap();
        world.insert_resource(Dead(dead));
        let mut schedule = Schedule::new();
        if presentation {
            schedule.add_presentation_system("dead_probe", dead_probe);
        } else {
            schedule.add_system("dead_probe", dead_probe);
        }
        schedule.run(&mut world, &ctx());
        assert!(caught(&mut world).is_none());
        assert_eq!(log(&world), expected, "presentation phase: {presentation}");
    }
}

// --- The boundary outbox ------------------------------------------------------------------------

fn discarded_write(world: &mut World, _: &SystemCtx) {
    let sim = world.resource::<Targets>().unwrap().sim;
    let _ = world.get_mut::<Pos>(sim);
}

fn discarded_read(world: &mut World, _: &SystemCtx) {
    let pres = world.resource::<Targets>().unwrap().pres;
    let _ = world.get::<Pos>(pres).ok().flatten();
}

fn option_reads(world: &mut World, _: &SystemCtx) {
    let (sim, pres) = {
        let t = world.resource::<Targets>().unwrap();
        (t.sim, t.pres)
    };
    let rows = [
        (
            "component_mut of the sim id",
            world.component_mut::<Pos>(sim).is_none(),
        ),
        (
            "component of the presentation id",
            world.component::<Pos>(pres).is_none(),
        ),
    ];
    for (label, none) in rows {
        Log::note(world, label, none);
    }
}

#[test]
fn a_discarded_crossing_is_recorded_once_until_drained() {
    let (mut world, _) = world_with_pair();
    let mut schedule = Schedule::new();
    schedule.add_presentation_system("peek", discarded_write);
    for _ in 0..5 {
        schedule.run(&mut world, &ctx());
    }
    let taken = world.take_boundary_crossings();
    assert_eq!(taken.len(), 1);
    assert!(
        taken[0].key.ends_with("peek/get_mut/Pos"),
        "{}",
        taken[0].key
    );
    assert_eq!(taken[0].diagnostic.code, code::PRESENTATION_BOUNDARY);
    schedule.run(&mut world, &ctx());
    assert_eq!(world.take_boundary_crossings().len(), 1);
}

#[test]
fn a_sim_read_of_a_presentation_entity_is_recorded() {
    let (mut world, _) = world_with_pair();
    let mut schedule = Schedule::new();
    schedule.add_system("pry", discarded_read);
    schedule.run(&mut world, &ctx());
    assert_eq!(world.pending_boundary_crossings(), ["pry/get/Pos"]);
}

#[test]
fn a_dead_id_or_a_host_access_records_nothing() {
    let (mut world, targets) = world_with_pair();
    let dead = world.spawn();
    world.despawn(dead).unwrap();
    world.insert_resource(Dead(dead));
    let mut schedule = Schedule::new();
    schedule.add_system("dead_probe", dead_probe);
    schedule.add_presentation_system("dead_probe_p", dead_probe);
    schedule.run(&mut world, &ctx());
    assert!(world.take_boundary_crossings().is_empty());
    // Host phase: no system, no phase guard.
    world.get_mut::<Pos>(targets.pres).unwrap();
    world.get::<Pos>(targets.sim).unwrap();
    assert!(world.take_boundary_crossings().is_empty());
}

#[test]
fn option_reads_across_the_boundary_are_none_and_recorded() {
    let (mut world, _) = world_with_pair();
    let mut sim = Schedule::new();
    sim.add_system("sim_option", option_reads);
    sim.run(&mut world, &ctx());
    let mut pres = Schedule::new();
    pres.add_presentation_system("pres_option", option_reads);
    pres.run(&mut world, &ctx());
    // Sim: component_mut on a sim entity is fine, component on a presentation id is refused.
    // Presentation: component_mut on a sim id is refused, component on a presentation id is fine.
    assert_eq!(
        log(&world),
        [
            ("component_mut of the sim id", false),
            ("component of the presentation id", true),
            ("component_mut of the sim id", true),
            ("component of the presentation id", false),
        ]
        .map(|(l, b)| (l, b.to_string()))
    );
    let mut keys = world.pending_boundary_crossings();
    keys.sort();
    assert_eq!(keys, ["pres_option/get_mut/Pos", "sim_option/get/Pos"]);
}
