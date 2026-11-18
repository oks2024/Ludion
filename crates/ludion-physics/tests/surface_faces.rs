//! Per-triangle mesh tags and per-cell heightfield tags: a hit, a shape cast, a closest-point query
//! and the motor report the face nearest their point (lowest index on ties); an overlay still wins;
//! an untagged mesh reports the entity tag; an undeclared local name is one `L1697` and falls
//! through; a tag-only mesh edit rebuilds; a restore answers before any write.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CellSurfaces, CharacterMotor, ColliderMeshes, NoParent, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, QueryFilter, ShapeRejection, SurfaceId, SurfaceOverlay, SurfaceTable, SurfaceTag,
    TriangleMesh, install, move_character, sync_mesh_surfaces,
};
use ludion_spatial::{Body, Collider, Shape};

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const CONCRETE: SurfaceId = SurfaceId(0);
const GRATE: SurfaceId = SurfaceId(1);
const CARPET: SurfaceId = SurfaceId(2);
const DIRT: SurfaceId = SurfaceId(3);

/// Each step's x position and the ground surface the motor reported there.
type SurfaceLog = Arc<Mutex<Vec<(f32, Option<SurfaceId>)>>>;

fn settings() -> PhysicsSettings {
    PhysicsSettings {
        surfaces: Some(SurfaceTable::new(
            vec![
                "concrete".into(),
                "metal_grate".into(),
                "carpet".into(),
                "dirt".into(),
            ],
            None,
        )),
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn stepped() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, settings());
    schedule
}

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

fn physics(world: &World) -> &PhysicsWorld {
    world.resource::<PhysicsWorld>().unwrap()
}

fn rejections(world: &mut World) -> Vec<ShapeRejection> {
    world
        .resource_mut::<PhysicsWorld>()
        .unwrap()
        .take_rejections()
}

fn down(physics: &PhysicsWorld, x: f32, z: f32) -> ludion_physics::QueryHit {
    physics
        .ray_cast([x, 2.0, z], [0.0, -1.0, 0.0], 5.0, &QueryFilter::new())
        .unwrap()
        .remove(0)
}

/// A flat strip over `x ∈ [0, 8]`, `z ∈ [-2, 2]` at `y = 0`: eight quads, two triangles each, 16
/// in all, tagged by `names` and `runs`. `tagged_strip` puts `metal_grate` on `x < 4` and `concrete`
/// beyond, in a local order (`metal_grate`, `concrete`) that differs from the scene's table.
fn strip(hash: u64, names: &[&str], runs: &[[u32; 2]]) -> TriangleMesh {
    let mut vertices = Vec::new();
    for i in 0..=8u32 {
        vertices.push([i as f32, 0.0, -2.0]);
        vertices.push([i as f32, 0.0, 2.0]);
    }
    let mut triangles = Vec::new();
    for i in 0..8u32 {
        let a = i * 2;
        triangles.push([a, a + 2, a + 1]);
        triangles.push([a + 1, a + 2, a + 3]);
    }
    let mut triangle_surfaces = Vec::new();
    for [count, index] in runs {
        triangle_surfaces.extend(std::iter::repeat_n(*index as u16, *count as usize));
    }
    TriangleMesh {
        vertices,
        triangles,
        hash,
        surfaces: names.iter().map(|name| name.to_string()).collect(),
        triangle_surfaces,
    }
}

fn tagged_strip(hash: u64) -> TriangleMesh {
    strip(hash, &["metal_grate", "concrete"], &[[8, 0], [8, 1]])
}

fn library(mesh: impl Fn() -> TriangleMesh + Send + Sync + 'static) -> ColliderMeshes {
    ColliderMeshes::new(Box::new(move |path| match path {
        "dock.mesh.yaml" => Ok(mesh()),
        other => Err(Diagnostic::new(
            code::ASSET_FILE_IO,
            format!("no mesh at `{other}`"),
        )),
    }))
}

/// A static mesh floor entity carrying `SurfaceTag { surface }`, or no tag when `surface` is empty.
fn mesh_floor(world: &mut World, surface: &str) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Mesh {
                    path: "dock.mesh.yaml".into(),
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    if !surface.is_empty() {
        world
            .insert(
                entity,
                SurfaceTag {
                    surface: surface.to_string(),
                    parts: Vec::new(),
                    cells: None,
                },
            )
            .unwrap();
    }
    entity
}

