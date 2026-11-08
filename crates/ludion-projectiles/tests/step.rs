//! The swept step: integration, lifetime, hit responses, tunnelling, ownership.

mod common;

use common::*;
use ludion_core::ids::EntityId;
use ludion_projectiles::{
    HitEvent, HitOutcome, ProjectileDesc, ProjectileId, ProjectilePool, Response, SKIN, StepEvents,
    step,
};
use ludion_spatial::Shape;
use ludion_spatial::spatial3d::{Handle, Index3d};

fn run(
    pool: &mut ProjectilePool,
    physics: &ludion_physics::PhysicsWorld,
    index: &Index3d,
    dt: f32,
    cfg: &ludion_projectiles::StepConfig,
) -> StepEvents {
    let mut events = StepEvents::with_capacity(64, 64);
    step(pool, dt, physics, index, cfg, &mut events);
    events
}

fn one(pool: &mut ProjectilePool, desc: ProjectileDesc) -> ProjectileId {
    pool.spawn(desc).unwrap()
}

#[test]
fn gravity_and_drag_follow_the_closed_form() {
    let physics = empty_physics();
    let index = Index3d::new();
    let mut cfg = config(&physics);
    cfg.gravity = [0.0, -10.0, 0.0];
    let mut pool = ProjectilePool::with_capacity(4);
    let mut d = ProjectileDesc::new([1.0, 2.0, 3.0], [6.0, 0.0, 0.0]);
    d.gravity_scale = 2.0;
    d.drag = 0.5;
    let id = one(&mut pool, d);
    run(&mut pool, &physics, &index, DT, &cfg);
    let damp = 1.0 / (1.0 + 0.5 * DT);
    let v = [6.0 * damp, (-10.0 * 2.0 * DT) * damp, 0.0];
    assert!(near(pool.velocity(id).unwrap(), v, 1e-6));
    let p = [1.0 + v[0] * DT, 2.0 + v[1] * DT, 3.0];
    assert!(near(pool.position(id).unwrap(), p, 1e-6));
}

#[test]
fn lifetime_expiry_emits_and_frees() {
    let physics = empty_physics();
    let index = Index3d::new();
    let cfg = config(&physics);
    let mut pool = ProjectilePool::with_capacity(4);
    let mut d = ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]);
    d.lifetime = 0.05;
    d.tag = 9;
    let id = one(&mut pool, d);
    for _ in 0..2 {
        assert!(
            run(&mut pool, &physics, &index, 0.02, &cfg)
                .expired
                .is_empty()
        );
    }
    let events = run(&mut pool, &physics, &index, 0.02, &cfg);
    assert_eq!(events.expired.len(), 1);
    assert_eq!(events.expired[0].id, id);
    assert_eq!(events.expired[0].tag, 9);
    assert!(!pool.contains(id));
    assert_eq!(pool.len(), 0);
}

#[test]
fn despawn_on_a_world_hit_reports_handle_point_normal_tag() {
    let (world, ids) = box_world(&[([5.5, 0.0, 0.0], [0.5, 1.0, 1.0])]);
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(4);
    let mut d = ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]);
    d.tag = 42;
    let id = one(&mut pool, d);
    let events = run(&mut pool, physics, &Index3d::new(), DT, &cfg);
    let hit: HitEvent = events.hits[0];
    assert_eq!(events.hits.len(), 1);
    assert_eq!(hit.id, id);
    assert_eq!(hit.handle, Handle::entity(ids[0]));
    assert_eq!(hit.tag, 42);
    assert_eq!(hit.outcome, HitOutcome::Despawned);
    assert!(near(hit.point, [5.0, 0.0, 0.0], 1e-5));
    assert!(near(hit.normal, [-1.0, 0.0, 0.0], 1e-6));
    assert!((hit.t - 0.5).abs() < 1e-5);
    assert_eq!(hit.velocity, [600.0, 0.0, 0.0]);
    assert!(!pool.contains(id));
}

