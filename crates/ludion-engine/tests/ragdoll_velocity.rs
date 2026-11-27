//! `RagdollSwitch::part_velocity` (Neon Shroud request 0008 W2): a posed voxel switch adds the drawn
//! pose's own motion to each part's `Velocity` — a two-sample finite difference of the part's posed
//! world transform at the switch tick and one tick earlier.
//!
//! The reference for every check is the **drawn** pose: step to tick `N - 1`, extract the part model
//! matrices, step to `N`, extract again, then switch at `N`. Per part, the spawn velocity equals
//! `(center_N - center_{N-1}) / dt` linearly and the relative-rotation rate angularly, within
//! [`TOLERANCE`]. A live `BlendClock` blend is the case that fails without the closed-form earlier
//! sample (the clock's phase at `N - 1` is the same as at `N` for a one-tick-earlier `animation_pose`).
//!
//! The fixture is the humanoid-ish assembly of `assembly_blends.rs`, copied: its single-voxel parts
//! have the bounds the switch takes a part's center from.

use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat3, Mat4, Quat, Vec3};
use ludion_core::math::sqrt;
use ludion_core::time::{GameClock, GameTime, SimTime, TickRate};
use ludion_ecs::SystemCtx;
use ludion_ecs::World;
use ludion_engine::{
    Animation, AssemblyData, BlendClock, Engine, Mesh, QualityParams, RagdollSwitch,
    animation_events, blend_clocks, extract_at, load_assembly, switch_to_ragdoll,
    switch_to_ragdoll_with,
};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_spatial::Velocity;

const ASSEMBLY_REL: &str = "models/humanoid.assembly.yaml";
const PARTS: usize = 5;

/// Relative tolerance of every velocity comparison: `|a - b| <= TOLERANCE * (1 + |b|)`.
const TOLERANCE: f64 = 1e-3;

const ASSEMBLY: &str = "format: 1
kind: assembly
name: humanoid
size:
  - 3
  - 3
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: leg
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 0
      - 0
      - 0
  - name: torso
    file: parts/cell.part.yaml
    place:
      - 1
      - 1
      - 0
  - name: head
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 1
      - 2
      - 0
  - name: arm
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 2
      - 1
      - 0
  - name: foot
    file: parts/cell.part.yaml
    parent: leg
    place:
      - 0
      - 0
      - 0
clips:
  idle: anims/idle.anim.yaml
  run: anims/run.anim.yaml
  walk: anims/walk.anim.yaml
masks:
  arm_only:
    joints:
      - arm
blends:
  locomotion:
    sync: foot_l
    points:
      - clip: idle
        at: 0.0
      - clip: walk
        at: 2.0
      - clip: run
        at: 5.0
";

fn track(target: &str, channel: &str, from: [f64; 3], to: [f64; 3], duration: f64) -> String {
    let keys = |t: f64, v: [f64; 3]| {
        format!(
            "      - t: {t:?}\n        v:\n          - {:?}\n          - {:?}\n          - {:?}\n",
            v[0], v[1], v[2]
        )
    };
    format!(
        "  {target}:\n    {channel}:\n{}{}",
        keys(0.0, from),
        keys(duration, to)
    )
}

/// A looping clip with `foot_l` at a quarter and `foot_r` at three quarters (the blend's `sync`).
fn clip(name: &str, duration: f64, tracks: &str) -> String {
    format!(
        "format: 1\nkind: animation\nname: {name}\nduration: {duration:?}\nloop: true\nevents:\n  - t: {:?}\n    name: foot_l\n  - t: {:?}\n    name: foot_r\ntracks:\n{tracks}",
        duration * 0.25,
        duration * 0.75
    )
}

