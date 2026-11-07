//! The engine's host for the projectile pool: [`Projectiles`], [`ProjectileConfig`], the
//! [`projectiles_step`] system and the reporting spawn path [`spawn_projectile`].
//!
//! A game opts in with `Engine::builder().projectiles(config)` and schedules
//! `.system("projectiles_step", projectiles_step)` **after** `spatial3d_rebuild`. The engine installs
//! a [`Projectiles`] resource at every world build; a scene switch gives a fresh pool unless the
//! builder also asks `carry_resource::<Projectiles>()`. `WorldHash` folds the pool in as one block
//! only while the resource exists, and snapshots carry the pool, its last events and the `spatial3d`
//! index, so a restored run continues bit-identically.
//!
//! The pool's physics reads are those of the last completed physics step (the physics systems run
//! after the game's), and its events are valid until the next [`projectiles_step`]. Projectiles stay
//! out of the target index: only entities carrying a `SpatialVolume` can be hit by a piercing shot.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::TickRate;
use ludion_ecs::{SystemCtx, World};
use ludion_physics::{PhysicsWorld, QueryFilter};
use ludion_projectiles::{
    MAX_DECODE_CAPACITY, ProjectileDesc, ProjectileId, ProjectilePool, SpawnError, StepConfig,
    StepEvents, step,
};
use ludion_spatial::spatial3d::Index3d;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diagnostics::report_full;

/// How a game sizes and filters its projectile pool.
///
/// # Example
/// ```
/// use ludion_engine::ProjectileConfig;
///
/// let config = ProjectileConfig {
///     capacity: 256,
///     ignore_layers: vec!["pickups".to_string()],
///     ..ProjectileConfig::default()
/// };
/// assert!(config.validate().is_ok());
/// assert!(ProjectileConfig { capacity: 0, ..config }.validate().is_err());
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectileConfig {
    /// Most live projectiles; `1..=32768`.
    pub capacity: u32,
    /// Acceleration in m/s², scaled per shot by its `gravity_scale` (default 0: inert).
    pub gravity: [f32; 3],
    /// Layer mask of the `spatial3d` index the shots can hit.
    pub targets: u32,
    /// Physics layers the sweep ignores (names from the scene's `physics.layers`).
    pub ignore_layers: Vec<String>,
    /// Whether the sweep hits sensor colliders.
    pub include_sensors: bool,
}

impl Default for ProjectileConfig {
    fn default() -> Self {
        Self {
            capacity: 1024,
            gravity: [0.0, -9.81, 0.0],
            targets: u32::MAX,
            ignore_layers: Vec::new(),
            include_sensors: false,
        }
    }
}

impl ProjectileConfig {
    /// Checks the config.
    ///
    /// # Errors
    /// `L4027` if `capacity` is outside `1..=32768`, `gravity` is not finite, or a layer name is
    /// empty.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let invalid = |path: &str, what: String, help: &str| {
            Diagnostic::new(code::PROJECTILE_CONFIG_INVALID, what)
                .with_path(path)
                .with_help(help)
        };
        if self.capacity == 0 || self.capacity > MAX_DECODE_CAPACITY {
            return Err(invalid(
                "/capacity",
                format!(
                    "projectile `capacity` is {}, outside 1..={MAX_DECODE_CAPACITY}",
                    self.capacity
                ),
                "set `ProjectileConfig::capacity` to the most shots live at once",
            ));
        }
        if self.gravity.iter().any(|g| !g.is_finite()) {
            return Err(invalid(
                "/gravity",
                format!("projectile `gravity` {:?} is not finite", self.gravity),
                "use three finite numbers in m/s², e.g. [0.0, -9.81, 0.0]",
            ));
        }
        if let Some(i) = self.ignore_layers.iter().position(String::is_empty) {
            return Err(invalid(
                &format!("/ignore_layers/{i}"),
                "a projectile `ignore_layers` name is empty".to_string(),
                "name a layer from the scene's `physics.layers`, or remove the entry",
            ));
        }
        Ok(())
    }
}