#[test]
fn despawn_on_a_target_hit() {
    let physics = empty_physics();
    let target = Handle::slot(7, 3);
    let index = index_of(&[sphere(target, [5.0, 0.0, 0.0], 1.0, 1)]);
    let mut cfg = config(&physics);
    cfg.targets = 1;
    let mut pool = ProjectilePool::with_capacity(4);
    let mut d = ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]);
    d.tag = 5;
    one(&mut pool, d);
    let events = run(&mut pool, &physics, &index, DT, &cfg);
    assert_eq!(events.hits.len(), 1);
    assert_eq!(events.hits[0].handle, target);
    assert_eq!(events.hits[0].tag, 5);
    assert!(near(events.hits[0].point, [4.0, 0.0, 0.0], 1e-5));
    assert!(near(events.hits[0].normal, [-1.0, 0.0, 0.0], 1e-6));
    assert!(pool.is_empty());
}

fn wall_x() -> (ludion_ecs::World, Vec<EntityId>) {
    // The plane x = 5, solid beyond it.
    box_world(&[([6.0, 0.0, 0.0], [1.0, 50.0, 50.0])])
}

#[test]
fn ricochet_angles_follow_restitution() {
    let (world, _) = wall_x();
    let physics = physics_ref(&world);
    let cfg = config(physics);
    for (e, expect) in [(1.0, [-10.0, 0.0, 10.0]), (0.5, [-5.0, 0.0, 10.0])] {
        let mut pool = ProjectilePool::with_capacity(2);
        let mut d = ProjectileDesc::new([0.0; 3], [10.0, 0.0, 10.0]);
        d.response = Response::Reflect {
            restitution: e,
            max_bounces: 3,
        };
        let id = one(&mut pool, d);
        let events = run(&mut pool, physics, &Index3d::new(), 1.0, &cfg);
        assert_eq!(events.hits.len(), 1);
        assert_eq!(events.hits[0].outcome, HitOutcome::Reflected);
        assert!(near(pool.velocity(id).unwrap(), expect, 1e-6));
        assert_eq!(pool.bounces(id), Some(1));
        // Half the tick is left; it travels away from the wall.
        let p = pool.position(id).unwrap();
        assert!(p[0] < 5.0);
        assert!((p[2] - 10.0).abs() < 1e-3 + SKIN);
    }
}

#[test]
fn an_inner_corner_reflects_twice_without_a_repeat() {
    let (world, ids) = box_world(&[
        ([6.0, 0.0, 0.0], [1.0, 50.0, 50.0]),
        ([0.0, 0.0, 6.0], [50.0, 50.0, 1.0]),
    ]);
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let v = [300.0, 0.0, 290.0];
    let mut d = ProjectileDesc::new([4.0, 0.0, 4.0], v);
    d.response = Response::Reflect {
        restitution: 1.0,
        max_bounces: 8,
    };
    let id = one(&mut pool, d);
    let events = run(&mut pool, physics, &Index3d::new(), DT, &cfg);
    assert_eq!(events.hits.len(), 2);
    assert_eq!(events.hits[0].handle, Handle::entity(ids[0]));
    assert_eq!(events.hits[1].handle, Handle::entity(ids[1]));
    assert!(near(pool.velocity(id).unwrap(), [-v[0], 0.0, -v[2]], 1e-3));
    let p = pool.position(id).unwrap();
    assert!(p[0] < 5.0 && p[2] < 5.0, "ended inside a wall: {p:?}");
}

#[test]
fn a_hit_at_the_very_end_of_the_tick_does_not_repeat() {
    let reach = 300.0 * DT;
    let surface = reach * (1.0 - 1e-6);
    let (world, _) = box_world(&[([surface + 0.5, 0.0, 0.0], [0.5, 50.0, 50.0])]);
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [300.0, 0.0, 0.0]);
    d.response = Response::Reflect {
        restitution: 1.0,
        max_bounces: 8,
    };
    let id = one(&mut pool, d);
    let first = run(&mut pool, physics, &Index3d::new(), DT, &cfg);
    assert_eq!(first.hits.len(), 1);
    assert!(first.hits[0].t > 0.999);
    for _ in 0..3 {
        assert!(
            run(&mut pool, physics, &Index3d::new(), DT, &cfg)
                .hits
                .is_empty()
        );
    }
    assert_eq!(pool.bounces(id), Some(1));
}