#[test]
fn a_motor_walking_across_the_mesh_reports_concrete_then_grate() {
    let mut world = World::new();
    world.insert_resource(library(|| {
        strip(1, &["concrete", "metal_grate"], &[[8, 0], [8, 1]])
    }));
    mesh_floor(&mut world, "");
    let hero = world.spawn();
    world.insert(hero, TestPose::at([0.5, 0.0, 0.0])).unwrap();
    world
        .insert(
            hero,
            CharacterMotor {
                radius: 0.28,
                height: 0.9,
                slope_limit_deg: 45.0,
                step_height: 0.25,
                skin_width: 0.01,
                ..CharacterMotor::default()
            },
        )
        .unwrap();
    let log: SurfaceLog = Arc::default();
    let sink = Arc::clone(&log);
    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "walk",
        Arc::new(move |world: &mut World, _: &SystemCtx| {
            if let Ok(report) = move_character::<TestPose, NoParent>(world, hero, [0.1, -0.1, 0.0])
            {
                let x = pose_of(world, hero).translation[0];
                sink.lock()
                    .unwrap()
                    .push((x, report.ground.map(|g| g.surface).unwrap_or(None)));
            }
        }),
    );
    install::<TestPose, NoParent>(&mut schedule, settings());
    run(&mut world, &mut schedule, 0..70);

    let log = log.lock().unwrap();
    let mut seen = Vec::new();
    for &(x, surface) in log.iter() {
        if (x - 4.0).abs() < 0.4 {
            continue;
        }
        let want = if x < 4.0 { CONCRETE } else { GRATE };
        assert_eq!(surface, Some(want), "at x = {x}");
        if seen.last() != Some(&surface) {
            seen.push(surface);
        }
    }
    assert_eq!(seen, [Some(CONCRETE), Some(GRATE)]);
}

#[test]
fn every_query_reports_the_nearest_triangle() {
    let mut world = World::new();
    world.insert_resource(library(|| tagged_strip(1)));
    let floor = mesh_floor(&mut world, "carpet");
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    let physics = physics(&world);
    let filter = QueryFilter::new();

    assert_eq!(down(physics, 1.5, 0.3).surface, Some(GRATE));
    assert_eq!(down(physics, 6.5, -0.7).surface, Some(CONCRETE));
    for (x, want) in [(1.5, GRATE), (6.5, CONCRETE)] {
        let cast = physics
            .shape_cast(
                &Shape::Sphere { radius: 0.1 },
                [x, 2.0, 0.3],
                IDENTITY,
                [0.0, -1.0, 0.0],
                5.0,
                &filter,
            )
            .unwrap();
        assert_eq!(cast[0].entity, floor);
        assert_eq!(cast[0].surface, Some(want), "shape cast at {x}");
        let closest = physics.closest_points([x, 0.3, 0.3], 1.0, &filter).unwrap();
        assert_eq!(closest[0].surface, Some(want), "closest point at {x}");
    }
    assert_eq!(
        physics.surface_of(floor),
        Some(CARPET),
        "the entity tag stays the contact surface"
    );
}

#[test]
fn a_ray_on_the_shared_edge_reports_the_lower_index_triangle() {
    let mut world = World::new();
    world.insert_resource(library(|| tagged_strip(1)));
    mesh_floor(&mut world, "");
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    // Triangle 7 (grate) and triangle 8 (concrete) share the line x = 4.
    assert_eq!(down(physics(&world), 4.0, 0.0).surface, Some(GRATE));
    assert_eq!(down(physics(&world), 4.0, 1.7).surface, Some(GRATE));
}

#[test]
fn an_overlay_still_wins_and_an_untagged_mesh_reports_the_entity_tag() {
    let mut world = World::new();
    world.insert_resource(library(|| tagged_strip(1)));
    mesh_floor(&mut world, "carpet");
    let patch = world.spawn();
    world.insert(patch, TestPose::at([6.0, 0.0, 0.0])).unwrap();
    world
        .insert(
            patch,
            Collider {
                shape: Shape::Cylinder {
                    half_height: 0.05,
                    radius: 1.0,
                },
                body: Body::new_static(),
                layer: None,
                sensor: true,
            },
        )
        .unwrap();
    world
        .insert(
            patch,
            SurfaceOverlay {
                surface: "dirt".to_string(),
                priority: 0,
            },
        )
        .unwrap();
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    assert_eq!(down(physics(&world), 6.0, 0.0).surface, Some(DIRT));
    assert_eq!(down(physics(&world), 7.8, 0.0).surface, Some(CONCRETE));

    let mut plain = World::new();
    plain.insert_resource(library(|| strip(2, &[], &[])));
    mesh_floor(&mut plain, "carpet");
    let mut schedule = stepped();
    run(&mut plain, &mut schedule, 0..2);
    assert_eq!(down(physics(&plain), 1.5, 0.0).surface, Some(CARPET));
}

#[test]
fn an_undeclared_local_name_rejects_once_and_those_triangles_fall_through() {
    let mut world = World::new();
    world.insert_resource(library(|| {
        strip(1, &["metal_grat", "concrete"], &[[8, 0], [8, 1]])
    }));
    let floor = mesh_floor(&mut world, "carpet");
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..4);
    let found = rejections(&mut world);
    assert_eq!(found.len(), 1, "one L1697 for a standing fault: {found:?}");
    assert_eq!(found[0].code, code::MESH_SURFACE_UNDECLARED);
    assert_eq!(found[0].entity, floor);
    assert!(
        found[0].reason.contains("metal_grat"),
        "{}",
        found[0].reason
    );
    assert_eq!(down(physics(&world), 1.5, 0.0).surface, Some(CARPET));
    assert_eq!(down(physics(&world), 6.5, 0.0).surface, Some(CONCRETE));
}

