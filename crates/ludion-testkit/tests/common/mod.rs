//! The gallery scenes' game halves, shared by their CPU proofs (`character_cycle.rs`,
//! `camera_rig.rs`), `gallery.rs` (the driven goldens) and `examples/` (the runnable hosts).
//!
//! **character-cycle** — the engine plays clips; this system chooses them: the mannequin idles, runs,
//! jumps, runs, spins, runs and returns to idle, crossfading each switch and leaving the one-shots on
//! their `end` events (`ludion docs character-animation`).
//!
//! **ragdoll-skin-switch** — [`switch_skin`] turns the running mannequin into its `SkinBone` ragdoll at
//! [`SWITCH_SKIN_TICK`]; the parts land at the rest pose (`ludion docs physics-joints`).
//!
//! **ragdoll-retire** — [`retire_settled`] drops a settled ragdoll's colliders, joints and velocities
//! so its bodies leave the physics world while the skin stays posed (`ludion docs physics-joints`
//! "Ragdoll budget").
//!
//! **camera-rig** — [`drive_rig`] swaps the rail camera to a side-on rig at [`SWITCH_TICK`] (a timed
//! blend), cuts back with `snap_rig` at [`SNAP_TICK`] and pulses trauma at [`SHAKE_TICK`]
//! (`ludion docs camera`).

#![allow(dead_code)]

use ludion::prelude::{Body, Collider, Joint, JointMotor, PhysicsWorld, Velocity};
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::SkinBone;
use ludion_engine::{
    Animation, AnimationEvents, Engine, EngineBuilder, Mesh, PrefabInstance, Sprite,
};
use ludion_engine::{
    BlendIn, CameraRig, RigConfig, RigMode, TrackRig, add_trauma, animation_events, crossfade,
    snap_rig, switch_skin_to_ragdoll,
};

/// Ticks the mannequin idles before its first run.
pub const IDLE_TICKS: u64 = 60;
/// Ticks each run lasts: two strides at 60 Hz (the run clip is 0.8 s at overlay speed 1.2).
pub const RUN_TICKS: u64 = 80;
/// Seconds of the idle → run crossfade.
pub const RUN_IN_FADE: f32 = 0.25;

/// What ends a clip's turn.
enum Leave {
    /// This many ticks after the clip started.
    After(u64),
    /// The clip's implicit `end` event (a one-shot finishing).
    OnEnd,
}

/// `(clip, entered from, leaves on, next clip, fade seconds)`; a `None` origin matches any.
/// Every entry into `run` is a crossfade, and `crossfade` keeps `Animation.from` after the fade
/// ends, so `run` reads its predecessor there and the cycle needs no state of its own. A fade of
/// `0.0` is a hard cut.
const CYCLE: [(&str, Option<&str>, Leave, &str, f32); 6] = [
    ("idle", None, Leave::After(IDLE_TICKS), "run", RUN_IN_FADE),
    ("run", Some("idle"), Leave::After(RUN_TICKS), "jump", 0.1),
    ("run", Some("jump"), Leave::After(RUN_TICKS), "spin", 0.0),
    ("run", Some("spin"), Leave::After(RUN_TICKS), "idle", 0.3),
    ("jump", None, Leave::OnEnd, "run", 0.15),
    ("spin", None, Leave::OnEnd, "run", 0.15),
];

/// Advances every `Animation` along [`CYCLE`], writing only on a switch and stamping with
/// `ctx.game_frame`, so the component stays quiet in the hash between switches.
pub fn cycle_character(world: &mut World, ctx: &SystemCtx) {
    let events = world
        .resource::<AnimationEvents>()
        .cloned()
        .unwrap_or_default();
    for (entity, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        let elapsed = ctx
            .game_frame
            .as_u64()
            .saturating_sub(animation.started.as_u64());
        let ended = events.fired(entity, &animation.clip, "end");
        let next = CYCLE
            .iter()
            .find(|(clip, from, _, _, _)| {
                *clip == animation.clip && from.is_none_or(|from| from == animation.from)
            })
            .and_then(|(_, _, leave, next, fade)| {
                let due = match leave {
                    Leave::After(ticks) => elapsed >= *ticks,
                    Leave::OnEnd => ended,
                };
                due.then_some((*next, *fade))
            });
        if let Some((clip, fade)) = next {
            crossfade(animation, clip, fade, ctx.game_frame);
        }
    }
}

/// The scene's engine: `animation_events` first, so `cycle_character` reads this tick's events.
pub fn cycle_builder() -> EngineBuilder {
    Engine::builder()
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&Animation, &Sprite)>()
        .query::<(&Animation, &PrefabInstance)>()
        .system("cycle_character", cycle_character)
        .query::<(&mut Animation,)>()
}