#[test]
fn max_bounces_is_honoured() {
    let (world, _) = box_world(&[
        ([6.0, 0.0, 0.0], [1.0, 50.0, 50.0]),
        ([-6.0, 0.0, 0.0], [1.0, 50.0, 50.0]),
    ]);
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.5, 0.0, 0.0], [300.0, 0.0, 0.0]);
    d.response = Response::Reflect {
        restitution: 1.0,
        max_bounces: 2,
    };
    let id = one(&mut pool, d);
    let mut outcomes = Vec::new();
    for _ in 0..20 {
        let ev = run(&mut pool, physics, &Index3d::new(), DT, &cfg);
        outcomes.extend(ev.hits.iter().map(|h| h.outcome));
    }
    assert_eq!(
        outcomes,
        [
            HitOutcome::Reflected,
            HitOutcome::Reflected,
            HitOutcome::Despawned
        ]
    );
    assert!(!pool.contains(id));
}

fn pierce_scene() -> (Index3d, [Handle; 3]) {
    let (a, b, c) = (Handle::slot(0, 0), Handle::slot(1, 0), Handle::slot(2, 0));
    let index = index_of(&[
        sphere(a, [5.0, 0.0, 0.0], 3.0, 1),
        sphere(b, [5.0, 0.0, 0.0], 1.0, 1),
        sphere(c, [12.0, 0.0, 0.0], 1.0, 1),
    ]);
    (index, [a, b, c])
}

#[test]
fn pierce_hits_each_target_once_across_ticks() {
    let physics = empty_physics();
    let (index, handles) = pierce_scene();
    let mut cfg = config(&physics);
    cfg.targets = 1;
    let mut pool = ProjectilePool::with_capacity(2);
    // 1.2 m per tick: the 6 m wide sphere A takes five ticks to cross.
    let mut d = ProjectileDesc::new([0.0; 3], [72.0, 0.0, 0.0]);
    d.response = Response::Pierce { max: 5 };
    let id = one(&mut pool, d);
    let mut seen = Vec::new();
    for _ in 0..20 {
        let ev = run(&mut pool, &physics, &index, DT, &cfg);
        seen.extend(ev.hits.iter().map(|h| (h.handle, h.outcome)));
    }
    let got: Vec<Handle> = seen.iter().map(|s| s.0).collect();
    assert_eq!(got, handles);
    assert!(seen.iter().all(|s| s.1 == HitOutcome::Pierced));
    assert_eq!(pool.pierces(id), Some(3));
    assert!(pool.contains(id));
}

#[test]
fn pierce_max_and_world_hits_despawn() {
    let physics = empty_physics();
    let (index, handles) = pierce_scene();
    let mut cfg = config(&physics);
    cfg.targets = 1;
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [72.0, 0.0, 0.0]);
    d.response = Response::Pierce { max: 1 };
    let id = one(&mut pool, d);
    let mut seen = Vec::new();
    for _ in 0..20 {
        let ev = run(&mut pool, &physics, &index, DT, &cfg);
        seen.extend(ev.hits.iter().map(|h| (h.handle, h.outcome)));
    }
    assert_eq!(
        seen,
        [
            (handles[0], HitOutcome::Pierced),
            (handles[1], HitOutcome::Despawned)
        ]
    );
    assert!(!pool.contains(id));

    let (world, _) = box_world(&[([5.5, 0.0, 0.0], [0.5, 5.0, 5.0])]);
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut d = ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]);
    d.response = Response::Pierce { max: 5 };
    let id = one(&mut pool, d);
    let ev = run(&mut pool, physics, &Index3d::new(), DT, &cfg);
    assert_eq!(ev.hits[0].outcome, HitOutcome::Despawned);
    assert!(!pool.contains(id));
}