/// The pool resource: config, [`ProjectilePool`], the last tick's events and the step's scratch.
///
/// Installed by the engine when the builder is given `.projectiles(..)`.
pub struct Projectiles {
    config: ProjectileConfig,
    pool: ProjectilePool,
    events: StepEvents,
    /// Built once from the config so the step allocates nothing.
    query: QueryFilter,
    /// Stand-ins for a world with no physics or no built index; never serialized.
    fallback_physics: Option<PhysicsWorld>,
    fallback_index: Index3d,
    /// Whether `projectiles_step` ever ran on this resource. Host-only: not in the pool codec, so
    /// never hashed or snapshotted.
    stepped: bool,
}

impl Projectiles {
    /// A pool sized and filtered by `config`.
    ///
    /// # Errors
    /// `L4027` if the config is invalid.
    pub fn new(config: ProjectileConfig) -> Result<Self, Diagnostic> {
        config.validate()?;
        let pool = ProjectilePool::with_capacity(config.capacity as usize);
        Ok(Self::with_pool(config, pool, StepEvents::default()))
    }

    /// A resource around a decoded pool (snapshot restore).
    pub(crate) fn with_pool(
        config: ProjectileConfig,
        pool: ProjectilePool,
        events: StepEvents,
    ) -> Self {
        let mut query = config
            .ignore_layers
            .iter()
            .fold(QueryFilter::new(), |q, name| q.ignore_layer(name.clone()));
        if config.include_sensors {
            query = query.include_sensors();
        }
        Self {
            config,
            pool,
            events,
            query,
            fallback_physics: None,
            fallback_index: Index3d::new(),
            stepped: false,
        }
    }

    /// The config the pool was built from.
    pub fn config(&self) -> &ProjectileConfig {
        &self.config
    }

    /// The pool.
    pub fn pool(&self) -> &ProjectilePool {
        &self.pool
    }

    /// The pool, mutably: a game steers a shot with `set_velocity` or ends it with `despawn`.
    pub fn pool_mut(&mut self) -> &mut ProjectilePool {
        &mut self.pool
    }

    /// The last [`projectiles_step`]'s hits and expiries, valid until the next step.
    pub fn events(&self) -> &StepEvents {
        &self.events
    }

    /// Fires one shot.
    ///
    /// # Errors
    /// [`SpawnError`] if the pool is full or a field is out of range.
    pub fn spawn(&mut self, desc: ProjectileDesc) -> Result<ProjectileId, SpawnError> {
        self.pool.spawn(desc)
    }
}

/// Fires one shot into the world's [`Projectiles`], reporting a refusal as `L4026` (once per cause)
/// and returning `None`.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::spawn_projectile;
/// use ludion_projectiles::ProjectileDesc;
///
/// // A world with no pool refuses the shot.
/// let mut world = World::new();
/// assert!(spawn_projectile(&mut world, ProjectileDesc::new([0.0; 3], [60.0, 0.0, 0.0])).is_none());
/// ```
pub fn spawn_projectile(world: &mut World, desc: ProjectileDesc) -> Option<ProjectileId> {
    let (key, message, help) = match world.resource_mut::<Projectiles>() {
        None => (
            "no-pool".to_string(),
            "no projectile pool: a shot was fired in a game that configured none".to_string(),
            "add `.projectiles(ProjectileConfig::default())` to the builder".to_string(),
        ),
        Some(res) => match res.spawn(desc) {
            Ok(id) => return Some(id),
            Err(SpawnError::PoolFull) => (
                "pool-full".to_string(),
                "the projectile pool is full".to_string(),
                "raise `ProjectileConfig::capacity` or shorten `lifetime`".to_string(),
            ),
            Err(error @ SpawnError::InvalidDesc(field)) => (
                field.to_string(),
                error.to_string(),
                format!("give `{field}` a finite value in range"),
            ),
        },
    };
    report_full(
        world,
        key,
        Diagnostic::new(code::PROJECTILE_SPAWN_REFUSED, message).with_help(help),
    );
    None
}

