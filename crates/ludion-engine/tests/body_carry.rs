//! The drag carry through the engine (`docs/motor.md` §Carrying): a six-part ragdoll on a `body`
//! layer hangs off a kinematic anchor that is a `Parent` child of a walking motor. The carrier walks
//! a figure-eight past a wall; it passes through its own body, the body still meets the wall, and
//! the run is identical across thread counts and a snapshot/restore mid-walk.

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::math::{atan2, sin_cos};
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, Name, Transform, move_character};
use ludion_physics::ParentPose;
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};

const TICKS: u64 = 600;
const SNAPSHOT_AT: usize = 300;
/// Each lobe of the figure-eight is a circle this many ticks round.
const LOBE_TICKS: u64 = 300;
const RADIUS: f32 = 1.0;
/// The wall's near face. It is a beam over the carrier's head (y 1.0 up): the capsule passes under it
/// while the torso and head hang in its way.
const WALL_FACE: f32 = 2.2;
const PARTS: [&str; 6] = ["torso", "head", "arm-l", "arm-r", "leg-l", "leg-r"];
/// The parts at beam height and each one's half-extent along `x`.
const BEAM_PARTS: [(&str, f32); 2] = [("torso", 0.15), ("head", 0.1)];

/// What the carrier's system saw each tick.
#[derive(Default)]
struct Walk(Vec<Row>);

struct Row {
    feet: [f32; 3],
    torso: [f32; 3],
    anchor_gap: f32,
    /// The farthest `x` any ragdoll part's near-wall face reached, unrotated.
    part_x: f32,
    /// Every entity the move names as a blocker or as ground.
    blockers: Vec<EntityId>,
    text: String,
}

fn fixture_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-body-carry-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    dir
}

/// A block-style number sequence under a key at `indent` spaces: scenes forbid non-empty flow style.
fn seq(indent: usize, values: &[f32]) -> String {
    let pad = " ".repeat(indent + 2);
    values
        .iter()
        .map(|value| format!("\n{pad}- {value:?}"))
        .collect()
}

fn entity_ref(indent: usize, index: usize) -> String {
    let pad = " ".repeat(indent + 2);
    format!("\n{pad}index: {index}\n{pad}generation: 0")
}

/// One rigid part: a box at `center`, on `body`, jointed by a ball to entity `parent` at
/// `pivot` (in the parent's local frame) with `limit` degrees about each axis, as entity `index`.
fn part(
    name: &str,
    index: usize,
    center: [f32; 3],
    half: [f32; 3],
    parent: usize,
    pivot: [f32; 3],
) -> String {
    let limit = if name == "head" { 20.0 } else { 90.0 };
    let limits: String = (0..3)
        .map(|_| format!("\n              - - -{limit:?}\n                - {limit:?}"))
        .collect::<Vec<_>>()
        .join("");
    format!(
        "  - name: {name}
    components:
      Transform:
        translation:{}
      Collider:
        shape:
          box:
            half_extents:{}
        body:
          rigid:
            mass: 1.0
        layer: body
      Velocity: {{}}
      Joint:
        a:{}
        b:{}
        kind:
          ball:
            limits:{limits}
        anchor:{}
        collide: false
",
        seq(8, &center),
        seq(12, &half),
        entity_ref(8, parent),
        entity_ref(8, index),
        seq(8, &pivot),
    )
}