#[test]
fn stop_parks_and_still_expires() {
    let physics = empty_physics();
    let index = index_of(&[sphere(Handle::slot(0, 0), [5.0, 0.0, 0.0], 1.0, 1)]);
    let mut cfg = config(&physics);
    cfg.targets = 1;
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]);
    d.response = Response::Stop;
    d.lifetime = 3.5 * DT;
    let id = one(&mut pool, d);
    let ev = run(&mut pool, &physics, &index, DT, &cfg);
    assert_eq!(ev.hits[0].outcome, HitOutcome::Stopped);
    assert_eq!(pool.is_parked(id), Some(true));
    let parked_at = pool.position(id).unwrap();
    assert!(near(parked_at, [4.0, 0.0, 0.0], 1e-5));
    for _ in 0..2 {
        let ev = run(&mut pool, &physics, &index, DT, &cfg);
        assert!(ev.hits.is_empty() && ev.expired.is_empty());
        assert_eq!(pool.position(id), Some(parked_at));
    }
    assert!(pool.contains(id));
    let ev = run(&mut pool, &physics, &index, DT, &cfg);
    assert_eq!(ev.expired.len(), 1);
    assert!(!pool.contains(id));
}

#[test]
fn the_owner_is_never_hit() {
    let (world, ids) = box_world(&[([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])]);
    let physics = physics_ref(&world);
    let owner = Handle::entity(ids[0]);
    let index = index_of(&[sphere(owner, [0.0, 0.0, 0.0], 1.5, 1)]);
    let mut cfg = config(physics);
    cfg.targets = 1;
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [120.0, 0.0, 0.0]);
    d.owner = Some(owner);
    let id = one(&mut pool, d);
    let ev = run(&mut pool, physics, &index, DT, &cfg);
    assert!(ev.hits.is_empty());
    assert!(pool.contains(id));
    // Without the owner it hits the box at t = 0.
    let mut d = ProjectileDesc::new([0.0; 3], [120.0, 0.0, 0.0]);
    d.owner = None;
    one(&mut pool, d);
    let ev = run(&mut pool, physics, &index, DT, &cfg);
    assert_eq!(ev.hits.len(), 1);
}

#[test]
fn thin_targets_are_not_tunnelled_at_extreme_speed() {
    let speed = 1e4f32;
    let reach = speed * DT;
    for radius in [0.0f32, 0.05] {
        let (world, ids) = box_world(&[([100.0, 0.0, 0.0], [0.005, 5.0, 5.0])]);
        let physics = physics_ref(&world);
        let cfg = config(physics);
        let expect_t = (100.0 - 0.005 - radius) / reach;

        let mut pool = ProjectilePool::with_capacity(2);
        let mut d = ProjectileDesc::new([0.0; 3], [speed, 0.0, 0.0]);
        d.radius = radius;
        one(&mut pool, d);
        let ev = run(&mut pool, physics, &Index3d::new(), DT, &cfg);
        assert_eq!(ev.hits.len(), 1, "physics, radius {radius}");
        assert_eq!(ev.hits[0].handle, Handle::entity(ids[0]));
        assert!(
            (ev.hits[0].t - expect_t).abs() < 1e-4,
            "{} vs {expect_t}",
            ev.hits[0].t
        );

        let empty = empty_physics();
        let target = Handle::slot(1, 0);
        let index = index_of(&[aabb(target, [99.995, -5.0, -5.0], [100.005, 5.0, 5.0], 1)]);
        let mut cfg = config(&empty);
        cfg.targets = 1;
        let mut d = ProjectileDesc::new([0.0; 3], [speed, 0.0, 0.0]);
        d.radius = radius;
        one(&mut pool, d);
        let ev = run(&mut pool, &empty, &index, DT, &cfg);
        assert_eq!(ev.hits.len(), 1, "index, radius {radius}");
        assert_eq!(ev.hits[0].handle, target);
        assert!((ev.hits[0].t - expect_t).abs() < 1e-4);
    }
}