/// The tick the running mannequin switches to its ragdoll.
pub const SWITCH_SKIN_TICK: u64 = 40;
/// The parts prefab the switch spawns, relative to the gallery root.
pub const SKIN_SWITCH_PARTS: &str = "prefabs/mannequin-ragdoll.prefab.yaml";

/// Switches the scene's skin (document position 3) to its ragdoll once, on [`SWITCH_SKIN_TICK`]; the
/// equality keeps a world restored after the switch from firing it again.
pub fn switch_skin(world: &mut World, ctx: &SystemCtx) {
    if ctx.game_frame.as_u64() == SWITCH_SKIN_TICK {
        switch_skin_to_ragdoll(world, EntityId::new(3, 0), SKIN_SWITCH_PARTS)
            .expect("the mannequin switches to its ragdoll");
    }
}

/// The ragdoll-skin-switch engine: just the switching system.
pub fn skin_switch_builder() -> EngineBuilder {
    Engine::builder().system("switch_skin", switch_skin)
}

/// Every entity whose `SkinBone` names `skin`, in ascending id order.
pub fn skin_parts(world: &mut World, skin: EntityId) -> Vec<EntityId> {
    let mut parts: Vec<EntityId> = world
        .query_ids::<(&SkinBone,)>()
        .filter(|(_, (bone,))| bone.skin == skin)
        .map(|(id, _)| id)
        .collect();
    parts.sort();
    parts
}

/// Whether `parts` is non-empty and every part's body is asleep (false without a `PhysicsWorld` or
/// when a part has no body).
pub fn group_settled(world: &World, parts: &[EntityId]) -> bool {
    let Some(physics) = world.resource::<PhysicsWorld>() else {
        return false;
    };
    !parts.is_empty()
        && parts
            .iter()
            .all(|&part| physics.is_sleeping(part) == Some(true))
}

/// Whether a `Joint` ties a part of `parts` to a `Body::Static` collider outside the group.
pub fn group_pinned(world: &mut World, parts: &[EntityId]) -> bool {
    let joints: Vec<Joint> = world
        .query_ids::<(&Joint,)>()
        .map(|(_, (joint,))| joint.clone())
        .collect();
    joints.iter().any(|joint| {
        let pinned_to = |part: EntityId, other: EntityId| {
            parts.contains(&part)
                && !parts.contains(&other)
                && matches!(
                    world.get::<Collider>(other),
                    Ok(Some(Collider {
                        body: Body::Static(_),
                        ..
                    }))
                )
        };
        pinned_to(joint.a, joint.b) || pinned_to(joint.b, joint.a)
    })
}

/// What [`retire_group`] took off one part: the input of [`readmit_group`].
pub struct RetiredPart {
    pub id: EntityId,
    pub collider: Collider,
    pub joint: Option<Joint>,
}

/// Retires `parts`: removes `Collider`, `Joint`, `JointMotor` and `Velocity` from each (keeping
/// `Transform` and `SkinBone`) and returns what it took off.
pub fn retire_group(world: &mut World, parts: &[EntityId]) -> Vec<RetiredPart> {
    let mut retired = Vec::new();
    for &id in parts {
        let joint = world.remove::<Joint>(id).expect("a live part");
        let _ = world.remove::<JointMotor>(id);
        let _ = world.remove::<Velocity>(id);
        if let Some(collider) = world.remove::<Collider>(id).expect("a live part") {
            retired.push(RetiredPart {
                id,
                collider,
                joint,
            });
        }
    }
    retired
}

/// Re-inserts the `Collider` and `Joint` that [`retire_group`] removed.
pub fn readmit_group(world: &mut World, retired: &[RetiredPart]) {
    for part in retired {
        world
            .insert(part.id, part.collider.clone())
            .expect("a live part");
        if let Some(joint) = &part.joint {
            world.insert(part.id, joint.clone()).expect("a live part");
        }
    }
}

/// Retires the switched skin's parts once they have all fallen asleep.
pub fn retire_settled(world: &mut World, _ctx: &SystemCtx) {
    let parts: Vec<EntityId> = skin_parts(world, EntityId::new(3, 0))
        .into_iter()
        .filter(|&part| world.has::<Collider>(part))
        .collect();
    if group_settled(world, &parts) {
        retire_group(world, &parts);
    }
}

/// The ragdoll-retire engine: the switch, then the settle-driven retire.
pub fn retire_builder() -> EngineBuilder {
    Engine::builder()
        .system("switch_skin", switch_skin)
        .system("retire_settled", retire_settled)
}

/// The tick the rail camera blends to the side-on rig.
pub const SWITCH_TICK: u64 = 60;
/// The tick the camera cuts back to the behind rig.
pub const SNAP_TICK: u64 = 180;
/// The tick of the trauma pulse.
pub const SHAKE_TICK: u64 = 220;

