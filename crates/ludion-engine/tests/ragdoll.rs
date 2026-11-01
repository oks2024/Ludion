//! `switch_to_ragdoll` end to end (`Plan/physics.md` W3's ragdoll-authoring plan): a 4-part figure
//! (a hinge, a ball, and an unlisted → fixed limb, all off one torso) switches into one rigid body
//! per part, matches what `extract` draws at the instant it ran, scales as documented, refuses
//! cleanly, and — jointed and dropped into a `physics: {space: free}` scene — falls as one rigid
//! figure, deterministically.

use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_core::time::{FrameIndex, SimTime, TickRate};
use ludion_ecs::{Component, World};
use ludion_engine::{
    Animation, AssemblyData, Engine, Mesh, MeshSource, Name, QualityParams, Transform, extract_at,
    load_assembly, switch_to_ragdoll,
};
use ludion_spatial::{Body, Collider, Joint, JointKind, Shape, Velocity};

// --- Fixture -------------------------------------------------------------------------------------

const ASSEMBLY_REL: &str = "models/figure.assembly.yaml";

/// A torso (the root) with three children: `arm-hinge` and `arm-ball` bound in `joints:`, and
/// `trinket` deliberately absent from it — the plan's "a part with no `joints:` entry is `fixed`"
/// case.
const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: figure
size:
  - 5
  - 6
  - 2
palette:
  A:
    color:
      - 0.8
      - 0.2
      - 0.2
      - 1.0
parts:
  - name: torso
    file: parts/torso.part.yaml
    place:
      - 2
      - 2
      - 0
  - name: arm-hinge
    file: parts/limb.part.yaml
    parent: torso
    place:
      - 4
      - 2
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
  - name: arm-ball
    file: parts/limb.part.yaml
    parent: torso
    place:
      - 0
      - 2
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
  - name: trinket
    file: parts/limb.part.yaml
    parent: torso
    place:
      - 2
      - 5
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
joints:
  arm-hinge:
    kind: hinge
    axis:
      - 0.0
      - 0.0
      - 1.0
    limits:
      - -45.0
      - 45.0
  arm-ball:
    kind: ball
    limits:
      -
        - -30.0
        - 30.0
      -
        - -30.0
        - 30.0
      -
        - -30.0
        - 30.0
clips:
  wave: anims/wave.anim.yaml
";

const TORSO: &str = "\
format: 1
kind: part
size:
  - 2
  - 2
  - 2
slices:
  -
    - \"AA\"
    - \"AA\"
  -
    - \"AA\"
    - \"AA\"
";

const LIMB: &str = "\
format: 1
kind: part
size:
  - 1
  - 1
  - 1
slices:
  -
    - \"A\"
";

/// Rotates `arm-hinge` from rest to 30° about +Z over one looping second — the parity test's
/// mid-clip instant.
const WAVE_CLIP: &str = "\
format: 1
kind: animation
name: wave
duration: 1.0
loop: true
tracks:
  arm-hinge:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 0.0
          - 0.0
          - 30.0
";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-ragdoll-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Writes the assembly, its two part files, and its clip into a fresh project directory.
fn fixture_project(name: &str) -> PathBuf {
    let root = fresh_dir(name);
    write(&root, ASSEMBLY_REL, ASSEMBLY);
    write(&root, "models/parts/torso.part.yaml", TORSO);
    write(&root, "models/parts/limb.part.yaml", LIMB);
    write(&root, "models/anims/wave.anim.yaml", WAVE_CLIP);
    root
}

fn assembly_data(root: &Path) -> AssemblyData {
    load_assembly(ludion_assets::AssetRoot::content(root), ASSEMBLY_REL)
        .expect("the fixture assembly loads")
}

fn get<T: Component + Clone>(world: &World, id: EntityId) -> Option<T> {
    world.get::<T>(id).ok().flatten().cloned()
}

fn bare_mesh(shape: MeshSource) -> Mesh {
    Mesh {
        shape,
        ..Mesh::default()
    }
}