#[test]
fn the_nearer_hit_wins_and_a_full_tie_goes_to_physics() {
    let (world, ids) = box_world(&[([6.0, 0.0, 0.0], [1.0, 5.0, 5.0])]);
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.targets = 1;
    // A target in front of the wall wins.
    let near_target = Handle::slot(0, 0);
    let index = index_of(&[sphere(near_target, [3.0, 0.0, 0.0], 0.5, 1)]);
    let mut pool = ProjectilePool::with_capacity(2);
    one(&mut pool, ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]));
    let ev = run(&mut pool, physics, &index, DT, &cfg);
    assert_eq!(ev.hits[0].handle, near_target);
    // Same surface, same t: the box face at x = 5 and a target whose face is also x = 5.
    let index = index_of(&[aabb(
        Handle::slot(9, 0),
        [5.0, -1.0, -1.0],
        [5.5, 1.0, 1.0],
        1,
    )]);
    one(&mut pool, ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]));
    let ev = run(&mut pool, physics, &index, DT, &cfg);
    assert_eq!(ev.hits[0].handle, Handle::entity(ids[0]));
}

#[test]
fn a_ball_that_reflects_does_not_re_hit_the_same_wall() {
    let (world, _) = wall_x();
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [300.0, 0.0, 120.0]);
    d.radius = 0.05;
    d.response = Response::Reflect {
        restitution: 0.9,
        max_bounces: 8,
    };
    let id = one(&mut pool, d);
    let mut hits = 0;
    for _ in 0..4 {
        hits += run(&mut pool, physics, &Index3d::new(), DT, &cfg)
            .hits
            .len();
    }
    assert_eq!(hits, 1);
    assert_eq!(pool.bounces(id), Some(1));
}

#[test]
fn pierce_does_not_rehit_the_collider_of_a_pierced_entity() {
    let capsule = Shape::Capsule {
        half_height: 0.5,
        radius: 0.4,
    };
    let (world, ids) = shape_world(&[([5.0, 0.0, 0.0], capsule)]);
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.targets = 1;
    let target = Handle::entity(ids[0]);
    let index = index_of(&[sphere(target, [5.0, 0.0, 0.0], 1.5, 1)]);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]);
    d.response = Response::Pierce { max: 2 };
    let id = one(&mut pool, d);
    let mut seen = Vec::new();
    for _ in 0..3 {
        let ev = run(&mut pool, physics, &index, DT, &cfg);
        seen.extend(ev.hits.iter().map(|h| (h.handle, h.outcome)));
    }
    assert_eq!(seen, vec![(target, HitOutcome::Pierced)]);
    assert!(pool.contains(id));
    assert!(pool.position(id).unwrap()[0] > 6.5);
}

#[test]
fn a_ricochet_far_from_the_origin_bounces_once_and_leaves() {
    let (world, _) = box_world(&[([10_001.0, 0.0, 0.0], [1.0, 50.0, 50.0])]);
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([9_990.0, 0.0, 0.0], [600.0, 0.0, 0.0]);
    d.response = Response::Reflect {
        restitution: 1.0,
        max_bounces: 8,
    };
    let id = one(&mut pool, d);
    let mut reflected = 0;
    for _ in 0..30 {
        reflected += run(&mut pool, physics, &Index3d::new(), DT, &cfg)
            .hits
            .iter()
            .filter(|h| h.outcome == HitOutcome::Reflected)
            .count();
    }
    assert_eq!(reflected, 1);
    assert!(pool.velocity(id).unwrap()[0] < 0.0);
    assert!(pool.position(id).unwrap()[0] < 10_000.0);
}

#[test]
fn a_reflect_shot_touching_a_wall_and_moving_away_has_no_event() {
    let (world, _) = wall_x();
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([4.9, 0.0, 0.0], [-10.0, 0.0, 0.0]);
    d.radius = 0.2;
    d.response = Response::Reflect {
        restitution: 1.0,
        max_bounces: 3,
    };
    let id = one(&mut pool, d);
    let events = run(&mut pool, physics, &Index3d::new(), DT, &cfg);
    assert!(events.hits.is_empty());
    assert_eq!(pool.bounces(id), Some(0));
}