fn clips() -> Vec<(&'static str, String)> {
    let idle = format!(
        "{}{}",
        track("torso", "rotation", [0.0; 3], [0.0, 0.0, 6.0], 2.0),
        track("arm", "rotation", [0.0; 3], [5.0, 0.0, 0.0], 2.0)
    );
    let walk = format!(
        "{}{}{}",
        track("leg", "rotation", [0.0; 3], [30.0, 0.0, 0.0], 1.0),
        track("arm", "rotation", [0.0; 3], [45.0, 0.0, 0.0], 1.0),
        track("head", "translation", [0.0; 3], [0.0, 1.0, 0.0], 1.0)
    );
    let run = format!(
        "{}{}{}",
        track("leg", "rotation", [0.0; 3], [60.0, 0.0, 0.0], 0.5),
        track("arm", "rotation", [0.0; 3], [90.0, 0.0, 0.0], 0.5),
        track("torso", "rotation", [0.0; 3], [0.0, 20.0, 0.0], 0.5)
    );
    vec![
        ("idle", clip("idle", 2.0, &idle)),
        ("walk", clip("walk", 1.0, &walk)),
        ("run", clip("run", 0.5, &run)),
    ]
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-ragdoll-velocity-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir, ASSEMBLY_REL, ASSEMBLY);
    write(
        &dir,
        "models/parts/cell.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    for (stem, text) in clips() {
        write(&dir, &format!("models/anims/{stem}.anim.yaml"), &text);
    }
    dir
}

/// One `runner` in free-gravity physics, with `animation` (indented 8, empty for none) and optional
/// `layers` (list items indented 10).
fn scene(animation: &str, layers: &str) -> String {
    let animation = if animation.is_empty() {
        String::new()
    } else {
        format!("      Animation:\n{animation}")
    };
    let layered = if layers.is_empty() {
        String::new()
    } else {
        format!("      AnimationLayers:\n        layers:\n{layers}")
    };
    format!(
        "format: 1
seed: 1
physics:
  space: free
entities:
  - name: runner
    components:
      Transform:
        translation:
          - 1.0
          - 5.0
          - 2.0
      Mesh:
        shape:
          assembly: {ASSEMBLY_REL}
{animation}{layered}"
    )
}

type Driver = fn(&mut World, &SystemCtx);

/// Idle, then a 0.5 s crossfade into `locomotion` at 30.
fn fade_in(world: &mut World, ctx: &SystemCtx) {
    if ctx.game_frame.as_u64() == 30 {
        for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
            animation.params = [5.0, 0.0];
            ludion_engine::crossfade(animation, "locomotion", 0.5, ctx.game_frame);
        }
    }
}

/// Slows the game on every frame.
fn slow(world: &mut World, _ctx: &SystemCtx) {
    world.resource_mut::<GameClock>().unwrap().scale(0.5);
}

