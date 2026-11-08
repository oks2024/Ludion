//! The engine hosts the projectile pool: install, hash, snapshot and refusal paths.
//!
//! A turret fires a mixed volley (Despawn, Reflect, Pierce, Stop) every five ticks down four lanes
//! of enemies toward a static wall. `pre_reader` runs before the rebuild and the step, so it
//! observes the carried previous-tick events and index; the carried state is therefore in the
//! hashed `Ledger`.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, EngineBuilder, ProjectileConfig, Projectiles, Transform, TransformHierarchy,
    projectiles_step, spawn_projectile,
};
use ludion_projectiles::{ProjectileDesc, Response};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::spatial3d::{self, Filter, Handle, HandleKind, Index3d, Neighbor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the readers have seen; a component so it reaches `WorldHash`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Ledger {
    pre_hits: u64,
    pre_nearest: u64,
    hits: u64,
}

const SCENE: &str = "\
format: 1
seed: 5
physics:
  space: free
entities:
  - name: ledger
    components:
      Ledger:
        pre_hits: 0
        pre_nearest: 0
        hits: 0
  - name: wall
    components:
      Transform:
        translation:
          - 20.0
          - 0.0
          - 4.5
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 6.0
              - 8.0
        body: static
  - name: enemy0
    components:
      Transform:
        translation:
          - 8.0
          - 0.0
          - 0.0
      SpatialVolume: {}
  - name: enemy1
    components:
      Transform:
        translation:
          - 8.0
          - 0.0
          - 3.0
      SpatialVolume: {}
  - name: enemy2
    components:
      Transform:
        translation:
          - 12.0
          - 0.0
          - 6.0
      SpatialVolume: {}
  - name: enemy3
    components:
      Transform:
        translation:
          - 12.0
          - 0.0
          - 9.0
      SpatialVolume: {}
";

fn ledger(world: &World) -> Option<EntityId> {
    world
        .entities()
        .find(|&id| matches!(world.get::<Ledger>(id), Ok(Some(_))))
}

fn pre_reader(world: &mut World, _ctx: &SystemCtx) {
    let hits = world
        .resource::<Projectiles>()
        .map_or(0, |p| p.events().hits.len() as u64);
    let nearest = world.resource::<Index3d>().map_or(0, |index| {
        let mut out: Vec<Neighbor> = Vec::new();
        index.k_nearest([0.0; 3], 2, 100.0, &Filter::new(u32::MAX), &mut out);
        out.iter().map(|n| u64::from(n.handle.index) + 1).sum()
    });
    if let Some(id) = ledger(world)
        && let Ok(Some(ledger)) = world.get_mut::<Ledger>(id)
    {
        ledger.pre_hits += hits;
        ledger.pre_nearest += nearest;
    }
}

fn volley(frame: u64) -> Vec<ProjectileDesc> {
    let z = 3.0 * (frame / 5 % 4) as f32;
    let mut despawn = ProjectileDesc::new([0.0, 0.0, z], [120.0, 0.0, 0.0]);
    despawn.lifetime = 0.25;
    despawn.tag = 1;
    let mut reflect = ProjectileDesc::new([0.0, 3.0, 4.0], [120.0, 0.0, 0.0]);
    reflect.response = Response::Reflect {
        restitution: 0.8,
        max_bounces: 2,
    };
    reflect.gravity_scale = 1.0;
    reflect.lifetime = 0.3;
    reflect.tag = 2;
    let mut pierce = ProjectileDesc::new([0.0, 0.0, z], [120.0, 0.0, 0.0]);
    pierce.response = Response::Pierce { max: 2 };
    pierce.lifetime = 0.25;
    pierce.tag = 3;
    let mut stop = ProjectileDesc::new([0.0, 1.5, 5.0], [120.0, 0.0, 0.0]);
    stop.response = Response::Stop;
    stop.lifetime = 0.2;
    stop.tag = 4;
    vec![despawn, reflect, pierce, stop]
}

fn fire(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64().is_multiple_of(5) {
        for desc in volley(ctx.frame.as_u64()) {
            spawn_projectile(world, desc);
        }
    }
}

fn damage(world: &mut World, _ctx: &SystemCtx) {
    let Some(projectiles) = world.resource::<Projectiles>() else {
        return;
    };
    let hits: Vec<Handle> = projectiles.events().hits.iter().map(|h| h.handle).collect();
    let count = hits.len() as u64;
    for handle in hits {
        if handle.kind == HandleKind::Entity {
            let _ = world.despawn(EntityId::new(handle.index, handle.generation));
        }
    }
    if let Some(id) = ledger(world)
        && let Ok(Some(ledger)) = world.get_mut::<Ledger>(id)
    {
        ledger.hits += count;
    }
}

fn base() -> EngineBuilder {
    Engine::builder().components(|reg| {
        reg.register::<Ledger>("Ledger")?;
        Ok(())
    })
}

fn scheduled(builder: EngineBuilder, step: bool) -> EngineBuilder {
    let builder = builder
        .system("pre_reader", pre_reader)
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("fire", fire);
    let builder = if step {
        builder.system("projectiles_step", projectiles_step)
    } else {
        builder
    };
    builder.system("damage", damage)
}

fn config() -> ProjectileConfig {
    ProjectileConfig {
        capacity: 16,
        ..ProjectileConfig::default()
    }
}

fn engine_with(config: Option<ProjectileConfig>, step: bool) -> Engine {
    let builder = match config {
        Some(config) => base().projectiles(config),
        None => base(),
    };
    let mut engine = scheduled(builder, step).build().expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn engine() -> Engine {
    engine_with(Some(config()), true)
}

fn trace(engine: &mut Engine, n: usize) -> Vec<u64> {
    (0..n)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) -> Result<(), String> {
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .map(|_| ())
        .map_err(|d| d.code.as_str().to_string())
}

fn probe_next_id(engine: &mut Engine) -> ludion_projectiles::ProjectileId {
    spawn_projectile(
        engine.world_mut(),
        ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]),
    )
    .expect("a free slot")
}

