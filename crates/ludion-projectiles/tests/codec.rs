//! The byte codec: a decoded pool continues bit-identically; hostile bytes are `Err`, not a panic.

mod common;

use common::*;
use ludion_projectiles::{ProjectileDesc, ProjectilePool, Response, StepEvents, step};
use ludion_spatial::spatial3d::Handle;

fn desc(i: u32) -> ProjectileDesc {
    let mut d = ProjectileDesc::new(
        [0.0, 1.0 + (i % 3) as f32 * 0.4, 0.0],
        [30.0 + i as f32, 0.5 * i as f32 - 2.0, 0.0],
    );
    d.gravity_scale = 0.5;
    d.drag = 0.05;
    d.radius = if i.is_multiple_of(2) { 0.0 } else { 0.1 };
    d.lifetime = 0.25 + 0.05 * (i % 5) as f32;
    d.tag = i;
    d.owner = (i.is_multiple_of(4)).then(|| Handle::slot(99, 3));
    d.response = match i % 4 {
        0 => Response::Despawn,
        1 => Response::Reflect {
            restitution: 0.8,
            max_bounces: 3,
        },
        2 => Response::Pierce { max: 3 },
        _ => Response::Stop,
    };
    d
}

fn event_bits(e: &StepEvents) -> Vec<u32> {
    let mut b = Vec::new();
    for h in &e.hits {
        b.extend([
            h.id.index,
            h.id.generation,
            h.handle.index,
            h.tag,
            h.outcome as u32,
        ]);
        b.extend(h.point.map(f32::to_bits));
        b.extend(h.normal.map(f32::to_bits));
        b.extend(h.velocity.map(f32::to_bits));
        b.push(h.t.to_bits());
    }
    for x in &e.expired {
        b.extend([x.id.index, x.id.generation, x.tag]);
        b.extend(x.position.map(f32::to_bits));
    }
    b
}

/// A churned pool mid-flight with every response kind, a parked shot and pierce memory.
fn churned() -> (ProjectilePool, StepEvents) {
    let (world, _) = box_world(&[([20.0, 3.0, 0.0], [0.5, 4.0, 4.0])]);
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let items: Vec<_> = (0..4)
        .map(|k| {
            sphere(
                Handle::slot(500 + k, 0),
                [4.0 + 3.0 * k as f32, 1.0, 0.0],
                1.0,
                1,
            )
        })
        .collect();
    let index = index_of(&items);
    let mut pool = ProjectilePool::with_capacity(64);
    let mut events = StepEvents::with_capacity(64, 64);
    let mut n = 0;
    for tick in 0..24u32 {
        if tick.is_multiple_of(3) {
            for _ in 0..4 {
                let _ = pool.spawn(desc(n));
                n += 1;
            }
        }
        step(&mut pool, DT, physics, &index, &cfg, &mut events);
    }
    assert!(
        pool.ids().any(|id| pool.is_parked(id) == Some(true)),
        "a parked shot"
    );
    assert!(
        pool.ids().any(|id| pool.pierces(id).unwrap_or(0) > 0),
        "pierce memory"
    );
    assert!(pool.ids().any(|id| id.generation > 0), "recycled slots");
    (pool, events)
}

/// Legacy shots (no spin, orientation or per-shot targets) must keep these exact bytes and
/// digests forever: `WorldHash`, snapshots and the replay corpus depend on them. Never re-pin.
#[test]
fn legacy_pool_bytes_and_digest_are_pinned() {
    let (mut pool, events) = churned();
    let mut bytes = Vec::new();
    pool.encode(&mut bytes);
    events.encode(&mut bytes);
    assert_eq!(xxhash_rust::xxh3::xxh3_64(&bytes), 6993480639543105117);
    assert_eq!(bytes.len(), 3145);
    assert_eq!(pool.digest(), 1391443476169914171);

    let (world, _) = box_world(&[([20.0, 3.0, 0.0], [0.5, 4.0, 4.0])]);
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let index = index_of(&[sphere(Handle::slot(500, 0), [4.0, 1.0, 0.0], 1.0, 1)]);
    let mut ev = StepEvents::default();
    for _ in 0..30 {
        step(&mut pool, DT, physics, &index, &cfg, &mut ev);
    }
    assert_eq!(pool.digest(), 3877882609219401894);
}