fn floor() -> (ludion_ecs::World, Vec<EntityId>) {
    // The plane y = 0, solid below it.
    box_world(&[([0.0, -1.0, 0.0], [50.0, 1.0, 50.0])])
}

#[test]
fn settle_parks_on_the_floor_and_expires() {
    let (world, _) = floor();
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let index = Index3d::new();
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0, 2.0, 0.0], [3.0, 0.0, 0.0]);
    d.gravity_scale = 1.0;
    d.response = Response::settle(0.4, 0.5, 0.5);
    d.spin = [1.0, 2.0, 3.0];
    d.lifetime = 4.0;
    let id = one(&mut pool, d);
    let mut parked_at = None;
    let mut expired = 0;
    for tick in 0..250 {
        let ev = run(&mut pool, physics, &index, DT, &cfg);
        expired += ev.expired.len();
        if parked_at.is_some() {
            assert!(
                ev.hits.is_empty(),
                "a parked shot emits no hits (tick {tick})"
            );
            if pool.contains(id) {
                assert_eq!(pool.position(id), parked_at);
            }
        } else if pool.is_parked(id) == Some(true) {
            assert!(ev.hits.iter().any(|h| h.outcome == HitOutcome::Stopped));
            parked_at = pool.position(id);
        }
    }
    let at = parked_at.expect("the gib came to rest");
    assert!(
        at[1] > 0.0 && at[1] < 0.01,
        "rests just above the floor: {at:?}"
    );
    assert_eq!(expired, 1);
    assert!(!pool.contains(id));
}

#[test]
fn settle_parks_with_zero_velocity_and_spin_and_keeps_its_orientation() {
    let (world, _) = floor();
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let index = Index3d::new();
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
    d.gravity_scale = 1.0;
    d.response = Response::settle(0.3, 0.6, 1.0);
    d.spin = [0.0, 4.0, 5.0];
    let id = one(&mut pool, d);
    for _ in 0..200 {
        run(&mut pool, physics, &index, DT, &cfg);
        if pool.is_parked(id) == Some(true) {
            break;
        }
    }
    assert_eq!(pool.is_parked(id), Some(true));
    assert_eq!(pool.spin(id), Some([0.0; 3]));
    assert_eq!(pool.velocity(id), Some([0.0; 3]));
    let q = pool.orientation(id).unwrap();
    for _ in 0..10 {
        run(&mut pool, physics, &index, DT, &cfg);
        assert_eq!(
            pool.orientation(id).unwrap().map(f32::to_bits),
            q.map(f32::to_bits)
        );
    }
}

#[test]
fn settle_bounces_off_a_wall_without_parking() {
    let (world, _) = wall_x();
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [2.0, 0.0, 0.0]);
    d.response = Response::settle(0.5, 0.0, 10.0);
    let id = one(&mut pool, d);
    let ev = run(&mut pool, physics, &Index3d::new(), 5.0, &cfg);
    assert_eq!(ev.hits.len(), 1);
    assert_eq!(ev.hits[0].outcome, HitOutcome::Reflected);
    assert_eq!(pool.is_parked(id), Some(false));
    assert!(pool.velocity(id).unwrap()[0] < 0.0);
    assert!(pool.position(id).unwrap()[0] < 5.0);
    assert_eq!(pool.bounces(id), Some(1));
}

#[test]
fn settle_has_no_bounce_cap() {
    let (world, _) = box_world(&[
        ([6.0, 0.0, 0.0], [1.0, 50.0, 50.0]),
        ([-6.0, 0.0, 0.0], [1.0, 50.0, 50.0]),
    ]);
    let physics = physics_ref(&world);
    let cfg = config(physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [300.0, 0.0, 0.0]);
    d.response = Response::settle(1.0, 0.0, 0.1);
    let id = one(&mut pool, d);
    for _ in 0..60 {
        run(&mut pool, physics, &Index3d::new(), DT, &cfg);
    }
    assert!(pool.bounces(id).unwrap() > 20);
    assert!(pool.contains(id));
    assert_eq!(pool.is_parked(id), Some(false));
}

