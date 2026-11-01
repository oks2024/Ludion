#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `shmup-stress` — not a game: a **measuring instrument** built the way a Cave-class bullet hell
//! would be built, to answer one question (`Plan/bullet-hell-stress.md`). Can the engine, as
//! shipped, carry ~3,000 live bullets with hundreds spawned and despawned every frame against one
//! tiny player hitbox, and where does the cost land — ECS churn, `spatial::rebuild`, or sprite
//! extract? Vigil's proven ceilings are 300 projectiles and 500 enemies; nothing above them has ever
//! been measured.
//!
//! The shape is the shmup's own: a head-on orthographic camera over the upright plane `(x, y)`
//! ([`Plane::Xy`], which is where the sprites and the tilemap live), bullets 27 px across on a
//! 1080p screen, and pattern sources authored as singletons. There is no RNG anywhere — every
//! bullet's existence and position is a closed-form function of the frame index, so the live census
//! at any frame is a number a test can assert rather than a number a run happens to produce.
//!
//! **Two shapes carry the same census, and comparing them is the instrument**
//! (`Plan/projectile-patterns.md` Wave 2). A **volley** scene stores one hashed [`Volley`] entity
//! per fired ring and evaluates each bullet's position, life and hit circle as a closed form of
//! `(volley, index, now)`; an **entity** scene stores one [`Bullet`] entity per bullet and
//! integrates it. Same 2,975 live bullets, same art, same framing.
//!
//! Four scenes vary one thing each:
//! - `scenes/steady.yaml` — **volleys**, the standing field: four [`PatternPlayer`]s at `started`
//!   0/1/2/3 play `scenes/patterns/steady.yaml`, a ring of 25 every 4 frames, holding ~3,000 live
//!   bullets in 119 volley entities.
//! - `scenes/burst.yaml` — **volleys**, the bullet-cancel worst case: the same field at twice the
//!   density, wiped whole every 60 frames and refilled over the frames after. The wipe destroys 60
//!   entities where the entity shape would destroy 3,000.
//! - `scenes/steady-drawn.yaml` — **entities**, the baseline: `steady`'s census made of one
//!   [`Bullet`] each, every one an atlas sprite over 40 regions of one atlas, a closed-form share of
//!   them additive, behind the same scrolling [`Backdrop`] tilemap. Its difference from `steady` is
//!   the price of the entity-per-bullet shape; it is also the sprite layer's own Wave 4 measurement
//!   (`Plan/sprite-engine.md`).
//! - `scenes/steady-indexed.yaml` — **entities**, `steady-drawn` with [`Arena::bullets_collide`]
//!   set, so every bullet carries a dynamic circle [`Collider`] and enters the spatial index. The
//!   pair exists to price `spatial_rebuild` at that entry count against its own absence — the
//!   hypothesis being that bullets must *not* live in the index and that player-vs-bullet is a plain
//!   circle test ([`player_hit_test`], which never touches the index by design). Volleys settle the
//!   question by construction: they hold no collider and cannot enter the index.
//!
//! Two budgets are calibrated, both on the pinned RTX 3090:
//! `budgets/pinned-rtx3090-sprite-stress.toml` over `steady-drawn` and
//! `budgets/pinned-rtx3090-shmup-stress.toml` over `steady`. The other two scenes stay
//! measurement-only: numbers are read off a reference box, never off whichever machine ran the
//! suite.

use ludion::prelude::*;

mod components;
mod systems;
mod tunables;

pub use components::*;
pub use systems::*;
pub use tunables::*;

// --- Wiring --------------------------------------------------------------------------------------

/// Registers this game's components on top of a registry that already holds the built-ins (including
/// the `Collider` builtin). Append-only: new components register after the existing ones so no prior
/// component's index shifts.
///
/// # Errors
/// An `L0005` diagnostic if a name collides with a built-in or another game component.
pub fn register(registry: &mut Registry) -> Result<(), Diagnostic> {
    registry.register::<Arena>("Arena")?;
    registry.register::<Emitter>("Emitter")?;
    registry.register::<Bullet>("Bullet")?;
    registry.register::<Player>("Player")?;
    registry.register::<HitCount>("HitCount")?;
    registry.register::<BulletArt>("BulletArt")?;
    registry.register::<Backdrop>("Backdrop")
}

/// This game's full component registry: the built-ins plus this game's components.
///
/// # Panics
/// If [`register`] fails — a build-time bug (a name colliding with a built-in), not a runtime state.
pub fn registry() -> Registry {
    let mut registry = builtin_registry();
    register(&mut registry).expect("components register cleanly on top of the built-ins");
    registry
}