#[test]
fn a_decoded_pool_continues_identically() {
    let (a, ea) = churned();
    let mut bytes = Vec::new();
    a.encode(&mut bytes);
    ea.encode(&mut bytes);
    let mut cursor = bytes.as_slice();
    let b = ProjectilePool::decode(&mut cursor).unwrap();
    let eb = StepEvents::decode(&mut cursor).unwrap();
    assert!(cursor.is_empty());
    assert_eq!(a.digest(), b.digest());
    assert_eq!(event_bits(&ea), event_bits(&eb));

    let (world, _) = box_world(&[([20.0, 3.0, 0.0], [0.5, 4.0, 4.0])]);
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let index = index_of(&[sphere(Handle::slot(500, 0), [4.0, 1.0, 0.0], 1.0, 1)]);
    let (mut a, mut b) = (a, b);
    let (mut ea, mut eb) = (StepEvents::default(), StepEvents::default());
    for k in 0..20u32 {
        step(&mut a, DT, physics, &index, &cfg, &mut ea);
        step(&mut b, DT, physics, &index, &cfg, &mut eb);
        assert_eq!(a.digest(), b.digest(), "tick {k}");
        assert_eq!(event_bits(&ea), event_bits(&eb), "tick {k}");
        if k.is_multiple_of(4) {
            let d = desc(k);
            assert_eq!(a.spawn(d), b.spawn(d), "next id, tick {k}");
        }
    }
    assert_eq!(a.len(), b.len());
}

#[test]
fn a_resized_pool_continues_identically() {
    let (reference, _) = churned();
    let high = reference.high_water();
    assert!(high > 0 && high < reference.capacity());
    let (world, _) = box_world(&[([20.0, 3.0, 0.0], [0.5, 4.0, 4.0])]);
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let index = index_of(&[sphere(Handle::slot(500, 0), [4.0, 1.0, 0.0], 1.0, 1)]);

    for capacity in [reference.capacity() * 4, high] {
        let (mut a, _) = churned();
        let (mut b, _) = churned();
        assert!(b.resize(capacity));
        assert_eq!(b.capacity(), capacity);
        assert_eq!(a.digest(), b.digest());
        let (mut ea, mut eb) = (StepEvents::default(), StepEvents::default());
        for k in 0..20u32 {
            step(&mut a, DT, physics, &index, &cfg, &mut ea);
            step(&mut b, DT, physics, &index, &cfg, &mut eb);
            assert_eq!(a.digest(), b.digest(), "capacity {capacity}, tick {k}");
            assert_eq!(
                event_bits(&ea),
                event_bits(&eb),
                "capacity {capacity}, tick {k}"
            );
        }
        if capacity > high {
            assert_eq!(a.spawn(desc(7)), b.spawn(desc(7)), "next id");
        }
    }
    let mut pool = reference;
    assert!(
        !pool.resize(high - 1),
        "below the high-water mark is refused"
    );
    assert_eq!(pool.capacity(), 64);
}

#[test]
fn nan_and_negative_zero_survive() {
    let mut pool = ProjectilePool::with_capacity(2);
    let mut d = ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]);
    d.drag = 0.0;
    let id = pool.spawn(d).unwrap();
    assert!(pool.set_velocity(id, [-0.0, 1.0, 2.0]));
    let mut bytes = Vec::new();
    pool.encode(&mut bytes);
    let back = ProjectilePool::decode(&mut bytes.as_slice()).unwrap();
    assert_eq!(
        back.velocity(id).unwrap().map(f32::to_bits),
        [(-0.0f32).to_bits(), 1.0f32.to_bits(), 2.0f32.to_bits()]
    );
}

#[test]
fn events_round_trip() {
    let (_, events) = churned();
    let mut ev = events;
    ev.expired.push(ludion_projectiles::Expired {
        id: ludion_projectiles::ProjectileId {
            index: 3,
            generation: 2,
        },
        position: [f32::NAN, -0.0, 1.0],
        tag: 9,
    });
    let mut bytes = Vec::new();
    ev.encode(&mut bytes);
    let back = StepEvents::decode(&mut bytes.as_slice()).unwrap();
    assert_eq!(event_bits(&ev), event_bits(&back));
}