#[test]
fn spin_integrates_about_the_world_axis_and_stays_a_unit_quaternion() {
    let physics = empty_physics();
    let cfg = config(&physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [0.0; 3]);
    d.spin = [0.0, std::f32::consts::PI, 0.0];
    let id = one(&mut pool, d);
    for _ in 0..30 {
        run(&mut pool, &physics, &Index3d::new(), DT, &cfg);
    }
    let q = pool.orientation(id).unwrap();
    let h = std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (q[0].abs() + q[2].abs() < 1e-2) && (q[1] - h).abs() < 1e-2 && (q[3] - h).abs() < 1e-2,
        "{q:?}"
    );
    let n = q.iter().map(|c| c * c).sum::<f32>();
    assert!((n - 1.0).abs() < 1e-5);
}

#[test]
fn a_shot_without_spin_keeps_the_identity_orientation_bit_exactly() {
    let physics = empty_physics();
    let cfg = config(&physics);
    let mut pool = ProjectilePool::with_capacity(2);
    let id = one(&mut pool, ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]));
    for _ in 0..5 {
        run(&mut pool, &physics, &Index3d::new(), DT, &cfg);
    }
    assert_eq!(
        pool.orientation(id).unwrap().map(f32::to_bits),
        [0.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)
    );
}

#[test]
fn per_shot_targets_override_the_pool_mask() {
    let physics = empty_physics();
    let index = index_of(&[sphere(Handle::slot(7, 0), [5.0, 0.0, 0.0], 1.0, 1)]);
    for (pool_mask, shot_mask, hits) in [(1, Some(0), 0), (0, Some(1), 1), (1, None, 1)] {
        let mut cfg = config(&physics);
        cfg.targets = pool_mask;
        let mut pool = ProjectilePool::with_capacity(2);
        let mut d = ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]);
        d.targets = shot_mask;
        one(&mut pool, d);
        let ev = run(&mut pool, &physics, &index, DT, &cfg);
        assert_eq!(ev.hits.len(), hits, "pool {pool_mask} shot {shot_mask:?}");
    }
}

#[test]
fn invalid_gib_fields_name_the_field() {
    use ludion_projectiles::SpawnError::InvalidDesc;
    let base = ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]);
    let mut pool = ProjectilePool::with_capacity(1);
    type Edit = Box<dyn Fn(&mut ProjectileDesc)>;
    let cases: [(&str, Edit); 8] = [
        ("spin", Box::new(|d| d.spin = [f32::NAN, 0.0, 0.0])),
        ("orientation", Box::new(|d| d.orientation = [0.0; 4])),
        (
            "orientation",
            Box::new(|d| d.orientation = [f32::INFINITY, 0.0, 0.0, 1.0]),
        ),
        (
            "response.restitution",
            Box::new(|d| d.response = Response::settle(1.5, 0.5, 0.5)),
        ),
        (
            "response.friction",
            Box::new(|d| d.response = Response::settle(0.5, -0.1, 0.5)),
        ),
        (
            "response.rest_speed",
            Box::new(|d| d.response = Response::settle(0.5, 0.5, -1.0)),
        ),
        (
            "response.rest_speed",
            Box::new(|d| d.response = Response::settle(0.5, 0.5, f32::NAN)),
        ),
        (
            "response.floor_cos",
            Box::new(|d| {
                d.response = Response::Settle {
                    restitution: 0.5,
                    friction: 0.5,
                    rest_speed: 0.5,
                    floor_cos: 2.0,
                }
            }),
        ),
    ];
    for (field, f) in cases {
        let mut d = base;
        f(&mut d);
        assert_eq!(pool.spawn(d), Err(InvalidDesc(field)), "{field}");
    }
}
