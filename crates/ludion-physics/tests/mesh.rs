//! `shape: { mesh: <path> }`: the trimesh build, the seam fix, the rigid and unavailable-document
//! rejections, and the hash-driven rebuild (an edited document, a restore).

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    ColliderMeshes, PhysicsSpace, PhysicsWorld, QueryFilter, ShapeKind, TriangleMesh, install,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape};

/// A flat 2×2-quad grid over `x, z ∈ [-1, 1]` at height `y`: nine vertices, eight triangles wound
/// so every face normal points up. The hash is the height's bits, so a raised grid is a new mesh.
fn grid(y: f32) -> TriangleMesh {
    let mut vertices = Vec::new();
    for z in [-1.0, 0.0, 1.0] {
        for x in [-1.0, 0.0, 1.0] {
            vertices.push([x, y, z]);
        }
    }
    let mut triangles = Vec::new();
    for row in 0..2u32 {
        for col in 0..2u32 {
            let a = row * 3 + col;
            let (b, c, d) = (a + 1, a + 3, a + 4);
            triangles.push([a, c, b]);
            triangles.push([b, c, d]);
        }
    }
    TriangleMesh {
        vertices,
        triangles,
        hash: u64::from(y.to_bits()),
    }
}

/// What the test loader says about a path it does not know: a coded diagnostic of its own, so a
/// rejection's `related` can be checked against it.
fn unreadable(path: &str) -> Diagnostic {
    Diagnostic::new(code::ASSET_FILE_IO, format!("no mesh at `{path}`"))
}

fn library(y: f32) -> ColliderMeshes {
    ColliderMeshes::new(Box::new(move |path| match path {
        "floor.mesh.yaml" => Ok(grid(y)),
        other => Err(unreadable(other)),
    }))
}

fn mesh_collider(path: &str, body: Body) -> Collider {
    Collider {
        shape: Shape::Mesh { path: path.into() },
        body,
        layer: None,
        sensor: false,
    }
}

fn spawn(world: &mut World, collider: Collider, at: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    entity
}

fn schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    schedule
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("the write system installs the world once a body claims it")
}

fn ray_down_y(world: &World) -> f32 {
    let hits = physics(world)
        .ray_cast(
            [0.25, 5.0, 0.25],
            [0.0, -1.0, 0.0],
            10.0,
            &QueryFilter::new(),
        )
        .expect("a finite ray");
    assert_eq!(hits.len(), 1, "{hits:?}");
    hits[0].point[1]
}

/// A floor mesh entity, then a box, so a rebuild (which re-appends to the table) is observable.
fn floor_and_box(world: &mut World) -> (EntityId, EntityId) {
    let floor = spawn(
        world,
        mesh_collider("floor.mesh.yaml", Body::new_static()),
        [0.0; 3],
    );
    let marker = spawn(
        world,
        Collider {
            shape: Shape::Box {
                half_extents: [0.5; 3],
            },
            body: Body::new_static(),
            layer: None,
            sensor: false,
        },
        [50.0, 0.0, 0.0],
    );
    (floor, marker)
}

#[test]
fn a_static_mesh_is_a_trimesh_a_ray_and_a_capsule_cast_hit() {
    let mut world = World::new();
    world.insert_resource(library(0.0));
    let (floor, _) = floor_and_box(&mut world);
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);

    assert_eq!(physics(&world).shape_kind(floor), Some(ShapeKind::TriMesh));
    assert_eq!(
        pose_of(&world, floor).translation,
        [0.0; 3],
        "a static body must not move"
    );
    assert!((ray_down_y(&world)).abs() < 1e-5);

    let capsule = Shape::Capsule {
        half_height: 0.25,
        radius: 0.25,
    };
    let hits = physics(&world)
        .shape_cast(
            &capsule,
            [0.3, 3.0, 0.4],
            [0.0, 0.0, 0.0, 1.0],
            [0.0, -1.0, 0.0],
            10.0,
            &QueryFilter::new(),
        )
        .expect("a finite cast");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].normal[1] > 0.99, "{hits:?}");
    assert!((hits[0].distance - 2.5).abs() < 1e-3, "{hits:?}");
}

#[test]
fn a_capsule_sliding_over_the_seams_meets_no_sideways_normal() {
    let mut world = World::new();
    world.insert_resource(library(0.0));
    floor_and_box(&mut world);
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);

    // Descends 0.01 per unit along x, landing on the grid's centre vertex, where two quad seams
    // and both diagonals meet: an unfixed trimesh reports the neighbouring face's edge normal.
    let capsule = Shape::Capsule {
        half_height: 0.25,
        radius: 0.25,
    };
    let hits = physics(&world)
        .shape_cast(
            &capsule,
            [-1.0, 0.51, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [1.0, -0.01, 0.0],
            1.5,
            &QueryFilter::new(),
        )
        .expect("a finite cast");
    assert!(!hits.is_empty(), "the capsule must land on the grid");
    for hit in &hits {
        assert!(hit.normal[1].abs() > 0.99, "sideways seam normal: {hits:?}");
    }
}