/// Freezes the game from frame 30 on.
fn freeze(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() >= 30 {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

fn engine_for(name: &str, text: &str, driver: Option<Driver>) -> (Engine, PathBuf) {
    let root = project(name);
    let mut builder = Engine::builder().project_root(&root);
    if let Some(driver) = driver {
        builder = builder
            .system("driver", driver)
            .query::<(&mut Animation,)>();
    }
    let mut engine = builder
        .system("blend_clocks", blend_clocks)
        .query::<(&Animation, &Mesh)>()
        .query::<(&ludion_engine::Animation, &ludion_engine::PrefabInstance)>()
        .query::<(&mut BlendClock,)>()
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&BlendClock,)>()
        .build()
        .expect("the game builds");
    engine.load_inline_scene(text).expect("the scene loads");
    (engine, root)
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("steps");
}

fn at(tick: u64) -> SimTime {
    SimTime::from_ticks(tick, TickRate::DEFAULT)
}

fn matrices_at(engine: &Engine, tick: u64) -> Vec<Mat4> {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        at(tick),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
    .meshes
    .iter()
    .map(|mesh| mesh.model)
    .collect()
}

fn data(root: &Path) -> AssemblyData {
    load_assembly(ludion_assets::AssetRoot::content(root), ASSEMBLY_REL)
        .expect("the assembly loads")
}

fn figure(engine: &Engine) -> EntityId {
    engine
        .world()
        .entities()
        .find(|&id| engine.world().component::<Mesh>(id).is_some())
        .expect("the runner")
}

fn velocities(world: &World, spawned: &[EntityId]) -> Vec<Velocity> {
    spawned
        .iter()
        .map(|&id| {
            world
                .get::<Velocity>(id)
                .ok()
                .flatten()
                .copied()
                .expect("a part moves")
        })
        .collect()
}

fn with_velocity() -> RagdollSwitch {
    let mut opts = RagdollSwitch::default();
    opts.part_velocity = true;
    opts
}

/// A part's center in the world: the drawn model matrix applied to the part mesh's bounds center.
fn centers(data: &AssemblyData, models: &[Mat4]) -> Vec<Vec3> {
    data.parts
        .iter()
        .zip(models)
        .map(|(part, model)| {
            let mut min = Vec3::splat(f32::MAX);
            let mut max = Vec3::splat(f32::MIN);
            for p in part.mesh.positions.chunks_exact(3) {
                let p = Vec3::new(p[0], p[1], p[2]);
                min = min.min(p);
                max = max.max(p);
            }
            model.transform_point3((min + max) * 0.5)
        })
        .collect()
}

fn rotation(model: &Mat4) -> Quat {
    let q = Quat::from_mat3(&Mat3::from_mat4(*model));
    let len = sqrt(q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w);
    Quat::from_xyzw(q.x / len, q.y / len, q.z / len, q.w / len)
}

/// The drawn motion between `before` and `after` models `dt` seconds apart: per part,
/// `(linear, angular)` in metres and radians per second.
fn drawn_motion(
    data: &AssemblyData,
    before: &[Mat4],
    after: &[Mat4],
    dt: f64,
) -> Vec<([f64; 3], [f64; 3])> {
    let (c0, c1) = (centers(data, before), centers(data, after));
    (0..data.parts.len())
        .map(|i| {
            let linear = ((c1[i] - c0[i]).as_dvec3() / dt).to_array();
            let (q0, q1) = (rotation(&before[i]), rotation(&after[i]));
            let mut d = q1 * q0.inverse();
            if d.w < 0.0 {
                d = -d;
            }
            let (axis, angle) = d.to_axis_angle();
            let angular = (axis.as_dvec3() * f64::from(angle) / dt).to_array();
            (linear, angular)
        })
        .collect()
}

fn close(label: &str, got: [f32; 3], want: [f64; 3]) {
    for axis in 0..3 {
        let (g, w) = (f64::from(got[axis]), want[axis]);
        assert!(
            (g - w).abs() <= TOLERANCE * (1.0 + w.abs()),
            "{label} axis {axis}: got {g}, drawn {w}"
        );
    }
}

fn magnitude(motion: &[([f64; 3], [f64; 3])]) -> f64 {
    motion
        .iter()
        .flat_map(|(l, a)| l.iter().chain(a))
        .map(|v| v.abs())
        .fold(0.0, f64::max)
}

/// Steps to `tick - 1` and `tick`, switches at `tick` with `part_velocity`, and checks every part's
/// spawn velocity against the drawn one-tick difference.
fn check_drawn_difference(label: &str, text: &str, driver: Option<Driver>, tick: u64) {
    let (mut engine, root) = engine_for(label, text, driver);
    let data = data(&root);
    step(&mut engine, tick - 1);
    let before = matrices_at(&engine, tick - 1);
    step(&mut engine, 1);
    let after = matrices_at(&engine, tick);
    assert_eq!(after.len(), PARTS, "{label}");
    let dt = TickRate::DEFAULT.seconds_per_tick();
    let want = drawn_motion(&data, &before, &after, dt);
    assert!(
        magnitude(&want) > 1e-2,
        "{label}: the drawn pose must move for this test to mean anything"
    );
    let entity = figure(&engine);
    let spawned =
        switch_to_ragdoll_with(engine.world_mut(), entity, &data, at(tick), with_velocity())
            .expect("a valid switch");
    for (i, v) in velocities(engine.world(), &spawned).iter().enumerate() {
        close(&format!("{label} part {i} linear"), v.linear, want[i].0);
        close(&format!("{label} part {i} angular"), v.angular, want[i].1);
    }
    let _ = std::fs::remove_dir_all(&root);
}

const WALK: &str = "        clip: walk\n";
const LOCOMOTION: &str =
    "        clip: locomotion\n        params:\n          - 3.0\n          - 0.0\n";

#[test]
fn a_mid_clip_switch_takes_the_drawn_one_tick_difference() {
    check_drawn_difference("clip", &scene(WALK, ""), None, 20);
}

/// The case the closed form exists for: the clock phase at `N - 1` equals the one at `N`.
#[test]
fn a_live_blend_switch_takes_the_drawn_one_tick_difference() {
    check_drawn_difference("blend", &scene(LOCOMOTION, ""), None, 20);
}

#[test]
fn a_crossfade_in_progress_takes_the_drawn_one_tick_difference() {
    check_drawn_difference(
        "fade",
        &scene("        clip: idle\n", ""),
        Some(fade_in),
        45,
    );
}

#[test]
fn an_override_layer_takes_the_drawn_one_tick_difference() {
    let clip_layer = "          - name: sprint\n            clip: run\n            mask: arm_only\n            weight: 1.0\n";
    let blend_layer = "          - name: stride\n            clip: locomotion\n            weight: 1.0\n            mask: arm_only\n            params:\n              - 5.0\n              - 0.0\n";
    check_drawn_difference("layer-clip", &scene(WALK, clip_layer), None, 20);
    check_drawn_difference("layer-blend", &scene(WALK, blend_layer), None, 20);
}

/// Velocity is per gameplay second: a figure switched under a 0.5 time scale takes the velocity the
/// drawn pose has per gameplay tick, which an unscaled switch at the same gameplay tick also takes.
#[test]
fn a_time_scaled_switch_reads_the_drawn_pose_in_gameplay_time() {
    let root = project("scaled");
    let mut engine = {
        let mut engine = Engine::builder()
            .project_root(&root)
            .system("slow", slow)
            .build()
            .expect("builds");
        engine.load_inline_scene(&scene(WALK, "")).unwrap();
        engine
    };
    let data = data(&root);
    step(&mut engine, 40);
    let now = at(engine.frame().as_u64());
    let game: GameTime = engine
        .world()
        .resource::<GameClock>()
        .unwrap()
        .game_instant(now);
    assert_eq!(
        game.as_q16() % 65536,
        0,
        "an even sim tick is a whole gameplay tick"
    );
    let g = game.as_q16() / 65536;
    assert!(g < 40, "the clock is scaled: gameplay tick {g}");
    let dt = TickRate::DEFAULT.seconds_per_tick();
    // Extraction maps a sim tick through the clock (floored), and the plain clip is stateless: the
    // latest sim ticks drawing gameplay ticks `g - 1` and `g` give the drawn one-tick difference.
    let sim = engine.frame().as_u64();
    let clock = *engine.world().resource::<GameClock>().unwrap();
    let before = (0..sim)
        .rev()
        .find(|&t| clock.game_time(at(t)).ticks() == g - 1)
        .expect("a sim tick draws the previous gameplay tick");
    let want = drawn_motion(
        &data,
        &matrices_at(&engine, before),
        &matrices_at(&engine, sim),
        dt,
    );
    assert!(magnitude(&want) > 1e-2);
    let entity = figure(&engine);
    let spawned = switch_to_ragdoll_with(engine.world_mut(), entity, &data, game, with_velocity())
        .expect("a valid switch");
    for (i, v) in velocities(engine.world(), &spawned).iter().enumerate() {
        close(&format!("scaled part {i} linear"), v.linear, want[i].0);
        close(&format!("scaled part {i} angular"), v.angular, want[i].1);
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A hold freezes gameplay time: the drawn pose is static, and the switch still reads the latent
/// velocity, the same as an unheld switch at that gameplay tick. (A hold does not reach physics -
/// `docs/game-clock.md` - so the parts fall on regardless.)
#[test]
fn a_held_switch_keeps_the_latent_velocity() {
    let (mut held, root_a) = {
        let root = project("held");
        let mut engine = Engine::builder()
            .project_root(&root)
            .system("freeze", freeze)
            .build()
            .expect("builds");
        engine.load_inline_scene(&scene(WALK, "")).unwrap();
        (engine, root)
    };
    let data = data(&root_a);
    step(&mut held, 40);
    let now = at(held.frame().as_u64());
    let game = held
        .world()
        .resource::<GameClock>()
        .unwrap()
        .game_instant(now);
    assert_eq!(game.as_q16() % 65536, 0);
    let g = game.as_q16() / 65536;
    assert!(g < 40, "gameplay time froze at tick {g}");
    let entity = figure(&held);
    let spawned = switch_to_ragdoll_with(held.world_mut(), entity, &data, game, with_velocity())
        .expect("a valid switch");
    let held_velocity = velocities(held.world(), &spawned);

    let (mut free, root_b) = engine_for("unheld", &scene(WALK, ""), None);
    step(&mut free, g);
    let entity = figure(&free);
    let spawned_free =
        switch_to_ragdoll_with(free.world_mut(), entity, &data, at(g), with_velocity())
            .expect("a valid switch");
    assert_eq!(held_velocity, velocities(free.world(), &spawned_free));

    for root in [root_a, root_b] {
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn a_rest_figure_gets_the_same_velocity_with_or_without_the_flag() {
    let (mut engine, root) = engine_for("rest", &scene("", ""), None);
    let data = data(&root);
    step(&mut engine, 20);
    let entity = figure(&engine);
    let flagged =
        switch_to_ragdoll_with(engine.world_mut(), entity, &data, at(20), with_velocity())
            .expect("a valid switch");
    let (mut plain, root_b) = engine_for("rest-plain", &scene("", ""), None);
    step(&mut plain, 20);
    let entity = figure(&plain);
    let unflagged =
        switch_to_ragdoll(plain.world_mut(), entity, &data, at(20)).expect("a valid switch");
    assert_eq!(
        velocities(engine.world(), &flagged),
        velocities(plain.world(), &unflagged)
    );
    for root in [root, root_b] {
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn the_default_switch_velocity_is_switch_to_ragdoll_bit_for_bit() {
    let text = scene(LOCOMOTION, "");
    let (mut a, root_a) = engine_for("default-a", &text, None);
    let (mut b, root_b) = engine_for("default-b", &text, None);
    let data = data(&root_a);
    step(&mut a, 20);
    step(&mut b, 20);
    let (ea, eb) = (figure(&a), figure(&b));
    let plain = switch_to_ragdoll(a.world_mut(), ea, &data, at(20)).expect("a valid switch");
    let with = switch_to_ragdoll_with(b.world_mut(), eb, &data, at(20), RagdollSwitch::default())
        .expect("a valid switch");
    let bits = |v: Vec<Velocity>| -> Vec<[u32; 6]> {
        v.iter()
            .map(|v| {
                let [a, b, c] = v.linear.map(f32::to_bits);
                let [d, e, f] = v.angular.map(f32::to_bits);
                [a, b, c, d, e, f]
            })
            .collect()
    };
    assert_eq!(
        bits(velocities(a.world(), &plain)),
        bits(velocities(b.world(), &with))
    );
    // And the flag really does change a posed figure.
    let (mut c, root_c) = engine_for("default-c", &text, None);
    step(&mut c, 20);
    let ec = figure(&c);
    let flagged =
        switch_to_ragdoll_with(c.world_mut(), ec, &data, at(20), with_velocity()).unwrap();
    assert_ne!(
        bits(velocities(a.world(), &plain)),
        bits(velocities(c.world(), &flagged))
    );
    for root in [root_a, root_b, root_c] {
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn a_part_velocity_fall_is_identical_at_1_2_8_threads() {
    let traces: Vec<Vec<u64>> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let (mut engine, root) =
                    engine_for(&format!("threads-{n}"), &scene(LOCOMOTION, ""), None);
                let data = data(&root);
                step(&mut engine, 20);
                let entity = figure(&engine);
                switch_to_ragdoll_with(engine.world_mut(), entity, &data, at(20), with_velocity())
                    .expect("a valid switch");
                let hashes = (0..90)
                    .map(|_| {
                        engine.advance_live();
                        engine.world_hash().unwrap().as_u64()
                    })
                    .collect();
                let _ = std::fs::remove_dir_all(&root);
                hashes
            })
        })
        .collect();
    assert_eq!(traces[0], traces[1], "1 vs 2 threads");
    assert_eq!(traces[0], traces[2], "1 vs 8 threads");
}
