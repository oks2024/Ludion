//! **A turret on a moving platform, and a prop that is grabbed and thrown** — parent/child transforms
//! end to end: `Transform` is local under a `Parent`, `world_transform` reads the composed pose, and
//! `reparent` moves an entity between parents without teleporting it.
//!
//! `drive` moves the `platform` root, `aim` yaws the `turret` in *local* space, and the `barrel`
//! hangs off the turret — neither child ever writes a world coordinate. `grab_and_release` parents a
//! physics prop to the `hand` and later lets it go with a throw.
//!
//! **A rigid body cannot sit under a parent.** The prop is made kinematic *before* the `reparent`
//! (`L1221` otherwise) and rigid again after it is un-parented: the pattern for any held object.
//!
//! Everything above the `main` divider is copyable into a real game. See `ludion docs hierarchy`.
//!
//! Run it headless (prints the barrel and the prop for a few ticks):
//!
//! ```text
//! cargo run -p ludion --example hierarchy
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The tick the hand grabs the prop, and the tick it throws it.
const GRAB: u64 = 10;
const RELEASE: u64 = 30;

/// One tick's readings, in the order the systems saw them.
#[derive(Clone, Copy, Debug)]
struct Sample {
    frame: u64,
    platform: [f32; 3],
    turret_yaw: f32,
    barrel: [f32; 3],
    hand: [f32; 3],
    prop: [f32; 3],
    prop_parented: bool,
}

/// What the example records: every tick's `Sample`, and the prop's world position either side of
/// the release `reparent`.
#[derive(Default)]
struct Log {
    samples: Vec<Sample>,
    before_release: [f32; 3],
    after_release: [f32; 3],
}

/// The first entity named `name`.
fn find(world: &World, name: &str) -> EntityId {
    world
        .entities()
        .find(|&id| matches!(world.get::<Name>(id), Ok(Some(n)) if n.0 == name))
        .unwrap_or_else(|| panic!("the scene has no entity named {name}"))
}

/// An entity's composed world translation.
fn world_position(world: &World, id: EntityId) -> [f32; 3] {
    world_transform(world, id)
        .expect("the entity is alive")
        .trs
        .translation
}

/// Moves the platform a tenth of a metre along +x. Its children ride along.
fn drive(world: &mut World, _ctx: &SystemCtx) {
    let platform = find(world, "platform");
    if let Ok(Some(transform)) = world.get_mut::<Transform>(platform) {
        transform.translation[0] += 0.1;
    }
}

/// Yaws the turret three degrees a tick. `rotation` is local: the platform's motion is not its
/// business, and the barrel follows the turret.
fn aim(world: &mut World, ctx: &SystemCtx) {
    let turret = find(world, "turret");
    if let Ok(Some(transform)) = world.get_mut::<Transform>(turret) {
        transform.rotation[1] = ctx.frame.0 as f32 * 3.0;
    }
}

/// Grabs the prop into the hand at [`GRAB`] and throws it at [`RELEASE`], then logs the tick.
fn grab_and_release(world: &mut World, ctx: &SystemCtx) {
    let (hand, prop) = (find(world, "hand"), find(world, "prop"));
    if ctx.frame.0 == GRAB {
        // Kinematic first: `reparent` refuses a rigid body under a parent (L1221).
        if let Ok(Some(collider)) = world.get_mut::<Collider>(prop) {
            collider.body = Body::new_kinematic();
        }
        reparent(world, prop, Some(hand), Keep::World).expect("a kinematic prop can be held");
    }
    let mut released = None;
    if ctx.frame.0 == RELEASE {
        let before = world_position(world, prop);
        reparent(world, prop, None, Keep::World).expect("un-parenting never refuses");
        if let Ok(Some(collider)) = world.get_mut::<Collider>(prop) {
            collider.body = Body::Rigid(RigidBody::default());
        }
        let throw = Velocity {
            linear: [0.0, 0.0, 6.0],
            angular: [0.0; 3],
        };
        world.insert(prop, throw).expect("the prop is alive");
        released = Some((before, world_position(world, prop)));
    }

    let (platform, turret, barrel) = (
        find(world, "platform"),
        find(world, "turret"),
        find(world, "barrel"),
    );
    let sample = Sample {
        frame: ctx.frame.0,
        platform: world_position(world, platform),
        turret_yaw: world
            .get::<Transform>(turret)
            .ok()
            .flatten()
            .map_or(0.0, |t| t.rotation[1]),
        barrel: world_position(world, barrel),
        hand: world_position(world, hand),
        prop: world_position(world, prop),
        prop_parented: world.has::<Parent>(prop),
    };
    if world.resource::<Log>().is_none() {
        world.insert_resource(Log::default());
    }
    if let Some(log) = world.resource_mut::<Log>() {
        log.samples.push(sample);
        if let Some((before, after)) = released {
            log.before_release = before;
            log.after_release = after;
        }
    }
}