/// The track rig with the eye `offset` and a one-second blend in.
fn rig_at(offset: [f32; 3]) -> RigConfig {
    RigConfig {
        mode: RigMode::Track(TrackRig {
            path: "paths/camera-track.path.yaml".into(),
            offset,
            ..TrackRig::default()
        }),
        blend_in: BlendIn {
            seconds: 1.0,
            ..BlendIn::default()
        },
        ..RigConfig::default()
    }
}

/// The side-on rig: the eye five metres to the rail's right.
pub fn side_rig() -> RigConfig {
    rig_at([5.0, 1.5, 0.0])
}

/// The scene's authored rig: behind and above the hero.
pub fn behind_rig() -> RigConfig {
    rig_at([0.0, 2.2, 4.5])
}

/// Writes the rig on `SWITCH_TICK` (a blend), `SNAP_TICK` (a cut) and `SHAKE_TICK` (trauma), keyed
/// on `ctx.game_frame`. Game systems run before `camera_rig`, so a write at tick N is seen at N.
pub fn drive_rig(world: &mut World, ctx: &SystemCtx) {
    let tick = ctx.game_frame.as_u64();
    if ![SWITCH_TICK, SNAP_TICK, SHAKE_TICK].contains(&tick) {
        return;
    }
    for (_, (rig,)) in world.query_ids::<(&mut CameraRig,)>() {
        match tick {
            SWITCH_TICK => rig.rig = side_rig(),
            SNAP_TICK => {
                rig.rig = behind_rig();
                snap_rig(rig);
            }
            _ => add_trauma(rig, 0.8),
        }
    }
}

/// The camera-rig scene's engine.
pub fn rig_builder() -> EngineBuilder {
    Engine::builder()
        .system("drive_rig", drive_rig)
        .query::<(&mut CameraRig,)>()
}

/// **projectile-muzzle** — [`fire_shots`] fires one shot per tick for [`SHOT_TICKS`] ticks from the
/// eye along -Z, alternating a billboard tracer (tag 0) and a stretched sphere (tag 1); the scene's
/// `muzzle` entity makes them draw from the gun and converge onto that line
/// (`ludion docs viewmodel`).
pub const SHOT_TICKS: u64 = 10;
/// The tick the muzzle golden is rendered at: the last shots are still near the gun.
pub const SHOT_GOLDEN_TICK: u64 = 12;

pub fn fire_shots(world: &mut World, ctx: &SystemCtx) {
    let tick = ctx.frame.as_u64();
    if tick >= SHOT_TICKS {
        return;
    }
    let mut desc =
        ludion::prelude::projectiles::ProjectileDesc::new([0.0, 1.6, 0.0], [0.0, 0.0, -25.0]);
    desc.tag = (tick % 2) as u32;
    ludion_engine::spawn_projectile(world, desc);
}

/// The muzzle-convergence scene's engine: a pool, the two looks and the muzzle name.
pub fn projectile_builder() -> EngineBuilder {
    use ludion_engine::{ProjectileLook, ProjectileVisual, ProjectileVisuals};
    let mut visuals = ProjectileVisuals {
        muzzle: Some("muzzle".to_string()),
        ..ProjectileVisuals::default()
    };
    visuals.tags.insert(
        0,
        ProjectileVisual {
            look: ProjectileLook::Billboard { softness: 0.7 },
            size: 0.06,
            stretch: 0.02,
            color: [1.0, 0.55, 0.15, 1.0],
            emissive: 5.0,
            additive: true,
            from_muzzle: true,
            medium: 1.0,
            ..ProjectileVisual::default()
        },
    );
    visuals.tags.insert(
        1,
        ProjectileVisual {
            look: ProjectileLook::Sphere,
            size: 0.05,
            stretch: 0.02,
            color: [0.3, 0.8, 1.0, 1.0],
            emissive: 4.0,
            additive: false,
            from_muzzle: true,
            medium: 1.0,
            ..ProjectileVisual::default()
        },
    );
    Engine::builder()
        .projectiles(ludion_engine::ProjectileConfig::default())
        .projectile_visuals(visuals)
        .system(
            "spatial3d_rebuild",
            ludion::prelude::spatial3d::rebuild::<
                ludion_engine::Transform,
                ludion_engine::TransformHierarchy,
            >,
        )
        .system("fire_shots", fire_shots)
        .system("projectiles_step", ludion_engine::projectiles_step)
}

/// **projectile-gibs** — [`throw_gibs`] throws [`GIB_COUNT`] debris shots (ten ticks of a hundred)
/// from above the middle of a walled room; they bounce, tumble and settle on its floor.
pub const GIB_COUNT: u64 = 1000;
/// Ticks of throwing.
pub const GIB_THROW_TICKS: u64 = 10;
/// The tick the gibs golden is rendered at: every gib has settled, the fading tag is nearly gone.
pub const GIB_GOLDEN_TICK: u64 = 150;
/// Seconds the longest-lived gib lives.
pub const GIB_LIFETIME: f32 = 6.0;