/// Describes the engine that hosts the stress arena.
///
/// Registration order is execution order, and here it is also the shape of the measurement — each
/// system is one line of the profile report, so the order is chosen so that no cost is charged to
/// the wrong one.
///
/// `spatial_rebuild` runs **first**, as everywhere else in this workspace, so any query reads a
/// fresh index; in the volley scenes and in `steady-drawn` it has nothing but the player to index
/// and costs ~nothing, while in `steady-indexed` it carries the whole bullet field — that
/// difference, at one system boundary, is the arc's central number.
///
/// The two shapes then run as mirrored pairs, each pair's members adjacent so a report reads as one
/// comparison. **`play_patterns` and `emit_bullets` stand this frame's volleys up**, patterns first
/// (they are the shape the arena is migrating to, and nothing downstream distinguishes them);
/// **`move_bullets` and `advance_volleys` reap**, in that order, so both shapes retire a bullet at
/// the same point in the frame and one census convention covers both — a bullet spawned this frame
/// is already ticked this frame (the `− 1` in the entity scenes' closed form), and a volley fired
/// this frame is already aged zero here.
///
/// `cancel_bullets` then wipes both shapes on a cancel frame, *after* the reap and before anything
/// refills (so a cancel frame's census is exactly zero and the refill is unambiguously the next
/// frame's work); `player_hit_test` scans the surviving field last, when it is settled, running the
/// entity scan and [`volley_hits`] into the one tally; and `scroll_backdrop` moves the one entity
/// none of the others look at — registered last so adding a drawn scene moved no existing profile
/// line.
///
/// The game's [`Plane`] is [`Plane::Xy`], the upright plane every scene here frames head-on: a
/// volley's headings, a bullet quad's orientation and the spatial index all read it, so it is a
/// property of the arena and not of a scene.
///
/// The camera is authored and never touched: this arena measures bullets, not framing.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(register)
        .plane(Plane::Xy)
        // `kind: pattern` documents under the content root (`scenes/patterns/`), plus their
        // validator — one call, and the whole of the volley opt-in.
        .patterns()
        // Query declarations (the L1126 drift gate): each `.query::<...>()` names a component access
        // the system's body — or a helper it calls — actually makes, `&T` a read and `&mut T` a
        // write. Multi-component shapes come from typed queries; the **one-component** shapes are the
        // per-entity accessors the `&mut self` borrow model forces (`get` a read, `insert`/`get_mut`
        // a write), which the world recorder observes too. (`has`, `entities`, `despawn`, and the
        // spatial index record nothing: they name no component value.)
        .system(
            "spatial_rebuild",
            spatial::rebuild::<Transform, TransformHierarchy>,
        )
        // Reads each collider's shape and placement to refill the index — one typed join, so the
        // pair is the whole observed shape.
        .query::<(&Collider, &Transform)>()
        .system("play_patterns", play_patterns)
        // Joins each `PatternPlayer` with its muzzle `Transform`, then writes one volley entity per
        // entry whose frame has come — a `Transform` and a `Volley` inserted per spawn.
        .query::<(&PatternPlayer, &Transform)>()
        // conditional: only a player with a live `target` reads that target's pose to aim a volley.
        .query::<(&Transform,)>()
        .query::<(&mut Transform,)>()
        .query::<(&mut Volley,)>()
        .system("emit_bullets", emit_bullets)
        // Reads the arena for `bullets_collide` and the plane, the art singleton for the atlas the
        // bullets sample, joins each emitter with its muzzle position, then builds every bullet
        // component by component.
        .query::<(&Arena,)>()
        // conditional: only a scene that authors the singleton (`steady-drawn`) has one to read.
        .query::<(&BulletArt,)>()
        .query::<(&Emitter, &Transform)>()
        .query::<(&mut Bullet,)>()
        .query::<(&mut Sprite,)>()
        .query::<(&mut Transform,)>()
        // conditional: only an arena with `bullets_collide` (the `steady-indexed` scene) gives a
        // bullet a collider.
        .query::<(&mut Collider,)>()
        .system("move_bullets", move_bullets)
        // Reads the arena's bounds, then walks every bullet's own state and pose.
        .query::<(&Arena,)>()
        .query::<(&mut Bullet, &mut Transform)>()
        .system("advance_volleys", advance_volleys)
        // Reads each volley to decide whether its last bullet is gone or past its kind's `life`.
        .query::<(&Volley,)>()
        .system("cancel_bullets", cancel_bullets)
        // Reads the arena's cancel cadence, then scans the live field — both shapes — to despawn it.
        .query::<(&Arena,)>()
        // conditional: only an arena with `cancel_every > 0` (the `burst` scene) ever scans.
        .query::<(&Bullet,)>()
        .query::<(&Volley,)>()
        .system("player_hit_test", player_hit_test)
        // Reads the player's hitbox and position, scans the whole field against it — the entity
        // bullets by hand, the volleys through `volley_hits` — and tallies both into one counter.
        .query::<(&Player,)>()
        .query::<(&Transform,)>()
        .query::<(&Bullet, &Transform)>()
        .query::<(&Volley,)>()
        .query::<(&mut HitCount,)>()
        .system("scroll_backdrop", scroll_backdrop)
        // Reads the arena for the plane, then moves each backdrop grid along it. Registered last
        // because it touches nothing the other five read or write, which leaves their profile lines
        // exactly where they were before a scene had a backdrop.
        .query::<(&Arena,)>()
        // conditional: only a scene that authors a backdrop (`steady-drawn`) has a grid to move.
        .query::<(&Backdrop, &mut Transform)>()
        .default_scene("scenes/steady.yaml")
}