/// The scene: floor 0, wall 1, carrier 2, anchor 3, then the six parts at 4..=9, the torso first.
fn scene() -> String {
    let shoulder = [0.0, 1.4, 0.3];
    let torso_at = [0.0, 1.1, 0.3];
    let at = |dx: f32, dy: f32, dz: f32| [torso_at[0] + dx, torso_at[1] + dy, torso_at[2] + dz];
    let mut text = format!(
        "format: 1
seed: 5
physics:
  space: free
  layers:
    - name: body
    - name: carrying
      ignores:
        - body
    - name: anchor
      ignores:
        - default
        - body
        - carrying
        - anchor
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 20.0
              - 0.5
              - 20.0
        body: static
  - name: wall
    components:
      Transform:
        translation:
          - {:?}
          - 1.9
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.9
              - 3.0
        body: static
  - name: carrier
    components:
      Transform: {{}}
      CharacterMotor:
        radius: 0.28
        height: 0.9
        slope_limit_deg: 45.0
        step_height: 0.25
        layer: carrying
  - name: anchor
    components:
      Parent:{}
      Transform:
        translation:{}
      Collider:
        shape:
          sphere:
            radius: 0.05
        body: kinematic
        layer: anchor
",
        WALL_FACE + 0.5,
        entity_ref(8, 2),
        seq(8, &shoulder),
    );
    let parts = [
        part("torso", 4, torso_at, [0.15, 0.3, 0.1], 3, [0.0; 3]),
        part("head", 5, at(0.0, 0.4, 0.0), [0.1; 3], 4, [0.0, 0.3, 0.0]),
        part(
            "arm-l",
            6,
            at(-0.2, 0.0, 0.0),
            [0.05, 0.25, 0.05],
            4,
            [-0.2, 0.25, 0.0],
        ),
        part(
            "arm-r",
            7,
            at(0.2, 0.0, 0.0),
            [0.05, 0.25, 0.05],
            4,
            [0.2, 0.25, 0.0],
        ),
        part(
            "leg-l",
            8,
            at(-0.08, -0.6, 0.0),
            [0.06, 0.3, 0.06],
            4,
            [-0.08, -0.3, 0.0],
        ),
        part(
            "leg-r",
            9,
            at(0.08, -0.6, 0.0),
            [0.06, 0.3, 0.06],
            4,
            [0.08, -0.3, 0.0],
        ),
    ];
    for part in parts {
        text.push_str(&part);
    }
    text
}

fn named(world: &World, wanted: &str) -> EntityId {
    world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has `{wanted}`"))
}

/// The carrier's feet at tick `t` on two tangent circles of [`RADIUS`] joined at the origin.
fn path(t: u64) -> [f32; 2] {
    let lobe = (t / LOBE_TICKS) % 2;
    let angle = core::f32::consts::TAU * (t % LOBE_TICKS) as f32 / LOBE_TICKS as f32;
    let (s, c) = sin_cos(angle);
    if lobe == 0 {
        [RADIUS - RADIUS * c, RADIUS * s]
    } else {
        [-RADIUS + RADIUS * c, RADIUS * s]
    }
}

fn walk(world: &mut World, ctx: &SystemCtx) {
    let t = ctx.frame.as_u64();
    let carrier = named(world, "carrier");
    let (from, to) = (path(t), path(t + 1));
    let step = [to[0] - from[0], to[1] - from[1]];
    let yaw = atan2(step[0], step[1]).to_degrees();
    world
        .get_mut::<Transform>(carrier)
        .unwrap()
        .unwrap()
        .rotation = [0.0, yaw, 0.0];
    let Ok(report) = move_character(world, carrier, [step[0], -0.1, step[1]]) else {
        return;
    };
    let feet = world
        .get::<Transform>(carrier)
        .unwrap()
        .unwrap()
        .translation;
    let position = |name: &str| {
        let id = named(world, name);
        world.get::<Transform>(id).unwrap().unwrap().translation
    };
    let torso = position("torso");
    let anchor_id = named(world, "anchor");
    let anchor = ludion_engine::TransformHierarchy::world_pose(world, anchor_id)
        .expect("the anchor has a world pose")
        .0;
    let d = [
        torso[0] - anchor[0],
        torso[1] - anchor[1],
        torso[2] - anchor[2],
    ];
    let gap = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let part_x = BEAM_PARTS
        .iter()
        .map(|(name, half)| position(name)[0] + half)
        .fold(f32::MIN, f32::max);
    let mut blockers: Vec<EntityId> = report.walls.iter().map(|hit| hit.entity).collect();
    blockers.extend(report.ground.iter().map(|ground| ground.entity));
    let row = Row {
        feet,
        torso,
        anchor_gap: gap,
        part_x,
        blockers,
        text: format!("{feet:?} {torso:?} {report:?}"),
    };
    if world.resource::<Walk>().is_none() {
        world.insert_resource(Walk::default());
    }
    world.resource_mut::<Walk>().unwrap().0.push(row);
}

