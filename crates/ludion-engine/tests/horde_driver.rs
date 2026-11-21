//! The physics-horde driver: the game-side systems that turn `examples/gallery/physics-horde.yaml`
//! into Serious Isaac's M2 physics shape (request 0020) — 500 `CharacterMotor`s on scripted
//! displacements, 120 eleven-part `SkinBone` ragdolls (the first 24 pinned) re-thrown on a timer,
//! and 5,000 pool projectiles sweeping the colliders.
//!
//! Every input is the tick and a sorted entity id, so all driver state is world state: a snapshot,
//! a restore and a replay need no custom component, and positions, impulses and shots carry no
//! transcendentals. `physics_horde.rs` gates the scene; the horde bench reuses [`build`] and
//! [`horde_builder`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Mesh, MeshSource, ProjectileConfig, Projectiles, SkinBone, Transform,
    TransformHierarchy, move_characters, projectiles_step, spawn_projectile,
    switch_skin_to_ragdoll,
};
use ludion_physics::{CharacterMotor, MotorError, PhysicsWorld};
use ludion_projectiles::{ProjectileDesc, Response};
use ludion_spatial::spatial3d;
use ludion_spatial::{Body, Collider, Joint, JointKind, Shape};

pub const PARTS: &str = "prefabs/mannequin-ragdoll.prefab.yaml";
pub const MOTORS: usize = 500;
pub const RAGDOLLS: usize = 120;
pub const PINNED: usize = 24;
pub const SHOTS: usize = 5_000;
/// Ticks between re-throws of a quarter of the unpinned groups.
pub const RETHROW_EVERY: u64 = 30;

/// Muzzle points inside the room, at 1.0–1.8 m.
const MUZZLES: [[f32; 3]; 4] = [
    [-14.0, 1.2, -6.0],
    [-6.0, 1.8, 7.0],
    [10.0, 1.5, -10.0],
    [14.0, 1.0, 9.0],
];

pub fn gallery_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

