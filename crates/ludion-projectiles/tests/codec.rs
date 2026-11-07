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
