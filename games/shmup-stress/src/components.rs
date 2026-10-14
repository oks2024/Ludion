//! The arena's seven components: the authored `Arena` / `Emitter` pair, the runtime `Bullet`, the
//! `Player` / `HitCount` singletons the hit test reads and writes, and the two draw-side singletons
//! (`BulletArt`, `Backdrop`) that turn the field from flat quads into atlas sprites over a scrolling
//! grid.

/// Which world plane an arena lays its field on — the one thing a scene must say before it can put a
/// [`Tilemap`](ludion::prelude::Tilemap) behind the bullets, because a tilemap's cells are always XY
/// quads (`+X` per column, `−Y` per row) and a camera can only ever face one of the two planes.
///
/// It changes no cost: a bullet's position is two coordinates and a constant third either way, and
/// both variants integrate through the same monomorphized loop, so a scene's `move_bullets` and
/// `player_hit_test` lines stay comparable across the pair.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ArenaPlane {
    /// The ground plane `(x, z)`, seen from above by a camera pitched `−90°`. A `Tilemap` is
    /// edge-on to that camera, so a ground arena has no backdrop.
    #[default]
    Ground,
    /// The upright plane `(x, y)`, seen head-on by an unrotated camera down `−Z` — a shmup's own
    /// framing, and the one plane a `Tilemap` lies flat in.
    Upright,
}

impl ArenaPlane {
    /// The index into `Transform.translation` of the plane's *vertical* axis — the one an emitter's
    /// second coordinate and a bullet's second velocity component address (`z` on the ground plane,
    /// `y` on the upright one). The horizontal axis is `0` in both.
    pub const fn vertical_axis(self) -> usize {
        match self {
            ArenaPlane::Ground => 2,
            ArenaPlane::Upright => 1,
        }
    }

    /// The index of the axis perpendicular to the plane, which every bullet holds constant at
    /// [`BULLET_DEPTH`](crate::BULLET_DEPTH) and no system ever reads.
    pub const fn off_axis(self) -> usize {
        3 - self.vertical_axis()
    }
}

/// The arena singleton: the bounds every bullet is culled against plus the knobs that select which
/// stress variant a scene is. Exactly one entity carries it; a world without one runs no emitter and
/// no cancel (every system that reads it early-outs).
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct Arena {
    /// Half the arena's side, in world units: a bullet is despawned once `|x|` or `|z|` exceeds it.
    /// Dimension a scene so bullets expire by `life` **before** they reach this — then the live
    /// census is a closed form in the emitters alone (see each scene's header comment).
    pub half_extent: f32,
    /// Whether a spawned bullet also carries a dynamic circle [`Collider`](ludion::prelude::Collider),
    /// i.e. whether bullets enter the spatial index. The `steady-drawn` / `steady-indexed` pair is
    /// exactly this flag flipped, so `spatial_rebuild` at ~3,000 entries is measured against its own
    /// absence. A volley scene never sets it: a `Volley` carries no collider by construction.
    pub bullets_collide: bool,
    /// Frames between full bullet cancels (`0` = never). On a cancel frame every live bullet is
    /// despawned at once — the bullet-hell worst case, where a whole field is destroyed and refilled.
    pub cancel_every: u32,
    /// The live-bullet count this scene is dimensioned to hold. **Informative only** — nothing reads
    /// it; it is the authored copy of the header comment's closed form, so a census test and the
    /// scene cannot silently disagree about what was intended.
    pub target_live: u32,
    /// Which world plane the field lives on. Everything else about the arena is plane-agnostic — the
    /// census, the bounds, the hit test — so this is framing, not mechanism.
    pub plane: ArenaPlane,
}

/// A pattern source: every [`rate`](Emitter::rate) frames it lays down an even ring of
/// [`count`](Emitter::count) bullets around its own [`Transform`](ludion::prelude::Transform),
/// rotated by [`spin`](Emitter::spin) radians more than the volley before it. No RNG — the whole
/// field is a closed-form function of the frame index.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct Emitter {
    /// Bullets per volley, spread evenly over the full turn (`0` emits nothing).
    pub count: u32,
    /// Frames between volleys (`0` emits nothing). A volley fires when `(frame + phase) % rate == 0`.
    pub rate: u32,
    /// Muzzle speed, in world units per second.
    pub speed: f32,
    /// Frames a bullet lives before it is despawned.
    pub life: u32,
    /// Frame offset applied before the `% rate` test, so a scene can stagger its emitters into a
    /// flat per-frame spawn rate instead of a periodic spike.
    pub phase: u32,
    /// Radians added to the ring's start angle per volley — the slow twist that makes a static
    /// pattern read as a bullet-hell curtain.
    pub spin: f32,
}