#[test]
fn hostile_bytes_are_errors() {
    let (pool, events) = churned();
    let mut pb = Vec::new();
    pool.encode(&mut pb);
    let mut eb = Vec::new();
    events.encode(&mut eb);
    for n in 0..pb.len() {
        assert!(
            ProjectilePool::decode(&mut &pb[..n]).is_err(),
            "pool prefix {n}"
        );
    }
    for n in 0..eb.len() {
        assert!(
            StepEvents::decode(&mut &eb[..n]).is_err(),
            "events prefix {n}"
        );
    }
    // Flip every byte in turn: decode must return (Ok or Err) without panicking, and the
    // header, counts and tags must be caught.
    for i in 0..pb.len() {
        let mut m = pb.clone();
        m[i] ^= 0xff;
        let _ = ProjectilePool::decode(&mut m.as_slice());
    }
    for i in 0..eb.len() {
        let mut m = eb.clone();
        m[i] ^= 0xff;
        let _ = StepEvents::decode(&mut m.as_slice());
    }
    for i in 0..5 {
        let mut m = pb.clone();
        m[i] ^= 0xff;
        assert!(
            ProjectilePool::decode(&mut m.as_slice()).is_err(),
            "header byte {i}"
        );
    }
    let mut m = pb.clone();
    m[13..17].copy_from_slice(&1000u32.to_le_bytes()); // live
    assert!(ProjectilePool::decode(&mut m.as_slice()).is_err());
    let mut m = pb.clone();
    m[5..9].copy_from_slice(&u32::MAX.to_le_bytes()); // capacity
    assert!(ProjectilePool::decode(&mut m.as_slice()).is_err());
}

