//! `switch_ragdoll_to_assembly` and `ragdoll_landing` (Neon Shroud request 0008 W3,
//! `Plan/paired-animations.md` decisions 10–14): a voxel ragdoll folds back into its root, which
//! crossfades from a `PoseSnapshot` of the parts.
//!
//! The reference is the **drawn** ragdoll: the part model matrices extracted at the switch tick equal
//! the folded root's assembly instance matrices at the same tick, within [`TOLERANCE`]. The fixture is
//! the humanoid-ish assembly of `ragdoll_velocity.rs`, copied.

use std::path::{Path, PathBuf};

use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_core::time::{FrameIndex, SimTime, TickRate};
use ludion_ecs::Parent;
use ludion_engine::components::PoseSnapshot;
use ludion_engine::{
    Animation, AssemblyData, BlendClock, Engine, Mesh, QualityParams, RagdollToAssembly, Transform,
    animation_events, blend_clocks, crossfade, extract_at, load_assembly, ragdoll_landing,
    switch_ragdoll_to_assembly, switch_to_ragdoll,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};

const ASSEMBLY_REL: &str = "models/humanoid.assembly.yaml";
const PARTS: usize = 5;

/// Absolute tolerance of every matrix and position comparison, per element.
const TOLERANCE: f32 = 1e-4;

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
        "ludion-ragdoll-recover-{}-{name}",
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

/// One `runner` in free-gravity physics: `animation` (indented 8, empty for none), a root rotation
/// in degrees, a uniform scale, and optionally a `CharacterMotor`.
fn scene(animation: &str, rotation: [f32; 3], scale: f32, motor: bool) -> String {
    let animation = if animation.is_empty() {
        String::new()
    } else {
        format!("      Animation:\n{animation}")
    };
    let motor = if motor {
        "      CharacterMotor: {}\n"
    } else {
        ""
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
        rotation:
          - {:?}
          - {:?}
          - {:?}
        scale:
          - {scale:?}
          - {scale:?}
          - {scale:?}
      Mesh:
        shape:
          assembly: {ASSEMBLY_REL}
{animation}{motor}",
        rotation[0], rotation[1], rotation[2]
    )
}

const WALK: &str = "        clip: walk\n";