#[test]
fn a_tag_only_mesh_edit_rebuilds_and_reports_the_new_tags() {
    let mut world = World::new();
    world.insert_resource(library(|| tagged_strip(1)));
    mesh_floor(&mut world, "");
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    assert_eq!(down(physics(&world), 1.5, 0.0).surface, Some(GRATE));

    // The same triangles with the two regions swapped: only the tags and the document hash changed.
    world.insert_resource(library(|| {
        strip(2, &["metal_grate", "concrete"], &[[8, 1], [8, 0]])
    }));
    run(&mut world, &mut schedule, 2..4);
    assert_eq!(down(physics(&world), 1.5, 0.0).surface, Some(CONCRETE));
    assert_eq!(down(physics(&world), 6.5, 0.0).surface, Some(GRATE));
}

#[test]
fn a_restore_answers_mesh_tags_before_any_write() {
    let mut world = World::new();
    world.insert_resource(library(|| tagged_strip(1)));
    mesh_floor(&mut world, "carpet");
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    let before = down(physics(&world), 1.5, 0.0).surface;
    assert_eq!(before, Some(GRATE));

    let blob = physics(&world).to_bytes().unwrap();
    let mut restored = PhysicsWorld::from_bytes(&blob, TickRate::DEFAULT).unwrap();
    restored.sync_surface_tags(&world);
    restored.apply_settings(&settings());
    world.insert_resource(restored);
    assert_eq!(
        down(physics(&world), 1.5, 0.0).surface,
        Some(CARPET),
        "without the mesh sync only the entity tag is known"
    );
    sync_mesh_surfaces(&mut world);
    assert_eq!(down(physics(&world), 1.5, 0.0).surface, before);
    assert_eq!(down(physics(&world), 6.5, 0.0).surface, Some(CONCRETE));
    assert!(rejections(&mut world).is_empty());
}

// --- Heightfield cells ----------------------------------------------------------------------

/// A flat 3-row by 4-column heightfield spanning `x ∈ [-3, 3]`, `z ∈ [-2, 2]`: 2 × 3 cells of 2 m,
/// row-major with the row along `z` and the column along `x`. `runs` tags cells 0..6 in order.
fn terrain(world: &mut World, runs: Vec<[u32; 2]>, names: &[&str]) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at([0.0; 3])).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Heightfield {
                    rows: 3,
                    cols: 4,
                    heights: vec![0.0; 12],
                    scale: [6.0, 1.0, 4.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    world
        .insert(
            entity,
            SurfaceTag {
                surface: "carpet".to_string(),
                parts: Vec::new(),
                cells: Some(CellSurfaces {
                    surfaces: names.iter().map(|name| name.to_string()).collect(),
                    runs,
                }),
            },
        )
        .unwrap();
    entity
}

#[test]
fn heightfield_cells_report_their_surface_in_authored_row_major_order() {
    let mut world = World::new();
    // Cell 2 is (row 0, column 2): x in [1, 3], z in [-2, 0]. The rest are dirt.
    terrain(
        &mut world,
        vec![[2, 0], [1, 1], [3, 0]],
        &["dirt", "concrete"],
    );
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..2);
    let physics = physics(&world);
    assert_eq!(down(physics, 2.0, -1.0).surface, Some(CONCRETE));
    assert_eq!(
        down(physics, 2.0, 1.0).surface,
        Some(DIRT),
        "cell 5 is row 1"
    );
    assert_eq!(down(physics, -2.0, -1.0).surface, Some(DIRT), "cell 0");
    assert_eq!(down(physics, 0.0, -1.0).surface, Some(DIRT), "cell 1");
    assert_eq!(down(physics, 0.0, 1.0).surface, Some(DIRT), "cell 4");
    // The seam: a point exactly on the x = 1 edge between cells 1 and 2 takes the cell the
    // quantiser floors it into.
    let seam = down(physics, 1.0, -1.0).surface;
    assert_eq!(
        seam,
        Some(CONCRETE),
        "an edge point belongs to the higher-index cell"
    );
}

#[test]
fn an_undeclared_cell_name_is_one_l1690_and_the_entity_tag_answers() {
    let mut world = World::new();
    let ground = terrain(&mut world, vec![[6, 0]], &["dirtt"]);
    let mut schedule = stepped();
    run(&mut world, &mut schedule, 0..4);
    let found = rejections(&mut world);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, code::SURFACE_UNKNOWN);
    assert_eq!(found[0].entity, ground);
    assert_eq!(down(physics(&world), 0.0, 0.0).surface, Some(CARPET));
}