#[test]
fn a_mid_flight_restore_continues_bit_identically() {
    let mut uninterrupted = engine();
    let reference = trace(&mut uninterrupted, 40);

    let mut first = engine();
    trace(&mut first, 20);
    let pool = first.world().resource::<Projectiles>().unwrap();
    assert!(!pool.pool().is_empty(), "shots are in flight");
    assert!(
        pool.pool().ids().any(|id| id.generation > 0),
        "slots were recycled"
    );
    assert!(first.world().resource::<Index3d>().is_some());
    let snapshot = snapshot_of(&mut first);
    assert!(snapshot.get("projectiles").is_some());
    assert!(snapshot.get("spatial_index_3d").is_some());

    let mut resumed = engine();
    restore(&mut resumed, snapshot).unwrap();
    let tail = trace(&mut resumed, 20);
    assert_eq!(tail, reference[20..], "ticks 21..40");

    let ledger = |e: &Engine| {
        let world = e.world();
        let id = ledger(world).unwrap();
        *world.get::<Ledger>(id).unwrap().unwrap()
    };
    let end = ledger(&resumed);
    assert!(
        end.hits > 0 && end.pre_hits > 0,
        "the run saw hits: {end:?}"
    );
    assert_eq!(end, ledger(&uninterrupted));
    assert_eq!(
        probe_next_id(&mut resumed),
        probe_next_id(&mut uninterrupted)
    );
}

/// A mid-flight snapshot from the default build (capacity 16, default gravity).
fn mid_flight_snapshot() -> (Value, usize) {
    let mut first = engine();
    trace(&mut first, 20);
    let high = first
        .world()
        .resource::<Projectiles>()
        .unwrap()
        .pool()
        .high_water();
    assert!(high > 0);
    (snapshot_of(&mut first), high)
}