fn engine_for(name: &str, text: &str) -> (Engine, PathBuf) {
    let root = project(name);
    let mut engine = Engine::builder()
        .project_root(&root)
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

fn advance(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
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

fn close(label: &str, got: &[Mat4], want: &[Mat4]) {
    assert_eq!(got.len(), want.len(), "{label}: instance count");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        for (a, b) in g.to_cols_array().iter().zip(w.to_cols_array()) {
            assert!(
                (a - b).abs() <= TOLERANCE,
                "{label}: part {i} differs: {g:?} vs {w:?}"
            );
        }
    }
}

/// A figure switched to a ragdoll at tick 10 and left to fall.
struct Fall {
    engine: Engine,
    root: PathBuf,
    figure: EntityId,
    parts: Vec<EntityId>,
    data: AssemblyData,
    /// The tick the ragdoll is at (and the fold-back happens at).
    tick: u64,
}

fn fall(name: &str, text: &str, fall_ticks: usize) -> Fall {
    let (mut engine, root) = engine_for(name, text);
    let data = data(&root);
    let figure = figure(&engine);
    advance(&mut engine, 10);
    let parts =
        switch_to_ragdoll(engine.world_mut(), figure, &data, at(10)).expect("a valid switch");
    advance(&mut engine, fall_ticks);
    let tick = engine.frame().as_u64();
    Fall {
        engine,
        root,
        figure,
        parts,
        data,
        tick,
    }
}

impl Fall {
    fn recover(&mut self, clip: &str, fade: f32) -> Result<RagdollToAssembly, Diagnostic> {
        switch_ragdoll_to_assembly(
            self.engine.world_mut(),
            self.figure,
            &self.parts,
            &self.data,
            clip,
            fade,
            at(self.tick),
        )
    }

    fn transform(&self) -> Transform {
        self.engine
            .world()
            .get::<Transform>(self.figure)
            .unwrap()
            .cloned()
            .expect("the root has a Transform")
    }

    fn done(self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn the_first_drawn_frame_is_the_last_ragdoll_frame() {
    for (label, scale) in [("scale1", 1.0), ("scale2", 2.0)] {
        let mut f = fall(label, &scene(WALK, [0.0; 3], scale, false), 20);
        let ragdoll = matrices_at(&f.engine, f.tick);
        assert_eq!(ragdoll.len(), PARTS);
        f.recover("walk", 0.3).expect("a valid fold");
        for &part in &f.parts {
            assert!(!f.engine.world().is_alive(part), "{label}: a part survived");
        }
        close(label, &matrices_at(&f.engine, f.tick), &ragdoll);
        f.done();
    }
}

#[test]
fn the_fade_ends_on_the_clip() {
    let mut f = fall("fade-end", &scene(WALK, [0.0; 3], 1.0, false), 20);
    f.recover("walk", 0.3).unwrap();
    let late = f.tick + 40;
    let faded = matrices_at(&f.engine, late);
    let id = f.figure;
    {
        let animation = f
            .engine
            .world_mut()
            .get_mut::<Animation>(id)
            .unwrap()
            .unwrap();
        animation.fade = 0.0;
        animation.fade_started = FrameIndex::ZERO;
    }
    f.engine.world_mut().remove::<PoseSnapshot>(id).unwrap();
    assert_eq!(matrices_at(&f.engine, late), faded, "not bit-identical");
    f.done();
}

#[test]
fn a_hard_cut_starts_the_clip_at_once_and_keeps_no_snapshot() {
    let mut f = fall("cut", &scene(WALK, [0.0; 3], 1.0, false), 20);
    f.recover("walk", 0.0).unwrap();
    let id = f.figure;
    assert!(!f.engine.world().has::<PoseSnapshot>(id));
    let animation = f
        .engine
        .world()
        .get::<Animation>(id)
        .unwrap()
        .cloned()
        .unwrap();
    assert_eq!(animation.clip, "walk");
    assert_eq!(animation.fade, 0.0);
    f.done();
}

fn lowest_y(data: &AssemblyData, models: &[Mat4]) -> f32 {
    let mut lowest = f32::INFINITY;
    for (part, model) in data.parts.iter().zip(models) {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in part.mesh.positions.chunks_exact(3) {
            let p = Vec3::new(p[0], p[1], p[2]);
            min = min.min(p);
            max = max.max(p);
        }
        for corner in 0..8 {
            let c = Vec3::new(
                if corner & 1 == 0 { min.x } else { max.x },
                if corner & 2 == 0 { min.y } else { max.y },
                if corner & 4 == 0 { min.z } else { max.z },
            );
            lowest = lowest.min(model.transform_point3(c).y);
        }
    }
    lowest
}

/// A rest figure turned face up (+90 about X) or face down (-90), folded back at once.
#[test]
fn face_up_yaw_and_feet_follow_the_placement_rule() {
    for (label, angle, face_up, head_z) in
        [("up", 90.0, true, 1.0f32), ("down", -90.0, false, -1.0)]
    {
        let mut f = fall(label, &scene("", [angle, 0.0, 0.0], 1.0, false), 0);
        let ragdoll = matrices_at(&f.engine, f.tick);
        let torso = f
            .data
            .parts
            .iter()
            .position(|p| p.parent.is_none())
            .unwrap();
        let pivot = Vec3::from(f.data.parts[torso].pivot);
        let pivot_world = ragdoll[torso].transform_point3(pivot);
        let lowest = lowest_y(&f.data, &ragdoll);
        let landing = f.recover("", 0.25).expect("a valid fold");
        assert_eq!(landing.face_up, face_up, "{label}");
        assert!(
            (landing.feet[1] - lowest).abs() <= TOLERANCE,
            "{label}: feet y"
        );
        assert!(
            (landing.feet[0] - pivot_world.x).abs() <= TOLERANCE,
            "{label}: feet x"
        );
        assert!(
            (landing.feet[2] - pivot_world.z).abs() <= TOLERANCE,
            "{label}: feet z"
        );
        // The entity's -Z lies along the head's ground direction.
        let rotation = Quat::from_array(euler_xyz_degrees_to_quat(f.transform().rotation));
        let forward = rotation * Vec3::NEG_Z;
        assert!(
            (forward.z - head_z).abs() <= TOLERANCE && forward.x.abs() <= TOLERANCE,
            "{label}: -Z is {forward:?}, the head lies toward z {head_z}"
        );
        assert_eq!(f.transform().translation, landing.feet, "{label}");
        // And the figure is drawn exactly where the ragdoll lay.
        close(label, &matrices_at(&f.engine, f.tick), &ragdoll);
        f.done();
    }
}

#[test]
fn the_landing_query_equals_the_switch_and_changes_nothing() {
    let mut f = fall("landing", &scene(WALK, [0.0; 3], 1.0, false), 20);
    let before = f.engine.world_hash().unwrap();
    let query = ragdoll_landing(f.engine.world(), f.figure, &f.parts, &f.data).expect("a landing");
    assert_eq!(f.engine.world_hash().unwrap(), before, "the query mutated");
    let switched = f.recover("walk", 0.3).unwrap();
    assert_eq!(query, switched);
    f.done();
}

#[test]
fn retired_parts_fold_back_too() {
    use ludion_spatial::{Collider, Joint, Velocity};
    let mut f = fall("retired", &scene(WALK, [0.0; 3], 1.0, false), 20);
    let ragdoll = matrices_at(&f.engine, f.tick);
    for &part in &f.parts {
        let world = f.engine.world_mut();
        world.remove::<Collider>(part).unwrap();
        world.remove::<Joint>(part).unwrap();
        world.remove::<Velocity>(part).unwrap();
    }
    f.recover("walk", 0.3)
        .expect("a retired ragdoll folds back");
    close("retired", &matrices_at(&f.engine, f.tick), &ragdoll);
    f.done();
}

fn l3036(label: &str, result: Result<RagdollToAssembly, Diagnostic>) -> Diagnostic {
    let diagnostic = result.expect_err(label);
    assert_eq!(diagnostic.code.to_string(), "L3036", "{label}");
    diagnostic
}

#[test]
fn every_refusal_is_l3036_and_leaves_the_world_alone() {
    let mut f = fall("refuse", &scene(WALK, [0.0; 3], 1.0, false), 2);
    let call = |f: &mut Fall, parts: &[EntityId], clip: &str| {
        switch_ragdoll_to_assembly(
            f.engine.world_mut(),
            f.figure,
            parts,
            &f.data,
            clip,
            0.3,
            at(f.tick),
        )
    };
    let hash = f.engine.world_hash().unwrap();
    let parts = f.parts.clone();

    let result = call(&mut f, &parts[..PARTS - 1], "walk");
    l3036("a short part list", result);
    let mut swapped = parts.clone();
    swapped.swap(0, 1);
    let result = call(&mut f, &swapped, "walk");
    l3036("a wrong part order", result);
    let result = call(&mut f, &parts, "wlak");
    let diagnostic = l3036("an unknown clip", result);
    assert!(
        diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("walk")),
        "no suggestion: {:?}",
        diagnostic.help
    );
    assert_eq!(f.engine.world_hash().unwrap(), hash, "a refusal mutated");

    let other = f.engine.world_mut().spawn();
    f.engine
        .world_mut()
        .insert(f.figure, Parent(other))
        .unwrap();
    let hash = f.engine.world_hash().unwrap();
    let result = call(&mut f, &parts, "walk");
    l3036("a parented root", result);
    assert_eq!(f.engine.world_hash().unwrap(), hash, "a refusal mutated");
    f.engine.world_mut().remove::<Parent>(f.figure).unwrap();

    f.recover("walk", 0.3).unwrap();
    let hash = f.engine.world_hash().unwrap();
    let result = call(&mut f, &[], "walk");
    l3036("a root that still has a Mesh", result);
    assert_eq!(f.engine.world_hash().unwrap(), hash, "a refusal mutated");
    f.done();
}

/// A fold-back, then a game crossfade into `run` `offset` ticks later, drawn 10 ticks after that: it
/// must equal the same crossfade on a figure that never ragdolled but stands where this one landed.
fn crossfade_matches_a_twin(label: &str, offset: u64) {
    let text = scene(WALK, [0.0; 3], 1.0, false);
    let mut f = fall(label, &text, 20);
    f.recover("walk", 0.3).unwrap();
    let (mut twin, twin_root) = engine_for(&format!("{label}-twin"), &text);
    let twin_id = figure(&twin);
    let landed = f.transform();
    let animation = f
        .engine
        .world()
        .get::<Animation>(f.figure)
        .unwrap()
        .cloned()
        .unwrap();
    twin.world_mut().insert(twin_id, landed).unwrap();
    twin.world_mut().insert(twin_id, animation).unwrap();
    let g = f.tick + offset;
    for (engine, id) in [(&mut f.engine, f.figure), (&mut twin, twin_id)] {
        let animation = engine
            .world_mut()
            .get_mut::<Animation>(id)
            .unwrap()
            .unwrap();
        crossfade(animation, "run", 0.5, FrameIndex::new(g));
    }
    assert_eq!(
        matrices_at(&f.engine, g + 10),
        matrices_at(&twin, g + 10),
        "{label}: the crossfade faded from the stale snapshot"
    );
    let _ = std::fs::remove_dir_all(twin_root);
    f.done();
}

#[test]
fn a_game_crossfade_during_the_snapshot_fade_fades_from_the_clip() {
    crossfade_matches_a_twin("xfade-during", 5);
}

#[test]
fn a_game_crossfade_after_the_snapshot_fade_fades_from_the_clip() {
    crossfade_matches_a_twin("xfade-after", 40);
}

#[test]
fn a_snapshot_for_another_fade_draws_as_no_snapshot() {
    let mut f = fall("mismatch", &scene(WALK, [0.0; 3], 1.0, false), 20);
    f.recover("walk", 0.3).unwrap();
    let probe = f.tick + 3;
    let bound = matrices_at(&f.engine, probe);
    let id = f.figure;
    f.engine
        .world_mut()
        .get_mut::<PoseSnapshot>(id)
        .unwrap()
        .unwrap()
        .started = FrameIndex::new(f.tick + 1);
    let mismatched = matrices_at(&f.engine, probe);
    f.engine.world_mut().remove::<PoseSnapshot>(id).unwrap();
    let absent = matrices_at(&f.engine, probe);
    assert_eq!(mismatched, absent, "a stale snapshot still drew");
    assert_ne!(bound, absent, "the bound snapshot changed nothing");
    f.done();
}

#[test]
fn a_ragdoll_switch_mid_snapshot_fade_spawns_at_the_drawn_pose_and_drops_the_snapshot() {
    let mut f = fall("remid", &scene(WALK, [0.0; 3], 1.0, false), 20);
    f.recover("walk", 0.3).unwrap();
    advance(&mut f.engine, 5);
    let now = f.engine.frame().as_u64();
    let drawn = matrices_at(&f.engine, now);
    assert!(f.engine.world().has::<PoseSnapshot>(f.figure));
    let spawned = switch_to_ragdoll(f.engine.world_mut(), f.figure, &f.data, at(now))
        .expect("a valid switch");
    assert_eq!(spawned.len(), PARTS);
    assert!(!f.engine.world().has::<PoseSnapshot>(f.figure));
    // The new parts reuse the freed entity slots, so the extract order is not the authored one:
    // compare by position (every part sits at its own, a unit or more apart).
    let by_position = |mut models: Vec<Mat4>| {
        models.sort_by(|a, b| {
            a.w_axis
                .to_array()
                .partial_cmp(&b.w_axis.to_array())
                .expect("finite translations")
        });
        models
    };
    close(
        "remid",
        &by_position(matrices_at(&f.engine, now)),
        &by_position(drawn),
    );
    f.done();
}

fn snapshot_of(engine: &mut Engine) -> serde_json::Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: serde_json::Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

/// Falls, folds back at the switch tick, and advances `after` ticks, returning the hash trace.
fn recovered(name: &str, after: usize) -> (Fall, Vec<u64>) {
    let mut f = fall(name, &scene(WALK, [0.0; 3], 1.0, false), 20);
    f.recover("walk", 0.3).unwrap();
    let hashes = advance(&mut f.engine, after);
    (f, hashes)
}

#[test]
fn a_snapshot_mid_fade_continues_hash_identical() {
    let (full, full_hashes) = recovered("restore-full", 60);
    let (mut head, head_hashes) = recovered("restore-head", 5);
    let snapshot = snapshot_of(&mut head.engine);
    let (mut resumed, root) = engine_for("restore-tail", &scene(WALK, [0.0; 3], 1.0, false));
    restore(&mut resumed, snapshot);
    assert!(
        resumed.world().has::<PoseSnapshot>(head.figure),
        "the restored root lost its PoseSnapshot"
    );
    let mut joined = head_hashes;
    joined.extend(advance(&mut resumed, 55));
    assert_eq!(
        joined, full_hashes,
        "a resumed fade differs from the uninterrupted one"
    );
    for dir in [full.root.clone(), head.root.clone(), root] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn the_fold_back_is_identical_at_1_2_8_threads() {
    let traces: Vec<Vec<u64>> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let (f, hashes) = recovered(&format!("threads-{n}"), 40);
                f.done();
                hashes
            })
        })
        .collect();
    assert_eq!(traces[0], traces[1], "1 vs 2 threads");
    assert_eq!(traces[0], traces[2], "1 vs 8 threads");
}

/// The landing a motor figure is revived at is where the fold-back already put it: no pop.
#[test]
fn reviving_a_motor_figure_at_the_landing_leaves_its_transform_bit_identical() {
    let mut f = fall("motor", &scene(WALK, [0.0; 3], 1.0, true), 20);
    let landing = f.recover("walk", 0.3).expect("a valid fold");
    let before = f.transform();
    // The despawned parts' bodies leave the physics world on the next step, so a revive waits one
    // tick (in a game, the get-up clip plays first).
    advance(&mut f.engine, 2);
    assert_eq!(f.transform(), before, "the root moved while waiting");
    let report = ludion_engine::resume_character(
        f.engine.world_mut(),
        f.figure,
        landing.feet,
        landing.root_yaw,
        None,
    )
    .expect("the motor resumes");
    assert!(report.resumed, "blocked by {:?}", report.blocked_by);
    assert_eq!(f.transform(), before, "the revive moved the root");
    f.done();
}