fn game(tag: &str) -> EngineBuilder {
    Engine::builder()
        .project_root(fixture_root(tag))
        .system("walk", walk)
}

fn rows(engine: &Engine) -> &[Row] {
    &engine
        .world()
        .resource::<Walk>()
        .expect("the carrier ran")
        .0
}

fn texts(engine: &Engine) -> Vec<String> {
    rows(engine).iter().map(|row| row.text.clone()).collect()
}

fn trace(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn full_run(tag: &str) -> (Vec<u64>, Vec<String>) {
    let mut engine = game(tag).build().expect("boots");
    engine.load_inline_scene(&scene()).unwrap();
    let hashes = trace(&mut engine, TICKS as usize);
    (hashes, texts(&engine))
}

#[test]
fn the_carrier_walks_through_its_body_and_the_body_still_meets_the_wall() {
    let mut engine = game("walk").build().expect("boots");
    engine.load_inline_scene(&scene()).unwrap();
    trace(&mut engine, TICKS as usize);
    let rows = rows(&engine);
    assert!(
        rows.len() > 500,
        "the carrier barely walked: {}",
        rows.len()
    );

    let ragdoll: Vec<EntityId> = PARTS
        .iter()
        .copied()
        .chain(["anchor"])
        .map(|name| named(engine.world(), name))
        .collect();
    for (tick, row) in rows.iter().enumerate() {
        assert!(
            !row.blockers.iter().any(|blocker| ragdoll.contains(blocker)),
            "tick {tick}: the carrier was blocked by its own body: {:?}",
            row.blockers
        );
        assert!(
            row.torso[1] > 0.5,
            "tick {tick}: the carried body dropped to {:?}",
            row.torso
        );
        assert!(
            row.anchor_gap < 0.35,
            "tick {tick}: the torso strayed {} from the anchor",
            row.anchor_gap
        );
    }
    let reach = rows.iter().map(|row| row.part_x).fold(f32::MIN, f32::max);
    // A rotated box leans a little past its unrotated face; a part never crosses the wall.
    assert!(
        reach <= WALL_FACE + 0.05,
        "a part passed through the wall: x {reach}"
    );
    assert!(
        reach >= WALL_FACE - 0.03,
        "the body never met the wall: x {reach}"
    );
    let travelled = rows.iter().map(|row| row.feet[0]).fold(f32::MIN, f32::max);
    assert!(
        travelled > 1.8,
        "the carrier never reached the wall lobe: {travelled}"
    );
}

#[test]
fn the_carry_is_the_same_at_one_two_and_eight_threads() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        [(1, "t1"), (2, "t2"), (8, "t8")]
            .map(|(n, tag)| scope.spawn(move || par::with_threads(n, || full_run(tag))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two);
    assert_eq!(one, eight);
}

#[test]
fn the_carry_survives_a_snapshot_restore_mid_walk() {
    let (reference, reference_texts) = full_run("reference");
    let distinct: std::collections::BTreeSet<u64> = reference.iter().copied().collect();
    assert!(distinct.len() > 300, "a flat trace proves nothing");

    let mut first = game("first").build().expect("boots");
    first.load_inline_scene(&scene()).unwrap();
    assert_eq!(trace(&mut first, SNAPSHOT_AT), reference[..SNAPSHOT_AT]);
    let document = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let taken = texts(&first).len();

    let mut resumed = game("resumed").build().expect("boots");
    resumed.load_inline_scene(&scene()).unwrap();
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1]
    );
    assert_eq!(
        trace(&mut resumed, TICKS as usize - SNAPSHOT_AT),
        reference[SNAPSHOT_AT..]
    );
    assert_eq!(texts(&resumed), reference_texts[taken..]);
}