#[test]
fn a_restore_takes_the_builders_config() {
    let (snapshot, high) = mid_flight_snapshot();
    let bigger = ProjectileConfig {
        capacity: 64,
        gravity: [0.0, -1.0, 0.0],
        ..ProjectileConfig::default()
    };
    let mut resumed = engine_with(Some(bigger.clone()), true);
    restore(&mut resumed, snapshot).unwrap();
    let res = resumed.world().resource::<Projectiles>().unwrap();
    assert_eq!(res.config(), &bigger);
    assert_eq!(res.pool().capacity(), 64);
    assert_eq!(res.pool().high_water(), high);
    assert!(!res.pool().is_empty(), "live shots kept");
    // The restored run follows the new gravity, not the snapshot's.
    let mut same = engine();
    let (snapshot, _) = mid_flight_snapshot();
    restore(&mut same, snapshot).unwrap();
    assert_ne!(trace(&mut resumed, 10), trace(&mut same, 10));
}

#[test]
fn a_restore_into_a_build_without_a_pool_drops_it() {
    let (snapshot, _) = mid_flight_snapshot();
    let mut bare = engine_with(None, true);
    restore(&mut bare, snapshot).unwrap();
    assert!(bare.world().resource::<Projectiles>().is_none());
}

#[test]
fn a_capacity_below_the_snapshots_high_water_starts_a_fresh_pool() {
    let (snapshot, high) = mid_flight_snapshot();
    let small = ProjectileConfig {
        capacity: (high - 1) as u32,
        ..ProjectileConfig::default()
    };
    let mut resumed = engine_with(Some(small), true);
    restore(&mut resumed, snapshot).expect("the rest of the restore proceeds");
    let res = resumed.world().resource::<Projectiles>().unwrap();
    assert!(res.pool().is_empty());
    assert_eq!(res.pool().capacity(), high - 1);
    let reported: Vec<_> = resumed
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::PROJECTILE_RESTORE_CAPACITY)
        .collect();
    assert_eq!(reported.len(), 1, "{reported:?}");
}

#[test]
fn no_pool_means_no_bytes() {
    // Both games schedule `fire`; the shots are refused (no pool), so neither world holds a pool.
    let mut with_step = engine_with(None, true);
    let mut without_step = engine_with(None, false);
    assert_eq!(trace(&mut with_step, 60), trace(&mut without_step, 60));
    let snapshot = snapshot_of(&mut with_step);
    assert!(snapshot.get("projectiles").is_none());
    assert!(snapshot_of(&mut without_step).get("projectiles").is_none());
    assert!(
        with_step
            .world()
            .resource::<Diagnostics>()
            .unwrap()
            .reported(code::PROJECTILE_SPAWN_REFUSED)
            .any(|key| key == "no-pool")
    );

    // A game without the rebuild files no 3D index.
    let mut bare = Engine::builder().build().unwrap();
    bare.load_inline_scene("format: 1\nseed: 1\n").unwrap();
    bare.advance_live();
    assert!(snapshot_of(&mut bare).get("spatial_index_3d").is_none());
}

#[test]
fn a_live_pool_changes_the_hash() {
    let mut pooled = engine();
    let mut pool_free = engine_with(None, true);
    let (a, b) = (trace(&mut pooled, 8), trace(&mut pool_free, 8));
    assert_ne!(a[7], b[7], "a live shot is hashed");
}

#[test]
fn a_bad_config_is_refused_at_build() {
    let error = base()
        .projectiles(ProjectileConfig {
            capacity: 0,
            ..ProjectileConfig::default()
        })
        .build()
        .err()
        .expect("capacity 0 is refused");
    assert_eq!(error.code, code::PROJECTILE_CONFIG_INVALID);
}

#[test]
fn an_unknown_ignore_layer_reports_once_and_freezes_the_pool() {
    let mut engine = engine_with(
        Some(ProjectileConfig {
            ignore_layers: vec!["nope".to_string()],
            ..config()
        }),
        true,
    );
    engine.advance_live();
    let before = engine
        .world()
        .resource::<Projectiles>()
        .unwrap()
        .pool()
        .digest();
    for _ in 0..4 {
        engine.advance_live();
    }
    let projectiles = engine.world().resource::<Projectiles>().unwrap();
    let keys: Vec<&str> = engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::PROJECTILE_CONFIG_INVALID)
        .collect();
    assert_eq!(keys, ["ignore_layers"]);
    // The first volley (tick 0) is in the pool and never moved or aged.
    assert_eq!(projectiles.pool().len(), 4);
    assert_eq!(projectiles.pool().digest(), before);
}

