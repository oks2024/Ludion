//! Runtime proof for root motion beyond unbound voxel assemblies (`Plan/animation-layers.md`
//! decisions 56 and 65-67, Neon Shroud request 0007 W6b-2a).
//!
//! - **Skinned.** A character's `$root` clip sums to its travel times the scale, a binding `speed`
//!   of 2 arrives in half the time, and a 1D blend sums its points over the `BlendClock` phase.
//! - **Prefab.** A prefab root sums its one-shot's travel; a non-root part reads `None`.
//! - **Entity clock.** A sequence-bound actor travels while a holding sequence freezes an unbound
//!   sibling.
//! - **Inert and deterministic.** Unconsumed skinned and prefab `$root` clips leave the world hash
//!   bit-identical, and a skinned blend matches at 1 and 8 threads.
//!
//! The fixtures are scratch projects; the skinned mesh is the gallery mannequin's.

use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::sequences::{self, SequenceBound};
use ludion_engine::{
    Animation, BlendClock, Engine, Mesh, RootMotion, Transform, blend_clocks, root_motion_delta,
};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;

const CHARACTER: &str = "models/skin.assembly.yaml";
const VOXEL: &str = "models/block.assembly.yaml";
const PREFAB: &str = "dash.prefab.yaml";
const BEAT: &str = "sequences/beat.yaml";
const PLAY_AT: usize = 50;
const LENGTH: usize = 40;

/// A clip turning `target`; `travel` adds a `$root` track walking that far toward `-Z`.
fn clip(target: &str, duration: f64, looped: bool, travel: Option<f64>) -> String {
    let mut text = format!(
        "format: 1\nkind: animation\nduration: {duration:?}\nloop: {looped}\nevents:\n  - t: 0.0\n    name: foot_l\ntracks:\n  {target}:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: {duration:?}\n        v:\n          - 0.0\n          - 30.0\n          - 0.0\n"
    );
    if let Some(travel) = travel {
        text.push_str(&format!(
            "  \"$root\":\n    translation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: {duration:?}\n        v:\n          - 0.0\n          - 0.0\n          - {:?}\n",
            -travel
        ));
    }
    text
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The skinned assembly (`lunge`, the same clip as `dash` at speed 2, and the 1D blend `stride`),
/// a one-voxel assembly with a looped `walk`, a two-part prefab with a one-shot `lunge`, and the
/// beat that binds an actor.
fn project(name: &str, with_root: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-root-motion-models-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let root = |travel: f64| with_root.then_some(travel);

    let mesh = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/meshes/skin-mannequin.mesh.yaml");
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::copy(mesh, dir.join("models/skin.mesh.yaml")).unwrap();
    write(
        &dir,
        CHARACTER,
        "format: 1
kind: assembly
name: skin
mesh: skin.mesh.yaml
clips:
  dash:
    clip: anims/lunge.anim.yaml
    speed: 2.0
  lunge: anims/lunge.anim.yaml
  stride_a: anims/stride_a.anim.yaml
  stride_b: anims/stride_b.anim.yaml
blends:
  stride:
    sync: foot_l
    points:
      - clip: stride_a
        at: 0.0
      - clip: stride_b
        at: 2.0
",
    );
    for (stem, text) in [
        ("lunge", clip("pelvis", 1.0, false, root(8.0))),
        ("stride_a", clip("pelvis", 1.0, true, root(8.0))),
        ("stride_b", clip("pelvis", 0.5, true, root(4.0))),
    ] {
        write(&dir, &format!("models/anims/{stem}.anim.yaml"), &text);
    }

    write(
        &dir,
        VOXEL,
        "format: 1
kind: assembly
name: block
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: cell.part.yaml
    place:
      - 0
      - 0
      - 0
clips:
  walk: walk.anim.yaml
",
    );
    write(
        &dir,
        "models/cell.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    write(
        &dir,
        "models/walk.anim.yaml",
        &clip("body", 1.0, true, root(8.0)),
    );

    write(
        &dir,
        PREFAB,
        "format: 1
kind: prefab
name: dash
clips:
  lunge: lunge.anim.yaml
entities:
  - name: base
    components:
      Transform: {}
  - name: lid
    components:
      Transform: {}
      Parent:
        index: 0
        generation: 0
",
    );
    write(&dir, "lunge.anim.yaml", &clip("lid", 1.0, false, root(3.0)));
    write(
        &dir,
        BEAT,
        &format!(
            "format: 1
kind: sequence
roles:
  - actor
length_ticks: {LENGTH}
clock: sim
holds_gameplay: true
tracks:
  animation:
    - at: 0
      role: actor
      clip: walk
"
        ),
    );
    dir
}

fn scene(entities: &str) -> String {
    format!("format: 1\nseed: 1\nentities:\n{entities}")
}

/// An entity playing `clip` on the assembly shaped `kind` at `rel`.
fn runner(name: &str, kind: &str, rel: &str, clip: &str, scale: [f64; 3]) -> String {
    format!(
        "  - name: {name}
    components:
      Transform:
        scale:
          - {:?}
          - {:?}
          - {:?}
      Mesh:
        shape:
          {kind}: {rel}
      Animation:
        clip: {clip}
",
        scale[0], scale[1], scale[2]
    )
}

fn skinned(name: &str, clip: &str) -> String {
    runner(name, "character", CHARACTER, clip, [1.0; 3])
}

fn blend_runner(name: &str, param: f64) -> String {
    format!(
        "{}        params:\n          - {param:?}\n          - 0.0\n",
        skinned(name, "stride")
    )
}

/// A prefab instance whose root and non-root part both carry the clip.
fn prefab_instance(name: &str) -> String {
    format!(
        "  - name: {name}
    prefab: {PREFAB}
    overrides:
      base:
        Animation:
          clip: lunge
      lid:
        Animation:
          clip: lunge
"
    )
}

/// What the root system read: `(entity, tick, delta)` for every animated entity every tick.
#[derive(Default)]
struct Seen(Vec<(EntityId, u64, Option<RootMotion>)>);

fn read_roots(world: &mut World, ctx: &SystemCtx) {
    let ids: Vec<_> = world
        .query_ids::<(&Animation, &Transform)>()
        .map(|(id, _)| id)
        .collect();
    let tick = ctx.frame.as_u64();
    let read: Vec<_> = ids
        .into_iter()
        .map(|id| (id, tick, root_motion_delta(&*world, id, ctx)))
        .collect();
    if world.resource::<Seen>().is_none() {
        world.insert_resource(Seen::default());
    }
    world.resource_mut::<Seen>().unwrap().0.extend(read);
}

fn build(root: &Path, text: &str, read: bool) -> Engine {
    let mut builder = Engine::builder()
        .project_root(root)
        .sequences()
        .system("blend_clocks", blend_clocks)
        .query::<(&Animation, &Mesh)>()
        .query::<(&ludion_engine::Animation, &ludion_engine::PrefabInstance)>()
        .query::<(&mut BlendClock,)>();
    if read {
        builder = builder
            .system("read_roots", read_roots)
            .query::<(&Animation, &Mesh)>()
            .query::<(&Animation, &Transform)>();
    }
    let mut engine = builder.build().expect("the game builds");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("steps");
}

fn ids(engine: &Engine) -> Vec<EntityId> {
    engine.world().entities().collect()
}

fn records(engine: &Engine, id: EntityId) -> Vec<(u64, Option<RootMotion>)> {
    engine
        .world()
        .resource::<Seen>()
        .expect("the root system ran")
        .0
        .iter()
        .filter(|record| record.0 == id)
        .map(|record| (record.1, record.2))
        .collect()
}

/// The summed `z` travel of `id`.
fn travelled(engine: &Engine, id: EntityId) -> f64 {
    records(engine, id)
        .iter()
        .filter_map(|(_, rm)| *rm)
        .map(|rm| f64::from(rm.translation[2]))
        .sum()
}

fn close(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() < 1e-4, "{what}: {a} vs {b}");
}