/// The game: three systems, in this order. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .system("drive", drive)
        .system("aim", aim)
        .system("grab_and_release", grab_and_release)
}

/// A free-space physics world without gravity (so the prop hovers until thrown), a camera, and the
/// hierarchy. `Parent` is the parent's position in this list: 1 = platform, 2 = turret, 4 = hand.
const SCENE: &str = "\
format: 1
seed: 1
physics:
  space: free
  gravity:
    - 0.0
    - 0.0
    - 0.0
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 6.0
          - 12.0
      Camera: {}
  - name: platform
    components:
      Transform: {}
  - name: turret
    components:
      Parent:
        index: 1
        generation: 0
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
  - name: barrel
    components:
      Parent:
        index: 2
        generation: 0
      Transform:
        translation:
          - 0.0
          - 0.0
          - 2.0
  - name: hand
    components:
      Parent:
        index: 1
        generation: 0
      Transform:
        translation:
          - 1.0
          - 1.0
          - 0.0
  - name: prop
    components:
      Transform:
        translation:
          - 3.0
          - 1.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.25
              - 0.25
              - 0.25
        body:
          rigid: {}
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs 60 ticks, prints the barrel and the prop every ten, and checks the hierarchy did what the
/// scene says. Deterministic, no window.
fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 60,
            hash: false,
        }))
        .expect("the sim steps");

    let log = engine.world().resource::<Log>().expect("the systems ran");
    for s in log.samples.iter().filter(|s| s.frame % 10 == 0) {
        println!(
            "frame {:>2}  barrel ({:.2}, {:.2}, {:.2})  prop ({:.2}, {:.2}, {:.2}){}",
            s.frame,
            s.barrel[0],
            s.barrel[1],
            s.barrel[2],
            s.prop[0],
            s.prop[1],
            s.prop[2],
            if s.prop_parented { "  held" } else { "" }
        );
    }

    let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
    // The barrel is the platform's position plus the turret's offset plus the barrel's offset
    // turned by the turret's yaw: nothing here ever wrote a world coordinate.
    let barrel_ok = log.samples.iter().all(|s| {
        let yaw = s.turret_yaw.to_radians();
        let expected = [
            s.platform[0] + 2.0 * ludion::math::sin(yaw),
            s.platform[1] + 1.0,
            s.platform[2] + 2.0 * ludion::math::cos(yaw),
        ];
        close(s.barrel, expected)
    });
    // While held, the prop keeps its offset from the hand as the platform carries both.
    let held: Vec<&Sample> = log
        .samples
        .iter()
        .filter(|s| s.frame > GRAB && s.frame < RELEASE)
        .collect();
    let first = held.first().expect("the prop was held");
    let offset = |s: &Sample| std::array::from_fn(|i| s.prop[i] - s.hand[i]);
    let held_ok = held
        .iter()
        .all(|s| s.prop_parented && close(offset(s), offset(first)))
        && held.last().is_some_and(|s| s.prop[0] > first.prop[0] + 1.0);
    // Released: no `Parent`, the world pose did not jump, and the throw moves it.
    let last = log.samples.last().expect("the sim ran");
    let released_ok = !last.prop_parented
        && close(log.before_release, log.after_release)
        && last.prop[2] > log.after_release[2] + 1.0;
    if !(barrel_ok && held_ok && released_ok) {
        eprintln!(
            "the hierarchy misbehaved: barrel {barrel_ok}, held {held_ok}, released {released_ok}"
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