#[test]
fn a_nan_velocity_and_a_full_pool_are_refused_with_l4026() {
    let mut engine = engine();
    engine.advance_live();
    let world = engine.world_mut();
    let nan = ProjectileDesc::new([0.0; 3], [f32::NAN, 0.0, 0.0]);
    assert!(spawn_projectile(world, nan).is_none());
    let mut spawned = 0;
    while spawn_projectile(world, ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0])).is_some() {
        spawned += 1;
        assert!(spawned <= 16);
    }
    let mut keys: Vec<String> = world
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::PROJECTILE_SPAWN_REFUSED)
        .map(str::to_string)
        .collect();
    keys.sort();
    assert_eq!(keys, ["pool-full", "velocity"]);
}

#[test]
fn a_corrupt_blob_is_l3008() {
    let mut first = engine();
    trace(&mut first, 7);
    let mut snapshot = snapshot_of(&mut first);
    let state = snapshot["projectiles"]["state"]
        .as_str()
        .unwrap()
        .to_string();
    snapshot["projectiles"]["state"] = Value::String(state[..state.len() / 2].to_string());
    let mut resumed = engine();
    assert_eq!(restore(&mut resumed, snapshot).unwrap_err(), "L3008");
}

#[test]
fn an_unstepped_live_pool_warns_once() {
    let reported = |engine: &Engine| {
        engine
            .world()
            .resource::<Diagnostics>()
            .unwrap()
            .reported(code::PROJECTILE_STEP_NOT_RUN)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let mut unstepped = engine_with(Some(config()), false);
    trace(&mut unstepped, 20);
    assert_eq!(reported(&unstepped), ["projectiles_step"]);

    let mut stepped = engine();
    trace(&mut stepped, 20);
    assert!(reported(&stepped).is_empty());
}

const ROOM: &str = "\
format: 1
seed: 5
physics:
  space: free
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 10.0
          - -1.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 30.0
              - 1.0
              - 30.0
        body: static
  - name: wall
    components:
      Transform:
        translation:
          - 14.0
          - 3.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 4.0
              - 30.0
        body: static
";

/// A few Settle gibs with spin and a per-shot target mask every five ticks.
fn fire_gibs(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    if !frame.is_multiple_of(5) {
        return;
    }
    for k in 0..3u32 {
        let f = k as f32;
        let mut d = ProjectileDesc::new(
            [0.0, 0.5 + 0.2 * f, -2.0 + 2.0 * f],
            [5.0 + f * 2.0, 1.0, 0.5 * f - 0.5],
        );
        d.gravity_scale = 1.0;
        d.lifetime = 1.5;
        d.tag = k;
        d.response = Response::settle(0.4, 0.5, 2.0);
        d.spin = [2.0 + f, 5.0, -3.0];
        d.orientation = [0.1 * f, 0.2, 0.0, 1.0];
        d.targets = Some(0);
        spawn_projectile(world, d);
    }
}

fn room_engine() -> Engine {
    let mut engine = base()
        .projectiles(ProjectileConfig {
            capacity: 128,
            ..ProjectileConfig::default()
        })
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("fire_gibs", fire_gibs)
        .system("projectiles_step", projectiles_step)
        .build()
        .expect("boots");
    engine.load_inline_scene(ROOM).unwrap();
    engine
}

#[test]
fn settling_gibs_restore_mid_flight_bit_identically() {
    let mut uninterrupted = room_engine();
    let reference = trace(&mut uninterrupted, 80);

    let mut first = room_engine();
    trace(&mut first, 50);
    let pool = first.world().resource::<Projectiles>().unwrap().pool();
    assert!(
        pool.ids().any(|id| pool.is_parked(id) == Some(true)),
        "a gib has come to rest"
    );
    assert!(
        pool.ids().any(|id| pool.is_parked(id) == Some(false)),
        "a gib is still flying"
    );
    assert!(pool.ids().any(|id| pool.spin(id) != Some([0.0; 3])));
    let snapshot = snapshot_of(&mut first);

    let mut resumed = room_engine();
    restore(&mut resumed, snapshot).unwrap();
    assert_eq!(trace(&mut resumed, 30), reference[50..], "ticks 51..80");
}