#[test]
fn a_skinned_root_clip_sums_to_the_authored_travel_times_the_scale() {
    let root = project("skinned", true);
    let text = scene(&runner(
        "a",
        "character",
        CHARACTER,
        "lunge",
        [2.0, 1.0, 3.0],
    ));
    let mut engine = build(&root, &text, true);
    step(&mut engine, 90);
    close(travelled(&engine, ids(&engine)[0]), -8.0 * 3.0, "z");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_skinned_binding_speed_of_two_arrives_in_half_the_time() {
    let root = project("speed", true);
    let text = scene(&format!(
        "{}{}",
        skinned("slow", "lunge"),
        skinned("fast", "dash")
    ));
    let mut engine = build(&root, &text, true);
    // 31 steps open at 0 and cover half a second.
    step(&mut engine, 31);
    let all = ids(&engine);
    close(travelled(&engine, all[0]), -4.0, "half way at speed 1");
    close(travelled(&engine, all[1]), -8.0, "arrived at speed 2");
    step(&mut engine, 60);
    close(travelled(&engine, all[0]), -8.0, "speed 1 arrives later");
    close(travelled(&engine, all[1]), -8.0, "speed 2 clamps");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_skinned_blend_sums_weight_times_phase_times_cycle_travel() {
    let root = project("blend", true);
    let mut engine = build(&root, &scene(&blend_runner("a", 0.5)), true);
    step(&mut engine, 121);
    let id = ids(&engine)[0];
    // The clock started at phase 0.
    let phase = engine
        .world()
        .get::<BlendClock>(id)
        .unwrap()
        .expect("the entity carries a BlendClock")
        .phase;
    assert!(phase > 2.0, "{phase}");
    let expected = phase * (0.75 * -8.0 + 0.25 * -4.0);
    let z = travelled(&engine, id);
    assert!((z - expected).abs() < 1e-5, "{z} vs {expected}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_prefab_root_sums_its_travel_and_a_part_reads_none() {
    let root = project("prefab", true);
    let mut engine = build(&root, &scene(&prefab_instance("a")), true);
    step(&mut engine, 90);
    let all = ids(&engine);
    assert_eq!(all.len(), 2, "the root and its lid");
    close(travelled(&engine, all[0]), -3.0, "the one-shot's travel");
    let part = records(&engine, all[1]);
    assert_eq!(part.len(), 90, "the part is read every tick");
    assert!(part.iter().all(|(_, rm)| rm.is_none()), "{part:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_sequence_bound_actor_travels_while_the_world_is_held() {
    let root = project("bound", true);
    let text = scene(&format!(
        "{}{}",
        runner("actor", "assembly", VOXEL, "walk", [1.0; 3]),
        runner("sibling", "assembly", VOXEL, "walk", [1.0; 3])
    ));
    let mut engine = build(&root, &text, true);
    let all = ids(&engine);
    let (actor, sibling) = (all[0], all[1]);
    step(&mut engine, PLAY_AT as u64);
    sequences::play(engine.world_mut(), BEAT, &[("actor", actor)]).unwrap();
    // The tick the beat starts on still advances the world; the hold is the ones after it.
    step(&mut engine, 2);
    let first_held = engine.frame().as_u64();
    step(&mut engine, LENGTH as u64 - 4);
    assert!(
        engine.world().has::<SequenceBound>(actor),
        "the actor is still bound"
    );
    let held_last = engine.frame().as_u64();
    let held = |id| {
        records(&engine, id)
            .into_iter()
            .filter(|(tick, _)| (first_held..held_last).contains(tick))
            .map(|(_, rm)| rm.expect("the clip carries $root").translation[2])
            .collect::<Vec<_>>()
    };
    let (bound, frozen) = (held(actor), held(sibling));
    assert_eq!(bound.len(), LENGTH - 4);
    for z in bound {
        close(
            f64::from(z),
            -8.0 / 60.0,
            "the bound actor's tick of travel",
        );
    }
    assert!(frozen.iter().all(|z| *z == 0.0), "{frozen:?}");
    let _ = std::fs::remove_dir_all(root);
}

fn trace_of(
    engine: &mut Engine,
    ticks: u64,
) -> Vec<(ludion_core::time::FrameIndex, ludion_sim::WorldHash)> {
    let mut trace = Vec::new();
    for _ in 0..ticks {
        let one = Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        });
        trace.extend(
            ludion_engine::drive_hashes(engine, vec![one])
                .expect("the session drives")
                .into_iter()
                .skip(1),
        );
    }
    trace
}

#[test]
fn unconsumed_skinned_and_prefab_root_tracks_do_not_move_the_hash() {
    let text = scene(&format!(
        "{}{}{}",
        skinned("a", "lunge"),
        blend_runner("b", 0.5),
        prefab_instance("c")
    ));
    let run = |name: &str, with_root: bool| {
        let root = project(name, with_root);
        let mut engine = build(&root, &text, false);
        let trace = trace_of(&mut engine, 45);
        let _ = std::fs::remove_dir_all(root);
        trace
    };
    let (with_root, without) = (run("inert-with", true), run("inert-without", false));
    assert_eq!(with_root.len(), 45);
    assert_eq!(with_root, without, "the world hash moved");
}

#[test]
fn a_skinned_blend_matches_at_1_and_8_threads() {
    let mut entities = String::new();
    for i in 0..40 {
        entities.push_str(&blend_runner(&format!("m{i}"), f64::from(i) / 20.0));
    }
    let text = scene(&entities);
    let runs: Vec<_> = [1usize, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let root = project(&format!("threads{n}"), true);
                let mut engine = build(&root, &text, true);
                let trace = trace_of(&mut engine, 30);
                let bits: Vec<_> = engine
                    .world()
                    .resource::<Seen>()
                    .expect("the root system ran")
                    .0
                    .iter()
                    .map(|(id, tick, rm)| {
                        let rm = rm.expect("every point carries $root");
                        let delta = [rm.translation[0], rm.translation[2], rm.yaw, rm.weight];
                        (*id, *tick, delta.map(f32::to_bits))
                    })
                    .collect();
                let _ = std::fs::remove_dir_all(root);
                (trace, bits)
            })
        })
        .collect();
    assert_eq!(runs[0].1.len(), 40 * 30);
    assert_eq!(runs[0], runs[1], "1 vs 8 threads");
}