fn header(capacity: u32, high_water: u32) -> Vec<u8> {
    let mut b = b"LPRJ".to_vec();
    b.push(1);
    for v in [capacity, high_water, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

#[test]
fn sizes_are_bounded_before_allocating() {
    let max = ludion_projectiles::MAX_DECODE_CAPACITY;
    let err = ProjectilePool::decode(&mut header(max, max).as_slice())
        .err()
        .unwrap();
    assert_eq!(err.reason(), "truncated");
    let err = ProjectilePool::decode(&mut header(max + 1, 0).as_slice())
        .err()
        .unwrap();
    assert_eq!(err.reason(), "capacity too large");
}

fn gib(i: u32) -> ProjectileDesc {
    let mut d = ProjectileDesc::new(
        [i as f32 * 0.3, 1.0 + (i % 3) as f32 * 0.5, 0.0],
        [2.0 + (i % 5) as f32, 1.0, 0.5 * i as f32 - 3.0],
    );
    d.gravity_scale = 1.0;
    d.lifetime = 1.5;
    d.tag = i;
    d.response = Response::settle(0.4, 0.5, 0.6);
    d.spin = [0.5 * i as f32, 3.0, -2.0];
    d.orientation = [0.1 * i as f32, 0.2, -0.3, 1.0];
    d.targets = i.is_multiple_of(2).then_some(1);
    if i == 3 {
        d.spin = [-0.0, 0.0, 0.0];
    }
    d
}

fn gib_scene() -> ludion_ecs::World {
    box_world(&[([0.0, -1.0, 0.0], [60.0, 1.0, 60.0])]).0
}

/// A pool mid-flight with Settle shots: some parked, some flying, spin, orientation and targets.
fn churned_gibs() -> (ProjectilePool, StepEvents) {
    let world = gib_scene();
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let index = index_of(&[sphere(Handle::slot(500, 0), [8.0, 1.0, 0.0], 1.0, 1)]);
    let mut pool = ProjectilePool::with_capacity(64);
    let mut events = StepEvents::with_capacity(64, 64);
    let mut n = 0;
    for tick in 0..40u32 {
        if tick.is_multiple_of(4) {
            for _ in 0..3 {
                let _ = pool.spawn(gib(n));
                n += 1;
            }
        }
        step(&mut pool, DT, physics, &index, &cfg, &mut events);
    }
    assert!(pool.ids().any(|id| pool.is_parked(id) == Some(true)));
    assert!(pool.ids().any(|id| pool.is_parked(id) == Some(false)));
    assert!(
        pool.ids()
            .any(|id| pool.spin(id).unwrap().map(f32::to_bits) == [0.0f32.to_bits(); 3])
    );
    assert!(pool.ids().any(|id| pool.targets(id) == Some(Some(1))));
    (pool, events)
}

#[test]
fn a_decoded_gib_pool_continues_identically() {
    let (a, ea) = churned_gibs();
    let mut bytes = Vec::new();
    a.encode(&mut bytes);
    ea.encode(&mut bytes);
    let mut cursor = bytes.as_slice();
    let b = ProjectilePool::decode(&mut cursor).unwrap();
    let eb = StepEvents::decode(&mut cursor).unwrap();
    assert!(cursor.is_empty());
    assert_eq!(a.digest(), b.digest());
    let mut again = Vec::new();
    b.encode(&mut again);
    eb.encode(&mut again);
    assert_eq!(bytes, again);
    for id in a.ids() {
        assert_eq!(
            a.orientation(id).unwrap().map(f32::to_bits),
            b.orientation(id).unwrap().map(f32::to_bits)
        );
        assert_eq!(
            a.spin(id).unwrap().map(f32::to_bits),
            b.spin(id).unwrap().map(f32::to_bits)
        );
        assert_eq!(a.targets(id), b.targets(id));
    }

    let world = gib_scene();
    let physics = physics_ref(&world);
    let mut cfg = config(physics);
    cfg.gravity = [0.0, -9.8, 0.0];
    let index = index_of(&[sphere(Handle::slot(500, 0), [8.0, 1.0, 0.0], 1.0, 1)]);
    let (mut a, mut b) = (a, b);
    let (mut ea, mut eb) = (StepEvents::default(), StepEvents::default());
    for k in 0..30u32 {
        step(&mut a, DT, physics, &index, &cfg, &mut ea);
        step(&mut b, DT, physics, &index, &cfg, &mut eb);
        assert_eq!(a.digest(), b.digest(), "tick {k}");
        assert_eq!(event_bits(&ea), event_bits(&eb), "tick {k}");
        if k.is_multiple_of(5) {
            let d = gib(k);
            assert_eq!(a.spawn(d), b.spawn(d), "next id, tick {k}");
        }
    }
}

#[test]
fn hostile_gib_bytes_are_errors() {
    let (pool, _) = churned_gibs();
    let mut pb = Vec::new();
    pool.encode(&mut pb);
    for n in 0..pb.len() {
        assert!(ProjectilePool::decode(&mut &pb[..n]).is_err(), "prefix {n}");
    }
    for i in 0..pb.len() {
        let mut m = pb.clone();
        m[i] ^= 0xff;
        let _ = ProjectilePool::decode(&mut m.as_slice());
    }
    // A one-slot legacy pool: the parked byte sits before generation (4) and alive (1).
    let mut one = ProjectilePool::with_capacity(1);
    one.spawn(ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]))
        .unwrap();
    let mut b = Vec::new();
    one.encode(&mut b);
    let at = b.len() - 6;
    assert_eq!(b[at], 0);
    for bad in [4u8, 5, 0x80, 0xff] {
        let mut m = b.clone();
        m[at] = bad;
        let err = ProjectilePool::decode(&mut m.as_slice()).err().unwrap();
        assert_eq!(err.reason(), "bad parked byte");
    }
    // An ext byte with its payload cut short, and a Settle tag with a truncated payload.
    let mut m = b.clone();
    m[at] = 2;
    assert!(ProjectilePool::decode(&mut m.as_slice()).is_err());
    let mut s = ProjectilePool::with_capacity(1);
    let mut d = ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]);
    d.response = Response::settle(0.4, 0.5, 0.5);
    s.spawn(d).unwrap();
    let mut b = Vec::new();
    s.encode(&mut b);
    assert!(ProjectilePool::decode(&mut b.as_slice()).is_ok());
    for cut in 1..16 {
        assert!(ProjectilePool::decode(&mut &b[..b.len() - cut]).is_err());
    }
}