/// A splitmix64 step: an index to a well-mixed word, so the throw needs no RNG resource.
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

pub fn throw_gibs(world: &mut World, ctx: &SystemCtx) {
    let tick = ctx.frame.as_u64();
    if tick >= GIB_THROW_TICKS {
        return;
    }
    for n in 0..GIB_COUNT / GIB_THROW_TICKS {
        let i = tick * (GIB_COUNT / GIB_THROW_TICKS) + n;
        let h = |k: u64| mix(i.wrapping_mul(16).wrapping_add(k));
        let (sin, cos) = ludion_core::math::sin_cos(unit(h(0), 0.0, std::f32::consts::TAU));
        let speed = unit(h(1), 0.5, 3.5);
        let mut desc = ludion::prelude::projectiles::ProjectileDesc::new(
            [0.0, 2.5, 0.0],
            [cos * speed, unit(h(2), 3.0, 6.0), sin * speed],
        );
        desc.gravity_scale = 1.0;
        desc.lifetime = if i % 3 == 2 { 3.0 } else { GIB_LIFETIME };
        desc.tag = (i % 3) as u32;
        desc.response = ludion::prelude::projectiles::Response::settle(0.35, 0.4, 1.0);
        desc.targets = Some(0);
        desc.spin = [
            unit(h(3), -12.0, 12.0),
            unit(h(4), -12.0, 12.0),
            unit(h(5), -12.0, 12.0),
        ];
        desc.orientation = [
            unit(h(6), -1.0, 1.0),
            unit(h(7), -1.0, 1.0),
            unit(h(8), -1.0, 1.0),
            unit(h(9), 0.1, 1.0),
        ];
        ludion_engine::spawn_projectile(world, desc);
    }
}

/// The gibs scene's engine: a pool of 1,024, three looks (asset mesh, oriented cube, a fading
/// asset mesh), all shadow-casting.
pub fn gib_builder() -> EngineBuilder {
    use ludion_engine::ProjectileLook;
    let crystal = || ProjectileLook::Mesh {
        asset: "meshes/crystal.mesh.yaml".to_string(),
    };
    gib_builder_with(&[
        (
            0,
            crystal(),
            0.35,
            [1.0, 1.4, 0.8],
            [0.9, 0.55, 0.25, 1.0],
            0.0,
        ),
        (
            1,
            ProjectileLook::Cube,
            0.2,
            [1.6, 0.5, 1.0],
            [0.25, 0.7, 0.75, 1.0],
            0.0,
        ),
        (
            2,
            crystal(),
            0.3,
            [1.0, 1.0, 1.0],
            [0.6, 0.35, 0.85, 1.0],
            1.5,
        ),
    ])
}

/// The gibs scene with the beacon voxel model on all three tags: tag 0 untinted (the palette
/// exactly), tag 1 tinted and stretched, tag 2 fading.
pub fn voxel_gib_builder() -> EngineBuilder {
    use ludion_engine::ProjectileLook;
    let beacon = || ProjectileLook::Voxel {
        model: "voxels/beacon.voxel.yaml".to_string(),
    };
    gib_builder_with(&[
        (0, beacon(), 0.05, [1.0; 3], [1.0; 4], 0.0),
        (
            1,
            beacon(),
            0.05,
            [1.4, 0.6, 1.0],
            [0.9, 0.55, 0.25, 1.0],
            0.0,
        ),
        (2, beacon(), 0.05, [1.0; 3], [1.0; 4], 1.5),
    ])
}

/// Visual rows: `(tag, look, size, scale, color, fade_out)`; every tag casts a shadow.
type GibRow = (
    u32,
    ludion_engine::ProjectileLook,
    f32,
    [f32; 3],
    [f32; 4],
    f32,
);

fn gib_builder_with(rows: &[GibRow]) -> EngineBuilder {
    use ludion_engine::{ProjectileVisual, ProjectileVisuals};
    let mut visuals = ProjectileVisuals::default();
    for (tag, look, size, scale, color, fade_out) in rows.iter().cloned() {
        visuals.tags.insert(
            tag,
            ProjectileVisual {
                look,
                size,
                scale,
                color,
                emissive: 0.0,
                additive: false,
                cast_shadows: true,
                fade_out,
                ..ProjectileVisual::default()
            },
        );
    }
    Engine::builder()
        .projectiles(ludion_engine::ProjectileConfig::default())
        .projectile_visuals(visuals)
        .system(
            "spatial3d_rebuild",
            ludion::prelude::spatial3d::rebuild::<
                ludion_engine::Transform,
                ludion_engine::TransformHierarchy,
            >,
        )
        .system("throw_gibs", throw_gibs)
        .system("projectiles_step", ludion_engine::projectiles_step)
}
