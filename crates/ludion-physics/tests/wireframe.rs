//! The wireframe read behind the `colliders` debug category: which edges a mesh keeps, how bodies
//! are tagged, and that reading is repeatable.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    ColliderMeshes, PhysicsSpace, PhysicsWorld, TriangleMesh, WireBody, feature_edges, install,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape};

type Segment = (WireBody, [f32; 3], [f32; 3]);

fn spawn(world: &mut World, shape: Shape, body: Body, at: [f32; 3]) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            Collider {
                shape,
                body,
                layer: None,
                sensor: false,
            },
        )
        .expect("a fresh entity accepts a collider");
    entity
}

fn cube_box() -> Shape {
    Shape::Box {
        half_extents: [0.5; 3],
    }
}

fn schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    schedule
}

fn segments(world: &World) -> Vec<Segment> {
    let physics = world
        .resource::<PhysicsWorld>()
        .expect("the write system installs the world once a body claims it");
    let mut out = Vec::new();
    physics.collider_wireframe(&mut |body, a, b| out.push((body, a, b)));
    out
}

fn midpoint(segment: &Segment) -> [f32; 3] {
    std::array::from_fn(|k| (segment.1[k] + segment.2[k]) / 2.0)
}

#[test]
fn a_flat_quad_keeps_its_outline_and_drops_the_diagonal() {
    let quad = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
    ];
    assert_eq!(feature_edges(&quad, &[[0, 2, 1], [0, 3, 2]]).len(), 4);
}

#[test]
fn a_closed_cube_keeps_its_twelve_edges() {
    let v = |x: f32, y: f32, z: f32| [x, y, z];
    let vertices = [
        v(0.0, 0.0, 0.0),
        v(1.0, 0.0, 0.0),
        v(1.0, 1.0, 0.0),
        v(0.0, 1.0, 0.0),
        v(0.0, 0.0, 1.0),
        v(1.0, 0.0, 1.0),
        v(1.0, 1.0, 1.0),
        v(0.0, 1.0, 1.0),
    ];
    let triangles = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 6, 2],
        [3, 7, 6],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    assert_eq!(feature_edges(&vertices, &triangles).len(), 12);
}

#[test]
fn a_folded_pair_keeps_the_crease() {
    let vertices = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    // Two triangles sharing edge 0-2 at a right angle: 3 + 3 - 1 shared = 5 distinct edges.
    assert_eq!(feature_edges(&vertices, &[[0, 2, 1], [0, 3, 2]]).len(), 5);
}

#[test]
fn a_box_is_twelve_segments_at_its_pose_tagged_by_body_kind() {
    let mut world = World::new();
    let fixed = spawn(&mut world, cube_box(), Body::new_static(), [10.0, 0.0, 0.0]);
    spawn(
        &mut world,
        cube_box(),
        Body::new_kinematic(),
        [20.0, 0.0, 0.0],
    );
    spawn(
        &mut world,
        cube_box(),
        Body::Rigid(RigidBody::default()),
        [30.0, 0.0, 0.0],
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);

    assert_eq!(pose_of(&world, fixed).translation, [10.0, 0.0, 0.0]);
    let drawn = segments(&world);
    assert_eq!(drawn.len(), 36, "{drawn:?}");
    for (chunk, (tag, x)) in drawn.chunks(12).zip([
        (WireBody::Static, 10.0),
        (WireBody::Kinematic, 20.0),
        (WireBody::Rigid { sleeping: false }, 30.0),
    ]) {
        assert!(chunk.iter().all(|s| s.0 == tag), "{tag:?}");
        for segment in chunk {
            assert!(
                (midpoint(segment)[0] - x).abs() <= 0.5 + 1e-4,
                "{segment:?} is not near x = {x}"
            );
        }
    }
}

#[test]
fn a_resting_rigid_body_reads_as_sleeping() {
    let mut world = World::new();
    spawn(
        &mut world,
        Shape::Box {
            half_extents: [20.0, 0.5, 20.0],
        },
        Body::new_static(),
        [0.0, -0.5, 0.0],
    );
    let ball = spawn(
        &mut world,
        Shape::Sphere { radius: 0.5 },
        Body::Rigid(RigidBody {
            sleep_after_ticks: Some(5),
            ..RigidBody::default()
        }),
        [0.0, 0.5, 0.0],
    );
    let mut schedule = schedule();
    let mut frame = 0;
    loop {
        tick(&mut world, &mut schedule, frame);
        frame += 1;
        let physics = world.resource::<PhysicsWorld>().expect("a world");
        if physics.is_sleeping(ball) == Some(true) {
            break;
        }
        assert!(frame < 300, "the ball never settled asleep");
    }
    assert!(
        segments(&world)
            .iter()
            .any(|s| s.0 == WireBody::Rigid { sleeping: true })
    );
}

/// A flat 2×2-quad grid over `x, z ∈ [-1, 1]` at height 0.
fn flat_grid() -> TriangleMesh {
    let mut vertices = Vec::new();
    for z in [-1.0, 0.0, 1.0] {
        for x in [-1.0, 0.0, 1.0] {
            vertices.push([x, 0.0, z]);
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
        hash: 7,
    }
}

#[test]
fn a_mesh_floor_draws_its_outline_without_interior_diagonals() {
    let mut world = World::new();
    world.insert_resource(ColliderMeshes::new(Box::new(|_| Ok(flat_grid()))));
    spawn(
        &mut world,
        Shape::Mesh {
            path: "floor.mesh.yaml".into(),
        },
        Body::new_static(),
        [0.0; 3],
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);

    let drawn = segments(&world);
    // The boundary of a 2×2 grid is 8 edges; every interior edge (grid line or diagonal) is flat.
    assert_eq!(drawn.len(), 8, "{drawn:?}");
    assert_eq!(drawn, segments(&world), "a second read must be identical");
}