pub fn scene_path() -> PathBuf {
    gallery_root().join("physics-horde.yaml")
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A word to a float in `[lo, hi)`.
fn unit(word: u64, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * ((word >> 40) as f32 / (1u64 << 24) as f32)
}

/// Every motor, in id order, walks a few centimetres a tick; the direction flips every 60 ticks and
/// alternates by index. Tick-derived, no transcendentals.
pub fn motor_walk(world: &mut World, ctx: &SystemCtx) {
    let mut motors: Vec<EntityId> = world
        .query_ids::<(&CharacterMotor,)>()
        .map(|(id, _)| id)
        .collect();
    motors.sort();
    let phase = ctx.frame.as_u64() / 60;
    let moves: Vec<(EntityId, [f32; 3])> = motors
        .into_iter()
        .enumerate()
        .map(|(i, id)| {
            let sign = if (i as u64 + phase).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            (id, [0.04 * sign, -0.1, 0.0])
        })
        .collect();
    // `NoPhysicsWorld` on tick 0 is expected.
    let mut out = vec![Err(MotorError::NoPhysicsWorld); moves.len()];
    move_characters(world, &moves, &mut out);
}

/// The point every converged motor walks to: the centre of the motor half (x 2.2–25, z ±14.4) at
/// the motors' spawn height.
pub const CONVERGE_POINT: [f32; 3] = [13.6, 0.8, 0.0];

/// Every motor, in id order, steps 0.1 m a tick toward [`CONVERGE_POINT`] (capped at the remaining
/// distance), so the crowd piles up in one spot: the motors ignore each other, and their query
/// bounds all overlap. Tick-derived, no transcendentals.
pub fn motor_converge(world: &mut World, _ctx: &SystemCtx) {
    let mut motors: Vec<(EntityId, [f32; 3])> = world
        .query_ids::<(&CharacterMotor, &Transform)>()
        .map(|(id, (_, at))| (id, at.translation))
        .collect();
    motors.sort_by_key(|(id, _)| *id);
    let moves: Vec<(EntityId, [f32; 3])> = motors
        .into_iter()
        .map(|(id, at)| {
            let (dx, dz) = (CONVERGE_POINT[0] - at[0], CONVERGE_POINT[2] - at[2]);
            let dist = ludion_core::math::sqrt(dx * dx + dz * dz);
            let step = dist.min(0.1);
            let (sx, sz) = if dist > 0.0 {
                (dx / dist * step, dz / dist * step)
            } else {
                (0.0, 0.0)
            };
            (id, [sx, -0.1, sz])
        })
        .collect();
    let mut out = vec![Err(MotorError::NoPhysicsWorld); moves.len()];
    move_characters(world, &moves, &mut out);
}

/// The skins in id order, the scene's authoring order.
fn skins(world: &mut World) -> Vec<EntityId> {
    let mut skins: Vec<EntityId> = world
        .query_ids::<(&Mesh,)>()
        .filter(|(_, (mesh,))| matches!(mesh.shape, MeshSource::Character(_)))
        .map(|(id, _)| id)
        .collect();
    skins.sort();
    skins
}

/// The parts of each skin that has switched, parts in id order.
fn parts_by_skin(world: &mut World) -> BTreeMap<EntityId, Vec<EntityId>> {
    let mut map: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
    for (id, (bone,)) in world.query_ids::<(&SkinBone,)>() {
        map.entry(bone.skin).or_default().push(id);
    }
    for parts in map.values_mut() {
        parts.sort();
    }
    map
}

/// Joins the torso to a static anchor spawned at its own pose, so the joint frames coincide.
fn pin_torso(world: &mut World, torso: EntityId) {
    let Ok(Some(&at)) = world.get::<Transform>(torso) else {
        return;
    };
    let anchor = world.spawn();
    let _ = world.insert(anchor, at);
    let _ = world.insert(
        anchor,
        Collider {
            shape: Shape::Sphere { radius: 0.05 },
            body: Body::new_static(),
            layer: None,
            sensor: false,
        },
    );
    let carrier = world.spawn();
    let _ = world.insert(
        carrier,
        Joint {
            a: anchor,
            b: torso,
            kind: JointKind::Ball { limits: None },
            anchor: [0.0; 3],
            break_impulse: None,
            collide: false,
            zero: None,
        },
    );
}

/// Tick 0 switches every skin and pins the first [`PINNED`]; every [`RETHROW_EVERY`] ticks a quarter
/// of the unpinned groups get one impulse per part, enough to lift a 0.05–0.09 kg part clear.
pub fn horde_ragdolls(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.game_frame.as_u64();
    if frame == 0 {
        for (n, skin) in skins(world).into_iter().enumerate() {
            let parts = switch_skin_to_ragdoll(world, skin, PARTS).expect("the mannequin switches");
            if n < PINNED {
                pin_torso(world, parts[1]);
            }
        }
        return;
    }
    if !frame.is_multiple_of(RETHROW_EVERY) {
        return;
    }
    let round = frame / RETHROW_EVERY;
    let skins = skins(world);
    let groups = parts_by_skin(world);
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    for (k, skin) in skins.iter().skip(PINNED).enumerate() {
        if !(k as u64 + round).is_multiple_of(4) {
            continue;
        }
        for (j, part) in groups.get(skin).into_iter().flatten().enumerate() {
            let sx = if (k + j) % 2 == 0 { 1.0 } else { -1.0 };
            let sz = if (k + 2 * j) % 3 == 0 { 1.0 } else { -1.0 };
            let _ = physics.apply_impulse(*part, [0.06 * sx, 0.25, 0.06 * sz]);
        }
    }
}

/// Tops the pool up to capacity from the four muzzles: about 40 m/s, mostly horizontal, from a
/// splitmix of `(tick, shot)`; reflecting sweeps that hit statics, motors and parts alike.
pub fn horde_shots(world: &mut World, ctx: &SystemCtx) {
    let live = world
        .resource::<Projectiles>()
        .map_or(SHOTS, |p| p.pool().len());
    let tick = ctx.frame.as_u64();
    for n in 0..SHOTS.saturating_sub(live) as u64 {
        let i = tick * SHOTS as u64 + n;
        let h = |k: u64| mix(i.wrapping_mul(8).wrapping_add(k));
        let muzzle = MUZZLES[(i % MUZZLES.len() as u64) as usize];
        let velocity = [
            unit(h(0), -1.0, 1.0) * 40.0,
            unit(h(1), -0.08, 0.15) * 40.0,
            unit(h(2), -1.0, 1.0) * 40.0,
        ];
        let mut desc = ProjectileDesc::new(muzzle, velocity);
        desc.radius = 0.05;
        desc.lifetime = 1.5;
        desc.response = Response::Reflect {
            restitution: 1.0,
            max_bounces: 3,
        };
        spawn_projectile(world, desc);
    }
}

/// The horde engine builder: the projectile pool and the five systems, in order.
pub fn horde_builder() -> EngineBuilder {
    builder_with(motor_walk)
}

/// [`horde_builder`] with the motors converging on one point; the system keeps the name
/// `"motor_walk"` so the bench's row list reads it.
pub fn converge_builder() -> EngineBuilder {
    builder_with(motor_converge)
}

fn builder_with(walk: fn(&mut World, &SystemCtx)) -> EngineBuilder {
    Engine::builder()
        .project_root(gallery_root())
        .projectiles(ProjectileConfig {
            capacity: SHOTS as u32,
            ..ProjectileConfig::default()
        })
        .system("motor_walk", walk)
        .system("horde_ragdolls", horde_ragdolls)
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("horde_shots", horde_shots)
        .system("projectiles_step", projectiles_step)
}

/// A fresh horde engine with the committed scene loaded.
pub fn build() -> Engine {
    load(horde_builder())
}

/// [`build`] with the converging motors.
pub fn build_converged() -> Engine {
    load(converge_builder())
}

fn load(builder: EngineBuilder) -> Engine {
    let mut engine = builder.build().expect("the horde engine builds");
    engine
        .load_inline_scene(&std::fs::read_to_string(scene_path()).expect("the committed scene"))
        .expect("the committed scene loads");
    engine
}