#[test]
fn a_rigid_mesh_is_l1211_a_bad_document_l1212_and_a_kinematic_mesh_builds() {
    let mut world = World::new();
    world.insert_resource(library(0.0));
    let rigid = spawn(
        &mut world,
        mesh_collider("floor.mesh.yaml", Body::Rigid(RigidBody::default())),
        [0.0; 3],
    );
    let missing = spawn(
        &mut world,
        mesh_collider("gone.mesh.yaml", Body::new_static()),
        [0.0; 3],
    );
    let kinematic = spawn(
        &mut world,
        mesh_collider("floor.mesh.yaml", Body::new_kinematic()),
        [0.0; 3],
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);

    let physics = world
        .resource_mut::<PhysicsWorld>()
        .expect("the kinematic mesh claimed the world");
    assert!(physics.shape_kind(rigid).is_none());
    assert!(physics.shape_kind(missing).is_none());
    assert_eq!(physics.shape_kind(kinematic), Some(ShapeKind::TriMesh));
    let rejections = physics.take_rejections();
    let codes: Vec<_> = rejections.iter().map(|r| (r.entity, r.code)).collect();
    assert_eq!(
        codes,
        vec![
            (rigid, code::COLLIDER_MESH_ON_RIGID),
            (missing, code::COLLIDER_MESH_UNAVAILABLE),
        ],
        "{rejections:?}"
    );
    assert!(
        rejections[1].reason.contains("gone.mesh.yaml"),
        "{rejections:?}"
    );
    assert_eq!(
        rejections[1].related,
        vec![unreadable("gone.mesh.yaml")],
        "the loader's diagnostic rides along as related"
    );
    assert!(rejections[0].related.is_empty(), "{rejections:?}");
}

#[test]
fn no_installed_library_is_l1212() {
    let mut world = World::new();
    let floor = spawn(
        &mut world,
        mesh_collider("floor.mesh.yaml", Body::new_static()),
        [0.0; 3],
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    let physics = world
        .resource_mut::<PhysicsWorld>()
        .expect("the write system installs the world");
    assert!(physics.shape_kind(floor).is_none());
    let rejections = physics.take_rejections();
    assert_eq!(rejections.len(), 1, "{rejections:?}");
    assert_eq!(rejections[0].code, code::COLLIDER_MESH_UNAVAILABLE);
    assert!(rejections[0].related.is_empty(), "{rejections:?}");
}

#[test]
fn a_changed_mesh_hash_rebuilds_the_body_at_the_next_tick() {
    let mut world = World::new();
    world.insert_resource(library(0.0));
    let (floor, marker) = floor_and_box(&mut world);
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    assert!(ray_down_y(&world).abs() < 1e-5);
    let order: Vec<_> = physics(&world).entities().collect();
    assert_eq!(order, vec![floor, marker]);

    // The same library again changes nothing.
    world.insert_resource(library(0.0));
    tick(&mut world, &mut schedule, 1);
    assert_eq!(physics(&world).entities().collect::<Vec<_>>(), order);

    // A new library that reads an edited document: the body is rebuilt (re-appended to the table).
    world.insert_resource(library(1.0));
    tick(&mut world, &mut schedule, 2);
    tick(&mut world, &mut schedule, 3);
    assert!((ray_down_y(&world) - 1.0).abs() < 1e-5);
    assert_eq!(
        physics(&world).entities().collect::<Vec<_>>(),
        vec![marker, floor]
    );
}

#[test]
fn a_material_and_a_mesh_edit_in_one_tick_rebuild_at_once() {
    let mut world = World::new();
    world.insert_resource(library(0.0));
    let (floor, marker) = floor_and_box(&mut world);
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    assert!(ray_down_y(&world).abs() < 1e-5);

    let mut edited = mesh_collider("floor.mesh.yaml", Body::new_static());
    edited.body = Body::Static(ludion_spatial::ContactMaterial {
        friction: 0.1,
        ..Default::default()
    });
    world.insert(floor, edited).expect("the floor is alive");
    world.insert_resource(library(1.0));
    tick(&mut world, &mut schedule, 1);
    assert!((ray_down_y(&world) - 1.0).abs() < 1e-5);
    assert_eq!(
        physics(&world).entities().collect::<Vec<_>>(),
        vec![marker, floor]
    );
}

#[test]
fn a_snapshot_round_trip_with_the_same_library_does_not_rebuild() {
    let mut world = World::new();
    world.insert_resource(library(0.0));
    let (floor, marker) = floor_and_box(&mut world);
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    let bytes = physics(&world).to_bytes().expect("the solver encodes");

    world.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("the bytes decode"),
    );
    world.insert_resource(library(0.0));
    tick(&mut world, &mut schedule, 1);
    assert_eq!(physics(&world).body_count(), 2);
    assert_eq!(
        physics(&world).entities().collect::<Vec<_>>(),
        vec![floor, marker],
        "an unchanged mesh keeps its body"
    );
    assert!(ray_down_y(&world).abs() < 1e-5);

    // The same bytes restored beside an edited document rebuild the mesh body.
    world.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("the bytes decode"),
    );
    world.insert_resource(library(1.0));
    tick(&mut world, &mut schedule, 2);
    tick(&mut world, &mut schedule, 3);
    assert!((ray_down_y(&world) - 1.0).abs() < 1e-5);
}

#[test]
fn a_mesh_is_not_a_query_shape() {
    let mut world = World::new();
    world.insert_resource(library(0.0));
    floor_and_box(&mut world);
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    let mesh = Shape::Mesh {
        path: "floor.mesh.yaml".into(),
    };
    assert!(
        physics(&world)
            .overlap(&mesh, [0.0; 3], [0.0, 0.0, 0.0, 1.0], &QueryFilter::new())
            .is_err()
    );
}