/// One live bullet: a ground-plane velocity and a countdown in frames. `move_bullets` owns both.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct Bullet {
    /// Ground-plane velocity `(x, z)` in world units per second.
    pub velocity: [f32; 2],
    /// Frames left before the bullet is despawned; decremented once per `move_bullets` pass.
    pub life: u32,
}

/// The one entity the hit test tests. It does not move — this arena measures the bullet field, not
/// the player — so its only field is the tiny hitbox a shmup is built around.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct Player {
    /// The hit-test radius, in world units — the "one tiny hitbox" of the genre.
    pub hitbox_radius: f32,
}

/// The hit tally, on its own singleton entity so the count is hashed and snapshotted like any other
/// sim state. Written by `player_hit_test`, which is deliberately a brute-force scan rather than a
/// spatial-index probe: proving player-vs-bullet is affordable *without* the index is the point.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct HitCount {
    /// Bullet-overlap events counted since the scene loaded (one per bullet per frame it overlaps).
    pub hits: u64,
}

/// The draw-side singleton: what an entity bullet is made of pixel-wise. Absent (the
/// `steady-indexed` scene), every bullet is the flat tinted quad the arena has always spawned;
/// present, every bullet samples one region of one atlas, and a closed-form share of them composite
/// additively.
///
/// It is a component and not a constant because the whole instrument is built on scenes varying one
/// thing against each other: `steady-indexed.yaml` is `steady-drawn.yaml` with this singleton
/// dropped and a collider added, and the volley scenes say the same thing in a `kind: pattern`
/// document's bullet kind instead.
#[derive(
    serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Debug, PartialEq, Default,
)]
#[serde(default)]
pub struct BulletArt {
    /// The `kind: atlas` document every bullet samples, relative to the content root (the scene's own
    /// directory). Empty means no atlas, which is the flat-quad archetype.
    pub atlas: String,
    /// The region-name prefix a bullet's frame is built from: frame `k` is `{frame_prefix}{k:02}`, so
    /// `bullets/b` with [`frames`](BulletArt::frames) `40` addresses `bullets/b00` … `bullets/b39`.
    pub frame_prefix: String,
    /// How many distinct regions the field cycles through. The `k` above is the spawn ordinal
    /// (`volley × count + ring index`) modulo this, so which region a bullet shows is a closed-form
    /// function of the frame index like everything else here.
    pub frames: u32,
    /// Every `additive_every`-th bullet **of a volley** composites additively (`0` = none). Counting
    /// within the volley rather than over the spawn ordinal is what keeps the additive share a closed
    /// form: `count / additive_every` rounded up, per volley, for as long as a volley lives.
    pub additive_every: u32,
}

/// The scrolling backdrop singleton, carried by the [`Tilemap`](ludion::prelude::Tilemap) entity it
/// moves. The grid slides along the arena plane's vertical axis and snaps back after
/// [`period`](Backdrop::period) frames — the pre-`uv_scroll` answer to a scrolling stage
/// (`Plan/sprite-engine.md` decision 9: a stage is one atlas, moved as one entity).
///
/// Seamless only when `period × speed × dt` equals the grid's own vertical repeat, which the scene
/// authoring the grid is responsible for; nothing here can check it.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct Backdrop {
    /// World units per second the grid slides along the plane's vertical axis; positive scrolls it
    /// toward the arena's negative side (downward in an upright arena).
    pub speed: f32,
    /// Frames in one scroll cycle. The offset is `(frame % period) × speed × dt`, never an
    /// accumulation, so the grid's position is a pure function of the frame index.
    pub period: u32,
    /// The vertical-axis coordinate the grid sits at with a zero offset. The system owns that axis of
    /// the entity's `Transform`; the scene authors the other two.
    pub origin: f32,
}