/// Advances the pool one tick: gravity, drag and lifetime, then the swept first hit against the
/// physics colliders and the `spatial3d` index, then each hit's response.
///
/// The crate's step allocates nothing at one thread; this system takes the [`Projectiles`] resource
/// out of the world and puts it back each tick, which re-boxes it (the ECS has no scoped take, and
/// the step needs the physics world and index while it mutates the pool).
///
/// **Schedule it after `spatial3d_rebuild`.** It reads the physics state of the last completed step
/// (the physics systems run after the game's), and the events it leaves in
/// [`Projectiles::events`] are valid until the next step. Projectiles are not entries of the target
/// index. A held frame (`game_dt == 0`, a hitstop) freezes the shots. A no-op in a game that
/// configured no pool.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, ProjectileConfig, projectiles_step};
///
/// let engine = Engine::builder()
///     .projectiles(ProjectileConfig::default())
///     .system("projectiles_step", projectiles_step)
///     .build()
///     .unwrap();
/// # let _ = engine;
/// ```
pub fn projectiles_step(world: &mut World, ctx: &SystemCtx) {
    let Some(mut res) = world.remove_resource::<Projectiles>() else {
        return;
    };
    res.stepped = true;
    let Projectiles {
        config,
        pool,
        events,
        query,
        fallback_physics,
        fallback_index,
        ..
    } = &mut res;
    if world.resource::<PhysicsWorld>().is_none() && fallback_physics.is_none() {
        *fallback_physics = Some(PhysicsWorld::new(TickRate::DEFAULT));
    }
    let physics = match world.resource::<PhysicsWorld>() {
        Some(physics) => physics,
        None => fallback_physics
            .as_ref()
            .expect("the fallback was created above"),
    };
    match physics.sweep_filter(query) {
        Ok(filter) => {
            let index = world.resource::<Index3d>().unwrap_or(fallback_index);
            let mut step_config = StepConfig::new(filter);
            step_config.gravity = config.gravity;
            step_config.targets = config.targets;
            step(pool, ctx.game_dt, physics, index, &step_config, events);
        }
        Err(error) => {
            let diagnostic = Diagnostic::new(
                code::PROJECTILE_CONFIG_INVALID,
                format!("projectile `ignore_layers` is not usable: {error}"),
            )
            .with_path("/ignore_layers")
            .with_help("name layers declared in the scene's `physics.layers`");
            world.insert_resource(res);
            report_full(world, "ignore_layers", diagnostic);
            return;
        }
    }
    world.insert_resource(res);
}

/// Reports a live pool whose `projectiles_step` never ran, once (`L4030`). A bare host's empty
/// schedule is not a game that forgot the system, so `game_systems` gates it. Runs only after a
/// tick. **Known limit:** a wrapper that steps only behind its own guard warns until its first call.
pub(crate) fn report_unrun_step(world: &mut World, game_systems: bool) {
    if !game_systems
        || world
            .resource::<Projectiles>()
            .is_none_or(|res| res.stepped || res.pool.is_empty())
        || crate::diagnostics::is_latched(world, code::PROJECTILE_STEP_NOT_RUN, "projectiles_step")
    {
        return;
    }
    report_full(
        world,
        "projectiles_step",
        Diagnostic::new(
            code::PROJECTILE_STEP_NOT_RUN,
            "the projectile pool holds live shots, but no `projectiles_step` system ever ran, so \
             they never move, hit or expire: register \
             `.system(\"projectiles_step\", projectiles_step)` after `spatial3d_rebuild`",
        )
        .with_help("`ludion explain L4030`"),
    );
}