// --- (a) At rest: spawn order, stripped root, joints -----------------------------------------------

/// At rest: the switch spawns one entity per part in authored order, strips the root's drawing/
/// physics components while keeping it (and its other components) alive, and each part carries the
/// components decisions 1-4 specify — joints included, the unlisted `trinket` implicitly fixed.
#[test]
fn switch_spawns_parts_in_authored_order_and_strips_the_root() {
    let root = fixture_project("rest");
    let data = assembly_data(&root);

    let mut world = World::new();
    let figure = world.spawn();
    world.insert(figure, Transform::default()).unwrap();
    world
        .insert(
            figure,
            bare_mesh(MeshSource::Assembly(ASSEMBLY_REL.to_string())),
        )
        .unwrap();
    world.insert(figure, Name("hero".to_string())).unwrap();

    let spawned = switch_to_ragdoll(
        &mut world,
        figure,
        &data,
        SimTime::from_ticks(0, TickRate::DEFAULT),
    )
    .expect("a valid switch");

    assert_eq!(spawned.len(), 4);
    let names: Vec<String> = spawned
        .iter()
        .map(|&id| get::<Name>(&world, id).unwrap().0)
        .collect();
    assert_eq!(
        names,
        [
            "hero/torso".to_string(),
            "hero/arm-hinge".to_string(),
            "hero/arm-ball".to_string(),
            "hero/trinket".to_string(),
        ],
        "authored part order, named off the root"
    );

    // The root survives, stripped of its drawing/physics components; its Transform and Name stay.
    assert!(world.is_alive(figure));
    assert!(get::<Mesh>(&world, figure).is_none());
    assert!(get::<Animation>(&world, figure).is_none());
    assert!(get::<Collider>(&world, figure).is_none());
    assert!(get::<Velocity>(&world, figure).is_none());
    assert!(get::<Transform>(&world, figure).is_some());
    assert_eq!(get::<Name>(&world, figure).unwrap().0, "hero");

    // Every part carries Transform, an `assembly_part` Mesh, a rigid convex Collider and Velocity.
    for (index, &id) in spawned.iter().enumerate() {
        assert!(world.is_alive(id));
        assert!(get::<Transform>(&world, id).is_some());
        let mesh = get::<Mesh>(&world, id).expect("every part carries a Mesh");
        assert_eq!(
            mesh.shape,
            MeshSource::AssemblyPart {
                assembly: ASSEMBLY_REL.to_string(),
                part: data.parts[index].name.clone(),
            }
        );
        let collider = get::<Collider>(&world, id).expect("every part carries a Collider");
        assert!(matches!(collider.shape, Shape::Convex { .. }));
        assert!(matches!(collider.body, Body::Rigid(_)));
        assert!(get::<Velocity>(&world, id).is_some());
    }

    // `torso` is the root part: no `parent`, so no `Joint`. The three limbs each carry one, `a`
    // always the torso, `collide: false`, and the anchor the plan's formula computes.
    assert!(get::<Joint>(&world, spawned[0]).is_none());
    for (index, name, expect_free) in [
        (1usize, "arm-hinge", true),
        (2, "arm-ball", true),
        (3, "trinket", false),
    ] {
        let joint =
            get::<Joint>(&world, spawned[index]).unwrap_or_else(|| panic!("{name} needs a Joint"));
        assert_eq!(joint.a, spawned[0]);
        assert_eq!(joint.b, spawned[index]);
        assert!(!joint.collide);
        let expected_anchor = [
            data.parts[index].offset[0] + data.parts[index].pivot[0] - data.parts[0].offset[0],
            data.parts[index].offset[1] + data.parts[index].pivot[1] - data.parts[0].offset[1],
            data.parts[index].offset[2] + data.parts[index].pivot[2] - data.parts[0].offset[2],
        ];
        for (axis, (&actual, &expected)) in joint.anchor.iter().zip(&expected_anchor).enumerate() {
            assert!(
                (actual - expected).abs() < 1e-5,
                "{name} anchor[{axis}]: {actual} vs expected {expected}"
            );
        }
        assert_eq!(
            !matches!(joint.kind, JointKind::Fixed { .. }),
            expect_free,
            "{name}'s joint kind"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

// --- (b) Parity with extract, at rest and mid-clip --------------------------------------------------

/// Every mesh instance `extract` draws, in ascending-entity order.
fn model_matrices(world: &World, root: &Path, now: SimTime) -> Vec<Mat4> {
    let scene = extract_at(
        world,
        (64, 64),
        ludion_assets::AssetRoot::content(root),
        now,
        QualityParams::FULL.particle_density,
    )
    .expect("extraction succeeds");
    scene.meshes.iter().map(|m| m.model).collect()
}

fn assert_mat4_close(a: Mat4, b: Mat4, tol: f32, label: &str) {
    let (a, b) = (a.to_cols_array(), b.to_cols_array());
    for (index, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        assert!(
            (x - y).abs() <= tol,
            "{label}: element {index} differs: {x} vs {y}"
        );
    }
}

/// Each part entity's post-switch model matrix matches the pre-switch whole-assembly instance's —
/// the switch reads the same pose `extract` would have drawn, at rest and mid-clip. The two never
/// need to be bit-identical (the root's Euler→quat goes through `ludion-physics`'s libm, `extract`'s
/// through glam's), hence the `1e-4` tolerance.
#[test]
fn switch_matches_extracts_pose_at_rest_and_mid_clip() {
    for (label, animated, tick) in [("rest", false, 0), ("mid-clip", true, 30)] {
        let root = fixture_project(&format!("parity-{label}"));
        let data = assembly_data(&root);
        let now = SimTime::from_ticks(tick, TickRate::DEFAULT);

        let mut world = World::new();
        let figure = world.spawn();
        world
            .insert(
                figure,
                Transform {
                    translation: [1.0, 2.0, -3.0],
                    rotation: [0.0, 25.0, 0.0],
                    scale: [1.0; 3],
                },
            )
            .unwrap();
        world
            .insert(
                figure,
                bare_mesh(MeshSource::Assembly(ASSEMBLY_REL.to_string())),
            )
            .unwrap();
        if animated {
            world
                .insert(
                    figure,
                    Animation {
                        clip: "wave".to_string(),
                        started: FrameIndex::ZERO,
                        speed: 1.0,
                        ..Animation::default()
                    },
                )
                .unwrap();
        }

        let pre = model_matrices(&world, &root, now);
        assert_eq!(pre.len(), 4, "one instance per part, pre-switch");

        switch_to_ragdoll(&mut world, figure, &data, now).expect("a valid switch");
        let post = model_matrices(&world, &root, now);
        assert_eq!(post.len(), 4, "one entity per part, post-switch");

        for (index, (&before, &after)) in pre.iter().zip(&post).enumerate() {
            assert_mat4_close(before, after, 1e-4, &format!("{label} part {index}"));
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Switching mid-crossfade places the parts at the blended pose extract draws (here a fade from the
/// rest pose into `wave`, halfway), not at the plain `wave` pose.
#[test]
fn switch_places_parts_at_the_blended_pose_mid_crossfade() {
    let root = fixture_project("parity-crossfade");
    let data = assembly_data(&root);
    let now = SimTime::from_ticks(30, TickRate::DEFAULT);

    let figure_with = |animation: Animation| {
        let mut world = World::new();
        let figure = world.spawn();
        world.insert(figure, Transform::default()).unwrap();
        world
            .insert(
                figure,
                bare_mesh(MeshSource::Assembly(ASSEMBLY_REL.to_string())),
            )
            .unwrap();
        world.insert(figure, animation).unwrap();
        (world, figure)
    };
    let (mut fading, figure) = figure_with(Animation {
        clip: "wave".to_string(),
        fade: 1.0,
        ..Animation::default()
    });
    let (plain, _) = figure_with(Animation {
        clip: "wave".to_string(),
        ..Animation::default()
    });

    let pre = model_matrices(&fading, &root, now);
    let hinge = 1;
    let plain_pose = model_matrices(&plain, &root, now);
    assert!(
        pre[hinge]
            .to_cols_array()
            .iter()
            .zip(plain_pose[hinge].to_cols_array())
            .any(|(x, y)| (x - y).abs() > 1e-3),
        "mid-fade the hinge is not yet at the plain wave pose"
    );

    switch_to_ragdoll(&mut fading, figure, &data, now).expect("a valid switch");
    let post = model_matrices(&fading, &root, now);
    assert_eq!(post.len(), pre.len());
    for (index, (&before, &after)) in pre.iter().zip(&post).enumerate() {
        assert_mat4_close(before, after, 1e-4, &format!("crossfade part {index}"));
    }

    let _ = std::fs::remove_dir_all(&root);
}

// --- (c) Scale 2 doubles points and anchor; non-uniform scale refuses -------------------------------

/// A fresh world switched at `scale`: the arm-hinge collider's convex points and its `Joint`'s
/// anchor.
fn switch_at_scale(data: &AssemblyData, scale: f32) -> (Vec<[f32; 3]>, [f32; 3]) {
    let mut world = World::new();
    let figure = world.spawn();
    world
        .insert(
            figure,
            Transform {
                translation: [0.0; 3],
                rotation: [0.0; 3],
                scale: [scale; 3],
            },
        )
        .unwrap();
    world
        .insert(
            figure,
            bare_mesh(MeshSource::Assembly(ASSEMBLY_REL.to_string())),
        )
        .unwrap();
    let spawned = switch_to_ragdoll(
        &mut world,
        figure,
        data,
        SimTime::from_ticks(0, TickRate::DEFAULT),
    )
    .expect("a valid switch");
    let collider = get::<Collider>(&world, spawned[1]).expect("arm-hinge has a Collider");
    let points = match collider.shape {
        Shape::Convex { points } => points,
        other => panic!("expected a convex hull, found {other:?}"),
    };
    let joint = get::<Joint>(&world, spawned[1]).expect("arm-hinge has a Joint");
    (points, joint.anchor)
}

#[test]
fn scale_two_doubles_collider_points_and_joint_anchor() {
    let root = fixture_project("scale");
    let data = assembly_data(&root);

    let (points1, anchor1) = switch_at_scale(&data, 1.0);
    let (points2, anchor2) = switch_at_scale(&data, 2.0);

    assert_eq!(points1.len(), points2.len());
    for (p1, p2) in points1.iter().zip(&points2) {
        for (&a, &b) in p1.iter().zip(p2) {
            assert!(
                (b - a * 2.0).abs() < 1e-5,
                "collider point does not double: {p1:?} -> {p2:?}"
            );
        }
    }
    for (&a, &b) in anchor1.iter().zip(&anchor2) {
        assert!(
            (b - a * 2.0).abs() < 1e-5,
            "joint anchor does not double: {anchor1:?} -> {anchor2:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn non_uniform_scale_refuses_and_leaves_the_world_unchanged() {
    let root = fixture_project("nonuniform");
    let data = assembly_data(&root);

    let mut world = World::new();
    let figure = world.spawn();
    world
        .insert(
            figure,
            Transform {
                translation: [0.0; 3],
                rotation: [0.0; 3],
                scale: [1.0, 2.0, 1.0],
            },
        )
        .unwrap();
    world
        .insert(
            figure,
            bare_mesh(MeshSource::Assembly(ASSEMBLY_REL.to_string())),
        )
        .unwrap();

    let before = world.entity_count();
    let error = switch_to_ragdoll(
        &mut world,
        figure,
        &data,
        SimTime::from_ticks(0, TickRate::DEFAULT),
    )
    .expect_err("non-uniform scale is refused");
    assert_eq!(error.code.as_str(), "L3036");
    assert_eq!(world.entity_count(), before, "nothing is spawned");
    assert!(
        get::<Mesh>(&world, figure).is_some(),
        "the root keeps its Mesh — the switch mutated nothing"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A uniform but non-positive scale must be
/// refused rather than building a negative-mass body (`scale = -1`) or a degenerate one
/// (`scale = 0`), both of which the old uniformity-only check let through.
#[test]
fn a_zero_or_negative_uniform_scale_refuses_and_leaves_the_world_unchanged() {
    let root = fixture_project("nonpositive");
    let data = assembly_data(&root);

    for scale in [[0.0, 0.0, 0.0], [-1.0, -1.0, -1.0]] {
        let mut world = World::new();
        let figure = world.spawn();
        world
            .insert(
                figure,
                Transform {
                    translation: [0.0; 3],
                    rotation: [0.0; 3],
                    scale,
                },
            )
            .unwrap();
        world
            .insert(
                figure,
                bare_mesh(MeshSource::Assembly(ASSEMBLY_REL.to_string())),
            )
            .unwrap();

        let before = world.entity_count();
        let error = switch_to_ragdoll(
            &mut world,
            figure,
            &data,
            SimTime::from_ticks(0, TickRate::DEFAULT),
        )
        .expect_err(&format!("scale {scale:?} is refused"));
        assert_eq!(error.code.as_str(), "L3036");
        assert_eq!(world.entity_count(), before, "nothing is spawned");
        assert!(
            get::<Mesh>(&world, figure).is_some(),
            "the root keeps its Mesh — the switch mutated nothing"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// A scale whose cubed volume overflows `f32` gives a part an infinite mass; the switch refuses
/// before spawning anything rather than handing the solver a body it cannot integrate.
#[test]
fn a_scale_that_overflows_a_part_mass_refuses_and_leaves_the_world_unchanged() {
    let root = fixture_project("mass-overflow");
    let data = assembly_data(&root);
    let mut world = World::new();
    let figure = world.spawn();
    world
        .insert(
            figure,
            Transform {
                translation: [0.0; 3],
                rotation: [0.0; 3],
                scale: [1.0e13; 3],
            },
        )
        .unwrap();
    world
        .insert(
            figure,
            bare_mesh(MeshSource::Assembly(ASSEMBLY_REL.to_string())),
        )
        .unwrap();

    let before = world.entity_count();
    let error = switch_to_ragdoll(
        &mut world,
        figure,
        &data,
        SimTime::from_ticks(0, TickRate::DEFAULT),
    )
    .expect_err("a part mass past f32 range is refused");
    assert_eq!(error.code.as_str(), "L3036");
    assert!(
        !error.message.contains("  "),
        "no run of spaces: {}",
        error.message
    );
    assert_eq!(world.entity_count(), before, "nothing is spawned");
    assert!(get::<Mesh>(&world, figure).is_some());

    let _ = std::fs::remove_dir_all(&root);
}

// --- (d) A non-assembly entity is refused ------------------------------------------------------------

#[test]
fn a_non_assembly_entity_is_refused() {
    let root = fixture_project("non-assembly");
    let data = assembly_data(&root);

    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Transform::default()).unwrap();
    world.insert(entity, bare_mesh(MeshSource::Cube)).unwrap();

    let error = switch_to_ragdoll(
        &mut world,
        entity,
        &data,
        SimTime::from_ticks(0, TickRate::DEFAULT),
    )
    .expect_err("a non-assembly Mesh is refused");
    assert_eq!(error.code.as_str(), "L3036");

    let _ = std::fs::remove_dir_all(&root);
}

// --- (e) Falls jointed, deterministically, in a `physics: {space: free}` scene ---------------------

const FALL_TICKS: usize = 200;
const FIGURE_START_Y: f32 = 5.0;
/// Far enough below that nothing collides in [`FALL_TICKS`] — a "static floor" the scene's `physics:`
/// block needs, kept out of the way so free fall (uniform gravity on every part) stays the whole
/// story: no impact impulses to make the per-tick anchor-distance check flaky.
const FLOOR_Y: f32 = -1000.0;

fn physics_scene() -> String {
    format!(
        "format: 1\nseed: 7\nphysics:\n  space: free\nentities:\n  - name: floor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - {FLOOR_Y}\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 50.0\n              - 0.5\n              - 50.0\n        body: static\n  - name: figure\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - {FIGURE_START_Y}\n          - 0.0\n      Mesh:\n        shape:\n          assembly: {ASSEMBLY_REL}\n"
    )
}

/// The world-space distance between the `index`-th part's `Joint` anchor (evaluated in its parent's
/// current pose) and the child's own translation — constant for a rigid hinge/ball/fixed joint
/// regardless of how far the whole figure has rotated, so a stretch or separation shows up here.
fn anchor_child_distance(world: &World, spawned: &[EntityId], index: usize) -> f32 {
    let joint = get::<Joint>(world, spawned[index]).expect("every non-root part carries a Joint");
    let parent = get::<Transform>(world, joint.a).expect("the parent is alive");
    let child = get::<Transform>(world, joint.b).expect("the child is alive");
    let parent_rotation = Quat::from_array(euler_xyz_degrees_to_quat(parent.rotation));
    let anchor_world = Vec3::from(parent.translation) + parent_rotation * Vec3::from(joint.anchor);
    (anchor_world - Vec3::from(child.translation)).length()
}

/// Runs the switch-then-fall scenario once, returning the per-tick `WorldHash` trace and the torso's
/// final translation.
fn run_fall(name: &str) -> (Vec<u64>, [f32; 3]) {
    let root = fixture_project(name);
    let data = assembly_data(&root);

    let mut engine = Engine::builder()
        .project_root(root.clone())
        .build()
        .expect("boots");
    engine
        .load_inline_scene(&physics_scene())
        .expect("the scene loads");

    // `floor` is document position 0, `figure` is 1 — a freshly loaded scene assigns entity ids by
    // document order at generation 0 (the `physics_joints.rs` convention).
    let figure = EntityId::new(1, 0);
    let spawned = switch_to_ragdoll(
        engine.world_mut(),
        figure,
        &data,
        SimTime::from_ticks(0, TickRate::DEFAULT),
    )
    .expect("a valid switch");

    let initial_distances: Vec<f32> = (1..spawned.len())
        .map(|index| anchor_child_distance(engine.world(), &spawned, index))
        .collect();

    let mut hashes = Vec::with_capacity(FALL_TICKS);
    for tick in 0..FALL_TICKS {
        engine.advance_live();
        hashes.push(engine.world_hash().unwrap().as_u64());
        for (offset, distance0) in initial_distances.iter().enumerate() {
            let index = offset + 1;
            let distance = anchor_child_distance(engine.world(), &spawned, index);
            assert!(
                (distance - distance0).abs() <= 0.05,
                "{name}: part {index} drifted from its anchor at tick {tick}: {distance} vs the \
                 initial {distance0}"
            );
        }
    }

    let torso = get::<Transform>(engine.world(), spawned[0]).expect("the torso is alive");
    let _ = std::fs::remove_dir_all(&root);
    (hashes, torso.translation)
}

#[test]
fn the_figure_falls_jointed_and_deterministically() {
    let (hashes_a, torso_a) = run_fall("fall-a");
    let (hashes_b, torso_b) = run_fall("fall-b");

    assert_eq!(
        hashes_a, hashes_b,
        "two runs of the same switch-then-fall must be hash-identical"
    );
    assert_eq!(torso_a, torso_b);
    assert!(
        torso_a[1] < FIGURE_START_Y - 1.0,
        "the figure must fall under gravity: torso.y = {} from a start of {FIGURE_START_Y}",
        torso_a[1]
    );
}
