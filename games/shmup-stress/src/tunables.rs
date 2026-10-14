//! The arena's constants. Everything a scene can vary lives on [`Arena`](crate::Arena),
//! [`Emitter`](crate::Emitter), [`BulletArt`](crate::BulletArt) or [`Backdrop`](crate::Backdrop);
//! what is left here is the bullet archetype itself, which is uniform by design — the measurement
//! compares scenes, so the thing being counted must not move between them.

/// The bullet quad's world-space size. Camera-facing (`billboard: true`) and unlit, so it draws the
/// same square under any framing a scene authors and takes the no-light-lookup path
/// (`Plan/sprite-engine.md` decision 2) three thousand quads want. A `BulletArt` scene omits this on
/// the `Sprite` and derives the identical extent from the atlas region instead — 12 texels at 40
/// texels to the unit.
pub const BULLET_SIZE: [f32; 2] = [0.3, 0.3];

/// The bullet's collision radius — both the circle `Collider` an indexed scene gives it and the
/// radius `player_hit_test` adds to the player's hitbox. Half the quad's width, so the hit test
/// agrees with what is drawn.
pub const BULLET_RADIUS: f32 = 0.15;

/// The off-plane coordinate every bullet holds: `y` above the ground plane, `z` in front of the
/// upright one (see [`ArenaPlane`](crate::ArenaPlane)). Off the plane so the quads are not
/// z-fighting anything, and irrelevant to the sim, which only ever reads the two plane axes.
pub const BULLET_DEPTH: f32 = 0.5;

/// Bullet tints, chosen per emitter by its row index. One curtain per emitter reads as one pattern
/// rather than a uniform fog, and the assignment is positional (not random), so it is stable across
/// runs and identical between the `steady-drawn` and `steady-indexed` variants.
pub const BULLET_COLORS: [[f32; 4]; 4] = [
    [1.00, 0.35, 0.45, 1.0],
    [0.40, 0.80, 1.00, 1.0],
    [1.00, 0.85, 0.35, 1.0],
    [0.65, 0.45, 1.00, 1.0],
];
