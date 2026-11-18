//! The engine's built-in visual component set, and [`builtin_registry`].
//!
//! These are the components a milestone scene (Breakout/Sokoban with dynamic lighting, HD-2D) is
//! built from. Each derives `Serialize` + `Deserialize` + `schemars::JsonSchema`, so it round-trips
//! through the scene format, publishes a schema for `check-scene`, and is registered by name in
//! [`builtin_registry`]. Registration order is the canonical component order for scene files and the
//! world hash.
//!
//! # A note on field types
//!
//! Components use plain serde/schemars-friendly fields (arrays and scalars), not `glam` types
//! directly: `glam` does not implement `JsonSchema`, and arrays keep scenes author-friendly.
//! Rotations are **Euler XYZ angles in degrees** — far easier to hand-author than quaternions.
//! `extract()` converts these into `glam` matrices for the renderer.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use ludion_core::audio::AudioCategory;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::{
    euler_xyz_degrees_to_quat, quat_mul, quat_rotate, quat_to_euler_xyz_degrees,
};
use ludion_core::number::engine_number;
use ludion_core::text::single_char;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Parent, Registry, SemanticValidate};
use ludion_physics::PhysicsPose;
use ludion_spatial::{
    Collider, GroundPosition, Joint, JointMotor, JointPoses, Plane, PoseBlend, SpatialVolume,
    Velocity, WorldPosition,
};

pub mod animation_layers;
pub mod audio;
pub mod blend_clock;
pub mod lane;
pub mod light_exposure;
pub mod nav_route;
pub mod path_follow;
pub mod patrol;
pub mod patterns;
pub mod perception;
pub mod prefab;
pub mod ui;
pub mod validators;

pub use animation_layers::{
    AnimationLayer, AnimationLayers, LayerMode, fade_layer, layer_weight, layer_weight_at,
    play_layer,
};
pub use audio::{AudioListener, ListenerOrientation};
pub use blend_clock::BlendClock;
pub use lane::{LaneConstraint, LaneEnds};
pub use light_exposure::{Combine, Facing, LightExposure, MAX_EXPOSURE_POINTS};
pub use nav_route::{NavGoal, NavReplan, NavRoute, NavRouteStatus};
pub use path_follow::{PathFollower, PathMode, PathOrient};
pub use patrol::{PatrolFollower, PatrolPhase, PatrolResume, PatrolState};
pub use patterns::{PatternPlayer, VOLLEY_MAX_BULLETS, Volley, VolleyShape};
pub use perception::{
    Hearing, Look, NoiseZone, NoiseZoneShape, PERCEIVER_MEMORY_MAX_RECORDS, PERCEPTION_MAX_PARTS,
    PendingNoise, Perceiver, PerceiverMemory, PerceptionState, Require, Sense, SightPoint,
    SightTarget, SightZone, StimulusRecord, SweepShape, ZoneSweep,
};
pub use prefab::PrefabInstance;
pub use ui::{
    UiAnchorBehind, UiAnchorShow, UiClock, UiEase, UiNav, UiSlice, UiSliceCenter, UiTween,
    UiTweenField, UiTweenRepeat, UiTweenTrack, UiWorldAnchor,
};

/// An entity's human-readable name — the world-side home of a scene entity's `name:` field.
///
/// This is a **reserved** component ([`Registry::register_reserved`]): a scene document authors it
/// only through the entity `name:` field, never as a `components:` block, so there is exactly one
/// way to name an entity (the regularity pillar). The engine moves the field into a `Name` at load
/// and back out to the field at save (`Engine::world_to_scene`); it is otherwise an
/// ordinary component — it enters the world hash and snapshots, and `component.get` reads it back.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct Name(pub String);

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A position, orientation, and scale — relative to the entity's [`Parent`] when
/// it has one, else in world space (see `world_transform`).
///
/// All fields have sensible defaults, so a scene may give a partial `Transform` (or omit it
/// entirely, in which case the entity is treated as being at the identity transform).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Position `[x, y, z]`, local to the `Parent` or world-space for a root (default `[0, 0, 0]`).
    #[serde(default)]
    pub translation: [f32; 3],
    /// Orientation as Euler XYZ angles in **degrees** (default `[0, 0, 0]`).
    #[serde(default)]
    pub rotation: [f32; 3],
    /// Per-axis scale (default `[1, 1, 1]`).
    #[serde(default = "one3")]
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            translation: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

impl WorldPosition for Transform {
    fn world_position(&self) -> [f32; 3] {
        self.translation
    }
}

impl GroundPosition for Transform {
    /// A collider sits at the `plane`'s two in-plane components of `translation` — the off-plane
    /// component, rotation, and scale are ignored (the 2D-sim / 3D-eye pillar). This is the one place
    /// the `Transform`→collision-position mapping lives, so `ludion-spatial` need not name
    /// `Transform`.
    fn ground_position(&self, plane: Plane) -> [f32; 2] {
        let axes = plane.axes();
        [self.translation[axes[0]], self.translation[axes[1]]]
    }
}

impl PhysicsPose for Transform {
    /// The pose seam `ludion-physics` reads and writes — the one place the engine's Euler-XYZ
    /// degrees meet the solver's quaternion, through the same conversion `extract()` builds model
    /// matrices from.
    ///
    /// **`scale` is untouched by the mirror**: a rigid body's collider is sized by its `Collider`
    /// shape, never by the entity's scale, so the solver has no scale to give back and a game's
    /// authored value survives every tick.
    fn translation(&self) -> [f32; 3] {
        self.translation
    }

    fn set_translation(&mut self, translation: [f32; 3]) {
        self.translation = translation;
    }

    fn rotation_quat(&self) -> [f32; 4] {
        euler_xyz_degrees_to_quat(self.rotation)
    }

    fn set_rotation_quat(&mut self, quat: [f32; 4]) {
        self.rotation = quat_to_euler_xyz_degrees(quat);
    }
}

/// How a [`Sprite`] composites over what it draws atop. A per-instance flag, not a second pipeline:
/// both modes run through the one premultiplied `One, OneMinusSrcAlpha` blend, so a flag bit alone
/// separates them and blend never splits a batch (`Plan/sprite-engine.md` decision 4).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpriteBlend {
    /// Ordinary `over` compositing — the premultiplied texel blends by its own alpha.
    #[default]
    Alpha,
    /// Additive: the shader withholds the output alpha, which under `One, OneMinusSrcAlpha` turns the
    /// same `over` into a plain add — glows, muzzle flashes, energy bolts.
    Additive,
}

/// A textured, tinted quad. With `billboard: true` it always faces the camera (the "billboard" of
/// the sprite/billboard batcher); with `billboard: false` its orientation comes from its
/// [`Transform`]. One component, one flag — not two component types.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Sprite {
    /// Texture asset path (relative to the content root — the directory of the scene the engine was started with), or `None` for a plain white quad.
    #[serde(default)]
    #[schemars(with = "Option<ludion_ecs::AssetPathSchema>")]
    pub texture: Option<String>,
    /// A `kind: atlas` document path (relative to the content root), or `None`. The region named by
    /// [`frame`](Sprite::frame) supplies the pixels and the UV rect. Mutually exclusive with
    /// [`texture`](Sprite::texture): a quad takes its pixels from exactly one place (`L1151`).
    #[serde(default)]
    #[schemars(with = "Option<ludion_ecs::AssetPathSchema>")]
    pub atlas: Option<String>,
    /// The region name inside [`atlas`](Sprite::atlas). Required with `atlas` and meaningless
    /// without it (`L1151`).
    #[serde(default)]
    pub frame: Option<String>,
    /// Quad size `[width, height]` in world units, or `None` to derive it: an atlas region's `rect`
    /// width/height divided by the atlas `pixels_per_unit`, else `[1, 1]`. An explicit size is an
    /// *override* — untextured quads and deliberate stretches. Either way the drawn quad is exactly
    /// this size, further multiplied by the entity's `Transform.scale` x/y.
    #[serde(default)]
    pub size: Option<[f32; 2]>,
    /// RGBA tint multiplied into the sampled texel (default opaque white).
    #[serde(default = "white4")]
    pub color: [f32; 4],
    /// Whether the quad faces the camera (default `false`).
    #[serde(default)]
    pub billboard: bool,
    /// Whether the quad takes light (default `false`). Unlit is texel × tint with no light lookup —
    /// what a bullet swarm needs; lit is the clustered-forward two-sided shading a flat surface in a
    /// 3D scene wants.
    #[serde(default)]
    pub lit: bool,
    /// The sort key (default `0`). Sprites draw in `(layer, view depth back-to-front, scene order)`
    /// order, `layer` the primary key — a pure ordering, never a Z displacement (it would leak into
    /// picking, shadows and debug draws). A higher `layer` draws in front.
    #[serde(default)]
    pub layer: i16,
    /// How the quad composites (default [`SpriteBlend::Alpha`]). A per-instance flag that never splits
    /// a batch.
    #[serde(default)]
    pub blend: SpriteBlend,
    /// Mirror the sampled art left↔right (default `false`). Applied to the UV rect at extract, so no
    /// second copy of the region and no shader branch.
    #[serde(default)]
    pub flip_x: bool,
    /// Mirror the sampled art top↔bottom (default `false`).
    #[serde(default)]
    pub flip_y: bool,
    /// Hit-flash amount in `[0, 1]` (default `0.0`): the final colour lerps toward
    /// [`flash_color`](Sprite::flash_color) by this much. `0.0` is render-identical by construction,
    /// so a sprite that never flashes pays nothing.
    #[serde(default)]
    pub flash: f32,
    /// The colour a hit-flash lerps toward (default white `[1, 1, 1]`). HDR-capable — a value above
    /// `1` blooms.
    #[serde(default = "white3")]
    pub flash_color: [f32; 3],
    /// How many times the frame tiles across the quad, along its right and up axes (default
    /// `[1, 1]` — one copy). A value other than `[1, 1]` here, or a non-zero
    /// [`uv_scroll`](Sprite::uv_scroll), makes the quad **wrap** its frame — the atlas region, or the
    /// whole texture — instead of clamping to it, so an endless parallax strip is one quad rather
    /// than two transforms chasing each other. Each element is validated `>= 0` (`L1014`).
    ///
    /// Known limit: wrapping repeats the *region*, and the sampler still clamps at the texture edge,
    /// so with a linear filter the column where a tile restarts blends toward whatever the atlas
    /// packed next door. Pad the frame, give it its own image, or filter nearest.
    #[serde(default = "one2")]
    #[schemars(inner(range(min = 0.0)))]
    pub uv_repeat: [f32; 2],
    /// How fast the frame scrolls across the quad, in **UV units per second** along its right and up
    /// axes (default `[0, 0]` — still). `1.0` moves one full frame width per second. The offset is
    /// derived from sim time when the frame is extracted, so scrolling adds nothing to the world hash
    /// and nothing to the sim. A non-zero value wraps the quad exactly as
    /// [`uv_repeat`](Sprite::uv_repeat) does, with the same seam limit.
    #[serde(default)]
    pub uv_scroll: [f32; 2],
    /// Scale in `[0, 1]` on the medium's optical depth along the view ray to this sprite (default none,
    /// meaning `1.0`, today's look): `0` shows no air between the eye and the sprite. Render-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub medium: Option<f32>,
}

impl Default for Sprite {
    fn default() -> Self {
        Sprite {
            texture: None,
            atlas: None,
            frame: None,
            size: None,
            color: [1.0; 4],
            billboard: false,
            lit: false,
            layer: 0,
            blend: SpriteBlend::Alpha,
            flip_x: false,
            flip_y: false,
            flash: 0.0,
            flash_color: [1.0; 3],
            uv_repeat: [1.0; 2],
            uv_scroll: [0.0; 2],
            medium: None,
        }
    }
}

impl SemanticValidate for Sprite {
    /// The atlas reference is a pair, and it competes with `texture` (`L1151`): `frame` without
    /// `atlas`, `atlas` without `frame`, and `texture` alongside `atlas` are each rejected, and
    /// `color`/`flash_color` are HDR colours (`L1263`). Runs on a schema-valid value; guards
    /// defensively so a schema-invalid one adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Some(object) = value.as_object() else {
            return Vec::new();
        };
        let mut out = atlas_reference_diagnostics(
            |field| object.get(field).is_some_and(|v| !v.is_null()),
            "",
        );
        out.extend(value_color_diagnostics("Sprite", value, "color"));
        out.extend(value_color_diagnostics("Sprite", value, "flash_color"));
        out
    }
}

/// The three `L1151` rules an atlas reference must satisfy, shared by [`Sprite`] and each
/// [`Tilemap`] legend entry — one home, so the two can never disagree about what a half-written
/// reference is. `present` answers whether a field is set; `prefix` is the JSON Pointer the
/// offending fields hang off (`""` for a `Sprite`, `/legend/<glyph>` for a legend entry).
fn atlas_reference_diagnostics(present: impl Fn(&str) -> bool, prefix: &str) -> Vec<Diagnostic> {
    let (texture, atlas, frame) = (present("texture"), present("atlas"), present("frame"));
    let mut out = Vec::new();
    if frame && !atlas {
        out.push(
            Diagnostic::new(
                code::SPRITE_ATLAS_UNRESOLVED,
                "`frame` names an atlas region, but no `atlas` is set".to_string(),
            )
            .with_path(format!("{prefix}/frame"))
            .with_help("add `atlas: <path to a kind: atlas document>`, or drop `frame`"),
        );
    }
    if atlas && !frame {
        out.push(
            Diagnostic::new(
                code::SPRITE_ATLAS_UNRESOLVED,
                "`atlas` is set, but no `frame` names the region to draw".to_string(),
            )
            .with_path(format!("{prefix}/atlas"))
            .with_help("add `frame: <region name>` from the atlas's `regions:` map"),
        );
    }
    if texture && atlas {
        out.push(
            Diagnostic::new(
                code::SPRITE_ATLAS_UNRESOLVED,
                "`texture` and `atlas` are both set; a quad takes its pixels from one place"
                    .to_string(),
            )
            .with_path(format!("{prefix}/atlas"))
            .with_help("keep `texture` for a loose PNG, or `atlas` + `frame` for a packed region"),
        );
    }
    out
}

/// Where a [`Mesh`]'s geometry comes from: one of the three built-in procedural primitives, a mesh
/// asset document loaded from disk, a voxel model greedily meshed at load, or a multi-part voxel
/// assembly.
///
/// # Serialized form (serde external tagging)
///
/// A primitive is a bare string; every document-backed source is a one-key map whose key is the
/// variant name:
///
/// ```yaml
/// shape: cube                            # a primitive
/// shape:
///   asset: "meshes/rock.mesh.yaml"       # a mesh asset (path relative to the content root)
/// shape:
///   voxel: "voxels/lantern.voxel.yaml"   # a voxel model (path relative to the content root)
/// shape:
///   assembly: "models/player.assembly.yaml"  # a multi-part voxel model
/// shape:
///   sculpt: "sculpts/idol.sculpt.yaml"   # an implicit-surface model
/// shape:
///   character: "characters/hero.character.yaml"  # a skinned mesh bound to named clips
/// shape:
///   assembly_part:                       # one part of an assembly, drawn on its own entity
///     assembly: "models/player.assembly.yaml"
///     part: lower-arm-l
/// ```
///
/// The `asset`/`voxel`/`assembly`/`sculpt`/`character` paths are rooted exactly like [`Sprite::texture`] —
/// relative to the content root, the directory of the scene the engine was started with (inside a
/// `kind: prefab` they are relative to the prefab file, rebased at parse). `asset`
/// names a `*.mesh.yaml` document (`ludion_scene::check_mesh`); `voxel` names a `*.voxel.yaml`
/// document (`ludion_scene::check_voxel`), greedily meshed into per-quad-colored geometry; `assembly`
/// names a `*.assembly.yaml` document (`ludion_scene::check_assembly`) plus the part files it
/// declares, each meshed on its own; `sculpt` names a `*.sculpt.yaml` document
/// (`ludion_scene::check_sculpt`), its primitive parts meshed as one smooth surface; `character`
/// names a `*.character.yaml` document (`ludion_scene::check_character`) whose skinned mesh is
/// drawn like an `asset` and whose `clips:` map `Animation.clip` resolves through. All resolve to
/// vertex data at extract time. A voxel model's origin is bottom-centered (it sits on the ground) and
/// so are an assembly's assembled grid and a sculpt's surface; `Mesh.color` still tints them, and
/// `Transform.scale` sizes them (1 voxel = 1 unit; a sculpt is authored in world units).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MeshSource {
    /// A unit cube.
    #[default]
    Cube,
    /// A unit plane in the XZ plane.
    Plane,
    /// A unit-radius sphere.
    Sphere,
    /// A mesh asset document at this content-root-relative path (`meshes/*.mesh.yaml`).
    Asset(#[schemars(with = "ludion_ecs::AssetPathSchema")] String),
    /// A voxel model document at this content-root-relative path (`voxels/*.voxel.yaml`), greedily
    /// meshed at load with per-quad palette colors.
    Voxel(#[schemars(with = "ludion_ecs::AssetPathSchema")] String),
    /// A multi-part voxel assembly document at this content-root-relative path (`*.assembly.yaml`),
    /// whose part files are read relative to the assembly file's own directory. One entity draws the
    /// whole model: extract expands it into one instance per part, each at its rest offset inside the
    /// assembled grid.
    Assembly(#[schemars(with = "ludion_ecs::AssetPathSchema")] String),
    /// A sculpt model document at this content-root-relative path (`sculpts/*.sculpt.yaml`): named
    /// primitive parts folded into one signed distance field, meshed at load into a smooth triangle
    /// surface with blended palette colors and materials.
    Sculpt(#[schemars(with = "ludion_ecs::AssetPathSchema")] String),
    /// A character document at this content-root-relative path (`*.character.yaml`): it names a
    /// skinned mesh and binds short clip names to animation clips. The entity draws the skinned mesh
    /// (a `SkinBone` part can name it), and `Animation.clip` names a key of the document's `clips:`.
    Character(#[schemars(with = "ludion_ecs::AssetPathSchema")] String),
    /// One named part of an assembly document, drawn on its own entity rather than the whole model
    /// (`Plan/physics.md` W3's ragdoll authoring): `assembly` is the same content-root-relative path
    /// `Assembly` takes, `part` is one of its `parts:` entries' `name`. Extract meshes the same part
    /// run `Assembly` does — no second load, no second GPU upload — and instances only that one
    /// part's geometry, at its own rest offset. The ragdoll switch is the one thing that spawns
    /// these entities; nothing stops a hand-authored one.
    AssemblyPart {
        /// The assembly document's path, exactly as `Assembly` takes it.
        #[schemars(with = "ludion_ecs::AssetPathSchema")]
        assembly: String,
        /// The part's name, as authored in the assembly's `parts:`.
        part: String,
    },
}

/// A low-poly mesh with a flat color: a built-in primitive or an asset mesh. Faceted shading is the
/// HD-2D look.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Mesh {
    /// Where the geometry comes from (default [`MeshSource::Cube`]).
    #[serde(default)]
    pub shape: MeshSource,
    /// Flat RGBA color (default opaque white).
    #[serde(default = "white4")]
    pub color: [f32; 4],
    /// Perceptual roughness, validated to `[0, 1]` (default `1.0` — fully matte). Applies only to
    /// meshes that do not author their own material (a voxel or assembly palette, or mesh-doc
    /// attributes); it is inert on meshes that do.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub roughness: f32,
    /// Metalness, validated to `[0, 1]` (default `0.0` — a dielectric). Applies only to meshes that
    /// do not author their own material; inert otherwise.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub metallic: f32,
    /// Emissive intensity, validated `>= 0` (default `0.0`), an HDR multiplier on the mesh's
    /// `color`. Applies only to meshes that do not author their own material; inert otherwise.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 8.0]))]
    pub emissive: f32,
    /// Hit-flash amount in `[0, 1]` (default `0.0`): the shaded colour lerps toward
    /// [`flash_color`](Mesh::flash_color) by this much. `0.0` is render-identical by construction —
    /// the generic hit-flash a game drives from its own damage timer instead of overwriting
    /// [`color`](Mesh::color).
    #[serde(default)]
    pub flash: f32,
    /// The colour a hit-flash lerps toward (default white `[1, 1, 1]`). HDR-capable — a value above
    /// `1` blooms.
    #[serde(default = "white3")]
    pub flash_color: [f32; 3],
    /// Wrap diffuse in `[0, 1]` (default `0.0`): direct lights soften across the terminator as
    /// `x = max((n·l + wrap) / (1 + wrap), 0)`, with [`scatter_color`](Mesh::scatter_color) tinting the
    /// total diffuse `x * mix(scatter, 1, 1 - wrap * (1 - x))` per channel (never below plain Lambert; white
    /// is plain wrap) — a cheap subsurface look for skin, leaves, wax. Specular, IBL,
    /// ambient and GI are unchanged. Applies on meshes with an authored material, like [`flash`](Mesh::flash).
    /// `0.0` is render-identical by construction.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub wrap: f32,
    /// The tint of the wrapped diffuse (default white `[1, 1, 1]`), linear RGB. A channel at `0` still
    /// receives some wrapped light near the terminator. Inert while [`wrap`](Mesh::wrap) is `0`.
    #[serde(default = "white3", skip_serializing_if = "is_white3")]
    pub scatter_color: [f32; 3],
    /// An unlit Fresnel edge (default none): see [`Rim`]. A mesh without its own `rim` takes its
    /// nearest `Parent` ancestor's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rim: Option<Rim>,
    /// Scale in `[0, 1]` on the medium's optical depth along the view ray to this mesh (default none,
    /// meaning `1.0`, today's look): `0` shows no air between the eye and the mesh. Render-only. A mesh
    /// without its own value takes its nearest `Parent` ancestor's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub medium: Option<f32>,
}

/// A rim term on a [`Mesh`]: adds `color · strength · (1 − n·v)^power` to the shaded colour. Unlit
/// and emissive-like — not scaled by ambient or haze at the surface, fogged by distance like
/// emissive — and inherited down `Parent` by meshes that author no `rim`. Strength `0` or a black
/// colour renders identically to no rim.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Rim {
    /// Linear RGB rim colour (default white), HDR-capable.
    #[serde(default = "white3")]
    pub color: [f32; 3],
    /// Intensity multiplier, validated `>= 0` (default `1.0`).
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub strength: f32,
    /// Falloff exponent, validated `> 0` (default `3.0`); higher hugs the silhouette.
    #[serde(default = "rim_power_default")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    #[schemars(extend("x-ui-range" = [0.5, 8.0]))]
    pub power: f32,
}

fn rim_power_default() -> f32 {
    3.0
}

impl Default for Rim {
    fn default() -> Self {
        Rim {
            color: [1.0; 3],
            strength: 1.0,
            power: 3.0,
        }
    }
}

impl Default for Mesh {
    fn default() -> Self {
        Mesh {
            shape: MeshSource::Cube,
            color: [1.0; 4],
            roughness: 1.0,
            metallic: 0.0,
            emissive: 0.0,
            flash: 0.0,
            flash_color: [1.0; 3],
            wrap: 0.0,
            scatter_color: [1.0; 3],
            rim: None,
            medium: None,
        }
    }
}

impl SemanticValidate for Mesh {
    /// `color`, `flash_color`, `scatter_color` and `rim.color` are colours (`L1263`); the scalars ride their schemars bounds.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = value_color_diagnostics("Mesh", value, "color");
        out.extend(value_color_diagnostics("Mesh", value, "flash_color"));
        out.extend(value_color_diagnostics("Mesh", value, "scatter_color"));
        if let Some(rim) = value.get("rim").filter(|rim| rim.is_object()) {
            out.extend(
                value_color_diagnostics("Mesh", rim, "color")
                    .into_iter()
                    .map(|mut d| {
                        d.path = d.path.map(|p| format!("/rim{p}"));
                        d
                    }),
            );
        }
        out
    }
}

/// A dynamic point light. Its position comes from the entity's [`Transform`].
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    /// Linear RGB color (default white).
    #[serde(default = "white3")]
    pub color: [f32; 3],
    /// Brightness multiplier, validated `>= 0` (default `1.0`).
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub intensity: f32,
    /// Distance at which the light falls to zero (default `10.0`), validated `> 0`.
    #[serde(default = "ten")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub range: f32,
    /// Opts the light into a shadow map (default `false`); lights that don't opt in, that lose
    /// the per-frame caster budget, or that are past `shadow_distance`, stay lit and unshadowed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cast_shadows: bool,
    /// Depth bias in world units, as on `DirectionalLight`, validated `>= 0` (default `0.02`).
    #[serde(
        default = "local_shadow_bias_default",
        skip_serializing_if = "is_local_shadow_bias_default"
    )]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.5]))]
    pub shadow_bias: f32,
    /// The shadow view's near plane in metres from the light (default `0.05`); the far plane is
    /// `range`. Finite, above 0 and below `range` (`L1312`).
    #[serde(
        default = "local_shadow_near_default",
        skip_serializing_if = "is_local_shadow_near_default"
    )]
    #[schemars(extend("x-ui-range" = [0.01, 1.0]))]
    pub shadow_near: f32,
    /// Camera distance past which the light casts no shadow (default none: no limit). Measured to
    /// the light's footprint surface (the camera's distance to the light, less `range`), the metric
    /// the caster budget ranks by. A light at or past it is not a candidate and takes no atlas
    /// cells; it still lights fully. Finite and above 0 (`L1322`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_distance: Option<f32>,
    /// Length of the band before `shadow_distance` over which the shadow fades from full to none,
    /// by smoothstep (default `0`: a hard cut). Needs `shadow_distance` and is at most it (`L1322`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    #[schemars(extend("x-ui-range" = [0.0, 50.0]))]
    pub shadow_fade: f32,
    /// How strongly the light scatters into the `PostProcess` medium (default `0`: not at all).
    /// `1.0` is physical for the medium; only lights that opt in pay for it (`ludion docs atmosphere`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub scatter: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        PointLight {
            color: [1.0; 3],
            intensity: 1.0,
            range: 10.0,
            cast_shadows: false,
            shadow_bias: local_shadow_bias_default(),
            shadow_near: local_shadow_near_default(),
            shadow_distance: None,
            shadow_fade: 0.0,
            scatter: 0.0,
        }
    }
}

impl SemanticValidate for PointLight {
    /// `color` is an HDR colour (`L1263`); `intensity` and `range` ride their schemars bounds. A
    /// `shadow_near` that is non-finite, at most 0, or not below `range` is `L1312`.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = value_color_diagnostics("PointLight", value, "color");
        out.extend(local_shadow_diagnostics(
            value,
            f64::from(PointLight::default().range),
        ));
        out.extend(local_shadow_distance_diagnostics(value));
        out
    }
}

/// A dynamic spot light: a point light confined to a cone. Its position comes from the entity's
/// [`Transform`].
///
/// Like [`DirectionalLight`], the axis is an explicit world-space `direction` field (`[x, y, z]`,
/// normalized at extract) rather than the entity's rotation, so a parent's rotation does not turn
/// it. Both angles are half-angles in degrees from the axis: full brightness inside `inner_angle`,
/// falling smoothly to zero at `outer_angle`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct SpotLight {
    /// Linear RGB color (default white).
    #[serde(default = "white3")]
    pub color: [f32; 3],
    /// Brightness multiplier, validated `>= 0` (default `1.0`).
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub intensity: f32,
    /// Distance at which the light falls to zero (default `10.0`), validated `> 0`.
    #[serde(default = "ten")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub range: f32,
    /// World-space direction the cone points, `[x, y, z]` (default `[0, -1, 0]`, straight down).
    /// Need not be unit length; normalized at extract. A zero-length or non-finite direction is
    /// rejected by the semantic validator (`L1272`).
    #[serde(default = "spot_direction")]
    pub direction: [f32; 3],
    /// Half-angle in degrees of the full-brightness core (default `20.0`); at most `outer_angle`
    /// and within `[0, 90)`, else `L1272` (the validator, not the schema, owns the bounds).
    #[serde(default = "spot_inner_angle")]
    #[schemars(extend("x-ui-range" = [0.0, 89.0]))]
    pub inner_angle: f32,
    /// Half-angle in degrees where the light reaches zero (default `30.0`); within `[0, 90)`
    /// (`L1272`).
    #[serde(default = "spot_outer_angle")]
    #[schemars(extend("x-ui-range" = [0.0, 89.0]))]
    pub outer_angle: f32,
    /// Opts the light into a shadow map (default `false`); lights that don't opt in, that lose
    /// the per-frame caster budget, or that are past `shadow_distance`, stay lit and unshadowed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cast_shadows: bool,
    /// Depth bias in world units, as on `DirectionalLight`, validated `>= 0` (default `0.02`).
    #[serde(
        default = "local_shadow_bias_default",
        skip_serializing_if = "is_local_shadow_bias_default"
    )]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.5]))]
    pub shadow_bias: f32,
    /// The shadow view's near plane in metres from the light (default `0.05`); the far plane is
    /// `range`. Finite, above 0 and below `range` (`L1312`).
    #[serde(
        default = "local_shadow_near_default",
        skip_serializing_if = "is_local_shadow_near_default"
    )]
    #[schemars(extend("x-ui-range" = [0.01, 1.0]))]
    pub shadow_near: f32,
    /// Camera distance past which the light casts no shadow (default none: no limit). Measured to
    /// the surface of the cone's bounding sphere (the footprint the caster budget ranks by, not the
    /// full `range` sphere). A light at or past it is not a candidate and takes no atlas
    /// cells; it still lights fully. Finite and above 0 (`L1322`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_distance: Option<f32>,
    /// Length of the band before `shadow_distance` over which the shadow fades from full to none,
    /// by smoothstep (default `0`: a hard cut). Needs `shadow_distance` and is at most it (`L1322`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    #[schemars(extend("x-ui-range" = [0.0, 50.0]))]
    pub shadow_fade: f32,
    /// How strongly the light scatters into the `PostProcess` medium (default `0`: not at all).
    /// `1.0` is physical for the medium; only lights that opt in pay for it (`ludion docs atmosphere`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub scatter: f32,
}

impl Default for SpotLight {
    fn default() -> Self {
        SpotLight {
            color: [1.0; 3],
            intensity: 1.0,
            range: 10.0,
            direction: [0.0, -1.0, 0.0],
            inner_angle: 20.0,
            outer_angle: 30.0,
            cast_shadows: false,
            shadow_bias: local_shadow_bias_default(),
            shadow_near: local_shadow_near_default(),
            shadow_distance: None,
            shadow_fade: 0.0,
            scatter: 0.0,
        }
    }
}

impl SemanticValidate for SpotLight {
    /// `color` is an HDR colour (`L1263`); `intensity` and `range` ride their schemars bounds. A
    /// zero-length or non-finite `direction`, an angle outside `[0, 90)` or non-finite, and an
    /// `inner_angle` above `outer_angle` are `L1272`. An absent angle compares as its default, so
    /// `inner_angle: 40` alone is caught. A `shadow_near` that is non-finite, at most 0, or not
    /// below `range` is `L1312`. A wrong *shape* is the schema's job (`L1014`) and is passed
    /// through here.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = value_color_diagnostics("SpotLight", value, "color");
        out.extend(local_shadow_diagnostics(
            value,
            f64::from(SpotLight::default().range),
        ));
        out.extend(local_shadow_distance_diagnostics(value));
        if let Some(direction) = value.get("direction").and_then(Value::as_array) {
            let components: Vec<f64> = direction.iter().filter_map(as_stored_f32).collect();
            if components.len() == 3 {
                let finite = components.iter().all(|c| c.is_finite());
                let length_squared: f64 = components.iter().map(|c| c * c).sum();
                if !finite || length_squared == 0.0 {
                    out.push(spot_light_invalid_direction("/direction"));
                }
            }
        }
        let default = SpotLight::default();
        let mut angles = [
            ("inner_angle", f64::from(default.inner_angle)),
            ("outer_angle", f64::from(default.outer_angle)),
        ];
        let mut angles_valid = true;
        for (name, angle) in &mut angles {
            if let Some(authored) = value.get(*name).and_then(as_stored_f32) {
                *angle = authored;
                if !authored.is_finite() || !(0.0..90.0).contains(&authored) {
                    out.push(spot_light_invalid_angle(name, authored));
                    angles_valid = false;
                }
            }
        }
        let [(_, inner), (_, outer)] = angles;
        if angles_valid && inner > outer {
            out.push(spot_light_inverted_cone(inner, outer));
        }
        out
    }
}

/// `L1272` — a `SpotLight`'s direction is zero-length or non-finite.
fn spot_light_invalid_direction(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::SPOT_LIGHT_INVALID_CONE,
        "the `SpotLight` direction is zero-length or non-finite; it must be a non-zero, finite \
         vector to normalize into the cone's axis",
    )
    .with_path(pointer.to_string())
    .with_help("give the direction a non-zero, finite `[x, y, z]` (e.g. `[0, -1, 0]`)")
}

/// `L1272` — a `SpotLight` angle is non-finite or outside `[0, 90)`.
fn spot_light_invalid_angle(name: &str, angle: f64) -> Diagnostic {
    Diagnostic::new(
        code::SPOT_LIGHT_INVALID_CONE,
        format!(
            "the `SpotLight` {name} {angle} is outside [0, 90) degrees; the angles are \
             half-angles from the cone's axis, and 90 or more is no longer a cone"
        ),
    )
    .with_path(format!("/{name}"))
    .with_help("use a finite half-angle below 90 degrees (defaults 20 and 30)")
}

/// `L1272` — a `SpotLight`'s core is wider than its cone.
fn spot_light_inverted_cone(inner: f64, outer: f64) -> Diagnostic {
    Diagnostic::new(
        code::SPOT_LIGHT_INVALID_CONE,
        format!(
            "the `SpotLight` inner_angle {inner} is greater than its outer_angle {outer}, so the \
             full-brightness core would be wider than the cone"
        ),
    )
    .with_path("/inner_angle")
    .with_help("make `inner_angle` at most `outer_angle` (equal gives a hard edge)")
}

/// `L1312` for a `PointLight`/`SpotLight`: an authored `shadow_near` that is non-finite or `<= 0`,
/// or a casting (or near-authoring) light whose near plane is not below its range. A non-casting
/// light that authors no `shadow_near` is never refused.
fn local_shadow_diagnostics(value: &Value, default_range: f64) -> Vec<Diagnostic> {
    let authored_near = value.get("shadow_near").and_then(as_stored_f32);
    if let Some(near) = authored_near
        && (!near.is_finite() || near <= 0.0)
    {
        return vec![local_light_invalid_shadow_near(near, None)];
    }
    let casts = value
        .get("cast_shadows")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if authored_near.is_some() || casts {
        let near = authored_near.unwrap_or_else(|| f64::from(local_shadow_near_default()));
        let range = value
            .get("range")
            .and_then(as_stored_f32)
            .unwrap_or(default_range);
        if range.is_finite() && range > 0.0 && near >= range {
            return vec![local_light_invalid_shadow_near(near, Some(range))];
        }
    }
    Vec::new()
}

/// `L1312` — a local light's `shadow_near` is unusable; `range` is set for the not-below-range case.
fn local_light_invalid_shadow_near(near: f64, range: Option<f64>) -> Diagnostic {
    let message = match range {
        Some(range) => format!(
            "the light's shadow_near {near} is not below its range {range}, so the shadow view \
             would see nothing"
        ),
        None => format!("the light's shadow_near {near} is non-finite or not above 0"),
    };
    Diagnostic::new(code::LOCAL_LIGHT_INVALID_SHADOW, message)
        .with_path("/shadow_near")
        .with_help("use a finite `shadow_near` above 0 and below `range` (default 0.05)")
}

/// `L1322` for a `PointLight`/`SpotLight`: an authored `shadow_distance` that is non-finite or
/// `<= 0`, or an authored `shadow_fade` that is non-finite, negative, set without a distance, or
/// longer than it. `shadow_fade: 0` without a distance is the default and accepted.
fn local_shadow_distance_diagnostics(value: &Value) -> Vec<Diagnostic> {
    let distance = value.get("shadow_distance").and_then(as_stored_f32);
    let fade = value.get("shadow_fade").and_then(as_stored_f32);
    let mut out = Vec::new();
    if let Some(d) = distance
        && (!d.is_finite() || d <= 0.0)
    {
        out.push(
            Diagnostic::new(
                code::LOCAL_LIGHT_INVALID_SHADOW_DISTANCE,
                format!("the light's shadow_distance {d} is non-finite or not above 0"),
            )
            .with_path("/shadow_distance")
            .with_help("use a finite `shadow_distance` above 0, or remove it for no limit"),
        );
    }
    let distance_ok = distance.filter(|d| d.is_finite() && *d > 0.0);
    if let Some(f) = fade {
        let message = if !f.is_finite() || f < 0.0 {
            Some(format!(
                "the light's shadow_fade {f} is non-finite or negative"
            ))
        } else if f > 0.0 && distance.is_none() {
            Some(format!(
                "the light's shadow_fade {f} has no shadow_distance to fade before"
            ))
        } else if let Some(d) = distance_ok
            && f > d
        {
            Some(format!(
                "the light's shadow_fade {f} is longer than its shadow_distance {d}"
            ))
        } else {
            None
        };
        if let Some(message) = message {
            out.push(
                Diagnostic::new(code::LOCAL_LIGHT_INVALID_SHADOW_DISTANCE, message)
                    .with_path("/shadow_fade")
                    .with_help(
                        "set `shadow_distance` above 0 and keep `shadow_fade` between 0 and it",
                    ),
            );
        }
    }
    out
}

/// A reflection probe: a captured cube of the scene around the entity, projected onto a box or
/// sphere proxy so reflections show the right parallax.
///
/// The [`Transform`] gives the capture point and the box's orientation; its scale is ignored, the
/// size lives in `shape`. The one oriented volume is both the influence region and the parallax
/// proxy. The weight is 1 inside the volume and `blend` is the distance outside it over which the
/// weight falls to 0, so a box fitted to its room gives the walls and floor full weight. Where
/// volumes overlap the higher `priority` wins (then boxes before spheres, then the smaller volume,
/// then entity id).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct ReflectionProbe {
    /// The influence and parallax volume (default a box of half-extents `[2, 2, 2]`).
    #[serde(default)]
    pub shape: ProbeShape,
    /// Outward fade distance in world units (default `0.5`), validated `>= 0` (`L1274`).
    #[serde(default = "probe_blend")]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub blend: f32,
    /// Higher wins where volumes overlap (default `0`).
    #[serde(default)]
    #[schemars(extend("x-ui-range" = [-8, 8]))]
    pub priority: i32,
}

/// A [`ReflectionProbe`]'s volume, externally tagged: `shape: { box: { extent: [4, 2, 3] } }` or
/// `shape: { sphere: { radius: 3 } }`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeShape {
    /// An oriented box.
    Box {
        /// Half-extents along the probe's local axes (each validated `> 0`, default `[2, 2, 2]`).
        #[serde(default = "probe_extent")]
        extent: [f32; 3],
    },
    /// A sphere about the probe's position.
    Sphere {
        /// Radius in world units (validated `> 0`, default `2`).
        #[serde(default = "probe_radius")]
        radius: f32,
    },
}

impl Default for ProbeShape {
    fn default() -> Self {
        ProbeShape::Box {
            extent: probe_extent(),
        }
    }
}

impl Default for ReflectionProbe {
    fn default() -> Self {
        ReflectionProbe {
            shape: ProbeShape::default(),
            blend: probe_blend(),
            priority: 0,
        }
    }
}

impl SemanticValidate for ReflectionProbe {
    /// A non-finite or non-positive `extent` component or `radius`, and a non-finite or negative
    /// `blend`, are `L1274` at the offending field. A wrong *shape* is the schema's job (`L1014`)
    /// and is passed through here.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let positive = |v: f64| v.is_finite() && v > 0.0;
        if let Some(shape) = value.get("shape") {
            if let Some(extent) = shape
                .get("box")
                .and_then(|b| b.get("extent"))
                .and_then(Value::as_array)
            {
                for (i, component) in extent.iter().enumerate() {
                    if let Some(authored) = as_stored_f32(component)
                        && !positive(authored)
                    {
                        out.push(reflection_probe_invalid(
                            &format!("/shape/box/extent/{i}"),
                            &format!("extent component {authored}"),
                        ));
                    }
                }
            }
            if let Some(radius) = shape
                .get("sphere")
                .and_then(|s| s.get("radius"))
                .and_then(as_stored_f32)
                && !positive(radius)
            {
                out.push(reflection_probe_invalid(
                    "/shape/sphere/radius",
                    &format!("radius {radius}"),
                ));
            }
        }
        if let Some(blend) = value.get("blend").and_then(as_stored_f32)
            && !(blend.is_finite() && blend >= 0.0)
        {
            out.push(
                Diagnostic::new(
                    code::REFLECTION_PROBE_INVALID,
                    format!(
                        "the `ReflectionProbe` blend {blend} is negative or non-finite; it is a \
                         distance outside the volume"
                    ),
                )
                .with_path("/blend")
                .with_help("use a finite blend of 0 or more (default 0.5)"),
            );
        }
        out
    }
}

/// `L1274` — a `ReflectionProbe` extent component or radius is not a finite value above 0.
fn reflection_probe_invalid(pointer: &str, what: &str) -> Diagnostic {
    Diagnostic::new(
        code::REFLECTION_PROBE_INVALID,
        format!(
            "the `ReflectionProbe` {what} is not a finite value above 0, so the volume is empty"
        ),
    )
    .with_path(pointer.to_string())
    .with_help("give every extent component and the radius a finite value above 0 (default 2)")
}

/// A directional "sun" light: one global light with a parallel direction, casting a single
/// orthographic shadow map.
///
/// Unlike [`PointLight`], the light's *direction* is an explicit field (`[x, y, z]`, normalized at
/// extract) rather than the entity's Transform rotation — a hand-authored vector reads far more
/// legibly than a quaternion, and the component stays agent-legible as text. The entity's
/// [`Transform`] translation is not ignored: it anchors the **center** of the shadow frustum, so a
/// game can parent the sun to the player and the shadow window follows the action (a static scene
/// just leaves it at the origin). The shadow tunables are explicit fields with defaults, so the
/// anchored box is deterministic trivially; setting `cascades` opts into a camera fit instead.
///
/// `extract()` resolves it into `RenderScene.sun`, which drives the mesh shader's sun term and the
/// depth-only shadow pass. A sunless scene renders through a zeroed `Sun` uniform — byte-identical
/// to the pre-sun pipeline.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct DirectionalLight {
    /// Direction the light travels, `[x, y, z]` (default `[-0.3, -1.0, -0.2]` — a slightly angled
    /// overhead sun, which casts legible ground shadows where a straight-down sun would hide them
    /// directly beneath objects). Need not be unit length; normalized at extract. A zero-length or
    /// non-finite direction is rejected by the semantic validator (`L1123`).
    #[serde(default = "sun_direction")]
    pub direction: [f32; 3],
    /// Linear RGB color (default white).
    #[serde(default = "white3")]
    pub color: [f32; 3],
    /// Brightness multiplier, premultiplied into `color` at extract, validated `>= 0` (default
    /// `1.0`).
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub intensity: f32,
    /// Orthographic half-size of the shadow frustum, in world units, validated `> 0` (default
    /// `40.0`).
    #[serde(default = "forty")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub shadow_extent: f32,
    /// Near plane of the shadow frustum (default `1.0`); must be less than `shadow_far` (`L1123`).
    #[serde(default = "one")]
    pub shadow_near: f32,
    /// Far plane of the shadow frustum (default `200.0`); must exceed `shadow_near` (`L1123`).
    #[serde(default = "two_hundred")]
    pub shadow_far: f32,
    /// Depth bias applied when sampling the shadow map, to fight acne (default `0.05`), in world
    /// units at a 2048² map: independent of `shadow_near`/`shadow_far`, grown with the texel at
    /// lower shadow quality, beside a slope-scaled bias the renderer adds. Validated `>= 0`.
    #[serde(default = "shadow_bias_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.5]))]
    pub shadow_bias: f32,
    /// Angular **radius** of the sun's disc, in degrees (default `0.0`, a point light), validated to
    /// `[0, 45]` by `L1282`. Above 0 the sun's **specular** uses the closest direction on the disc to
    /// the mirror ray, so a glossy surface shows a soft disc; diffuse is unchanged. The sun and the
    /// moon are about `0.27`. Pair it with `Environment.sun_disc_size = 2 ×` this for a matching sky
    /// disc.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(extend("x-ui-range" = [0.0, 2.0]))]
    pub angular_radius: f32,
    /// Cascade count (1–4). Absent keeps the anchored shadow box exactly; set, the sun fits that
    /// many cascades to the active camera, capped per quality by `sun_cascades`. Under cascades
    /// `shadow_extent` and the `Transform` anchor are unused by the camera's shadows;
    /// reflection-probe captures still use them (`L1313`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 4))]
    pub cascades: Option<u32>,
    /// View depth in metres that the last cascade reaches, validated `> 0` (default `100.0`). Read
    /// only under `cascades`.
    #[serde(
        default = "shadow_distance_default",
        skip_serializing_if = "is_shadow_distance_default"
    )]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    #[schemars(extend("x-ui-range" = [1.0, 500.0]))]
    pub shadow_distance: f32,
    /// The inner cascades' far bounds as fractions of `shadow_distance`: `cascades - 1` values,
    /// strictly ascending in (0, 1) (`L1123`). Absent uses the engine's practical split scheme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cascade_splits: Option<Vec<f32>>,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        DirectionalLight {
            direction: [-0.3, -1.0, -0.2],
            color: [1.0; 3],
            intensity: 1.0,
            shadow_extent: 40.0,
            shadow_near: 1.0,
            shadow_far: 200.0,
            shadow_bias: 0.05,
            angular_radius: 0.0,
            cascades: None,
            shadow_distance: shadow_distance_default(),
            cascade_splits: None,
        }
    }
}

/// A document number as an `f32` field will hold it: a value past `f32` range reads as `±inf`, so
/// the finiteness checks of a validator run on the stored number, not on the wider `f64`. Use it
/// only where the Rust field is `f32`. Underflow (`1e-46` narrows to `0.0`) is not mapped here: a
/// strictly-positive check calls `ludion_core::number::positive_f32` at its site.
#[doc(hidden)]
pub fn as_stored_f32(value: &Value) -> Option<f64> {
    value.as_f64().map(engine_number)
}

impl SemanticValidate for DirectionalLight {
    /// Rejects a `direction` that is zero-length or non-finite and a `shadow_near` that is not less
    /// than `shadow_far` (`L1123`, pointers `/direction` and `/shadow_far`, or `/shadow_near` if far
    /// is not authored), `cascade_splits` that need `cascades`, miss its `cascades - 1` length or
    /// are not strictly ascending in (0, 1) (`L1123`, pointer `/cascade_splits` or
    /// `/cascade_splits/{i}`), an `angular_radius` that is non-finite or outside `[0, 45]` degrees
    /// (`L1282`, pointer `/angular_radius`), and checks `color` as an HDR colour (`L1263`). An absent depth bound
    /// compares as its default, so `shadow_near: 300` alone is caught. Defensive: runs on a
    /// schema-clean value; a wrong *shape* (missing, non-array, wrong arity) is the schema's job
    /// (`L1014`) and is passed through here. An absent field (the default is valid) adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = value_color_diagnostics("DirectionalLight", value, "color");
        if let Some(direction) = value.get("direction").and_then(Value::as_array) {
            let components: Vec<f64> = direction.iter().filter_map(as_stored_f32).collect();
            if components.len() == 3 {
                let finite = components.iter().all(|c| c.is_finite());
                let length_squared: f64 = components.iter().map(|c| c * c).sum();
                if !finite || length_squared == 0.0 {
                    out.push(directional_light_invalid_direction("/direction"));
                }
            }
        }
        if let Some(radius) = value.get("angular_radius").and_then(as_stored_f32)
            && !(0.0..=45.0).contains(&radius)
        {
            out.push(directional_light_invalid_angular_radius(
                "/angular_radius",
                radius,
            ));
        }
        let default = DirectionalLight::default();
        let near = value.get("shadow_near").and_then(as_stored_f32);
        let far = value.get("shadow_far").and_then(as_stored_f32);
        if near.is_some() || far.is_some() {
            let near_value = near.unwrap_or(f64::from(default.shadow_near));
            let far_value = far.unwrap_or(f64::from(default.shadow_far));
            if near_value >= far_value {
                let pointer = if far.is_some() {
                    "/shadow_far"
                } else {
                    "/shadow_near"
                };
                out.push(directional_light_inverted_depth(
                    pointer, near_value, far_value,
                ));
            }
        }
        out.extend(cascade_splits_diagnostics(value));
        out
    }
}

/// `L1123` — authored `cascade_splits` that the light cannot turn into cascades: no `cascades`, the
/// wrong count, or a value that is not strictly ascending inside (0, 1). One diagnostic at most.
fn cascade_splits_diagnostics(value: &Value) -> Vec<Diagnostic> {
    const EXAMPLE: &str = "write `cascades: 4, cascade_splits: [0.06, 0.15, 0.4]`: cascades - 1 \
                           fractions of `shadow_distance`, strictly ascending in (0, 1)";
    let Some(splits) = value.get("cascade_splits").and_then(Value::as_array) else {
        return Vec::new();
    };
    let invalid = |pointer: String, message: String| {
        vec![
            Diagnostic::new(code::DIRECTIONAL_LIGHT_INVALID_FRUSTUM, message)
                .with_path(pointer)
                .with_help(EXAMPLE),
        ]
    };
    let Some(cascades) = value.get("cascades").and_then(Value::as_u64) else {
        return invalid(
            "/cascade_splits".to_string(),
            "the `DirectionalLight` authors `cascade_splits` without `cascades`, so there are no \
             cascades to split"
                .to_string(),
        );
    };
    if splits.len() as u64 + 1 != cascades {
        return invalid(
            "/cascade_splits".to_string(),
            format!(
                "the `DirectionalLight` has {cascades} cascades, which need {} `cascade_splits`, \
                 but {} are authored",
                cascades - 1,
                splits.len()
            ),
        );
    }
    let mut previous = 0.0_f64;
    for (index, split) in splits.iter().enumerate() {
        let Some(bound) = as_stored_f32(split) else {
            continue;
        };
        if !bound.is_finite() || bound <= previous || bound >= 1.0 {
            return invalid(
                format!("/cascade_splits/{index}"),
                format!(
                    "the `DirectionalLight` cascade_splits[{index}] is {bound}, which is not \
                     finite, not inside (0, 1), or not above the previous split ({previous})"
                ),
            );
        }
        previous = bound;
    }
    Vec::new()
}

/// `L1123` — a `DirectionalLight`'s shadow depth range is empty or inverted.
fn directional_light_inverted_depth(pointer: &str, near: f64, far: f64) -> Diagnostic {
    Diagnostic::new(
        code::DIRECTIONAL_LIGHT_INVALID_FRUSTUM,
        format!(
            "the `DirectionalLight` shadow depth range is empty: shadow_near {near} is not less \
             than shadow_far {far}, so the shadow frustum has no depth"
        ),
    )
    .with_path(pointer.to_string())
    .with_help("make `shadow_far` greater than `shadow_near` (defaults 1.0 and 200.0)")
}

/// `L1123` — a `DirectionalLight`'s direction is zero-length or non-finite.
fn directional_light_invalid_direction(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::DIRECTIONAL_LIGHT_INVALID_FRUSTUM,
        "the `DirectionalLight` direction is zero-length or non-finite; it must be a non-zero, \
         finite vector to normalize into a light direction",
    )
    .with_path(pointer.to_string())
    .with_help("give the direction a non-zero, finite `[x, y, z]` (e.g. `[-0.3, -1.0, -0.2]`)")
}

/// `L1282` — a `DirectionalLight`'s angular radius is non-finite or outside `[0, 45]` degrees.
fn directional_light_invalid_angular_radius(pointer: &str, radius: f64) -> Diagnostic {
    Diagnostic::new(
        code::DIRECTIONAL_LIGHT_INVALID_ANGULAR_RADIUS,
        format!(
            "the `DirectionalLight` angular_radius {radius} is non-finite or outside [0, 45] degrees"
        ),
    )
    .with_path(pointer.to_string())
    .with_help(
        "use a finite angular radius in [0, 45] degrees; the moon and the sun are about 0.27 \
         (default 0, a point light)",
    )
}

/// The scene's sky and lighting environment: an authored-gradient procedural sky, a flat ambient
/// term, and the IBL knobs. **Opt-in singleton** — the lowest-`EntityId` `Environment` wins
/// (the `DirectionalLight` precedent), and its **absence is meaningful**: a scene with no
/// `Environment` keeps the engine's hardcoded ambient/clear pipeline byte-for-byte, so adopting the
/// component is what opts a scene into the sky and image-based lighting.
///
/// This component owns *lighting* only; *imaging* — what the post stack does to the rendered image —
/// is [`PostProcess`]'s job.
///
/// Every field is `#[serde(default)]` with a behavior-preserving default, so authoring only a sky
/// gradient leaves ambient untouched (and an empty `Environment: {}` is the documented default
/// environment). Colors are **linear RGB and HDR-capable** — values may exceed 1.0.
///
/// `extract()` resolves it into `RenderScene.environment`, which drives the sky triangle, the
/// prefiltered IBL, and ambient. A scene with no `Environment` keeps the engine's historical
/// ambient/clear pipeline byte-for-byte.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    /// Sky gradient color straight up (the zenith), linear RGB, HDR-capable (default a dim night
    /// blue). May exceed 1.0.
    #[serde(default = "sky_zenith_default")]
    pub sky_zenith: [f32; 3],
    /// Sky gradient color at the horizon, linear RGB, HDR-capable (default a slightly brighter dusk
    /// tone). May exceed 1.0.
    #[serde(default = "sky_horizon_default")]
    pub sky_horizon: [f32; 3],
    /// Sky gradient color straight down (below the horizon — the "ground" hemisphere of the
    /// gradient), linear RGB, HDR-capable (default a near-black). May exceed 1.0.
    #[serde(default = "sky_ground_default")]
    pub sky_ground: [f32; 3],
    /// Angular diameter of the sun/moon disc drawn in the sky, in **degrees** (default `0.0` — no
    /// disc). The disc's **direction is not authored here** — it is read from the scene's sun
    /// (`DirectionalLight`) entity at render time, so the disc sits where the sun light comes from.
    /// The disc is drawn in the sky background only and excluded from the IBL prefilter (no
    /// double-counting with the analytic sun).
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 10.0]))]
    pub sun_disc_size: f32,
    /// Brightness of the sun/moon disc, linear (HDR), validated `>= 0` (default `1.0`). Only drawn
    /// when `sun_disc_size > 0`.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 10.0]))]
    pub sun_disc_intensity: f32,
    /// Flat ambient floor added to every lit surface (`ambient × diffuse` on meshes, `ambient` on lit
    /// sprites), linear RGB (default `[0, 0, 0]`: under an `Environment` the sky's IBL is the
    /// ambient, and this is an extra floor on top of it, not scaled by `ibl_intensity`).
    /// `extract()` sources `RenderScene.ambient` from it.
    #[serde(default = "ambient_default")]
    pub ambient: [f32; 3],
    /// Overall image-based-lighting intensity multiplier, validated `>= 0` (default `1.0`). Scales
    /// the prefiltered sky's contribution to lit surfaces.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub ibl_intensity: f32,
}

impl Default for Environment {
    fn default() -> Self {
        Environment {
            sky_zenith: sky_zenith_default(),
            sky_horizon: sky_horizon_default(),
            sky_ground: sky_ground_default(),
            sun_disc_size: 0.0,
            sun_disc_intensity: 1.0,
            ambient: ambient_default(),
            ibl_intensity: 1.0,
        }
    }
}

impl SemanticValidate for Environment {
    /// Rejects a negative or non-finite color component (`L1128`, e.g. `/sky_zenith/0`) in any of the
    /// four color fields — the invariant JSON Schema's `range` cannot reach (it does not apply to
    /// array elements), so it lives here (the `Collider.half_extents` precedent). The scalar fields
    /// carry their own schemars `range(min = 0)`, so a negative scalar is the schema's `L1014` and is
    /// not re-checked here. Defensive: runs on a schema-clean value; an absent field (its default is
    /// valid) or a wrong *shape* adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for field in ["sky_zenith", "sky_horizon", "sky_ground", "ambient"] {
            let Some(color) = value.get(field).and_then(Value::as_array) else {
                continue;
            };
            for (index, element) in color.iter().enumerate() {
                if let Some(component) = as_stored_f32(element)
                    && (!component.is_finite() || component < 0.0)
                {
                    out.push(environment_invalid_color(&format!("/{field}/{index}")));
                }
            }
        }
        out
    }
}

/// `L1128` — an `Environment` color component is negative or non-finite.
fn environment_invalid_color(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::ENVIRONMENT_INVALID_FIELD,
        "this `Environment` color component is negative or non-finite; a color is HDR (may exceed \
         1.0) but must be finite and non-negative",
    )
    .with_path(pointer.to_string())
    .with_help(
        "give every color component a finite, non-negative value (HDR values above 1.0 are fine)",
    )
}

/// How a four-element colour's alpha is checked by `L1263`.
#[derive(Clone, Copy)]
#[doc(hidden)]
pub enum AlphaRule {
    /// A coverage weight within `[0, 1]`: premultiplied `over` turns `a > 1` into a negative
    /// destination weight.
    Coverage,
    /// Finite and `>= 0`: the consumer clamps alpha to 1 at draw (a particle's fade).
    Clamped,
}

/// `L1263` — the colour rules for every HDR colour that reaches the scene target: each RGB element
/// finite and non-negative, and, in a four-element colour, alpha within `[0, 1]`. The scene target
/// is linear radiance, so a negative colour subtracts light, and premultiplied `over` turns `a > 1`
/// into a negative destination weight. `pointer` is the colour field (`/color`); each finding
/// appends its element index. (A particle's alpha is the exception: `>= 0`, see [`AlphaRule`].)
#[doc(hidden)]
pub fn color_diagnostics(owner: &str, pointer: &str, elements: &[f64]) -> Vec<Diagnostic> {
    elements
        .iter()
        .enumerate()
        .filter_map(|(index, &component)| {
            color_component_diagnostic(
                owner,
                pointer,
                index,
                elements.len(),
                component,
                AlphaRule::Coverage,
            )
        })
        .collect()
}

/// [`color_diagnostics`] over the `field` of a schema-clean component value. An absent field (its
/// default is valid) or a non-array adds nothing; a non-number element is the schema's job and is
/// skipped, keeping the true index of the rest.
#[doc(hidden)]
pub fn value_color_diagnostics(owner: &str, value: &Value, field: &str) -> Vec<Diagnostic> {
    value_color_diagnostics_with(owner, value, field, AlphaRule::Coverage)
}

/// [`value_color_diagnostics`] with the alpha rule chosen.
#[doc(hidden)]
pub fn value_color_diagnostics_with(
    owner: &str,
    value: &Value,
    field: &str,
    alpha: AlphaRule,
) -> Vec<Diagnostic> {
    let Some(array) = value.get(field).and_then(Value::as_array) else {
        return Vec::new();
    };
    let pointer = format!("/{field}");
    array
        .iter()
        .enumerate()
        .filter_map(|(index, element)| {
            color_component_diagnostic(
                owner,
                &pointer,
                index,
                array.len(),
                as_stored_f32(element)?,
                alpha,
            )
        })
        .collect()
}

/// One element of a colour of `len` elements: alpha is element 3 of four, everything else is RGB.
fn color_component_diagnostic(
    owner: &str,
    pointer: &str,
    index: usize,
    len: usize,
    component: f64,
    alpha: AlphaRule,
) -> Option<Diagnostic> {
    let (valid, rule, help) = if index == 3 && len == 4 {
        match alpha {
            AlphaRule::Coverage => (
                (0.0..=1.0).contains(&component),
                "alpha is a coverage weight and must be within 0..=1",
                "keep alpha within 0..=1; brighten with the RGB components",
            ),
            AlphaRule::Clamped => (
                component.is_finite() && component >= 0.0,
                "alpha must be finite and 0 or more",
                "give alpha 0 or more; the particle shader clamps it to 1, so a value above 1 \
                 holds full opacity longer through the fade",
            ),
        }
    } else {
        (
            component.is_finite() && component >= 0.0,
            "a colour is HDR (it may exceed 1.0) but each RGB component must be finite and \
             non-negative",
            "give every RGB component 0 or more (HDR values above 1.0 are fine)",
        )
    };
    (!valid).then(|| {
        Diagnostic::new(
            code::COLOR_COMPONENT_INVALID,
            format!("{owner} colour component {component} at index {index} is not valid; {rule}"),
        )
        .with_path(format!("{pointer}/{index}"))
        .with_help(help)
    })
}

/// A per-instance screen-door dither fade: a deterministic Bayer-matrix discard on the opaque mesh
/// pipeline (blend and depth-write untouched), with an optional world-height threshold. This is the
/// **render-only** occlusion-plumbing input — WHO fades and when is game policy (a camera→player
/// raycast with hysteresis, kept in the game); this component just carries the numbers the
/// renderer discards fragments with.
///
/// A mesh without its own `Fade` takes its nearest ancestor's through `Parent`, so fading a prefab's
/// root fades its parts; a part's own `Fade` wins.
///
/// Beauty pass only: shadows and point/spot lights ignore `Fade`; hide an entity with [`Visibility`].
///
/// `Mesh.color`'s alpha is **deliberately not repurposed** for this: that alpha is a tint channel
/// (currently unblended on the opaque path) and stays so — fade rides its own instance lane.
///
/// `extract()` carries it onto the mesh instance, where the mesh shader's Bayer-dither discard
/// consumes it. The default `(1.0, -f32::MAX)` can never discard, so a `Fade`-free scene renders
/// byte-identically to the pre-fade pipeline.
///
/// `fade`'s `[0, 1]` bound is fully expressible as a schemars `range`, so it rides schema validation
/// (`L1014` for an out-of-range value) like `Mesh`'s scalar material fields — no dedicated semantic
/// code is needed (and `height_min` is unbounded by design).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    /// Visibility amount, validated to `[0, 1]` (default `1.0` — fully visible). `0.0` dithers the
    /// instance fully away; intermediate values discard a Bayer-patterned fraction of fragments.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub fade: f32,
    /// World-space height (Y) threshold: only fragments **above** this height fade; those at or below
    /// it stay fully opaque (default `-f32::MAX` — every fragment is eligible to fade). Set it to a
    /// world height to top-fade only the upper stories of an occluder.
    #[serde(default = "neg_f32_max")]
    pub height_min: f32,
}

impl Default for Fade {
    fn default() -> Self {
        Fade {
            fade: 1.0,
            height_min: -f32::MAX,
        }
    }
}

/// Hides an entity, or only its shadow, from the extracted scene.
///
/// `visible: false` removes the entity from every render pass: its mesh, sprite, tilemap cell, text,
/// ground marker, drop shadow, decal, particles, volley bullets, point light and spot light are not
/// extracted, so it appears in no prepass, shadow pass or probe capture, and a hidden light lights
/// nothing. `cast_shadows: false` keeps the entity on screen but out of the sun map, the cascades,
/// the local shadow atlas and drop-shadow occlusion. `visible: false` always wins over
/// `cast_shadows`.
///
/// Effective visibility is an AND down the [`Parent`] chain: an entity is hidden
/// when it or **any** ancestor has `visible: false`, and a part's own `visible: true` never shows it
/// under a hidden root. `cast_shadows` folds the same way, so a shadowless root makes its whole tree
/// shadowless. A `ParentJoint` child always has a `Parent`, so it follows its parent. (This differs
/// from [`Fade`], where the nearest own component wins. Use `Fade` to dither a mesh that occludes the
/// player; use `Visibility` to hide it outright.)
///
/// Both fields are omitted from text and from the hash at their defaults, and `Visibility: {}` is the
/// default. A world without the component extracts byte-identically and pays one O(1) check.
///
/// Particles and volley bullets are closed forms of time: hiding skips them and touches no state, so
/// on unhide they show what their time function gives. A hidden mesh is still placed (never
/// resolved, skinned or drawn), and the static GI bake ignores the flag, so a toggle never moves the
/// bake signature. `render.bounds` still reports hidden entities. The component is hashed on sim
/// entities only, because presentation entities are never hashed.
///
/// Not affected: cameras, `DirectionalLight`, `Environment`, `PostProcess`, `GiVolume`,
/// `ReflectionProbe`, audio, physics colliders (a hidden entity still collides) and the UI
/// components, which keep their own `visible`.
///
/// ```yaml
/// components:
///   Visibility:
///     visible: false
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Visibility {
    /// Whether the entity, and every `Parent` descendant, is extracted (default `true`).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// Whether the entity, and every `Parent` descendant, casts shadows (default `true`).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub cast_shadows: bool,
}

impl Default for Visibility {
    fn default() -> Self {
        Visibility {
            visible: true,
            cast_shadows: true,
        }
    }
}

/// Which tonemap operator maps the composited HDR image onto the 8-bit output.
///
/// **Every variant carries a doc comment**, which is what makes schemars emit the `oneOf`-of-`const`
/// shape the editor inspector renders as a dropdown (see [`Anchor`] for the full rule).
///
/// Choosing an operator never changes the frame graph: all four are branches inside the one
/// `composite` pass, because a tonemap curve is a pure per-pixel function of already-sampled colour
/// (`Plan/post-process-pipeline.md` decision 2).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TonemapOperator {
    /// AgX: a log-encoded sigmoid in a Rec.2020 working space, and the engine's shipped operator.
    /// Bright saturated colours desaturate gracefully toward white the way film does, instead of
    /// skewing hue on the way to clipping. It ships deliberately muted and expects a grade on top.
    #[default]
    Agx,
    /// The Narkowicz ACES filmic fit. Punchy and cheap, but it shifts bright saturated colours toward
    /// yellow and then white as they clip — the failure case a night city lit by saturated torch
    /// light and emissive props hits hardest.
    Aces,
    /// The Khronos PBR Neutral operator: hue-preserving, leaving everything below its compression
    /// knee exactly as authored and desaturating only the highlights. The right choice when a
    /// surface's authored colour must survive to the screen.
    Neutral,
    /// No operator at all: the linear value passes straight through and the 8-bit output clamps it.
    /// A diagnostic setting — it shows what the scene pass actually produced, so an over-bright
    /// scene reads as flat white rather than as a curve's shoulder.
    None,
}

/// The scene's post-process (imaging) settings. **Opt-in singleton** — the lowest-`EntityId`
/// `PostProcess` wins (the [`Environment`] / [`DirectionalLight`] precedent), and its **absence is
/// meaningful**: a scene with no `PostProcess` renders through the engine's hardcoded post defaults
/// byte-for-byte.
///
/// The split with [`Environment`] is by subject: `Environment` authors *lighting* (sky, ambient,
/// image-based lighting), `PostProcess` authors *imaging* — what the post stack does to the image
/// the scene pass produced.
///
/// Every field is `#[serde(default)]` with a behavior-preserving default, so an empty
/// `PostProcess: {}` is exactly the engine's shipped imaging and renders identically to no component
/// at all. Every **scalar** rides its schemars `range` for validation (the `Fade` posture): an
/// out-of-range value is the schema's `L1014`, and a non-finite one cannot be authored — YAML's
/// `.nan` and `.inf` are unrepresentable in the scene format, and JSON has no spelling for them on
/// the wire. A misspelled [`TonemapOperator`] is the closed enum's own `L1014`, at the same pointer
/// and naming the allowed values. The one invariant left over is the **triples**, whose elements a
/// schemars `range` cannot reach — `gain`, `gamma` and `fog_color` live in this component's semantic
/// validator as `L1130`.
///
/// `extract()` resolves it into `RenderScene.post`, which the renderer reads for the bloom
/// bright-pass threshold and the composite pass's knobs (mix strength, fog, exposure, operator,
/// grade, vignette, dither).
///
/// # The composite's pinned order
///
/// Everything but the bloom chain happens in one pass, in one order, and that order is part of this
/// component's contract:
///
/// **chromatic aberration** (in the scene fetch) → `scene + bloom × bloom_strength + halation` →
/// **fog** → `exposure` → `tonemap` → **grade** → **vignette** → **grain** → `dither`
///
/// The split worth knowing is scene-referred against display-referred. Fog is atmospheric
/// scattering: it happens to the light on its way to the lens, so it blends in linear HDR before any
/// camera or display transform — a camera photographs fogged light, it does not fog a photograph.
/// The grade and the vignette are the opposite, a look and a lens darkening applied to the *finished
/// image*, so they land after the operator. The dither is strictly last, because it exists to hide
/// the output quantization.
///
/// # The medium: fog and light shafts are one thing
///
/// Fog and the light shafts are two **terms of one single-scattering participating medium**, not two
/// effects that happen to look related. `fog_density` is the medium's extinction σ_t, `fog_albedo`
/// the fraction of that extinction which scatters rather than absorbs (σ_s = `fog_albedo` · σ_t),
/// and `fog_anisotropy` the phase asymmetry `g`:
///
/// ```text
/// L = T(d)·L_surface + ∫₀ᵈ σ_s·T(t)·[ fog_color + p(θ)·V(t)·L_sun ] dt
/// ```
///
/// The first term is the extinction of what the surface sent toward the lens. Inside the integral,
/// the ambient radiance `L_ambient` is `mix(fog_color, sky(view_ray), fog_sky)` — the authored slab,
/// the [`Environment`] sky the medium actually sits under, or a blend — and the term has the closed
/// form `fog_albedo · (1 - T(d)) · L_ambient`, which is why the haze stays analytic and costs no march —
/// and `p(θ)·V(t)·L_sun` is the **directional** in-scatter, the sun's light the shaft march estimates
/// against the shadow map's visibility `V`. Both terms share the one extinction and the one albedo,
/// so a thicker medium hazes more *and* throws brighter, shadow-shaped beams, and the beams are
/// dimmed by the fog they travel through. There is deliberately no second density: the reach of an
/// exponential medium is a function of its extinction, and a scene that wants beams without haze
/// authors a thin medium and a strong `shafts`.
///
/// # Fog
///
/// `fog_density` is the gate and the strength: a surface at distance `d` past `fog_start` is buried
/// under `1 - exp(-fog_density * d)` of `fog_color`. `fog_height_falloff` adds an exponential height
/// gradient about `fog_height`, integrated analytically along the fogged segment of the camera ray;
/// at its `0.0` default the integral collapses to plain distance fog, so the height half is a skip
/// rather than a special case.
///
/// **`fog_start`, `fog_height` and `fog_height_falloff` shape the whole medium, not just the haze.**
/// The shaft march integrates the same optical depth and weighs each step by the medium's local
/// density, so a height medium thickens the god-rays toward the ground as it thickens the fog, and a
/// `fog_start` past the geometry removes both terms together.
///
/// **Fog does not apply where nothing was drawn.** A background pixel is at the far plane, which is
/// infinitely distant, so distance fog would replace it outright and flatten an authored
/// [`Environment`] sky into a slab of fog colour. Those pixels are skipped, so the haze has a horizon
/// to meet. `fog_sky` is how it meets it: at `0.0` only a `fog_color` picked near
/// [`Environment::sky_horizon`](Environment::sky_horizon) reads continuous, and at `1.0` the haze is
/// the sky along that ray and the two agree by construction.
///
/// # The grade, and its pinned order of operations
///
/// The seven grade fields — `contrast`, `saturation`, `temperature`, `tint`, `lift`, `gamma`,
/// `gain` — are **flat on this component, not a nested `grade:` map**: an override may create a
/// missing *final* field but never an intermediate node, so a nested map would make every knob a
/// scene left unauthored unsweepable, and tuning by `--sweep` is how this component is meant to be
/// used at all.
///
/// The grade is **display-referred**: it runs after the tonemap operator (which is why
/// [`TonemapOperator::Agx`], which ships deliberately muted, expects one on top) and before the
/// output dither. Grading has no canonical order of operations, so the order **is** part of this
/// component's contract:
///
/// 1. **white balance** — `temperature` (blue↔amber), then `tint` (green↔magenta), as one
///    luminance-preserving diagonal channel scale;
/// 2. **lift / gamma / gain** — the ASC CDL shape, `pow(max(x * gain + lift, 0), 1 / gamma)`;
/// 3. **contrast** — about a pivot of `0.5`, mid-grey in the display-referred range;
/// 4. **saturation** — a mix toward Rec.709 luma.
///
/// Every step is skipped by its own uniform comparison, so any subset may be neutral and an
/// all-identity grade is byte-identical to no grade at all.
///
/// The four grade *scalars* carry schemars ranges. The *triples* cannot — a `range` does not reach
/// array elements — so `gain >= 0`, `gamma > 0` and `fog_color >= 0` live in this component's
/// semantic validator as `L1130`, pointing at the element, exactly as `Environment`'s colour
/// components live in its own (`L1128`). `lift` is deliberately unbounded there: it is an offset, and
/// a negative one legitimately crushes blacks. A wrong-*shaped* triple is the schema's `L1014`.
///
/// # Ambient occlusion
///
/// `ssao` is the gate and the strength. Above zero, three passes are prepended to the frame: a
/// depth+normal prepass over the opaque meshes, the screen-space estimate, and a blur; at zero none
/// of them is built, so an AO-free scene renders through the exact frame graph it always did.
///
/// It is **not** part of the composite's pinned order above, because it is not an imaging step at
/// all: it modulates *lighting*, inside the scene pass, and only the ambient + IBL term. Direct
/// light, the sun and emissive are untouched — see `ssao`'s own note for why that split is the whole
/// point of the feature.
///
/// `ssao_radius` is in **world units**, which is what makes the effect identical at any resolution and
/// any render scale.
///
/// # Light shafts
///
/// `shafts` is the gate and the strength of the medium's directional term, and it is the one effect
/// here gated on the *scene* as well as on its own field. Two passes are added behind the scene pass —
/// a ray-march of the sun's shadow map along each eye ray, and the blur that cleans it up — when all
/// three of `shafts`, a [`DirectionalLight`] and a nonzero `fog_density` are present.
///
/// **Those are the effect's three mutes, and each is the absence of something the march needs.** The
/// march estimates one term of the medium above: it reads `fog_density` for its transmittance and its
/// reach, scales by `fog_albedo`, and shapes its lobe with `fog_anisotropy`. So no sun is no shadow
/// map to march, and no medium is nothing to scatter — a scene missing either simply gets no shafts,
/// the same "absence is meaningful" posture this whole component takes, not an error. What follows for
/// an author is that thickening the fog carries the beams with it, and that beams without haze are a
/// *thin* medium and a strong `shafts`, never a zero density.
///
/// Like ambient occlusion it is not part of the composite's pinned order in the sense of being a
/// per-pixel imaging step — the march is its own pair of passes — but its *result* is, and it lands
/// between the fog and the exposure: fog is the extinction of what the surface sent toward the lens,
/// and a shaft is light the medium scatters into the ray on top of whatever survived it.
///
/// # Depth of field
///
/// `dof` is the gate and the aperture. Above zero, two passes are added behind the scene pass — a
/// prefilter that halves the scene and records each texel's circle of confusion, and a disc gather
/// over it — and the composite blends the result in **first**, ahead of the bloom mix and the fog.
///
/// That position is decided by the gather's *input*, not by where a lens sits in the optical path: the
/// gather reads what the scene pass wrote, which is pre-fog and pre-shafts, so blending any later
/// would mix a fogged sharp image against an unfogged blurred one and thick fog would vanish exactly
/// where the defocus is strongest. Blending first applies one fog to one image.
///
/// The two fields are a **lens**: `focus_distance` is what is sharp, `dof` is how fast everything else
/// stops being. There is deliberately no in-focus range field — see `dof` — and no autofocus mode:
/// a game that wants one writes `focus_distance` from a system each tick: a sim system (hashed,
/// replays) or a presentation system on a presentation entity's `PostProcess` (unhashed; no sim
/// `PostProcess` may exist, it would win). An engine-side autofocus would make the image a function
/// of the camera's history.
///
/// # Film emulation
///
/// `chromatic_aberration`, `halation`, `grain` and `grain_size` model the *camera* rather than the
/// scene, and each is off by default and skipped rather than computed when it is. None of them adds
/// a pass: all three are pure per-pixel functions of colour the composite has already sampled, so
/// they cost a branch and a handful of ALU each.
///
/// Their positions in the order above are the same scene-referred / display-referred reading applied
/// to a camera. The aberration is a property of the **lens**, so it happens in the fetch itself —
/// it is not applied to the image, it is how the image was formed. Halation is light scattering back
/// off the film base into the emulsion, so it is scene-referred and joins the bloom mix whose blurred
/// bright field it borrows. Grain is the emulsion **recording** that light, so it lands behind the
/// vignette: light passes the lens and then hits the film. The dither is not a camera at all and
/// stays strictly last.
///
/// Two of the roster inherit something they cannot tune, and both are deliberate. Halation reuses the
/// bloom chain rather than blurring a second time, so its radius and threshold are bloom's. And the
/// grain is static — it carries no frame index, so a moving camera moves through a fixed field rather
/// than through a boiling one.
///
/// # Screen-space reflections
///
/// `ssr` is the strength and the gate (default `0.0`, off): it scales the trace's confidence, so `0.0`
/// is exactly the sky reflection and `1.0` trusts every hit. `ssr_fade` is the roughness band
/// `[start, end]` over which the trace gives way to the sky, `ssr_max_distance` is how far a ray
/// travels before it gives up, and `ssr_thickness` is how far behind a surface a ray may pass and
/// still count as a hit, both in world units. A machine's quality can mask the whole effect
/// (`Quality::ssr`), and a scene that authors none of it serializes exactly as before. The trace is
/// single-bounce on what the frame already drew, so a miss falls back to the sky.
///
/// # Tuning a knob without touching the scene
///
/// A sweep re-tunes a field the scene authors and creates one it does not, so one command renders a
/// contact sheet for any knob on any scene:
///
/// ```text
/// ludion render --scene examples/gallery/post-full.yaml --out sheet.png \
///   --sweep /entities/0/components/PostProcess/fog_density=0.0,0.01,0.02,0.04
/// ```
///
/// Judge the sheet, then commit the chosen value into the scene. A sheet is a judging instrument,
/// never a golden — regenerate it against current code rather than keeping one around.
/// `examples/gallery/post-full.yaml` authors this whole roster and records, in its header, the sheet
/// or the measurement behind every number in it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct PostProcess {
    /// Bloom threshold: the luminance above which a pixel blooms, validated `>= 0` (default `1.0` —
    /// the engine's shipped threshold).
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub bloom_threshold: f32,
    /// Bloom strength: how strongly the blurred bloom is mixed back in, validated `>= 0` (default
    /// `0.1` — the engine's shipped strength).
    ///
    /// The default is small because the chain is a six-level pyramid that composites the **sum of
    /// all six levels**: measured on three scenes at equal strength, it carries 5.69×, 5.74× and
    /// 6.11× the energy of the narrow separable Gaussian the engine shipped before it (mean 5.85×),
    /// which is the reference filter's own behaviour rather than a normalization bug. `0.1` is
    /// `0.6 / 5.85` — the strength at which the pyramid adds the same HDR light the old chain added
    /// at its own `0.6`. Reference implementations of this filter likewise pair it with strengths
    /// near `0.1`; a value authored against a single-blur bloom will blow the frame out.
    #[serde(default = "bloom_strength_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.5]))]
    pub bloom_strength: f32,
    /// Exposure: a **linear multiplier** on scene + bloom, applied immediately before the tonemap
    /// operator, validated `>= 0` (default `1.3`).
    ///
    /// Not EV stops. One field means one thing, and every other scalar here is neutral at its own
    /// natural value — this one is not neutral at `1.0`, and that is deliberate: `1.3` is what puts
    /// mid-grey where [`TonemapOperator::Agx`] expects it. Feed `0.18` linear (the photographic grey
    /// card) through the Narkowicz ACES fit and it lands at `0.267` output luminance; through AgX,
    /// at `0.215`. `1.3333` is the multiplier that closes that gap exactly, and `1.3` is it rounded
    /// to something an author can type and reason about.
    ///
    /// The two operators **cross** at about `0.09` scene luminance — below it AgX is the brighter of
    /// the two, above it the darker — so no single multiplier matches both ends, and this one
    /// deliberately anchors the middle. A scene that is mostly deep shadow will read a little
    /// brighter than it did under ACES, and one that is mostly diffuse daylight a little darker.
    ///
    /// Because exposure lands after the bright-pass, `bloom_threshold` keeps meaning *scene*
    /// luminance whatever the exposure is.
    #[serde(default = "exposure_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub exposure: f32,
    /// Which tonemap operator maps HDR onto the output (default [`TonemapOperator::Agx`]).
    #[serde(default)]
    pub tonemap: TonemapOperator,
    /// Whether to add ordered dither to the output (default `true`).
    ///
    /// The output is 8-bit, so a smooth low-luminance gradient — an authored night sky is the case
    /// this exists for — quantizes into visible bands. The dither is a sub-LSB ordered offset
    /// applied in the *encoded* domain, which trades those bands for fine stipple; it is
    /// deterministic and carries no frame index, so a single-frame render reproduces exactly. It is
    /// on by default because banding is a defect rather than a look: it costs one add and it is the
    /// difference between a `banding_score` of 0.90 and 0.29 on an authored gradient.
    #[serde(default = "yes")]
    pub dither: bool,
    /// Grade step 3 — contrast about a `0.5` pivot, validated `>= 0` (default `1.0` — no change).
    /// `0.0` flattens the image to mid-grey; above `1.0` pushes the ends apart and the output clamps
    /// them.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 2.0]))]
    pub contrast: f32,
    /// Grade step 4 — chroma as a mix toward Rec.709 luma, validated `>= 0` (default `1.0` — no
    /// change). `0.0` is greyscale; above `1.0` extrapolates past the original chroma.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 2.0]))]
    pub saturation: f32,
    /// Grade step 1 — white balance along the blue↔amber axis, validated `[-1, 1]` (default `0.0` —
    /// no change). Negative cools the image, positive warms it. Applied as a luminance-preserving
    /// channel scale, so it shifts colour without doubling as a brightness control.
    #[serde(default)]
    #[schemars(range(min = -1.0, max = 1.0))]
    pub temperature: f32,
    /// Grade step 1 — white balance along the magenta↔green axis, validated `[-1, 1]` (default `0.0`
    /// — no change), the second half of the same channel scale as `temperature`.
    #[serde(default)]
    #[schemars(range(min = -1.0, max = 1.0))]
    pub tint: f32,
    /// Grade step 2 — the ASC CDL **offset**, added per channel after `gain` (default `[0, 0, 0]` —
    /// no change). Raises or lowers the whole channel, so it is the shadow end's control.
    ///
    /// Unbounded, and deliberately so: a negative lift crushes the blacks, which is an ordinary
    /// grading move rather than a mistake.
    #[serde(default)]
    #[schemars(extend("x-ui-range" = [-0.5, 0.5]))]
    pub lift: [f32; 3],
    /// Grade step 2 — the ASC CDL **power**, applied per channel as its **reciprocal**, validated
    /// element-wise `> 0` (`L1130`; default `[1, 1, 1]` — no change).
    ///
    /// The reciprocal is the point: a *higher* `gamma` **brightens** the midtones, which is the
    /// convention every grading UI uses and the inverse of a bare `pow(x, gamma)`.
    #[serde(default = "one3")]
    #[schemars(extend("x-ui-range" = [0.5, 2.0]))]
    pub gamma: [f32; 3],
    /// Grade step 2 — the ASC CDL **slope**, multiplied per channel before `lift` is added, validated
    /// element-wise `>= 0` (`L1130`; default `[1, 1, 1]` — no change). Scales the whole channel, so it
    /// is the highlight end's control.
    #[serde(default = "one3")]
    #[schemars(extend("x-ui-range" = [0.0, 2.0]))]
    pub gain: [f32; 3],
    /// Fog: **extinction per world unit**, validated `>= 0` (default `0.0` — no fog, and the gate for
    /// the whole fog step).
    ///
    /// A surface at distance `d` past `fog_start` is buried under `1 - exp(-fog_density * d)` of
    /// `fog_color`, so the density is the reciprocal of the distance at which about 63% of a surface
    /// has been replaced. On a scene measured in metres, `0.01` is a haze and `0.1` is a soup.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.1]))]
    pub fog_density: f32,
    /// Fog: what the fog blends toward, linear RGB and HDR-capable, validated element-wise `>= 0`
    /// (`L1130`; default a mid grey).
    ///
    /// The default is achromatic because no colour suits a scene: fog colour is the look decision
    /// this field exists to make. The one number worth matching is
    /// [`Environment::sky_horizon`](Environment::sky_horizon) — fog is skipped on background pixels
    /// (see `fog_density`'s note on the sky), so a fog colour near the horizon's is what makes the
    /// horizon read as continuous rather than as a seam.
    #[serde(default = "fog_color_default")]
    pub fog_color: [f32; 3],
    /// Fog: the distance before which there is no fog at all (default `0.0` — fog from the camera),
    /// validated `>= 0`.
    ///
    /// The medium occupies `max(0, distance - fog_start)` of the ray, so a foreground inside the start
    /// distance is untouched. Without it, fog at any useful density is unusable on a scene with
    /// anything close to the camera. The shaft march reads it too: there are no beams inside the
    /// distance where there is no medium.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 100.0]))]
    pub fog_start: f32,
    /// Fog: the world height (Y) at which the density is exactly `fog_density` (default `0.0`).
    ///
    /// Only meaningful with `fog_height_falloff` set; on its own the medium is height-independent and
    /// this field is inert — in the marched beams as much as in the haze.
    #[serde(default)]
    pub fog_height: f32,
    /// Fog: **exponential density falloff per world unit of height**, validated `>= 0` (default
    /// `0.0` — height-independent fog, and a skip of the height half).
    ///
    /// The density at height `y` is `fog_density * exp(-fog_height_falloff * (y - fog_height))`, and
    /// the composite integrates that analytically along the fogged segment of the camera ray. At
    /// `0.0` the integral collapses to plain distance fog, which is why the neutral value is a skip
    /// rather than a special case. `0.1` puts the medium's half-height about seven world units above
    /// `fog_height`.
    ///
    /// It shapes the **beams** as well as the haze: the shaft march integrates the same height-varying
    /// optical depth and scales each step by the local density, so god-rays under a height medium
    /// thicken toward the ground.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.5]))]
    pub fog_height_falloff: f32,
    /// The medium: its **single-scattering albedo** α — the fraction of `fog_density` that scatters
    /// rather than absorbs — validated `[0, 1]` (default `1.0`, a purely scattering medium).
    ///
    /// The medium carries one extinction, and this splits it: σ_s = α · `fog_density` scatters,
    /// σ_a = (1 - α) · `fog_density` absorbs. Only the scattering half sends light toward the lens, so
    /// this one number scales **both** in-scatter terms — the ambient haze *and* the sun's beams. A
    /// dark absorbing smoke and a bright scattering haze are one knob apart at the same thickness,
    /// rather than two colours authored against two densities: at `0.2` a thick medium still buries
    /// the surfaces behind it but the fog and the shafts barely glow.
    ///
    /// `1.0` is exactly the fog the composite has always drawn — the ambient term is
    /// `α * (1 - exp(-optical_depth))` and at `α = 1` that is the old expression unchanged — so a
    /// scene which does not author an albedo renders byte-identically.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub fog_albedo: f32,
    /// The medium: how **sky-coupled** its ambient in-scatter is, validated `[0, 1]` (default `0.0` —
    /// the flat authored slab, and a skip).
    ///
    /// A real haze in-scatters the sky it sits under, so it is blue toward the zenith and warm in the
    /// horizon band rather than one colour everywhere. This blends the ambient radiance between
    /// `fog_color` and the [`Environment`] sky's own radiance along the view ray:
    ///
    /// ```text
    /// L_ambient = mix(fog_color, sky(view_ray), fog_sky)
    /// ```
    ///
    /// At `1.0` the haze *is* the sky the surface is seen against, which is what makes distant
    /// geometry fade into the horizon with no seam — the continuity `fog_color`'s note asks an author
    /// to hand-match. It is the aerial-perspective reading, and honestly an approximation: for a
    /// homogeneous medium under a fixed sky the true isotropic in-scatter is direction-independent,
    /// and what reads as haze is the sky *behind* the surface showing through it.
    ///
    /// **It needs an [`Environment`] to mean anything.** With no `Environment` there is no sky to
    /// in-scatter, so the renderer forces the blend to zero and the medium is the authored slab — a
    /// scene may carry this field with no sky and render byte-identically to one that omits it.
    /// `fog_albedo` scales the result as it scales every in-scatter;
    /// [`Environment::ibl_intensity`](Environment::ibl_intensity) does **not**, because that knob
    /// grades how much the sky lights *surfaces*.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub fog_sky: f32,
    /// The medium: its **Henyey–Greenstein phase asymmetry** `g`, validated **exclusive** `(-1, 1)`
    /// (default `0.7`, a forward-scattering haze).
    ///
    /// It decides how the sun's in-scattered light is spread over angle, which is the difference
    /// between beams and uniform fill. At `0.0` the medium scatters equally in every direction and the
    /// sun term reads as a flat lift over the whole frame; positive values throw light forward along
    /// the sun's own travel, so it concentrates into shafts that brighten sharply as the camera turns
    /// into the light; negative values scatter it back toward the source. `0.7` is the conventional
    /// figure for atmospheric haze and is what makes a shaft read as a shaft.
    ///
    /// The bound is **exclusive** because the phase function's denominator,
    /// `(1 + g² - 2g·cos θ)^{3/2}`, vanishes at `g = ±1` for a ray exactly along the light: perfect
    /// forward scattering is a delta function, not a value a finite integral can carry.
    #[serde(default = "fog_anisotropy_default")]
    #[schemars(extend("exclusiveMinimum" = -1.0, "exclusiveMaximum" = 1.0))]
    #[schemars(extend("x-ui-range" = [-0.9, 0.9]))]
    pub fog_anisotropy: f32,
    /// Vignette: corner darkening strength, validated `[0, 1]` (default `0.0` — none, and the gate).
    /// `1.0` takes the extreme corner to black.
    ///
    /// Unlike fog, the vignette is **display-referred**: it runs after the tonemap and the grade,
    /// because it is a lens darkening the finished image rather than something happening in the
    /// scene.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub vignette: f32,
    /// Vignette: the normalized screen distance at which the darkening begins, validated `[0, 1]`
    /// (default `0.5`).
    ///
    /// Distance is measured from the frame's centre in units where `1.0` is an edge midpoint and
    /// `√2` a corner, and the darkening ramps (smoothstep) from this radius out to the corner. So the
    /// geometry is a function of **normalized** screen position and nothing else: identical at any
    /// resolution and any render scale, and elliptical rather than circular — the vignette follows
    /// the frame's format instead of being cropped by it.
    ///
    /// There is deliberately no second shape knob: the outer edge is the corner, which the frame
    /// supplies, so the band is fully determined by where it starts.
    #[serde(default = "vignette_radius_default")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub vignette_radius: f32,
    /// Ambient occlusion: how much of the estimate to apply, validated `[0, 1]` (default `0.0` — off,
    /// and the gate for the whole effect).
    ///
    /// At `0.0` the depth+normal prepass, the SSAO pass and its blur are **not built at all**, so an
    /// AO-free scene renders through the exact frame graph it always did. `1.0` takes a fully occluded
    /// crease's ambient contribution to zero.
    ///
    /// By default it darkens the **ambient and image-based lighting term only** — never direct light,
    /// never the sun, never emissive. Multiplying the finished shading instead is the classic "AO looks
    /// like dirt" failure: it dims torch highlights and glowing props, which is the worst case for a
    /// night city lit by point lights. `ssao_direct` opens that rule by a bounded fraction for scenes
    /// whose ambient term is too small for the default split to show anything; emissive stays untouched
    /// at any setting.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub ssao: f32,
    /// Ambient occlusion: the sampling hemisphere's radius in **world units**, validated `>= 0`
    /// (default `0.5`).
    ///
    /// World units, not pixels, is what makes the effect resolution- and render-scale-invariant: a
    /// crease is a fixed size in the world, so the shading it gets is a fixed thing rather than a
    /// number of pixels. Set it to the size of the creases worth seeing — roughly half a metre on a
    /// scene measured in metres; a radius near a room's size turns the effect into broad shading
    /// rather than contact darkening.
    #[serde(default = "ssao_radius_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 2.0]))]
    pub ssao_radius: f32,
    /// Ambient occlusion: the world-space depth bias a sample must clear before it counts as occluded,
    /// validated `>= 0` (default `0.02`).
    ///
    /// Without it a flat surface partly occludes itself — depth precision and the finite prepass
    /// resolution put some samples marginally behind the surface they came from — and the effect reads
    /// as a uniform grey wash instead of as creases. Raise it if flat walls look dirty; lower it if
    /// contact shadows have pulled away from the contact.
    #[serde(default = "ssao_bias_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.1]))]
    pub ssao_bias: f32,
    /// Ambient occlusion: how much of the finished factor *also* attenuates direct light, validated
    /// `[0, 1]` (default `0.0` — the ambient + IBL term only, and byte-identical to the field's
    /// absence). Inert while `ssao` is zero.
    ///
    /// `ssao`'s own note explains why the default split exists. This field is for the case that split
    /// does not cover: a scene lit almost entirely by direct light — a night city of point lights and
    /// emissive props — where the ambient term is a few percent of the frame's illumination, so AO
    /// applied to it alone has nothing to remove and the effect is invisible at any radius or strength.
    ///
    /// The measurement, because "I turned SSAO on and nothing happened" is the question this field
    /// exists to answer: on `examples/gallery/post-full.yaml` — a night scene whose ambient is
    /// 0.014–0.024 — `ssao: 0` against `ssao: 1.0` moves at most **9 of 255 levels, on 8% of the
    /// frame**. Sweeping `ssao_radius` there is indistinguishable, because the radius was never the
    /// constraint: the ambient term's size was.
    ///
    /// It is a **fraction, not a switch**, because the "AO looks like dirt" failure is the *full*
    /// multiply into finished shading. A small value buys contact darkening where geometry meets
    /// geometry while leaving a torch's pool of light recognisably a torch; `1.0` is that full multiply,
    /// offered for judgement rather than recommended. Emissive is outside the shaded term entirely, so
    /// a glowing prop never dims however high this goes.
    ///
    /// It applies to the point lights and to the sun alike, and it multiplies the *finished* factor —
    /// so it scales with `ssao` rather than replacing it.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub ssao_direct: f32,
    /// Light shafts: how much of the medium's in-scattered sun light to add, validated `>= 0` (default
    /// `0.0` — off, and one of three gates for the whole effect).
    ///
    /// Above zero, two passes are added behind the scene pass: a ray-march of the sun's shadow map
    /// along each eye ray, and the blur that cleans it up. At zero neither is built, so a
    /// shaft-free scene renders through the exact frame graph it always did.
    ///
    /// **It also needs a [`DirectionalLight`] and a nonzero `fog_density`.** There is nothing to march
    /// without a shadow map and nothing to scatter without a medium — the estimate's coefficient is
    /// σ_s = `fog_albedo` · `fog_density` — so a scene missing either simply gets none of it, the same
    /// "absence is meaningful" posture this whole component takes, not an error. Neither absence is
    /// built and computed to zero: the passes are not scheduled at all.
    ///
    /// This is a **pure intensity** on a term the medium otherwise fully describes: reach, dimming,
    /// brightness and lobe all come from `fog_density`, `fog_albedo` and `fog_anisotropy`. A value
    /// above `1.0` is legal and simply pushes the beams past what the medium physically scatters.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub shafts: f32,
    /// Depth of field: the far field's circle of confusion as a **fraction of the frame's height**,
    /// validated `[0, 0.25]` (default `0.0` — everything in focus, and the gate for the whole effect).
    ///
    /// Above zero, two passes are added behind the scene pass: a prefilter that halves the scene and
    /// records each texel's circle of confusion, and a disc gather over it. At zero neither is built,
    /// so an all-in-focus scene renders through the exact frame graph it always did.
    ///
    /// This one field is the lens's **aperture**. A thin lens focused at `focus_distance` images a
    /// surface at distance `d` as a disc of `dof * |1 - focus_distance / d|`, so the value is literally
    /// the blur radius the far background reaches — `0.01` is a percent of the frame's height, about
    /// eleven pixels at 1080p, which is a strong but sane cinematic defocus. Screen-relative, so it is
    /// identical at any resolution and any render scale.
    ///
    /// **There is deliberately no separate in-focus range.** Under that same relation the sharp band
    /// is a *consequence* of the aperture — stopping down widens it — so a range field would let a
    /// scene author a lens that cannot exist and would make this field mean two things. Widen the
    /// sharp zone by lowering `dof`, exactly as a photographer does.
    ///
    /// The far field saturates at this value; the near field is clamped to it, reaching full blur at
    /// half the focus distance, because the gather's disc has a fixed radius. The upper bound of
    /// `0.25` is where a fixed 32-tap disc stops being able to stay smooth.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 0.25))]
    // The legal ceiling is where the disc stops being smooth; the useful one is lower, judged by eye.
    #[schemars(extend("x-ui-range" = [0.0, 0.15]))]
    pub dof: f32,
    /// Depth of field: the focal plane's distance from the camera in **world units**, validated
    /// `>= 0` (default `10.0`; inert while `dof` is zero).
    ///
    /// Everything at this distance is exactly sharp and the blur grows either side of it. World units,
    /// like `ssao_radius`, is what makes the effect a property of the scene rather than of the
    /// viewport.
    ///
    /// **A game focuses by writing this field, not by asking the engine to.** An autofocus mode would
    /// be a second mechanism — it would have to reduce the depth buffer and feed the answer back into
    /// the next frame, which makes the image a function of the camera's history rather than of world
    /// state, the exact property `Plan/post-process-pipeline.md` decision 5 rejects temporal AA for.
    /// A system that sets `focus_distance` from the player's own position each tick is
    /// deterministic and needs nothing from the renderer: a sim system replays (hashed), a
    /// presentation system on a presentation entity's `PostProcess` is unhashed.
    #[serde(default = "focus_distance_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 100.0]))]
    pub focus_distance: f32,
    /// Film emulation: transverse chromatic aberration — the **radial** per-channel displacement of
    /// the scene at an edge midpoint, in normalized screen units, validated `[0, 0.1]` (default `0.0`
    /// — off, and the gate).
    ///
    /// Red is fetched from nearer the frame's centre, blue from further out, green not displaced at
    /// all: the crown-glass direction, and a magnification difference rather than a focus error. The
    /// displacement is exactly zero at the centre, grows linearly outward and is √2 times this value
    /// at a corner, so `0.002` separates red from blue by about 0.4% of the frame at an edge midpoint
    /// — roughly eight pixels across a 1080p frame's width.
    ///
    /// Screen-relative, so the fringe is identical at any resolution and any render scale. Because
    /// the units are normalized per axis, the displacement is a fixed fraction of the width sideways
    /// and of the height vertically — the aberration follows the frame's format, exactly as the
    /// vignette's ellipse does.
    ///
    /// Only the scene is displaced, never the bloom, shaft or defocus layers: those are low-frequency
    /// by construction, and the fog reads each pixel's own depth, so displacing them would cost four
    /// texture fetches to change nothing an eye can find.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 0.1))]
    pub chromatic_aberration: f32,
    /// Film emulation: halation — a warm bleed around bright sources, validated `>= 0` (default `0.0`
    /// — off, and the gate).
    ///
    /// Light that passes through the emulsion, reflects off the film base and scatters back is what
    /// puts a red-orange halo around a highlight; it is added into the same linear-HDR mix as bloom,
    /// scene-referred, before the fog.
    ///
    /// **The colour is not authorable, deliberately.** Halation's hue is a property of the film stock
    /// rather than of the shot, and what a `halation_color` would really control is achromatic glow
    /// versus chromatic glow — achromatic glow being exactly what `bloom_strength` already spells. The
    /// constant tint is normalized to Rec.709 luma, so this field is a *strength* on the same scale as
    /// `bloom_strength` and trading one for the other does not change how much light is added.
    ///
    /// It reuses the bloom chain's blurred bright field rather than blurring again, which is what
    /// makes it near-free — and the consequence: it inherits `bloom_threshold`, the chain's radius
    /// **and the chain's gain**, and cannot be tuned apart from them. Since that chain is the
    /// six-level pyramid, a useful value sits near `bloom_strength`'s own `0.1`, not near `1.0`.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.5]))]
    pub halation: f32,
    /// Film emulation: monochrome additive grain amplitude, as a signed offset in the **sRGB-encoded**
    /// `[0, 1]` range, validated `[0, 1]` (default `0.0` — off, and the gate).
    ///
    /// `0.02` is a fine film-ish stipple — about ±5 of the 255 output steps. The encoded domain is
    /// the point: the output is 8-bit sRGB and the hardware encodes on store, so a fixed *linear*
    /// offset would be worth wildly different numbers of output steps at different luminances, most
    /// wrong in the deep shadows.
    ///
    /// The amplitude is **uniform, not weighted toward the midtones** the way real emulsion grain is.
    /// Grain is in the roster because it breaks up 8-bit night gradients, which is a deep-shadow
    /// problem, and a midtone weighting would remove the benefit exactly where it is wanted.
    ///
    /// The pattern is deterministic (an integer hash of the cell's coordinates — no transcendental,
    /// so it reproduces bit-for-bit across backends and processes) and **static**: it carries no frame
    /// index, so a single-frame render reproduces exactly and a moving camera moves through a fixed
    /// field.
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub grain: f32,
    /// Film emulation: the grain cell's side as a **fraction of the frame's height**, validated
    /// `[0, 1]` (default `0.002` — about two pixels at 1080p, and inert while `grain` is zero).
    ///
    /// A fraction of the frame and never a pixel count, because grain authored per render pixel would
    /// be averaged straight back out by the supersampling resolve at `--scale 2` — raising the quality
    /// setting would change the look, which this engine treats as a bug rather than a trade.
    ///
    /// The floor is inclusive by choice; the shader clamps the cell to one pixel of the target the
    /// grain is applied at, so `0.0` means the finest grain there is.
    #[serde(default = "grain_size_default")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 0.01]))]
    pub grain_size: f32,
    /// Screen-space reflections: the strength that scales the trace's confidence, validated `[0, 1]`
    /// (default `0.0` — off, and the gate).
    ///
    /// `0.0` is exactly the sky reflection; `1.0` takes the traced hit wherever one is found.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub ssr: f32,
    /// Screen-space reflections: the roughness band `[start, end]` over which the trace fades out
    /// to the sky, each in `[0, 1]` with `start < end` (default `[0.45, 0.65]`).
    ///
    /// Rough surfaces scatter the reflection past what one traced ray can describe, so the trace is
    /// full strength at `start` and gone at `end`.
    #[serde(
        default = "ssr_fade_default",
        skip_serializing_if = "is_ssr_fade_default"
    )]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub ssr_fade: [f32; 2],
    /// Screen-space reflections: the longest a ray travels before the trace gives up, in world units,
    /// validated `> 0` (default `30.0`).
    #[serde(
        default = "ssr_max_distance_default",
        skip_serializing_if = "is_ssr_max_distance_default"
    )]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [1.0, 100.0]))]
    pub ssr_max_distance: f32,
    /// Screen-space reflections: how far behind a surface a ray may pass and still count as a hit, in
    /// world units, validated `> 0` (default `0.5`).
    ///
    /// Too thin and rays slip through thin geometry; too thick and objects smear behind occluders.
    #[serde(
        default = "ssr_thickness_default",
        skip_serializing_if = "is_ssr_thickness_default"
    )]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.05, 2.0]))]
    pub ssr_thickness: f32,
}

impl SemanticValidate for PostProcess {
    /// Rejects a `gain` or `fog_color` element below zero, or a `gamma` element at or below zero
    /// (`L1130`, e.g. `/gamma/1`) — the CDL slope must be non-negative, its power (applied as a
    /// reciprocal) strictly positive, and a colour the fog blends toward is the `Environment` colour
    /// case exactly (`L1128`): HDR above 1.0 is fine, negative light is not. This is the invariant
    /// JSON Schema's `range` cannot reach (it does not apply to array elements), so it lives here.
    /// `lift` is deliberately absent: it is an offset, and a negative one legitimately crushes
    /// blacks. Every scalar field carries its own schemars `range`, so an out-of-range scalar is the
    /// schema's `L1014` and is not re-checked here. Defensive: runs on a schema-clean value; an
    /// absent field (its default is valid) or a wrong *shape* adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for (field, floor_is_open) in [("gain", false), ("gamma", true), ("fog_color", false)] {
            let Some(triple) = value.get(field).and_then(Value::as_array) else {
                continue;
            };
            for (index, element) in triple.iter().enumerate() {
                if let Some(component) = as_stored_f32(element)
                    && (component < 0.0
                        || (floor_is_open && !ludion_core::number::positive_f32(component)))
                {
                    out.push(post_process_invalid_triple(
                        field,
                        &format!("/{field}/{index}"),
                    ));
                }
            }
        }
        post_process_ssr_diagnostics(value, &mut out);
        out
    }
}

/// `L1273` — the `PostProcess` SSR rules a schema `range` cannot spell: the fade band's elements and
/// order, and a reach or thickness of exactly zero.
fn post_process_ssr_diagnostics(value: &Value, out: &mut Vec<Diagnostic>) {
    if let Some(band) = value.get("ssr_fade").and_then(Value::as_array) {
        let ends: Vec<Option<f64>> = band.iter().map(as_stored_f32).collect();
        for (index, end) in ends.iter().enumerate() {
            if let Some(end) = end
                && !(0.0..=1.0).contains(end)
            {
                out.push(post_process_invalid_ssr(
                    &format!("/ssr_fade/{index}"),
                    "a roughness is between 0 and 1",
                    "give both ends of `ssr_fade` a value from 0 to 1",
                ));
            }
        }
        if let [Some(start), Some(end)] = ends[..]
            && start >= end
        {
            out.push(post_process_invalid_ssr(
                "/ssr_fade",
                "the confidence divides by the band's width, so `start` must be below `end`",
                "order `ssr_fade` as `[start, end]` with start < end, e.g. `[0.45, 0.65]`",
            ));
        }
    }
    for (field, role, help) in [
        (
            "ssr_max_distance",
            "a ray that travels no distance can hit nothing",
            "give `ssr_max_distance` a value above 0 (the default is `30.0`)",
        ),
        (
            "ssr_thickness",
            "a surface with no thickness can never be hit",
            "give `ssr_thickness` a value above 0 (the default is `0.5`)",
        ),
    ] {
        if let Some(number) = value.get(field).and_then(as_stored_f32)
            && !ludion_core::number::positive_f32(number)
        {
            out.push(post_process_invalid_ssr(&format!("/{field}"), role, help));
        }
    }
}

fn post_process_invalid_ssr(pointer: &str, role: &str, help: &str) -> Diagnostic {
    Diagnostic::new(
        code::POST_PROCESS_INVALID_SSR,
        format!("this `PostProcess` screen-space reflection field is out of range; {role}"),
    )
    .with_path(pointer.to_string())
    .with_help(help.to_string())
}

/// `L1130` — a `PostProcess` triple element is outside its valid range.
fn post_process_invalid_triple(field: &str, pointer: &str) -> Diagnostic {
    let (bound, role, identity) = match field {
        "gamma" => (
            "greater than 0",
            "the CDL power is applied as its reciprocal, so 0 would divide by zero",
            "the identity grade is `[1, 1, 1]`",
        ),
        "fog_color" => (
            "0 or more",
            "a fog colour is HDR (it may exceed 1.0) but negative light does not exist",
            "a neutral grey is `[0.5, 0.5, 0.5]`",
        ),
        _ => (
            "0 or more",
            "the CDL slope scales a channel, and a negative slope only clamps to black",
            "the identity grade is `[1, 1, 1]`",
        ),
    };
    Diagnostic::new(
        code::POST_PROCESS_INVALID_TRIPLE,
        format!("this `PostProcess` `{field}` component is out of range; {role}"),
    )
    .with_path(pointer.to_string())
    .with_help(format!(
        "give every `{field}` component a value {bound} ({identity})"
    ))
}

impl Default for PostProcess {
    fn default() -> Self {
        PostProcess {
            bloom_threshold: 1.0,
            bloom_strength: 0.1,
            exposure: 1.3,
            tonemap: TonemapOperator::Agx,
            dither: true,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            lift: [0.0; 3],
            gamma: [1.0; 3],
            gain: [1.0; 3],
            fog_density: 0.0,
            fog_color: fog_color_default(),
            fog_start: 0.0,
            fog_height: 0.0,
            fog_height_falloff: 0.0,
            fog_albedo: one(),
            fog_sky: 0.0,
            fog_anisotropy: fog_anisotropy_default(),
            vignette: 0.0,
            vignette_radius: vignette_radius_default(),
            ssao: 0.0,
            ssao_radius: ssao_radius_default(),
            ssao_bias: ssao_bias_default(),
            ssao_direct: 0.0,
            shafts: 0.0,
            dof: 0.0,
            focus_distance: focus_distance_default(),
            chromatic_aberration: 0.0,
            halation: 0.0,
            grain: 0.0,
            grain_size: grain_size_default(),
            ssr: 0.0,
            ssr_fade: ssr_fade_default(),
            ssr_max_distance: ssr_max_distance_default(),
            ssr_thickness: ssr_thickness_default(),
        }
    }
}

/// The scene's global-illumination volume: the authored parameters of the GI trace grid and its
/// probe volume. **Opt-in singleton** — the lowest-`EntityId` `GiVolume` wins (the [`Environment`] /
/// [`PostProcess`] precedent), and its **absence is meaningful**: a scene with no `GiVolume`
/// voxelizes nothing and takes the pipeline it took before the component existed.
///
/// The GI voxelizer reads it when present, building a world-space grid from the scene's placed
/// geometry. The grid is an input to indirect lighting, not an image: authoring a `GiVolume` where
/// nothing samples the grid moves no pixel.
///
/// The volume's origin and extent are **not** authored — they derive from the scene's own world
/// bounds. `cell_size` is the one thing the scene cannot be asked to infer, because the right
/// resolution is a property of the art direction (wall thickness, prop scale) rather than of the
/// bounds: the engine hardcodes no look. `probe_spacing` and `normal_bias` govern the irradiance
/// probe volume laid over that grid, and `sky_visibility` is the strength the baked result is
/// applied at.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct GiVolume {
    /// World units per cell of the GI trace grid — its resolution (default `0.5`), validated
    /// **strictly greater than zero**.
    ///
    /// Cell count grows as the cube of the reciprocal, so halving this multiplies grid memory and
    /// voxelization cost by eight. The floor is exclusive because a zero or negative cell has no
    /// volume: the grid extent it implies is not a number of cells.
    #[serde(default = "gi_cell_size_default")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub cell_size: f32,
    /// World units between irradiance probes (default `1.0`), validated **strictly greater than
    /// zero**.
    ///
    /// Authored rather than an engine constant because probe density is an art-direction call: a
    /// game with 0.2-unit walls and one with 5-unit rooms want different densities. The floor is
    /// exclusive for the same reason `cell_size`'s is — a zero or negative spacing implies no
    /// lattice.
    #[serde(default = "gi_probe_spacing_default")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub probe_spacing: f32,
    /// World-space offset along the surface normal applied before sampling the probe volume
    /// (default `0.25`), validated **non-negative**.
    ///
    /// Stops a surface sampling a probe on the far side of the wall it sits on — the leak control
    /// that lives at sample time, not at bake time.
    #[serde(default = "gi_normal_bias_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 2.0]))]
    pub normal_bias: f32,
    /// How strongly baked sky visibility attenuates the ambient term (default `1.0`), validated
    /// `[0, 1]`.
    ///
    /// `0.0` is exactly the historical unoccluded look — the identity-value neutrality gate — and
    /// `1.0` is the full bake.
    #[serde(default = "gi_sky_visibility_default")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub sky_visibility: f32,
    /// How strongly baked one-bounce irradiance is **added** to the ambient term (default `0.0`),
    /// validated `>= 0`.
    ///
    /// `0.0` skips the bounce bake and the shader branch entirely — the default, deliberately
    /// opposite `sky_visibility`'s, because every scene that already authors a `GiVolume` (including
    /// every W1 fixture) must stay byte-identical until it opts in. `1.0` is the physical value;
    /// above `1.0` is a deliberate exaggeration, which is why the bound is a floor rather than the
    /// `[0, 1]` a UI would suggest — the engine does not know how much bounce an art direction wants.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 2.0]))]
    pub bounce: f32,
}

impl Default for GiVolume {
    fn default() -> Self {
        GiVolume {
            cell_size: gi_cell_size_default(),
            probe_spacing: gi_probe_spacing_default(),
            normal_bias: gi_normal_bias_default(),
            sky_visibility: gi_sky_visibility_default(),
            bounce: 0.0,
        }
    }
}

/// Marks an entity as dynamic for global illumination: this entity's `PointLight` is injected
/// per frame instead of baked, its `Mesh` neither occludes probe rays nor deposits albedo/emissive
/// into the voxelization, and moving it invalidates neither the voxelization nor the bake.
///
/// A field-less opt-in marker, authored `GiDynamic: {}`. Absent is today's behaviour exactly: every
/// scene that authors no `GiDynamic` bakes and voxelizes as if the component did not exist.
///
/// **The authoring burden this accepts: mark every entity a system moves.** Forgetting to mark a
/// moving light or mesh does not produce wrong pixels — the bake simply re-runs and re-voxelizes
/// every frame, because an unmarked instance's transform (or color, or emissive) still feeds the
/// voxelizer's instance signature. That is a performance cliff with no visibly wrong image, so the
/// symptom to look for is a `profile.capture`/`--dump-gi` GI cost that never goes to zero on an
/// otherwise-static scene, not a rendering artifact.
///
/// On an entity carrying neither a `Mesh` nor a `PointLight` the marker is inert: no diagnostic,
/// the same posture as any marker whose subject is absent.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
// A zero-field struct's derived schema omits `properties` entirely rather than emitting `{}`, and
// the scene validator's unknown-field check (`ludion-scene/src/validate.rs`) only fires when
// `properties` is present — so without this, an authored `GiDynamic: { anything: true }` would
// validate silently instead of raising the ordinary `L1014`. Forcing the empty map is what makes
// this component reject extras exactly like every component with real fields, no dedicated code.
#[schemars(extend("properties" = serde_json::json!({})))]
pub struct GiDynamic {}

/// Marks an entity whose state **stays behind** when the run leaves this scene: the engine captures
/// its full component bag on the way out of a district and re-applies it on the way back in, within
/// one session (see `crate::transition`).
///
/// A field-less opt-in marker, authored `Residue: {}` (or inserted from code on an entity a system
/// spawns). Absent is today's behaviour exactly: a scene that marks nothing captures nothing, and a
/// switch out of it and back rebuilds it from its authored text alone.
///
/// **Identity is the entity's `Name`.** A marked entity that carries one is matched back onto the
/// freshly instantiated entity of the same name — its authored state is replaced in place, so its
/// `EntityId` is the one the target scene minted and no other entity's id moves. A marked entity with
/// no `Name` is *anonymous*: it is never matched, and comes back as a fresh entity spawned before the
/// carried set (the dropped-pickup case).
///
/// **The state must stay alive to be remembered.** Capture reads the departing world, so an entity
/// despawned before the switch leaves nothing behind and the target scene's authored version returns.
/// A thing that has been "used up" therefore keeps its entity and flips a component to say so — the
/// destroyed spawner is a husk, not a hole.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
// A zero-field struct's derived schema omits `properties` entirely rather than emitting `{}`, and
// the scene validator's unknown-field check only fires when `properties` is present — the `GiDynamic`
// case, and the same fix: forcing the empty map makes an authored `Residue: { anything: true }` raise
// the ordinary `L1014`.
#[schemars(extend("properties" = serde_json::json!({})))]
pub struct Residue {}

/// The footprint a [`GroundMarker`] draws, in the marker's own local XZ plane.
///
/// # Serialized form (serde internal tagging)
///
/// A `type` key names the variant and its dimensions sit beside it — the [`Projection`] spelling,
/// which is the one every struct-variant enum in a component uses (external tagging is what
/// [`MeshSource`] needs for its bare-string and one-key-path variants, not a second choice here):
///
/// ```yaml
/// shape:
///   type: disc
///   radius: 3.0
/// shape:
///   type: segment
///   length: 8.0
///   width: 2.0
/// shape:
///   type: wedge
///   half_angle_deg: 30.0
///   outer_radius: 7.0
///   inner_radius: 3.0
///   fill: 0.5
/// ```
///
/// Every dimension is the marker's **maximum** extent in world units, further multiplied by the
/// entity's uniform `Transform.scale.x` — so a telegraph filling in as its warmup counts down is a
/// per-frame scale write, never a re-authored shape. All of them must be strictly positive and
/// finite (`L2106`). A wedge's `half_angle_deg` and `fill` are not lengths and are never scaled; a
/// wedge that cannot be drawn as authored is `L2113`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GroundMarkerShape {
    /// A filled circle centred on the entity — an area-of-effect or landing-point telegraph.
    Disc {
        /// The circle's radius in world units.
        radius: f32,
    },
    /// A filled capsule along the entity's local **+X** axis — a charge/line telegraph. The drawn
    /// footprint is exactly `length` × `width` world units only while `width <= length`; the
    /// underlying rounded-box SDF has no flat side to give once `width` catches up, so at
    /// `width == length` it is a true disc of that diameter, and past it the shape stops growing
    /// with `width` at all — the length-axis extent stays exactly `length` while the cross-axis
    /// extent narrows to `sqrt(length * (2 * width - length))`, a lens strictly narrower than the
    /// authored `width`. Keep `width <= length` unless a pinched lens is the intended look.
    Segment {
        /// Total length along local +X, in world units.
        length: f32,
        /// Total width across local +Z, in world units.
        width: f32,
    },
    /// A circular sector — a vision-cone telegraph — drawn as a wireframe: outline, optional inner
    /// range arc, a polar grid, and a fill that grows from the apex. The apex is at the entity
    /// origin and the wedge faces local **+X**. Not valid on a `DropShadow` (`L2113`).
    Wedge {
        /// Half the opening angle in degrees, in `(0, 180]`; the wedge spans `±half_angle_deg`
        /// about +X. Not scaled by `Transform.scale.x`.
        half_angle_deg: f32,
        /// Distance from the apex to the outer arc, in world units.
        outer_radius: f32,
        /// Range of an inner arc, in world units; `0` (the default) draws none. Must be below
        /// `outer_radius`.
        #[serde(default)]
        inner_radius: f32,
        /// Fraction of `outer_radius`, in `[0, 1]`, filled from the apex (default `0`). Not scaled.
        #[serde(default)]
        fill: f32,
        /// Metres between concentric grid arcs (default `1.0`); radial spokes are spaced so cells
        /// are about square at the rim.
        #[serde(default = "wedge_grid_spacing")]
        grid_spacing: f32,
    },
}

fn wedge_grid_spacing() -> f32 {
    1.0
}

impl Default for GroundMarkerShape {
    /// A unit-radius disc — the [`Sprite`]/[`Mesh`] posture, where a component authored with no
    /// dimensions draws something of size one rather than nothing.
    fn default() -> Self {
        GroundMarkerShape::Disc { radius: 1.0 }
    }
}

/// A flat, ground-anchored telegraph marker: an analytic disc, capsule or wedge lying in the entity's local
/// XZ plane, alpha-blended over the scene after the opaque meshes and before the sprites.
///
/// The marker's placement is the entity's [`Transform`]: translation positions it, rotation orients
/// it (local +X is a [`GroundMarkerShape::Segment`]'s length axis), and the **uniform**
/// `Transform.scale.x` multiplies its authored dimensions. `Transform.scale.x` scales the footprint
/// uniformly; the other two axes are ignored — animate a shape field (`radius`, `length`, `width`),
/// never the scale, to change one dimension without distorting the other. Extract lifts it by a
/// fixed world-space Y offset (`ludion_renderer::GROUND_MARKER_LIFT`) so a marker lying on a floor
/// never z-fights the geometry it marks; no scene picks that offset.
///
/// Plain, static data: the component carries a shape and a color and nothing else. Animation — a
/// warmup filling in, an urgency ramp — is a game system writing the shape field (a disc's `radius`,
/// a segment's `length`) and `color` each frame, the same shape as the hit-flash pattern that writes
/// `Mesh.color`; `cargo run -p ludion --example ground_marker` is the copyable form.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct GroundMarker {
    /// The marker's footprint (default a unit-radius disc).
    #[serde(default)]
    pub shape: GroundMarkerShape,
    /// Linear RGBA (default opaque white). HDR-capable: a value above `1.0` rides the bloom chain
    /// exactly like an emissive mesh, which is how urgency reads at a glance. Alpha multiplies the
    /// marker's analytic coverage, so it is the marker's overall opacity.
    #[serde(default = "white4")]
    pub color: [f32; 4],
}

impl Default for GroundMarker {
    fn default() -> Self {
        GroundMarker {
            shape: GroundMarkerShape::default(),
            color: [1.0; 4],
        }
    }
}

impl SemanticValidate for GroundMarker {
    /// Rejects a shape dimension that is not a strictly positive, finite number (`L2106`), with a
    /// JSON Pointer relative to the component root (`/shape/radius`, `/shape/length`,
    /// `/shape/width`, `/shape/outer_radius`, `/shape/grid_spacing`), and a wedge that cannot be
    /// drawn as authored (`L2113`, pointers `/shape/half_angle_deg`, `/shape/inner_radius`,
    /// `/shape/fill`). A degenerate footprint has no signed distance to evaluate, so it is a
    /// mis-authored marker rather than an invisible one. `color` is checked as an HDR colour
    /// (`L1263`).
    ///
    /// Checked by field name rather than by variant, so it is exactly as defensive as its siblings:
    /// the value may be schema-invalid, and every dimension present is checked whichever `type` it
    /// arrived under.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = value_color_diagnostics("GroundMarker", value, "color");
        out.extend(shape_dimension_diagnostics(
            value,
            "ground marker",
            "GroundMarker",
        ));
        out.extend(wedge_diagnostics(value));
        out
    }
}

/// `L2113` for a wedge `shape` that cannot be drawn: `half_angle_deg` outside `(0, 180]`,
/// `inner_radius` negative or not below `outer_radius`, `fill` outside `[0, 1]`. By field name,
/// like [`shape_dimension_diagnostics`]; an absent `inner_radius` / `fill` reads as its default `0`,
/// and the `inner_radius < outer_radius` check needs a numeric `outer_radius` to compare against.
fn wedge_diagnostics(value: &Value) -> Vec<Diagnostic> {
    let Some(shape) = value.get("shape") else {
        return Vec::new();
    };
    let invalid = |field: &str, v: f64, why: &str, help: &str| {
        Diagnostic::new(
            code::GROUND_MARKER_WEDGE_INVALID,
            format!("ground marker wedge `{field}` {v} is not valid; {why}"),
        )
        .with_path(format!("/shape/{field}"))
        .with_help(help.to_string())
    };
    let mut out = Vec::new();
    if let Some(half) = shape.get("half_angle_deg").and_then(as_stored_f32)
        && !(half > 0.0 && half <= 180.0)
    {
        out.push(invalid(
            "half_angle_deg",
            half,
            "it must be greater than 0 and at most 180 degrees",
            "give `half_angle_deg` a value in (0, 180]",
        ));
    }
    let inner = shape
        .get("inner_radius")
        .and_then(as_stored_f32)
        .unwrap_or(0.0);
    let outer = shape.get("outer_radius").and_then(as_stored_f32);
    if !(inner >= 0.0 && outer.is_none_or(|outer| inner < outer)) {
        out.push(invalid(
            "inner_radius",
            inner,
            "it must be zero or positive and below `outer_radius`",
            "give `inner_radius` a value in [0, outer_radius), or 0 for no inner arc",
        ));
    }
    let fill = shape.get("fill").and_then(as_stored_f32).unwrap_or(0.0);
    if !(0.0..=1.0).contains(&fill) {
        out.push(invalid(
            "fill",
            fill,
            "it is a fraction of `outer_radius` and must be within [0, 1]",
            "give `fill` a value in [0, 1]",
        ));
    }
    out
}

/// `L2106` for every non-positive or non-finite dimension in a component's `shape`, by field name.
fn shape_dimension_diagnostics(value: &Value, noun: &str, component: &str) -> Vec<Diagnostic> {
    let Some(shape) = value.get("shape") else {
        return Vec::new();
    };
    ["radius", "length", "width", "outer_radius", "grid_spacing"]
        .into_iter()
        .filter_map(|field| {
            let dimension = shape.get(field).and_then(as_stored_f32)?;
            (!ludion_core::number::positive_f32(dimension)).then(|| {
                Diagnostic::new(
                    code::GROUND_MARKER_NON_POSITIVE_DIMENSION,
                    format!(
                        "{noun} `{field}` {dimension} is not valid; every `{component}` shape \
                         dimension must be a strictly positive, finite number as an f32 (at least \
                         ~1.4e-45)"
                    ),
                )
                .with_path(format!("/shape/{field}"))
                .with_help("give the dimension a value greater than 0")
            })
        })
        .collect()
}

/// A projected, analytic drop shadow: a soft-edged footprint darkened onto whatever surface lies
/// straight below the entity, so a character reads its height above the ground at a glance.
///
/// Not a light and not a decal: the renderer projects the footprint along world **−Y** from the
/// entity's world position (through the `Parent` hierarchy) onto the first surface beneath it that
/// is not part of the caster itself — the entity's hierarchy root and every descendant. Put the
/// component on a child entity at the character's soles. The entity's rotation only yaws a
/// [`GroundMarkerShape::Segment`] (its +X is the length axis); the shape's dimensions are
/// multiplied by the uniform `Transform.scale.x`. Opacity fades linearly over the last
/// `fade_distance` metres before `max_distance` below the anchor. Render-only: it never touches
/// the sim, and a scene without one renders exactly as before.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct DropShadow {
    /// The footprint (default a disc of radius `0.5`). Dimensions follow the `GroundMarker`
    /// rules: strictly positive and finite (`L2106`).
    #[serde(default = "drop_shadow_shape")]
    pub shape: GroundMarkerShape,
    /// Peak darkness in `[0, 1]` (default `0.6`).
    #[serde(default = "drop_shadow_opacity")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub opacity: f32,
    /// Edge falloff in `[0, 1]` (default `0.5`): `0` is a hard anti-aliased edge, `1` a wide
    /// smudge.
    #[serde(default = "drop_shadow_softness")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub softness: f32,
    /// How far below the anchor, in metres, a surface may be and still receive the shadow
    /// (default `20.0`). Positive and finite (`L2110`).
    #[serde(default = "drop_shadow_max_distance")]
    #[schemars(extend("x-ui-range" = [0.5, 50.0]))]
    pub max_distance: f32,
    /// Length in metres of the linear fade ending at `max_distance` (default `4.0`); `0` cuts
    /// off sharply. Between `0` and `max_distance` (`L2110`).
    #[serde(default = "drop_shadow_fade_distance")]
    #[schemars(extend("x-ui-range" = [0.0, 20.0]))]
    pub fade_distance: f32,
}

fn drop_shadow_shape() -> GroundMarkerShape {
    GroundMarkerShape::Disc { radius: 0.5 }
}

fn drop_shadow_opacity() -> f32 {
    0.6
}

fn drop_shadow_softness() -> f32 {
    0.5
}

fn drop_shadow_max_distance() -> f32 {
    20.0
}

fn drop_shadow_fade_distance() -> f32 {
    4.0
}

impl Default for DropShadow {
    fn default() -> Self {
        DropShadow {
            shape: drop_shadow_shape(),
            opacity: drop_shadow_opacity(),
            softness: drop_shadow_softness(),
            max_distance: drop_shadow_max_distance(),
            fade_distance: drop_shadow_fade_distance(),
        }
    }
}

impl SemanticValidate for DropShadow {
    /// Rejects a shape dimension that is not strictly positive and finite (`L2106`, pointer
    /// `/shape/<field>`) and an unusable distance pair (`L2110`): `max_distance` not positive and
    /// finite, `fade_distance` negative or non-finite, or `fade_distance` above `max_distance`
    /// (pointed at `/fade_distance`). An absent distance is read at its default, so a lone
    /// `fade_distance: 30` is refused against the default `max_distance` of `20`. A lone
    /// `max_distance` below the default `fade_distance` is pointed at `/max_distance` instead, the
    /// field the author wrote.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = shape_dimension_diagnostics(value, "drop shadow", "DropShadow");
        if value
            .get("shape")
            .and_then(|shape| shape.get("type"))
            .and_then(Value::as_str)
            == Some("wedge")
        {
            out.push(
                Diagnostic::new(
                    code::GROUND_MARKER_WEDGE_INVALID,
                    "a drop shadow cannot use a `wedge` shape".to_string(),
                )
                .with_path("/shape/type")
                .with_help("a drop shadow is a disc or a segment"),
            );
        }
        let distance = |field: &str, default: f32| {
            value
                .get(field)
                .and_then(as_stored_f32)
                .unwrap_or(f64::from(default))
        };
        let max = distance("max_distance", drop_shadow_max_distance());
        let fade = distance("fade_distance", drop_shadow_fade_distance());
        let invalid = |field: &str, value: f64, why: &str, help: &str| {
            Diagnostic::new(
                code::DROP_SHADOW_INVALID_DISTANCE,
                format!("drop shadow `{field}` {value} is not valid; {why}"),
            )
            .with_path(format!("/{field}"))
            .with_help(help)
        };
        let max_ok = ludion_core::number::positive_f32(max);
        if !max_ok {
            out.push(invalid(
                "max_distance",
                max,
                "it must be a positive, finite number of metres",
                "give `max_distance` a value greater than 0",
            ));
        }
        let fade_f32 = fade as f32;
        if !fade_f32.is_finite() || fade_f32 < 0.0 {
            out.push(invalid(
                "fade_distance",
                fade,
                "it must be a finite number of metres, not negative",
                "give `fade_distance` a value between 0 and `max_distance`",
            ));
        } else if max_ok && fade_f32 > max as f32 {
            let fade_authored = value.get("fade_distance").and_then(as_stored_f32).is_some();
            let max_authored = value.get("max_distance").and_then(as_stored_f32).is_some();
            if !fade_authored && max_authored {
                out.push(invalid(
                    "max_distance",
                    max,
                    &format!("it is below the default `fade_distance` ({fade})"),
                    &format!(
                        "set `fade_distance` to at most `max_distance`, or raise `max_distance` to at least {fade}"
                    ),
                ));
                return out;
            }
            out.push(invalid(
                "fade_distance",
                fade,
                &format!("it cannot exceed `max_distance` ({max})"),
                "lower `fade_distance` to at most `max_distance`, or raise `max_distance`",
            ));
        }
        out
    }
}

/// How a [`DecalMap`]'s sampler addresses coordinates outside `[0, 1]`, the same on both axes.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MapWrap {
    /// Tile the image (the default).
    #[default]
    Repeat,
    /// Hold the edge texel.
    Clamp,
    /// Tile, mirroring every other copy.
    Mirror,
}

fn is_default_wrap(value: &MapWrap) -> bool {
    *value == MapWrap::Repeat
}

/// How a [`Decal`] composites onto the surface it covers. In every mode, coverage is
/// `texel.a × color.a × opacity × angle weight`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecalBlend {
    /// Lit paint: the decal is a surface (albedo = `texel.rgb × color.rgb`, its own roughness,
    /// metallic and normal) shaded by the lighting its receiver got, blended over the lit result by
    /// coverage. Paint, stripes, tape.
    #[default]
    Alpha,
    /// Unlit darkening: `lit × lerp(1, texel.rgb × color.rgb, coverage)`. Scorch, grime,
    /// footprints.
    Multiply,
    /// `lit + texel.rgb × color.rgb × coverage`, HDR, so it blooms. Glowing route marks,
    /// telegraphs.
    Add,
    /// A clear film: the multiply darkening plus a dielectric specular lobe at the decal's
    /// roughness and normal; the receiver's own colour stays visible. Wet patches, glue, varnish.
    Coat,
}

fn is_default_blend(value: &DecalBlend) -> bool {
    *value == DecalBlend::Alpha
}

/// One texture map of a [`Decal`]: a PNG path and how it is sampled. `path` is relative to the
/// document it is written in.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct DecalMap {
    /// The PNG, relative to the document it is written in.
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub path: String,
    /// The address mode (default `repeat`).
    #[serde(default, skip_serializing_if = "is_default_wrap")]
    pub wrap: MapWrap,
    /// A normal map's strength (default `1`); meaningless on the other maps.
    #[serde(default = "one_f32", skip_serializing_if = "is_one_f32")]
    pub scale: f32,
}

fn one_f32() -> f32 {
    1.0
}

#[allow(clippy::float_cmp)]
fn is_one_f32(value: &f32) -> bool {
    *value == 1.0
}

/// Which surfaces a [`Decal`] paints.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecalReceiverSet {
    /// Every unskinned mesh the box touches.
    #[default]
    All,
    /// Only the meshes of the decal's hierarchy root and its descendants.
    Hierarchy,
}

fn is_default_receivers(value: &DecalReceiverSet) -> bool {
    *value == DecalReceiverSet::All
}

fn decal_size_default() -> [f32; 3] {
    [1.0; 3]
}
fn decal_uv_rect_default() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}
fn decal_color_default() -> [f32; 4] {
    [1.0; 4]
}
fn decal_angle_fade_default() -> [f32; 2] {
    [60.0, 80.0]
}

#[allow(clippy::float_cmp)]
fn is_decal_size_default(value: &[f32; 3]) -> bool {
    *value == decal_size_default()
}
#[allow(clippy::float_cmp)]
fn is_decal_uv_rect_default(value: &[f32; 4]) -> bool {
    *value == decal_uv_rect_default()
}
#[allow(clippy::float_cmp)]
fn is_decal_color_default(value: &[f32; 4]) -> bool {
    *value == decal_color_default()
}
#[allow(clippy::float_cmp)]
fn is_decal_angle_fade_default(value: &[f32; 2]) -> bool {
    *value == decal_angle_fade_default()
}
#[allow(clippy::float_cmp)]
fn is_white3(value: &[f32; 3]) -> bool {
    *value == [1.0; 3]
}
#[allow(clippy::float_cmp)]
fn is_zero_f32(value: &f32) -> bool {
    *value == 0.0
}
fn is_zero_layer(value: &i16) -> bool {
    *value == 0
}

/// A projected decal: an oriented box, centred on the entity and oriented by its world rotation
/// through `Parent`, that paints textures onto the surfaces inside it.
///
/// The box projects along its local **−Y**, so an unrotated decal paints the floor below it; the
/// texture's `u` runs along +X and `v` along +Z. Its extent is `size` times the entity's world
/// scale, per axis. Render-only: it never touches the sim, and a scene without one renders exactly
/// as before. Every field is omitted from the canonical text at its default, so `Decal: {}` is the
/// whole component.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Decal {
    /// The box in metres, before the entity's world scale: `x` by `z` is the footprint, `y` the
    /// projection depth (default `[1, 1, 1]`). Every element strictly positive and finite (`L2106`).
    #[serde(
        default = "decal_size_default",
        skip_serializing_if = "is_decal_size_default"
    )]
    pub size: [f32; 3],
    /// The colour map, sRGB: its rgb multiplies `color` and its alpha is the coverage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_color_texture: Option<DecalMap>,
    /// A tangent-space normal map, applied for the `alpha` and `coat` blends only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal_texture: Option<DecalMap>,
    /// A glTF-packed map (`G` roughness, `B` metallic), applied for the `alpha` and `coat` blends
    /// only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metallic_roughness_texture: Option<DecalMap>,
    /// The sub-rectangle of the maps the box covers, `[u0, v0, u1, v1]` (default the whole image),
    /// in the `Sprite` UV convention. Each element in `[0, 1]`.
    #[serde(
        default = "decal_uv_rect_default",
        skip_serializing_if = "is_decal_uv_rect_default"
    )]
    #[schemars(inner(range(min = 0.0, max = 1.0)))]
    pub uv_rect: [f32; 4],
    /// RGBA tint multiplied into the colour map (default opaque white). HDR for the `add` blend
    /// (`L1263`).
    #[serde(
        default = "decal_color_default",
        skip_serializing_if = "is_decal_color_default"
    )]
    pub color: [f32; 4],
    /// Overall strength in `[0, 1]` (default `1`).
    #[serde(default = "one_f32", skip_serializing_if = "is_one_f32")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub opacity: f32,
    /// The roughness the decal paints, in `[0, 1]`; absent keeps the receiver's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub roughness: Option<f32>,
    /// The metallic value the decal paints, in `[0, 1]` (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub metallic: f32,
    /// How the decal composites (default [`DecalBlend::Alpha`]).
    #[serde(default, skip_serializing_if = "is_default_blend")]
    pub blend: DecalBlend,
    /// The angle fade `[start, end]` in degrees (default `[60, 80]`): the decal's weight is 1 where
    /// the receiving normal is within `start` of the box's local +Y and 0 beyond `end`. Holds
    /// `0 <= start <= end <= 90` (`L2112`).
    #[serde(
        default = "decal_angle_fade_default",
        skip_serializing_if = "is_decal_angle_fade_default"
    )]
    #[schemars(extend("x-ui-range" = [0.0, 90.0]))]
    pub angle_fade: [f32; 2],
    /// The sort key (default `0`). Decals draw in ascending `layer`; within a layer the lowest
    /// entity id draws last and wins.
    #[serde(default, skip_serializing_if = "is_zero_layer")]
    pub layer: i16,
    /// Which surfaces the decal paints (default every unskinned mesh).
    #[serde(default, skip_serializing_if = "is_default_receivers")]
    pub receivers: DecalReceiverSet,
}

impl Default for Decal {
    fn default() -> Self {
        Decal {
            size: decal_size_default(),
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            uv_rect: decal_uv_rect_default(),
            color: decal_color_default(),
            opacity: 1.0,
            roughness: None,
            metallic: 0.0,
            blend: DecalBlend::Alpha,
            angle_fade: decal_angle_fade_default(),
            layer: 0,
            receivers: DecalReceiverSet::All,
        }
    }
}

impl SemanticValidate for Decal {
    /// `color` is an HDR colour (`L1263`); a `size` element that is not strictly positive and
    /// finite is `L2106` at `/size/<n>`; an `angle_fade` element outside `[0, 90]` or non-finite is
    /// `L2112` at `/angle_fade/<n>`, and a pair with `start > end` is `L2112` at `/angle_fade`.
    /// Works by field name, so a schema-invalid value adds nothing it cannot read.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = value_color_diagnostics("Decal", value, "color");
        if let Some(size) = value.get("size").and_then(Value::as_array) {
            for (index, element) in size.iter().enumerate() {
                let Some(dimension) = as_stored_f32(element) else {
                    continue;
                };
                if !ludion_core::number::positive_f32(dimension) {
                    out.push(
                        Diagnostic::new(
                            code::GROUND_MARKER_NON_POSITIVE_DIMENSION,
                            format!(
                                "decal `size` element {index} is {dimension}, which is not valid; \
                                 every `Decal` size element must be a strictly positive, finite \
                                 number as an f32 (at least ~1.4e-45)"
                            ),
                        )
                        .with_path(format!("/size/{index}"))
                        .with_help("give the element a value greater than 0"),
                    );
                }
            }
        }
        if let Some(fade) = value.get("angle_fade").and_then(Value::as_array) {
            let mut all_valid = fade.len() == 2;
            let mut angles = [0.0_f32; 2];
            for (index, element) in fade.iter().enumerate() {
                let Some(angle) = as_stored_f32(element) else {
                    all_valid = false;
                    continue;
                };
                let angle = angle as f32;
                if !angle.is_finite() || !(0.0..=90.0).contains(&angle) {
                    all_valid = false;
                    out.push(
                        Diagnostic::new(
                            code::DECAL_INVALID_ANGLE_FADE,
                            format!(
                                "decal `angle_fade` element {index} is {angle}, which is not \
                                 valid; it must be a finite angle in degrees within 0 to 90"
                            ),
                        )
                        .with_path(format!("/angle_fade/{index}"))
                        .with_help("give `angle_fade` two angles with 0 <= start <= end <= 90"),
                    );
                } else if let Some(slot) = angles.get_mut(index) {
                    *slot = angle;
                }
            }
            let [start, end] = angles;
            if all_valid && start > end {
                out.push(
                    Diagnostic::new(
                        code::DECAL_INVALID_ANGLE_FADE,
                        format!(
                            "decal `angle_fade` start {start} is above its end {end}; the fade \
                             would run backwards"
                        ),
                    )
                    .with_path("/angle_fade")
                    .with_help("order the pair as [start, end] with start <= end"),
                );
            }
        }
        out
    }
}

/// A camera's projection. The pose comes from the entity's [`Transform`] (the camera looks down its
/// local −Z axis).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Projection {
    /// A perspective projection with a vertical field of view in degrees.
    Perspective {
        /// Vertical field of view, in degrees. Both ends of the open interval `0..180` are a
        /// degenerate projection, so the travel range is the usable photographic span around the
        /// default `60`, not the interval itself.
        #[schemars(extend("x-ui-range" = [10.0, 150.0]))]
        fov_y_deg: f32,
    },
    /// An orthographic projection with a given world-space view height (width follows the aspect).
    Orthographic {
        /// The world-space height the viewport spans.
        height: f32,
        /// Pixel-perfect snapping (default `false`). When `true`, extract snaps the view height to an
        /// integer texel scale and each sprite centre to the texel grid (reading the atlas
        /// `pixels_per_unit`), so crisp source art lands on whole pixels and stops shimmering under
        /// motion. Extract-only: the sim never sees it and the world hash is unchanged.
        #[serde(default)]
        pixel_snap: bool,
    },
}

impl Default for Projection {
    fn default() -> Self {
        Projection::Perspective { fov_y_deg: 60.0 }
    }
}

/// The scene's camera. Exactly one is expected; zero uses a default framing, more than one is an
/// error at extract time (`L2101`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// The projection (default 60° perspective).
    #[serde(default)]
    pub projection: Projection,
    /// Near clip plane distance, validated `> 0` and below `far` (default `0.1`).
    #[serde(default = "near_default")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub near: f32,
    /// Far clip plane distance, must exceed `near` (`L1264`; default `1000.0`).
    #[serde(default = "far_default")]
    pub far: f32,
    /// Mesh LOD bias added to the machine's quality bias (default none, meaning `0`): each unit
    /// halves every switch distance, so `+1` picks coarser levels sooner and `-1` holds fine levels
    /// longer. Finite, within `-4..=4` (`L1397`). Absent is omitted from the document and the hash;
    /// a value is hashed like any `Camera` field. See `ludion docs culling`. A free `render.view`
    /// camera takes the game camera's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = -4.0, max = 4.0))]
    pub lod_bias: Option<f32>,
    /// Levels a shadow draws coarser than the camera's own pick (default none, meaning `0`), at
    /// most 3 (`L1397`). Absent is omitted from the document and the hash; a value is hashed like any
    /// `Camera` field. See `ludion docs culling`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0, max = 3))]
    pub shadow_bias_levels: Option<u32>,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            projection: Projection::default(),
            near: 0.1,
            far: 1000.0,
            lod_bias: None,
            shadow_bias_levels: None,
        }
    }
}

impl Camera {
    /// The `(near, far)` a projection is built from: the camera's own when both are finite with
    /// `0 < near < far`, else [`Camera::default`]'s. The semantic validator refuses such a range
    /// (`L1014`, `L1264`); a game building the component in Rust bypasses it, and every projection
    /// built from a camera stays NaN-free by falling back instead.
    ///
    /// ```
    /// use ludion_components::Camera;
    ///
    /// let inverted = Camera { near: 5.0, far: 1.0, ..Camera::default() };
    /// assert_eq!(inverted.depth_range(), (0.1, 1000.0));
    /// let ok = Camera { near: 0.5, far: 50.0, ..Camera::default() };
    /// assert_eq!(ok.depth_range(), (0.5, 50.0));
    /// ```
    pub fn depth_range(&self) -> (f32, f32) {
        if self.near.is_finite() && self.far.is_finite() && 0.0 < self.near && self.near < self.far
        {
            (self.near, self.far)
        } else {
            let default = Camera::default();
            (default.near, default.far)
        }
    }
}

impl SemanticValidate for Camera {
    /// Rejects a `near` that is not less than `far` (`L1264`, pointer `/far`, or `/near` if far is
    /// not authored); `near > 0` rides the schema (`L1014`). An absent bound compares as its
    /// default, so `near: 2000` alone is caught.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Self::validate_lod(value);
        found.extend(Self::validate_depth(value));
        found
    }
}

impl Camera {
    /// `lod_bias` finite within `-4..=4` and `shadow_bias_levels` at most 3 (`L1397`).
    fn validate_lod(value: &Value) -> Vec<Diagnostic> {
        let mut found = Vec::new();
        if let Some(bias) = value.get("lod_bias").and_then(Value::as_f64)
            && !(-4.0..=4.0).contains(&bias)
        {
            found.push(
                Diagnostic::new(
                    code::CAMERA_LOD_INVALID,
                    format!("the `Camera` `lod_bias` {bias} is outside -4 to 4"),
                )
                .with_path("/lod_bias".to_string())
                .with_help("use a bias in -4..=4; each unit halves every LOD switch distance"),
            );
        }
        if let Some(levels) = value.get("shadow_bias_levels").and_then(Value::as_u64)
            && levels > 3
        {
            found.push(
                Diagnostic::new(
                    code::CAMERA_LOD_INVALID,
                    format!("the `Camera` `shadow_bias_levels` {levels} is above 3"),
                )
                .with_path("/shadow_bias_levels".to_string())
                .with_help("use 0 to 3; a mesh chain has at most four levels"),
            );
        }
        found
    }

    fn validate_depth(value: &Value) -> Vec<Diagnostic> {
        let default = Camera::default();
        let near = value.get("near").and_then(Value::as_f64);
        let far = value.get("far").and_then(Value::as_f64);
        if near.is_none() && far.is_none() {
            return Vec::new();
        }
        let near_value = near.unwrap_or(f64::from(default.near));
        let far_value = far.unwrap_or(f64::from(default.far));
        if near_value < far_value {
            return Vec::new();
        }
        let pointer = if far.is_some() { "/far" } else { "/near" };
        vec![
            Diagnostic::new(
                code::CAMERA_INVALID_DEPTH_RANGE,
                format!(
                    "the `Camera` depth range is empty: near {near_value} is not less than far \
                     {far_value}, so the projection has no depth"
                ),
            )
            .with_path(pointer.to_string())
            .with_help("make `far` greater than `near` (defaults 0.1 and 1000.0)"),
        ]
    }
}

/// A presentation-side offset of the rendered camera: the frame is drawn from the rig's pose moved
/// by `translation` and `rotation_deg` (camera-local, like shake) with `fov_delta` added to a
/// perspective field of view. It lives on a **presentation entity** (`presentation: true`, or
/// `spawn_presentation`), so it is never hashed and the sim, the rig, the audio listener and the
/// `camera` debug draw keep the rig's pose. A boot intro pulls in by easing `translation[2]` from
/// positive to 0. All fields default to 0 and are omitted from text at 0, and `CameraOffset: {}` is
/// the identity.
///
/// `translation` is `[x right, y up, z back]` in the camera's own frame; `rotation_deg` is
/// Euler-XYZ degrees, applied after the rig's rotation. `fov_delta` is ignored by an orthographic
/// camera. The lowest-id offset on a presentation entity wins (`L1334` for the rest); on a sim
/// entity it is never applied (`L1333`); a non-finite field is refused in a scene and ignores the
/// whole offset at runtime (`L1332`). Without a camera it does nothing.
///
/// ```yaml
/// presentation: true
/// components:
///   CameraOffset:
///     translation: [0.0, 0.0, 4.0]
///     fov_delta: -10.0
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct CameraOffset {
    /// Camera-local shift `[x right, y up, z back]` in world units (default `[0, 0, 0]`).
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub translation: [f32; 3],
    /// Camera-local rotation as Euler XYZ degrees, composed after the rig's rotation (default
    /// `[0, 0, 0]`).
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub rotation_deg: [f32; 3],
    /// Degrees added to a perspective camera's `fov_y_deg`, clamped to 1 to 179 (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub fov_delta: f32,
}

impl CameraOffset {
    /// The perspective field of view, in degrees, a result is clamped into.
    pub const FOV_RANGE: (f32, f32) = (1.0, 179.0);

    /// Whether every field is 0, so applying the offset changes nothing.
    ///
    /// ```
    /// use ludion_components::CameraOffset;
    ///
    /// assert!(CameraOffset::default().is_identity());
    /// assert!(!CameraOffset { fov_delta: 5.0, ..CameraOffset::default() }.is_identity());
    /// ```
    #[allow(clippy::float_cmp)]
    pub fn is_identity(&self) -> bool {
        is_zero3(&self.translation) && is_zero3(&self.rotation_deg) && self.fov_delta == 0.0
    }

    /// Whether every field is finite.
    ///
    /// ```
    /// use ludion_components::CameraOffset;
    ///
    /// assert!(CameraOffset::default().is_finite());
    /// assert!(!CameraOffset { fov_delta: f32::NAN, ..CameraOffset::default() }.is_finite());
    /// ```
    pub fn is_finite(&self) -> bool {
        self.translation
            .iter()
            .chain(&self.rotation_deg)
            .chain(std::iter::once(&self.fov_delta))
            .all(|value| value.is_finite())
    }

    /// The camera pose (`translation`, rotation quaternion `[x, y, z, w]`) with the offset applied:
    /// the translation is rotated by the rig's rotation, the rotation composes after it.
    ///
    /// ```
    /// use ludion_components::CameraOffset;
    ///
    /// let offset = CameraOffset { translation: [0.0, 0.0, 2.0], ..CameraOffset::default() };
    /// let (translation, rotation) = offset.pose([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0]);
    /// assert_eq!(translation, [1.0, 2.0, 5.0]);
    /// assert_eq!(rotation, [0.0, 0.0, 0.0, 1.0]);
    /// ```
    pub fn pose(&self, translation: [f32; 3], rotation: [f32; 4]) -> ([f32; 3], [f32; 4]) {
        let shift = quat_rotate(rotation, self.translation);
        (
            [0, 1, 2].map(|i| translation[i] + shift[i]),
            quat_mul(rotation, euler_xyz_degrees_to_quat(self.rotation_deg)),
        )
    }

    /// The projection with `fov_delta` added to a perspective field of view, clamped to
    /// [`CameraOffset::FOV_RANGE`]. A zero delta and an orthographic projection come back unchanged.
    ///
    /// ```
    /// use ludion_components::{CameraOffset, Projection};
    ///
    /// let offset = CameraOffset { fov_delta: -10.0, ..CameraOffset::default() };
    /// let narrowed = offset.projection(Projection::Perspective { fov_y_deg: 60.0 });
    /// assert_eq!(narrowed, Projection::Perspective { fov_y_deg: 50.0 });
    /// let ortho = Projection::Orthographic { height: 8.0, pixel_snap: false };
    /// assert_eq!(offset.projection(ortho), ortho);
    /// ```
    #[allow(clippy::float_cmp)]
    pub fn projection(&self, projection: Projection) -> Projection {
        match projection {
            Projection::Perspective { fov_y_deg } if self.fov_delta != 0.0 => {
                Projection::Perspective {
                    fov_y_deg: (fov_y_deg + self.fov_delta)
                        .clamp(Self::FOV_RANGE.0, Self::FOV_RANGE.1),
                }
            }
            other => other,
        }
    }

    /// Whether `fov_delta` would take `projection` outside [`CameraOffset::FOV_RANGE`], so
    /// [`CameraOffset::projection`] clamps it.
    ///
    /// ```
    /// use ludion_components::{CameraOffset, Projection};
    ///
    /// let offset = CameraOffset { fov_delta: 150.0, ..CameraOffset::default() };
    /// assert!(offset.clamps_fov(&Projection::Perspective { fov_y_deg: 60.0 }));
    /// assert!(!offset.clamps_fov(&Projection::Orthographic { height: 8.0, pixel_snap: false }));
    /// ```
    #[allow(clippy::float_cmp)]
    pub fn clamps_fov(&self, projection: &Projection) -> bool {
        match projection {
            Projection::Perspective { fov_y_deg } if self.fov_delta != 0.0 => {
                let sum = fov_y_deg + self.fov_delta;
                sum < Self::FOV_RANGE.0 || sum > Self::FOV_RANGE.1
            }
            _ => false,
        }
    }

    /// The rendered camera for a rig pose: a game anchoring screen UI with `world_to_pixel` passes
    /// these to follow the drawn frame instead of the rig. `scale`, `near` and `far` are kept.
    ///
    /// ```
    /// use ludion_components::{Camera, CameraOffset, Transform};
    ///
    /// let offset = CameraOffset { translation: [0.0, 0.0, 2.0], ..CameraOffset::default() };
    /// let (transform, camera) = offset.applied_to(&Transform::default(), &Camera::default());
    /// assert_eq!(transform.translation, [0.0, 0.0, 2.0]);
    /// assert_eq!(camera, Camera::default());
    /// ```
    pub fn applied_to(&self, transform: &Transform, camera: &Camera) -> (Transform, Camera) {
        let (translation, rotation) = self.pose(
            transform.translation,
            euler_xyz_degrees_to_quat(transform.rotation),
        );
        (
            Transform {
                translation,
                rotation: quat_to_euler_xyz_degrees(rotation),
                scale: transform.scale,
            },
            Camera {
                projection: self.projection(camera.projection),
                ..*camera
            },
        )
    }
}

impl SemanticValidate for CameraOffset {
    /// Rejects a field that is not finite once stored as `f32` (`L1332`, pointer `/translation/{i}`,
    /// `/rotation_deg/{i}` or `/fov_delta`). A wrong shape is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for field in ["translation", "rotation_deg"] {
            let Some(items) = value.get(field).and_then(Value::as_array) else {
                continue;
            };
            for (index, item) in items.iter().enumerate() {
                if as_stored_f32(item).is_some_and(|number| !number.is_finite()) {
                    out.push(camera_offset_diagnostic(&format!("/{field}/{index}")));
                }
            }
        }
        if value
            .get("fov_delta")
            .and_then(as_stored_f32)
            .is_some_and(|number| !number.is_finite())
        {
            out.push(camera_offset_diagnostic("/fov_delta"));
        }
        out
    }
}

fn camera_offset_diagnostic(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::CAMERA_OFFSET_INVALID,
        format!("the `CameraOffset` field `{pointer}` is not a finite number"),
    )
    .with_path(pointer.to_string())
    .with_help("write finite numbers; `ludion explain L1332`")
}

#[allow(clippy::float_cmp)]
fn is_zero3(value: &[f32; 3]) -> bool {
    *value == [0.0; 3]
}

/// Marks a mesh as a first-person **viewmodel** (a held weapon or tool): drawn with its own
/// projection and a depth slice in front of the world, so it never clips into a wall. Authored
/// `Viewmodel: {}`; a field-less flag, so any extra field raises the ordinary `L1014`.
///
/// The flag is inherited down the `Parent`/`ParentJoint` chain by **OR**: one flagged ancestor
/// makes every descendant mesh a viewmodel (unlike `Visibility`, whose fold is AND). Author the gun
/// on **presentation entities** (`presentation: true`) parented to the first-person camera, so the
/// flag stays out of `WorldHash`; on a sim entity it is hashed like any other component. It is
/// render-only: the sim never reads it.
///
/// The projection comes from the camera's optional [`ViewmodelLens`]; absent means its defaults.
/// That projection is independent of the gameplay `fov_y_deg`, of `CameraOffset.fov_delta` and of
/// the late latch. Flagged meshes cast no world shadow. When no single perspective first-person
/// `CameraRig` camera exists the mesh draws as an ordinary world mesh and is reported once
/// (`L1336`).
///
/// ```yaml
/// presentation: true
/// components:
///   Parent:
///     index: 0          # the first-person camera: the first entity in the list
///     generation: 0
///   Viewmodel: {}
///   Mesh:
///     shape: cube
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
// A zero-field struct's derived schema omits `properties`, and the unknown-field check only fires
// when it is present; forcing the empty map keeps `Viewmodel: { x: 1 }` an ordinary `L1014`.
#[schemars(extend("properties" = serde_json::json!({})))]
pub struct Viewmodel {}

/// Marks a renderable that does not move, so the renderer can keep it in a persistent buffer.
/// Authored `Static: {}`; a field-less flag, so any extra field raises the ordinary `L1014`.
///
/// Eligible: a `Mesh` with a primitive shape, an unskinned `asset`, a `voxel` or `sculpt` model
/// (LOD chains included), or a `MeshGrid`, mirrored placements included. Anything else (a skinned
/// asset, `assembly`, `assembly_part`, `character`, `SkinBone`, `ParentJoint`, `Viewmodel`, a
/// sprite, particle emitter, text, decal, light, `GroundMarker`, `Volley`, `PatternPlayer`) is
/// reported `L1341` and takes the dynamic path. A `Static` with no renderable is a transform
/// group: never reported, and its `Static` children stay static. Every `Parent` ancestor must be
/// `Static` too; otherwise the entity is reported `L1342` and takes the dynamic path, since a
/// static under a moving parent would go stale.
///
/// Writes stay legal: a written static entity is re-placed. One written on 30 consecutive frames
/// is reported once (`L1343`); remove `Static` from it. On a sim entity the flag is hashed like any
/// other component.
///
/// ```yaml
/// components:
///   Transform:
///     position: [4.0, 0.0, 2.0]
///   Static: {}
///   Mesh:
///     shape: cube
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
// A zero-field struct's derived schema omits `properties`, and the unknown-field check only fires
// when it is present; forcing the empty map keeps `Static: { x: 1 }` an ordinary `L1014`.
#[schemars(extend("properties" = serde_json::json!({})))]
pub struct Static {}

/// The projection and depth slice a [`Viewmodel`] is drawn with. It sits on the **camera entity**
/// (the first-person camera); absent means the defaults, and `ViewmodelLens: {}` is the default.
/// Every field defaults and is omitted from text at its default.
///
/// The lens projection is independent of the camera's gameplay `fov_y_deg`, of
/// `CameraOffset.fov_delta` and of the late latch, so a wide gameplay FOV does not stretch the gun.
/// `ViewmodelLens` is constant config; on the sim camera it is hashed. On an entity without a
/// `Camera` it is ignored and reported once (`L1337`). A field
/// out of range is `L1338`: a scene with one is refused, and at runtime the defaults are used.
///
/// `fov_y_deg` is the vertical field of view in degrees, finite in 1 to 179. `near` is the near
/// plane in world units, finite and above 0. `depth_slice` is the fraction of the depth range the
/// viewmodel occupies, finite in (0, 0.5].
///
/// ```yaml
/// components:
///   Camera: {}
///   ViewmodelLens:
///     fov_y_deg: 60.0
///     depth_slice: 0.02
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct ViewmodelLens {
    /// Vertical field of view of the viewmodel projection, degrees (default `55`).
    #[serde(
        default = "viewmodel_fov_default",
        skip_serializing_if = "is_viewmodel_fov_default"
    )]
    pub fov_y_deg: f32,
    /// Near plane of the viewmodel projection, world units (default `0.01`).
    #[serde(
        default = "viewmodel_near_default",
        skip_serializing_if = "is_viewmodel_near_default"
    )]
    pub near: f32,
    /// Fraction of the depth range the viewmodel is drawn in, in (0, 0.5] (default `0.01`).
    #[serde(
        default = "viewmodel_depth_slice_default",
        skip_serializing_if = "is_viewmodel_depth_slice_default"
    )]
    pub depth_slice: f32,
}

impl Default for ViewmodelLens {
    fn default() -> Self {
        ViewmodelLens {
            fov_y_deg: viewmodel_fov_default(),
            near: viewmodel_near_default(),
            depth_slice: viewmodel_depth_slice_default(),
        }
    }
}

fn viewmodel_fov_default() -> f32 {
    55.0
}
fn viewmodel_near_default() -> f32 {
    0.01
}
fn viewmodel_depth_slice_default() -> f32 {
    0.01
}
fn is_viewmodel_fov_default(value: &f32) -> bool {
    value.to_bits() == viewmodel_fov_default().to_bits()
}
fn is_viewmodel_near_default(value: &f32) -> bool {
    value.to_bits() == viewmodel_near_default().to_bits()
}
fn is_viewmodel_depth_slice_default(value: &f32) -> bool {
    value.to_bits() == viewmodel_depth_slice_default().to_bits()
}

impl SemanticValidate for ViewmodelLens {
    /// Rejects `fov_y_deg` outside [1, 179], `near` not above 0 and `depth_slice` outside (0, 0.5],
    /// or any of them non-finite (`L1338`, pointer `/fov_y_deg`, `/near` or `/depth_slice`). An
    /// absent field is its default and adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut check = |field: &str, ok: fn(f64) -> bool, expected: &str| {
            if let Some(number) = value.get(field).and_then(as_stored_f32)
                && (!number.is_finite() || !ok(number))
            {
                out.push(
                    Diagnostic::new(
                        code::VIEWMODEL_LENS_INVALID,
                        format!("the `ViewmodelLens` field `/{field}` must be {expected}"),
                    )
                    .with_path(format!("/{field}"))
                    .with_help("pick a value in range; `ludion explain L1338`"),
                );
            }
        };
        check(
            "fov_y_deg",
            |n| (1.0..=179.0).contains(&n),
            "finite and in 1 to 179 degrees",
        );
        check("near", |n| n > 0.0, "finite and above 0");
        check(
            "depth_slice",
            |n| n > 0.0 && n <= 0.5,
            "finite and in (0, 0.5]",
        );
        out
    }
}

/// A navigating agent. A game writes `target` each tick; `nav_steer` (in `ludion-engine`) writes the
/// desired velocity into [`NavSteering`]. A walking agent follows the shared flow field toward its
/// target; a flying one seeks it in 3D. Both add separation from their `spatial3d` neighbours. The
/// game's own mover applies the velocity. Every field defaults and is omitted from text at its
/// default; `NavAgent: {}` is the default agent.
///
/// `speed`, `arrive_radius`, `separation_radius` and `separation_strength` are finite and not
/// negative; `separation_neighbors` is at most 16. A field out of range is `L1340`. `neighbor_layers`
/// is the `Index3d` layer mask the neighbours are read from; `0` turns separation off.
///
/// ```yaml
/// components:
///   NavAgent:
///     speed: 3.5
///     neighbor_layers: 2
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct NavAgent {
    /// World position the agent heads for (default the origin).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub target: [f32; 3],
    /// Desired speed, metres per second (default `4`).
    #[serde(
        default = "nav_speed_default",
        skip_serializing_if = "is_nav_speed_default"
    )]
    pub speed: f32,
    /// A flyer seeks in 3D and never reads the nav grid (default `false`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub flying: bool,
    /// Inside this distance of the target the seek is zero; horizontal for a walker, 3D for a flyer
    /// (default `0.5`).
    #[serde(
        default = "nav_arrive_default",
        skip_serializing_if = "is_nav_arrive_default"
    )]
    pub arrive_radius: f32,
    /// Neighbours closer than this push the agent away (default `0.75`).
    #[serde(
        default = "nav_sep_radius_default",
        skip_serializing_if = "is_nav_sep_radius_default"
    )]
    pub separation_radius: f32,
    /// Speed added per metre of summed offset from the neighbours (default `2`).
    #[serde(
        default = "nav_sep_strength_default",
        skip_serializing_if = "is_nav_sep_strength_default"
    )]
    pub separation_strength: f32,
    /// Most neighbours considered, at most 16 (default `4`).
    #[serde(
        default = "nav_sep_neighbors_default",
        skip_serializing_if = "is_nav_sep_neighbors_default"
    )]
    pub separation_neighbors: u32,
    /// `Index3d` layer mask the neighbours come from; `0` disables separation (default all bits).
    #[serde(
        default = "nav_neighbor_layers_default",
        skip_serializing_if = "is_nav_neighbor_layers_default"
    )]
    pub neighbor_layers: u32,
}

/// Most separation neighbours a [`NavAgent`] may ask for.
pub const NAV_AGENT_MAX_NEIGHBORS: u32 = 16;

impl Default for NavAgent {
    fn default() -> Self {
        NavAgent {
            target: [0.0; 3],
            speed: nav_speed_default(),
            flying: false,
            arrive_radius: nav_arrive_default(),
            separation_radius: nav_sep_radius_default(),
            separation_strength: nav_sep_strength_default(),
            separation_neighbors: nav_sep_neighbors_default(),
            neighbor_layers: nav_neighbor_layers_default(),
        }
    }
}

fn is_zero_vec3(value: &[f32; 3]) -> bool {
    value.iter().all(|c| c.to_bits() == 0)
}
fn nav_speed_default() -> f32 {
    4.0
}
fn nav_arrive_default() -> f32 {
    0.5
}
fn nav_sep_radius_default() -> f32 {
    0.75
}
fn nav_sep_strength_default() -> f32 {
    2.0
}
fn nav_sep_neighbors_default() -> u32 {
    4
}
fn nav_neighbor_layers_default() -> u32 {
    u32::MAX
}
fn is_nav_speed_default(value: &f32) -> bool {
    value.to_bits() == nav_speed_default().to_bits()
}
fn is_nav_arrive_default(value: &f32) -> bool {
    value.to_bits() == nav_arrive_default().to_bits()
}
fn is_nav_sep_radius_default(value: &f32) -> bool {
    value.to_bits() == nav_sep_radius_default().to_bits()
}
fn is_nav_sep_strength_default(value: &f32) -> bool {
    value.to_bits() == nav_sep_strength_default().to_bits()
}
fn is_nav_sep_neighbors_default(value: &u32) -> bool {
    *value == nav_sep_neighbors_default()
}
fn is_nav_neighbor_layers_default(value: &u32) -> bool {
    *value == nav_neighbor_layers_default()
}

impl SemanticValidate for NavAgent {
    /// Rejects a non-finite `target` component (`/target/{i}`), a `speed`, `arrive_radius`,
    /// `separation_radius` or `separation_strength` that is negative or non-finite, and
    /// `separation_neighbors` above 16 (`L1340`). An absent field is its default and adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut reject = |pointer: String, what: String| {
            out.push(
                Diagnostic::new(code::NAV_AGENT_INVALID, what)
                    .with_path(pointer)
                    .with_help("use a finite, non-negative value; `ludion explain L1340`"),
            );
        };
        if let Some(items) = value.get("target").and_then(Value::as_array) {
            for (i, item) in items.iter().enumerate() {
                if let Some(n) = as_stored_f32(item)
                    && !n.is_finite()
                {
                    reject(
                        format!("/target/{i}"),
                        format!("the `NavAgent` target component {i} must be finite"),
                    );
                }
            }
        }
        for field in [
            "speed",
            "arrive_radius",
            "separation_radius",
            "separation_strength",
        ] {
            if let Some(n) = value.get(field).and_then(as_stored_f32)
                && (!n.is_finite() || n < 0.0)
            {
                reject(
                    format!("/{field}"),
                    format!("the `NavAgent` field `/{field}` must be finite and not negative"),
                );
            }
        }
        if let Some(n) = value.get("separation_neighbors").and_then(Value::as_u64)
            && n > u64::from(NAV_AGENT_MAX_NEIGHBORS)
        {
            reject(
                "/separation_neighbors".to_string(),
                format!(
                    "`NavAgent` `separation_neighbors` is {n}, above the limit of {NAV_AGENT_MAX_NEIGHBORS}"
                ),
            );
        }
        out
    }
}

/// The desired velocity `nav_steer` computed for a [`NavAgent`] this tick, in metres per second:
/// the seek toward its target plus separation. The game's mover reads it and moves the entity;
/// nothing else applies it. Defaults to zero.
///
/// ```yaml
/// components:
///   NavSteering: {}
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq)]
pub struct NavSteering {
    /// Desired velocity, world axes (default zero).
    #[serde(default, skip_serializing_if = "is_zero_vec3")]
    pub velocity: [f32; 3],
}

impl SemanticValidate for Text {
    /// `color` is an HDR colour (`L1263`); `size`, the world height of one line, is a strictly
    /// positive, finite number (`L1118`, pointer `/size`). An absent `size` (default `1.0`) adds
    /// nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = value_color_diagnostics("Text", value, "color");
        if let Some(size) = value.get("size").and_then(as_stored_f32) {
            require_positive_finite_ui_size(size, "/size", &mut out);
        }
        out
    }
}

/// A run of text (the embedded 8×8 face, or a baked face named by `font`) placed in world space at
/// the entity's [`Transform`] position.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Text {
    /// The text to render; characters the face lacks draw as its `.notdef` glyph (the built-in 8×8 face draws them blank). Default `""` (an empty run draws nothing) — drift-safety: every built-in field must survive
    /// deserializing from an empty object (see `ludion_testkit::assert_components_drift_safe`).
    #[serde(default)]
    pub text: String,
    /// World height of one line — the face's line height (default `1.0`).
    #[serde(default = "one")]
    pub size: f32,
    /// RGBA text color (default opaque white).
    #[serde(default = "white4")]
    pub color: [f32; 4],
    /// Logical face name, resolved through the content root's `fonts.yaml`; absent → the table's
    /// `default` entry, else the built-in 8×8 face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
}

impl Default for Text {
    fn default() -> Self {
        Text {
            text: String::new(),
            size: 1.0,
            color: [1.0; 4],
            font: None,
        }
    }
}

/// One legend entry: how a tilemap glyph looks. Reuses [`Sprite`]'s `texture`/`color` field shapes —
/// a tile is a flat textured, tinted quad exactly like a sprite, sized to one cell.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct TileAppearance {
    /// Texture asset path (relative to the content root — the directory of the scene the engine was started with), or `None` for a plain colored quad.
    #[serde(default)]
    #[schemars(with = "Option<ludion_ecs::AssetPathSchema>")]
    pub texture: Option<String>,
    /// A `kind: atlas` document path (relative to the content root), or `None`. The region named by
    /// [`frame`](TileAppearance::frame) supplies the pixels and the UV rect; the cell's world size
    /// stays [`Tilemap::cell_size`]. Mutually exclusive with
    /// [`texture`](TileAppearance::texture) (`L1151`).
    #[serde(default)]
    #[schemars(with = "Option<ludion_ecs::AssetPathSchema>")]
    pub atlas: Option<String>,
    /// The region name inside [`atlas`](TileAppearance::atlas). Required with `atlas` and
    /// meaningless without it (`L1151`).
    #[serde(default)]
    pub frame: Option<String>,
    /// RGBA tint multiplied into the sampled texel (default opaque white). To leave a cell visually
    /// empty, give its glyph a background color here — there is no implicit blank glyph.
    #[serde(default = "white4")]
    pub color: [f32; 4],
    /// Whether the cell takes light (default `false`) — the [`Sprite::lit`] meaning, per glyph.
    #[serde(default)]
    pub lit: bool,
    /// Mirror this glyph's art left↔right (default `false`) — the [`Sprite::flip_x`] meaning, per
    /// glyph. Applied to the UV rect at extract.
    #[serde(default)]
    pub flip_x: bool,
    /// Mirror this glyph's art top↔bottom (default `false`) — the [`Sprite::flip_y`] meaning.
    #[serde(default)]
    pub flip_y: bool,
}

impl Default for TileAppearance {
    fn default() -> Self {
        TileAppearance {
            texture: None,
            atlas: None,
            frame: None,
            color: [1.0; 4],
            lit: false,
            flip_x: false,
            flip_y: false,
        }
    }
}

/// A grid of single-glyph cells, authored inline in the scene and expanded into the sprite batcher
/// at render time — the content type a tile-based game (Sokoban, Breakout) reads for walls, floors,
/// and targets. A level edit is a glyph-grid diff, the minimal-diff pillar at its best.
///
/// # Authoring
///
/// - `cell_size` — the world-unit width and height of one square cell.
/// - `legend` — a map from a single-glyph string to a [`TileAppearance`]. A *glyph* is exactly one
///   Unicode scalar value (one Rust `char`); a legend key that is empty or longer is rejected
///   (`L1020`). Every glyph used in `rows` must have a legend entry — there is no implicit blank
///   glyph, so leave a cell "empty" by giving its glyph a background-colored tile.
/// - `rows` — a list of equal-length strings, one glyph per cell, top row first. Ragged rows (a row
///   whose glyph count differs) are `L1019`; a glyph with no legend entry is `L1020`.
///
/// Every field is `#[serde(default)]`, so an empty tilemap (`{}`) is valid and renders nothing
/// (drift-safe; see `ludion_testkit::assert_components_drift_safe`).
///
/// # Placement convention (unambiguous — game logic depends on it)
///
/// The grid is **axis-aligned in the world XY plane**. Only the entity's world translation (its
/// `Transform.translation` when unparented) is used, as the **grid origin — the center of cell `(col 0, row 0)`**, the first glyph of the first
/// row. `Transform.rotation` and `Transform.scale` are **ignored**: a tilemap is a grid of cells,
/// not a transformable sprite (this is what makes the coordinate helpers exact and unambiguous).
///
/// From the origin:
/// - **column** index increases along **+X** (`col * cell_size`);
/// - **row** index increases along **−Y** (`-row * cell_size`) — the first row is at the top and
///   later rows go *down*, matching reading order and the [`Text`] layout convention.
///
/// So cell `(col, row)`'s world center is
/// `[origin.x + col*cell_size, origin.y - row*cell_size, origin.z]`, and each cell is a
/// `cell_size × cell_size` non-billboard quad. Expansion order is **row-major** (rows outer, columns
/// inner), so the sprite instances — and therefore the render — are deterministic. For
/// `render.bounds`, the whole grid reports as **one renderable** whose AABB is the union of every
/// cell (the honest bound: a tilemap is one entity).
///
/// # Reading a tilemap from a game system
///
/// Use [`glyph_at`](Tilemap::glyph_at) to read a cell's glyph, and
/// [`cell_to_world`](Tilemap::cell_to_world) / [`world_to_cell`](Tilemap::world_to_cell) to map
/// between cell coordinates and world space (both take the grid origin — the tilemap entity's
/// world translation, its `Transform.translation` when unparented).
///
/// ```
/// use ludion_components::{TileAppearance, Tilemap};
/// use std::collections::BTreeMap;
///
/// let mut legend = BTreeMap::new();
/// legend.insert("#".to_string(), TileAppearance::default()); // wall
/// legend.insert(".".to_string(), TileAppearance::default()); // floor
/// let map = Tilemap {
///     cell_size: 2.0,
///     legend,
///     rows: vec!["##".to_string(), "#.".to_string()],
///     layer: 0,
/// };
///
/// assert_eq!(map.glyph_at(0, 0), Some('#'));
/// assert_eq!(map.glyph_at(1, 1), Some('.'));
/// assert_eq!(map.glyph_at(2, 0), None); // out of range
///
/// // Grid origin is this tilemap entity's Transform.translation.
/// let origin = [10.0, 5.0, 0.0];
/// // cell (1, 0) is one cell to the +X of the origin: 10 + 1*2 = 12, y unchanged.
/// assert_eq!(map.cell_to_world(origin, 1, 0), [12.0, 5.0, 0.0]);
/// // cell (0, 1) is one cell down (−Y): y = 5 - 1*2 = 3.
/// assert_eq!(map.cell_to_world(origin, 0, 1), [10.0, 3.0, 0.0]);
/// // world → cell is the exact inverse for a cell center.
/// assert_eq!(map.world_to_cell(origin, [12.0, 3.0, 0.0]), (1, 1));
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Tilemap {
    /// World-unit width and height of one square cell (default `1.0`), validated **strictly greater
    /// than zero**.
    #[serde(default = "one")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub cell_size: f32,
    /// Glyph → tile appearance. Keys are single glyphs (`L1020` otherwise).
    #[serde(default)]
    pub legend: BTreeMap<String, TileAppearance>,
    /// The grid: equal-length strings, one glyph per cell, top row first.
    #[serde(default)]
    pub rows: Vec<String>,
    /// The sort key shared by every cell (default `0`) — the [`Sprite::layer`] meaning, one layer for
    /// the whole tilemap (a stage is one background plane).
    #[serde(default)]
    pub layer: i16,
}

impl Default for Tilemap {
    fn default() -> Self {
        Tilemap {
            cell_size: 1.0,
            legend: BTreeMap::new(),
            rows: Vec::new(),
            layer: 0,
        }
    }
}

impl Tilemap {
    /// The number of rows in the grid.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The grid width in cells — the glyph count of the first row (`0` if empty). Validation
    /// (`L1019`) guarantees every row has this width, so it is *the* width for a valid tilemap.
    pub fn col_count(&self) -> usize {
        self.rows.first().map_or(0, |row| row.chars().count())
    }

    /// The glyph at cell `(col, row)`, or `None` if `(col, row)` is outside the grid. `col` indexes
    /// glyphs left-to-right, `row` indexes rows top-to-bottom.
    pub fn glyph_at(&self, col: usize, row: usize) -> Option<char> {
        self.rows.get(row)?.chars().nth(col)
    }

    /// The world-space center of cell `(col, row)`, given the grid `origin` (the tilemap entity's
    /// world translation, its `Transform.translation` when unparented — the center of cell `(0, 0)`). +X is column, −Y is row (see the
    /// [placement convention](Tilemap#placement-convention-unambiguous--game-logic-depends-on-it)).
    pub fn cell_to_world(&self, origin: [f32; 3], col: i32, row: i32) -> [f32; 3] {
        [
            origin[0] + col as f32 * self.cell_size,
            origin[1] - row as f32 * self.cell_size,
            origin[2],
        ]
    }

    /// The `(col, row)` of the cell containing world-space `point`, given the grid `origin`. The
    /// exact inverse of [`cell_to_world`](Tilemap::cell_to_world) at a cell center; the result may be
    /// outside the grid (negative or past the edges) — that is honest, callers check the range.
    pub fn world_to_cell(&self, origin: [f32; 3], point: [f32; 3]) -> (i32, i32) {
        let col = ((point[0] - origin[0]) / self.cell_size).round() as i32;
        let row = ((origin[1] - point[1]) / self.cell_size).round() as i32;
        (col, row)
    }
}

impl SemanticValidate for Tilemap {
    /// Cross-field checks JSON Schema cannot express, with JSON Pointers relative to the component
    /// root: legend keys are single glyphs (`L1020`, `/legend/<key>`), each legend entry's atlas
    /// reference holds together (`L1151`, `/legend/<key>/<field>`), its `color` is an HDR colour
    /// (`L1263`, `/legend/<key>/color/<n>`), rows are equal length
    /// (`L1019`, `/rows/<n>`), and every row glyph has a legend entry (`L1020`, `/rows/<n>`). Runs on
    /// a schema-valid value; guards defensively so a schema-invalid one adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(object) = value.as_object() else {
            return out;
        };

        // Legend keys must each be exactly one glyph (`L1020`), and each entry's `texture`/`atlas`/
        // `frame` trio must be a whole reference (`L1151`) — the `Sprite` rules, per glyph.
        let mut glyphs: Vec<char> = Vec::new();
        if let Some(Value::Object(legend)) = object.get("legend") {
            for (key, entry) in legend {
                match single_char(key) {
                    Some(c) => glyphs.push(c),
                    None => out.push(legend_key_not_a_glyph(key)),
                }
                if let Some(fields) = entry.as_object() {
                    let prefix = format!("/legend/{}", pointer_token(key));
                    out.extend(atlas_reference_diagnostics(
                        |field| fields.get(field).is_some_and(|v| !v.is_null()),
                        &prefix,
                    ));
                    for mut diagnostic in value_color_diagnostics("Tilemap legend", entry, "color")
                    {
                        diagnostic.path = diagnostic.path.map(|path| format!("{prefix}{path}"));
                        out.push(diagnostic);
                    }
                }
            }
        }

        let Some(Value::Array(rows)) = object.get("rows") else {
            return out;
        };
        let string_rows: Vec<(usize, &str)> = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.as_str().map(|s| (index, s)))
            .collect();

        // Ragged rows (`L1019`): every row's glyph count must equal the first row's.
        if let Some(&(_, first)) = string_rows.first() {
            let width = first.chars().count();
            for &(index, row) in &string_rows {
                let count = row.chars().count();
                if count != width {
                    out.push(ragged_row(index, width, count));
                }
            }
        }

        // Every row glyph must have a legend entry (`L1020`) — first offending cell per row.
        for &(index, row) in &string_rows {
            for (col, glyph) in row.chars().enumerate() {
                if !glyphs.contains(&glyph) {
                    out.push(glyph_not_in_legend(index, col, glyph));
                    break;
                }
            }
        }
        out
    }
}

/// `L1020` — a legend key is not exactly one glyph.
fn legend_key_not_a_glyph(key: &str) -> Diagnostic {
    Diagnostic::new(
        code::TILEMAP_INVALID_GLYPH,
        format!(
            "legend key `{key}` is not a single glyph ({} characters); a glyph is exactly one \
             Unicode scalar value",
            key.chars().count()
        ),
    )
    .with_path(format!("/legend/{}", pointer_token(key)))
    .with_help("use a one-character legend key, e.g. `#` or `.`")
}

/// `L1019` — a row's glyph count differs from row 0's.
fn ragged_row(index: usize, width: usize, count: usize) -> Diagnostic {
    Diagnostic::new(
        code::TILEMAP_RAGGED_ROWS,
        format!(
            "row {index} has {count} glyphs, but row 0 has {width}; every row must be the same width"
        ),
    )
    .with_path(format!("/rows/{index}"))
    .with_help("make every row string the same glyph count (pad with a legend glyph)")
}

/// `L1020` — a row uses a glyph with no legend entry.
fn glyph_not_in_legend(index: usize, col: usize, glyph: char) -> Diagnostic {
    Diagnostic::new(
        code::TILEMAP_INVALID_GLYPH,
        format!("row {index} uses glyph `{glyph}` at column {col}, which has no legend entry"),
    )
    .with_path(format!("/rows/{index}"))
    .with_help(format!(
        "add `{glyph}` to the tilemap `legend`, or fix the glyph in the row"
    ))
}

/// A grid of mesh cells in the entity's local XZ plane — a [`Tilemap`] for meshes. One entity draws
/// every cell, so a ground of hundreds of tiles is a glyph-grid diff instead of hundreds of entities.
///
/// ```yaml
/// MeshGrid:
///   cell_size: 4.0
///   legend:
///     a: { shape: { asset: meshes/cobble-a.mesh.yaml } }
///     b: { shape: { asset: meshes/cobble-b.mesh.yaml } }
///   rows:            # first row is the far (−Z) row; ' ' is an empty cell
///     - "abba "
///     - "baab "
///   orient:          # optional, same shape as rows; omitted = every cell 0
///     - "0123 "
///     - "4567 "
/// ```
///
/// - `cell_size` — the world-unit pitch of the grid (default `1.0`, strictly positive).
/// - `legend` — a map from a single-glyph string to a [`Mesh`]; every field means what it does on
///   the `Mesh` component, so every static shape works (asset, voxel, sculpt, assembly,
///   primitives); a `character` shape is `L1244`. A legend key that is not one glyph, or is a
///   space, is `L1020`.
/// - `rows` — equal-length strings, one glyph per cell, far row first. A space is the one empty cell
///   (it draws nothing and needs no legend entry). Ragged rows are `L1019`; a glyph with no legend
///   entry is `L1020`.
/// - `orient` — optional strings shaped like `rows`, one orientation glyph per cell (a space is
///   `0`). A shape mismatch or a glyph outside `0`-`7` is `L1243`.
///
/// Every field is `#[serde(default)]`, so `{}` is valid and draws nothing (drift-safe).
///
/// # Placement convention
///
/// Unlike a `Tilemap`, the grid lives in the entity's **local XZ plane** and the entity's whole
/// render pose (`Transform`, `Parent`) applies. Cell `(col, row)`'s local centre is
/// `(col*cell_size, 0, row*cell_size)`: column → +X, row → +Z, so the first row is the far row under
/// the default camera. Cell `(0, 0)` sits at the entity origin. Expansion is row-major.
///
/// # Orientation
///
/// Glyph `0`-`3` is that many quarter-turns about local +Y (exactly `Transform.rotation: [0, 90k,
/// 0]`); `4`-`7` is the turn `k-4` applied after a mirror of local X (exactly `Transform.scale:
/// [-1, 1, 1]` under that rotation). The cell matrix is `T(centre) · R_y(k) · M`.
///
/// Pure render data: no sim system reads it, so it works as a sim or a `presentation: true` entity
/// (recommended for colliderless ground). `render.bounds` reports the grid as one renderable whose
/// AABB is the union of its cells.
///
/// ```
/// use ludion_components::MeshGrid;
///
/// let grid = MeshGrid {
///     rows: vec!["ab ".to_string()],
///     orient: vec!["05 ".to_string()],
///     ..MeshGrid::default()
/// };
/// assert_eq!(grid.glyph_at(1, 0), Some('b'));
/// assert_eq!(grid.glyph_at(2, 0), Some(' ')); // an empty cell
/// assert_eq!(grid.glyph_at(3, 0), None); // out of range
/// assert_eq!(grid.orient_at(1, 0), 5); // mirrored, one quarter-turn
/// assert_eq!(grid.orient_at(2, 0), 0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct MeshGrid {
    /// World-unit pitch of the grid (default `1.0`, strictly positive).
    #[serde(default = "one")]
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub cell_size: f32,
    /// Glyph → mesh. Keys are single glyphs other than a space (`L1020` otherwise).
    #[serde(default)]
    pub legend: BTreeMap<String, Mesh>,
    /// The grid: equal-length strings, one glyph per cell, far (−Z) row first; a space is empty.
    #[serde(default)]
    pub rows: Vec<String>,
    /// Optional orientation per cell, shaped like `rows`: `0`-`3` quarter-turns, `4`-`7` mirrored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orient: Vec<String>,
}

impl Default for MeshGrid {
    fn default() -> Self {
        MeshGrid {
            cell_size: 1.0,
            legend: BTreeMap::new(),
            rows: Vec::new(),
            orient: Vec::new(),
        }
    }
}

impl MeshGrid {
    /// The number of rows in the grid.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The grid width in cells — the glyph count of the first row (`0` if empty).
    pub fn col_count(&self) -> usize {
        self.rows.first().map_or(0, |row| row.chars().count())
    }

    /// The glyph at cell `(col, row)`: `None` outside the grid, `Some(' ')` for an empty cell.
    pub fn glyph_at(&self, col: usize, row: usize) -> Option<char> {
        self.rows.get(row)?.chars().nth(col)
    }

    /// The orientation (`0..=7`) of cell `(col, row)`; `0` when `orient` is empty, the cell is
    /// outside it, or its glyph is a space.
    pub fn orient_at(&self, col: usize, row: usize) -> u8 {
        self.orient
            .get(row)
            .and_then(|line| line.chars().nth(col))
            .and_then(|glyph| glyph.to_digit(8))
            .map_or(0, |digit| digit as u8)
    }
}

impl SemanticValidate for MeshGrid {
    /// Cross-field checks JSON Schema cannot express, with JSON Pointers relative to the component
    /// root: legend entries are not `character` shapes (`L1244`, `/legend/<key>`), legend keys are
    /// single non-space glyphs (`L1020`, `/legend/<key>`), each legend mesh passes the `Mesh`
    /// checks (HDR colours, `L1263`, `/legend/<key>/color/<n>`), rows are equal length (`L1019`,
    /// `/rows/<n>`), every non-space row glyph has a legend entry (`L1020`, `/rows/<n>`), and
    /// `orient` matches `rows` in shape and holds only `0`-`7` or spaces (`L1243`, `/orient` or
    /// `/orient/<n>`). Runs on a schema-valid value; guards defensively.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(object) = value.as_object() else {
            return out;
        };

        let mut glyphs: Vec<char> = Vec::new();
        if let Some(Value::Object(legend)) = object.get("legend") {
            for (key, mesh) in legend {
                if mesh
                    .get("shape")
                    .and_then(Value::as_object)
                    .is_some_and(|shape| shape.contains_key("character"))
                {
                    out.push(
                        Diagnostic::new(
                            code::MESH_GRID_ANIMATED_SHAPE,
                            format!(
                                "legend entry `{key}` is a `character` shape; a grid's cells share \
                                 one entity, so they cannot pose or animate per cell"
                            ),
                        )
                        .with_path(format!("/legend/{}", pointer_token(key)))
                        .with_help("place animated characters as their own entities"),
                    );
                }
                match single_char(key) {
                    Some(' ') => out.push(
                        Diagnostic::new(
                            code::TILEMAP_INVALID_GLYPH,
                            "legend key ` ` is the empty cell; a space can never name a mesh",
                        )
                        .with_path(format!("/legend/{}", pointer_token(key)))
                        .with_help("use another glyph for the mesh; spaces in `rows` stay empty"),
                    ),
                    Some(c) => glyphs.push(c),
                    None => out.push(legend_key_not_a_glyph(key)),
                }
                let prefix = format!("/legend/{}", pointer_token(key));
                for mut diagnostic in Mesh::validate_semantics(mesh) {
                    diagnostic.path = diagnostic.path.map(|path| format!("{prefix}{path}"));
                    out.push(diagnostic);
                }
            }
        }

        let string_rows = string_lines(object.get("rows"));
        let width = string_rows.first().map(|&(_, row)| row.chars().count());
        if let Some(width) = width {
            for &(index, row) in &string_rows {
                let count = row.chars().count();
                if count != width {
                    out.push(ragged_row(index, width, count));
                }
            }
        }
        for &(index, row) in &string_rows {
            if let Some((col, glyph)) = row
                .chars()
                .enumerate()
                .find(|(_, glyph)| *glyph != ' ' && !glyphs.contains(glyph))
            {
                out.push(glyph_not_in_legend(index, col, glyph));
            }
        }

        let orient = string_lines(object.get("orient"));
        if orient.is_empty() {
            return out;
        }
        if orient.len() != string_rows.len() {
            out.push(
                Diagnostic::new(
                    code::MESH_GRID_ORIENT,
                    format!(
                        "`orient` has {} rows, but `rows` has {}; they must be the same shape",
                        orient.len(),
                        string_rows.len()
                    ),
                )
                .with_path("/orient")
                .with_help("give `orient` one string per row of `rows`, or delete it"),
            );
        }
        for &(index, line) in &orient {
            let count = line.chars().count();
            if let Some(width) = width.filter(|&width| width != count) {
                out.push(
                    Diagnostic::new(
                        code::MESH_GRID_ORIENT,
                        format!(
                            "`orient` row {index} has {count} glyphs, but `rows` rows have {width}"
                        ),
                    )
                    .with_path(format!("/orient/{index}"))
                    .with_help("make every `orient` row the same glyph count as the `rows`"),
                );
            }
            if let Some((col, glyph)) = line
                .chars()
                .enumerate()
                .find(|(_, glyph)| *glyph != ' ' && !matches!(glyph, '0'..='7'))
            {
                out.push(
                    Diagnostic::new(
                        code::MESH_GRID_ORIENT,
                        format!(
                            "`orient` row {index} has glyph `{glyph}` at column {col}, which is not an \
                             orientation"
                        ),
                    )
                    .with_path(format!("/orient/{index}"))
                    .with_help(
                        "use `0`-`3` (quarter-turns), `4`-`7` (mirrored quarter-turns) or a space",
                    ),
                );
            }
        }
        out
    }
}

/// The string elements of an optional JSON array, with their indices.
fn string_lines(value: Option<&Value>) -> Vec<(usize, &str)> {
    match value {
        Some(Value::Array(lines)) => lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| line.as_str().map(|s| (index, s)))
            .collect(),
        _ => Vec::new(),
    }
}

use ludion_core::diagnostics::pointer_token;

/// Which of the nine canvas points a screen-space UI element ([`UiRect`], [`UiText`]) pins to — and,
/// equally, which point of the element's own extent sits on it (its own pivot). For example,
/// [`Anchor::BottomRight`] pins the element's bottom-right corner to the canvas's bottom-right
/// corner (a corner HUD needs no size arithmetic); [`Anchor::Center`] centers the element on the
/// canvas center with zero offset. There is no separate `pivot` field — one anchor names both ends.
///
/// **Every variant carries a doc comment.** This is load-bearing, not style: `schemars` only emits
/// the `oneOf`-of-`const` schema shape (which the editor inspector needs to render a dropdown) when
/// every variant of a unit-only enum is doc-commented — an undocumented variant collapses the whole
/// schema to a bare `{"enum": [...]}`, which falls to the raw-JSON textarea instead.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// The canvas's top-left corner; the element's own top-left corner sits there.
    #[default]
    TopLeft,
    /// The canvas's top edge, horizontally centered; the element's own top-center point sits there.
    TopCenter,
    /// The canvas's top-right corner; the element's own top-right corner sits there.
    TopRight,
    /// The canvas's left edge, vertically centered; the element's own center-left point sits there.
    CenterLeft,
    /// The canvas's center; the element's own center sits there.
    Center,
    /// The canvas's right edge, vertically centered; the element's own center-right point sits there.
    CenterRight,
    /// The canvas's bottom-left corner; the element's own bottom-left corner sits there.
    BottomLeft,
    /// The canvas's bottom edge, horizontally centered; the element's own bottom-center point sits
    /// there.
    BottomCenter,
    /// The canvas's bottom-right corner; the element's own bottom-right corner sits there.
    BottomRight,
}

/// A screen-space colored or textured rectangle, anchored to one of the canvas's nine points.
///
/// # Coordinate model
///
/// UI geometry is authored against a virtual **reference canvas 1080 units tall**, width `1080 *
/// aspect` (the render target's aspect ratio) — origin **top-left**, **+Y down** (matching
/// `InputState.mouse.position`, so UI geometry and mouse hit-testing share one orientation). One
/// authored `offset`/`size` therefore fits every output resolution: at render time `scale =
/// target_height / 1080` maps canvas units to pixels uniformly.
///
/// `anchor` (see [`Anchor`]) names both the canvas point this element is placed at and the
/// element's own pivot point; `offset` is measured from that canvas point to that element point, in
/// canvas units. With `texture: None` the element is a flat-colored quad — the [`Sprite::texture`]
/// convention, not a separate rect/sprite pair.
///
/// `fill: true` is the exception to "UI anchors, never stretches": the rect covers the whole canvas
/// at any extent or aspect and `anchor`, `offset` and `size` are ignored.
///
/// An entity may carry a [`Transform`] alongside `UiRect`; it is simply ignored by the UI path —
/// screen space has no world placement.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiRect {
    /// Which canvas point (and element pivot) this rect is placed at (default [`Anchor::TopLeft`]).
    #[serde(default)]
    pub anchor: Anchor,
    /// Offset from the anchor point, in canvas units, **+Y down** (default `[0, 0]`).
    #[serde(default)]
    pub offset: [f32; 2],
    /// Size `[width, height]` in canvas units (default `[100, 100]`).
    #[serde(default = "hundred2")]
    pub size: [f32; 2],
    /// Texture asset path (relative to the content root — the directory of the scene the engine was
    /// started with), or `None` for a plain colored quad (default `None`).
    #[serde(default)]
    #[schemars(with = "Option<ludion_ecs::AssetPathSchema>")]
    pub texture: Option<String>,
    /// A `kind: atlas` document path (relative to the content root), or `None`. The region named by
    /// [`frame`](UiRect::frame) supplies the pixels and the UV sub-rect the rect samples — exactly
    /// the [`Sprite::atlas`] path, so a HUD icon can live in the game's committed atlas rather than a
    /// loose PNG. Mutually exclusive with [`texture`](UiRect::texture): a quad takes its pixels from
    /// one place (`L1151`).
    #[serde(default)]
    #[schemars(with = "Option<ludion_ecs::AssetPathSchema>")]
    pub atlas: Option<String>,
    /// The region name inside [`atlas`](UiRect::atlas). Required with `atlas` and meaningless without
    /// it (`L1151`).
    #[serde(default)]
    pub frame: Option<String>,
    /// Linear RGBA tint multiplied into the sampled texel (default opaque white).
    #[serde(default = "white4")]
    pub color: [f32; 4],
    /// Draw order: higher draws later, i.e. on top (default `0`).
    #[serde(default)]
    pub order: i32,
    /// Whether the rect draws at all (default `true`). `false` is the authored way to hide it — a
    /// boss health bar that exists from the start but must stay off until a boss appears, say —
    /// rather than the alpha-zero or zero-size conventions.
    #[serde(default = "yes")]
    pub visible: bool,
    /// Cover the whole render target (default `false`): the rect resolves to `[0, 0, width, height]`
    /// pixels at any extent or aspect, and `anchor`, `offset` and `size` are ignored (`L1316` warns
    /// when `offset` or `size` is authored non-default). A texture or atlas frame stretches over it
    /// like over a sized rect. The one exception to "UI anchors, never stretches".
    #[serde(default, skip_serializing_if = "is_false")]
    pub fill: bool,
    /// The sub-rect of the texture (or atlas region) the rect samples, `[u0, v0, u1, v1]` in 0..1
    /// with a top-left origin (default none: the whole image). Ignored beside [`slice`](UiRect::slice)
    /// (`L1613`) and needs a `texture` or `atlas`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uv_rect: Option<[f32; 4]>,
    /// Draw the rect as a nine-slice frame (default none): corners keep their size, edges and centre
    /// stretch. Wins over [`uv_rect`](UiRect::uv_rect) and needs a `texture` or `atlas`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<ui::UiSlice>,
}

impl Default for UiRect {
    fn default() -> Self {
        UiRect {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            size: [100.0, 100.0],
            texture: None,
            atlas: None,
            frame: None,
            color: [1.0; 4],
            order: 0,
            visible: true,
            fill: false,
            uv_rect: None,
            slice: None,
        }
    }
}

impl SemanticValidate for UiRect {
    /// Two independent checks: a `size` axis must be a strictly positive, finite number (`L1118`,
    /// pointer `/size/0`, `/size/1`), and the atlas reference must be whole — `frame` without
    /// `atlas`, `atlas` without `frame`, and `texture` alongside `atlas` are each `L1151`, the same
    /// rule a [`Sprite`] enforces (shared `atlas_reference_diagnostics`). Defensive: runs on a
    /// schema-clean value; the all-default rect (untextured, `size: [100, 100]`) adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if let Some(size) = value.get("size").and_then(Value::as_array) {
            for (index, element) in size.iter().enumerate().take(2) {
                if let Some(extent) = as_stored_f32(element) {
                    require_positive_finite_ui_size(extent, &format!("/size/{index}"), &mut out);
                }
            }
        }
        if let Some(object) = value.as_object() {
            out.extend(atlas_reference_diagnostics(
                |field| object.get(field).is_some_and(|v| !v.is_null()),
                "",
            ));
            if object.get("fill").and_then(Value::as_bool) == Some(true) {
                out.extend(ui_rect_fill_ignored(object));
            }
            ui::ui_rect_sampling_diagnostics(object, &mut out);
        }
        out
    }
}

/// `L1316` — a filled `UiRect` authors an `offset` or `size` that differs from the default.
fn ui_rect_fill_ignored(object: &serde_json::Map<String, Value>) -> Vec<Diagnostic> {
    let default = UiRect::default();
    let differs = |field: &str, default: [f32; 2]| {
        object
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|pair| {
                pair.iter()
                    .zip(default)
                    .any(|(v, d)| as_stored_f32(v).is_some_and(|x| x != f64::from(d)))
            })
    };
    [("offset", default.offset), ("size", default.size)]
        .into_iter()
        .filter(|&(field, value)| differs(field, value))
        .map(|(field, _)| {
            Diagnostic::new(
                code::UI_RECT_FILL_IGNORES_PLACEMENT,
                format!("this `UiRect` has `fill: true`, so its `{field}` is ignored"),
            )
            .with_path(format!("/{field}"))
            .with_help(format!(
                "remove `{field}`, or drop `fill: true` to place the rect by its anchor"
            ))
        })
        .collect()
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A run of screen-space text (the embedded 8×8 face, or a baked face named by `font`), anchored to
/// one of the canvas's nine points.
///
/// Shares [`UiRect`]'s coordinate model (a 1080-unit-tall reference canvas, origin top-left, +Y
/// down) and pivot rule (see [`Anchor`]): the anchor names both the canvas point and the element's
/// own pivot, measured over the text block's own extent — width = longest line's advance sum,
/// height = line count × `size` (`size` is the face's line height, as for [`Text`]) — so, for
/// example, a `Center` anchor centers the whole text block for free.
///
/// An entity may carry a [`Transform`] alongside `UiText`; it is ignored — screen space has no world
/// placement.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiText {
    /// Which canvas point (and element pivot) this text is placed at (default [`Anchor::TopLeft`]).
    #[serde(default)]
    pub anchor: Anchor,
    /// Offset from the anchor point, in canvas units, **+Y down** (default `[0, 0]`).
    #[serde(default)]
    pub offset: [f32; 2],
    /// The text to render (default `""` — an empty run draws nothing).
    #[serde(default)]
    pub text: String,
    /// Line height in canvas units (default `24.0`).
    #[serde(default = "twenty_four")]
    pub size: f32,
    /// Linear RGBA text color (default opaque white).
    #[serde(default = "white4")]
    pub color: [f32; 4],
    /// Draw order: higher draws later, i.e. on top (default `0`).
    #[serde(default)]
    pub order: i32,
    /// Greedy word-wrap width in canvas units; `0.0` (the default) means no wrap, and the run is
    /// laid out exactly as authored. Wrapping breaks at spaces and tabs and after `-`/`—`, hard-breaks
    /// a word longer than one line, and treats an authored `'\n'`, `"\r\n"` or `'\r'` as a paragraph
    /// break.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    pub wrap_width: f32,
    /// Whether the text draws at all (default `true`). `false` is the authored way to hide it — the
    /// same convention [`UiRect::visible`] offers.
    #[serde(default = "yes")]
    pub visible: bool,
    /// Logical face name, resolved through the content root's `fonts.yaml`; absent → the table's
    /// `default` entry, else the built-in 8×8 face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
}

impl Default for UiText {
    fn default() -> Self {
        UiText {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            text: String::new(),
            size: 24.0,
            color: [1.0; 4],
            order: 0,
            wrap_width: 0.0,
            visible: true,
            font: None,
        }
    }
}

impl SemanticValidate for UiText {
    /// Rejects a `size` that is not a strictly positive, finite number (`L1118`), pointer `/size`.
    /// Defensive: runs on a schema-clean value; an absent `size` (default `24.0` is valid) adds
    /// nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(size) = value.get("size").and_then(as_stored_f32) else {
            return out;
        };
        require_positive_finite_ui_size(size, "/size", &mut out);
        out
    }
}

/// One choice in a [`UiList`]: a line of text and whether the cursor may land on it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiListItem {
    /// The line's text (default `""`). Always one row; a line break draws as a space.
    #[serde(default)]
    pub text: String,
    /// Whether this item can be selected and activated (default `true`). A disabled item is drawn in
    /// [`UiList::disabled_color`] and skipped by [`UiList::navigate`] — the engine dims and skips, and
    /// knows nothing of *why* (affordability, prerequisites, and every other reason are game logic).
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl Default for UiListItem {
    fn default() -> Self {
        UiListItem {
            text: String::new(),
            enabled: true,
        }
    }
}

/// A vertical stack of selectable text lines with one cursor: the engine's one menu surface.
///
/// Shares [`UiRect`]'s coordinate model (a 1080-unit-tall reference canvas, origin top-left, +Y down)
/// and pivot rule (see [`Anchor`]), measured over the block the list draws: an optional `title` line
/// followed by one line per item, each `size` tall with `line_spacing` between them, as wide as the
/// longest line by the face's advance widths, as [`UiText`] measures. `background` is that block grown
/// by `padding` on all four sides, so the panel is auto-sized to its content and changing `padding`
/// never moves the text.
///
/// **An empty `items` vec draws nothing at all** — background and title included — exactly as an
/// empty [`UiText`] string draws nothing. `visible: false` is the authored way to hide a
/// non-empty list outright; an empty `items` still draws nothing either way.
///
/// `selected` is the cursor and is ordinary hashed state: it survives frames, rides a snapshot and a
/// scene switch, and shows in a golden. A value past the end is **clamped when drawn**, never a
/// diagnostic — a game that shrinks a list mid-frame must not be able to author an error.
/// [`navigate`](UiList::navigate) is how a game moves it; activation is that call's return value, not
/// a field.
///
/// **Write only on change.** `items` is variable-length data inside a hashed component, so a menu
/// rebuilt from fresh `String`s every frame churns the world hash for no reason. Build the items when
/// the menu's *content* changes and leave them alone otherwise.
///
/// An entity may carry a [`Transform`] alongside `UiList`; it is ignored — screen space has no world
/// placement.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct UiList {
    /// Which canvas point (and block pivot) this list is placed at (default [`Anchor::TopLeft`]).
    #[serde(default)]
    pub anchor: Anchor,
    /// Offset from the anchor point, in canvas units, **+Y down** (default `[0, 0]`).
    #[serde(default)]
    pub offset: [f32; 2],
    /// A heading drawn above the items and never selectable (default `""` — no title line at all).
    /// Always one row; a line break draws as a space.
    #[serde(default)]
    pub title: String,
    /// The choices, top to bottom (default empty — an empty list draws nothing).
    #[serde(default)]
    pub items: Vec<UiListItem>,
    /// The cursor: the index into `items` drawn in `selected_color` (default `0`). Clamped to the
    /// item count when drawn.
    #[serde(default)]
    pub selected: u32,
    /// Glyph cell height in canvas units (default `24.0`).
    #[serde(default = "twenty_four")]
    pub size: f32,
    /// Extra canvas units between one line and the next — a gap, not a line height, so `0.0` stacks
    /// the lines exactly like a multi-line [`UiText`] (default `0.0`).
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    pub line_spacing: f32,
    /// How far the background extends beyond the measured block, in canvas units, on every side
    /// (default `16.0`).
    #[serde(default = "sixteen")]
    #[schemars(range(min = 0.0))]
    pub padding: f32,
    /// Linear RGBA for an ordinary item's text, and for the title (default opaque white).
    #[serde(default = "white4")]
    pub color: [f32; 4],
    /// Linear RGBA for the item under the cursor (default a warm highlight).
    #[serde(default = "ui_list_selected_color_default")]
    pub selected_color: [f32; 4],
    /// Linear RGBA for an `enabled: false` item; takes precedence over `selected_color`, since an
    /// authored `selected` can point at a disabled item and it must still read as disabled (default a
    /// dimmed grey).
    #[serde(default = "ui_list_disabled_color_default")]
    pub disabled_color: [f32; 4],
    /// Linear RGBA of the auto-sized panel behind the block (default fully transparent). **Alpha `0`
    /// draws no panel at all** — the alpha convention every UI component already uses.
    #[serde(default)]
    pub background: [f32; 4],
    /// Draw order: higher draws later, i.e. on top (default `0`). The background and every line of
    /// one list share this key, and the background is emitted first, so the panel is always behind its
    /// own text.
    #[serde(default)]
    pub order: i32,
    /// How many items may be drawn at once; `0` (the default) is unlimited. Above the limit the
    /// drawn window follows `selected`, keeping the cursor centred and clamping at both ends — it is
    /// derived from the cursor every frame, never stored, so it cannot desynchronize from it.
    #[serde(default)]
    pub max_visible: u32,
    /// Whether the list draws at all (default `true`). `false` is the authored way to hide it —
    /// the same convention [`UiRect::visible`] offers, orthogonal to an empty `items`.
    #[serde(default = "yes")]
    pub visible: bool,
    /// Logical face name, resolved through the content root's `fonts.yaml`; absent → the table's
    /// `default` entry, else the built-in 8×8 face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
}

impl Default for UiList {
    fn default() -> Self {
        UiList {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            title: String::new(),
            items: Vec::new(),
            selected: 0,
            size: 24.0,
            line_spacing: 0.0,
            padding: 16.0,
            color: [1.0; 4],
            selected_color: ui_list_selected_color_default(),
            disabled_color: ui_list_disabled_color_default(),
            background: [0.0; 4],
            order: 0,
            max_visible: 0,
            visible: true,
            font: None,
        }
    }
}

impl SemanticValidate for UiList {
    /// Rejects a `size` that is not a strictly positive, finite number (`L1118`), pointer `/size` —
    /// the same glyph-cell height [`UiText`] validates, so it shares the code. `line_spacing` and
    /// `padding` carry their lower bound in the schema, so a bad value there is the ordinary `L1014`
    /// at the field. Defensive: runs on a schema-clean value; an absent `size` (default `24.0` is
    /// valid) adds nothing.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(size) = value.get("size").and_then(as_stored_f32) else {
            return out;
        };
        require_positive_finite_ui_size(size, "/size", &mut out);
        out
    }
}

/// Pushes an `L1118` when `value` is not a strictly positive, finite number (zero, negative, or
/// non-finite) — shared by `UiRect.size` (checked per axis), `UiText.size`, `UiList.size` and
/// `Text.size`.
fn require_positive_finite_ui_size(value: f64, pointer: &str, out: &mut Vec<Diagnostic>) {
    if !ludion_core::number::positive_f32(value) {
        out.push(ui_element_non_positive_size(value, pointer));
    }
}

/// `L1118` — a UI element's or `Text`'s size is not strictly positive and finite.
fn ui_element_non_positive_size(value: f64, pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::UI_ELEMENT_NON_POSITIVE_SIZE,
        format!(
            "size {value} is not valid; a `UiRect` size axis, a `UiText` size, a `UiList` size or \
             a `Text` size must be a strictly positive, finite number as an f32 (at least ~1.4e-45)"
        ),
    )
    .with_path(pointer.to_string())
    .with_help("give the UI element a size greater than 0")
}

/// Which clip this entity is playing, and since when (`Plan/voxel-animation.md` decisions 6 and 8).
///
/// One component, resolved against whatever the entity renders: on a [`Mesh`] whose `shape` is an
/// assembly or a character, `clip` names an entry of that assembly's `clips:` map and poses its parts; on a
/// [`Sprite`] drawn from an atlas, it names one of that atlas's `flipbooks:` and chooses the region
/// the quad shows. An entity carrying both reads the same name on each. On the **root part of a
/// prefab instance** it names an entry of the prefab's `clips:` map and poses every part of the
/// instance the clip has a track for, each relative to its authored rest, so the instance's placement
/// and overrides ride on top; a part with no track keeps its exact stored pose.
///
/// **The pose is visual-only.** It is a pure function of `(clip, sim time)` evaluated at
/// `extract()`; no system writes it back, and the entity keeps its one `Transform` and one
/// collision footprint however its limbs move. Pausing freezes the pose for free, and a replay
/// reproduces it because the only inputs are this component and the tick count. Once a game
/// registers `animation_events`, clip timing is sim input (`ludion docs character-animation`).
///
/// **Clip selection is game code.** There is no state machine: a movement system writes
/// `clip: "walk"`, a combat system writes `clip: "attack"`, and a transition is a hard cut unless
/// the game calls [`crossfade`], which blends the outgoing clip into the new one over `fade`
/// seconds (rotation slerps, translation and scale lerp; a sprite flipbook always hard-cuts). Write
/// the component only when the *choice* changes — re-stamping `started` every frame restarts the
/// clip every frame (and churns the world hash for a `String` that did not change).
///
/// The component is ordinary hashed sim state: it snapshots, rides a scene switch, and shows in a
/// `WorldHash`. That is why `started` is a [`FrameIndex`] and not accumulated seconds — an exact
/// integer, with elapsed clip seconds derived on demand as `(now − started) × seconds_per_tick ×
/// speed`. `set_clip_speed` changes the rate mid-clip by writing an [`anchor`](Animation::anchor): the
/// clip ticks consumed at the frame it was called, after which the new `speed` covers the ticks
/// since.
///
/// `hold` steps the pose cadence ("on twos"): the pose changes only every `hold` ticks.
///
/// On an entity that binds nothing under `clip` — a `Mesh.shape` that is not an assembly or a character, an
/// assembly or atlas with no such clip, a `Sprite` drawn from a loose `texture:` — this component is
/// inert: the model renders at its rest pose and the sprite shows its authored `frame`. An unknown
/// clip name is a silent degrade, never a diagnostic, because a clip is chosen at runtime by game
/// logic that no document-time check can see.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Animation {
    /// The clip's short name, as bound in the assembly's, character's or prefab's `clips:` map or the
    /// atlas's `flipbooks:` (default `""` — no clip, so the rest pose or the authored frame).
    #[serde(default)]
    pub clip: String,
    /// The frame playback started at; clip time is measured from it (default frame `0`). A frame in
    /// the future reads as the clip's first instant.
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// Playback rate multiplier: `2.0` plays twice as fast, `0.0` freezes on the first instant
    /// (default `1.0`). Written directly it re-times the whole clip from `started`; use
    /// `set_clip_speed` to change it mid-clip without a jump.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    pub speed: f32,
    /// Where `set_clip_speed` re-based the clip clock (default none). It applies only while its
    /// `started` equals this component's `started`, so a restamp or a crossfade retires it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ClipAnchor>,
    /// The outgoing clip of a crossfade (default `""` — none, or fading from the rest pose). Set by
    /// [`crossfade`]; omitted from the saved text at its default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    /// The frame the outgoing clip started at (default frame `0`).
    #[serde(default = "frame_zero", skip_serializing_if = "is_frame_zero")]
    #[schemars(with = "u64")]
    pub from_started: FrameIndex,
    /// The outgoing clip's playback rate (default `1.0`).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    #[schemars(range(min = 0.0))]
    pub from_speed: f32,
    /// The outgoing clip's clock anchor, carried from `anchor` by [`crossfade`] (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_anchor: Option<ClipAnchor>,
    /// The frame the crossfade began at (default frame `0`).
    #[serde(default = "frame_zero", skip_serializing_if = "is_frame_zero")]
    #[schemars(with = "u64")]
    pub fade_started: FrameIndex,
    /// Crossfade length in seconds; `0.0` (default) is a hard cut. Once `fade_started + fade` has
    /// passed the pose is the plain incoming clip and `from` is ignored.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(range(min = 0.0))]
    pub fade: f32,
    /// Sim ticks each pose is shown (default `1`, every tick; `0` is refused, `L1317`): `2` is 30
    /// poses a second at 60 Hz, `5` is 12. The grid is global — every entity with the same `hold`
    /// changes pose on the same ticks — and covers the incoming clock, the outgoing clock and the
    /// crossfade weight. Visual-only: events and the ragdoll pose keep the exact tick. A `0` written
    /// from Rust reads as `1`.
    #[serde(default = "one_u32", skip_serializing_if = "is_one_u32")]
    pub hold: u32,
    /// The blend-space parameters (default `[0, 0]`; omitted from the saved text at its default).
    /// When `clip` names a blend of the character, `params[0]` drives a 1D blend and
    /// `params[0..2]` a 2D one (`ludion docs animation-layers`); otherwise inert. Write it every
    /// tick the value changes: the weights follow it at once and the phase keeps its place.
    #[serde(default, skip_serializing_if = "is_zero_params")]
    pub params: [f32; 2],
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            clip: String::new(),
            started: FrameIndex::ZERO,
            speed: 1.0,
            anchor: None,
            from: String::new(),
            from_started: FrameIndex::ZERO,
            from_speed: 1.0,
            from_anchor: None,
            fade_started: FrameIndex::ZERO,
            fade: 0.0,
            hold: 1,
            params: [0.0; 2],
        }
    }
}

/// Where a `set_clip_speed` re-based the clip clock: at gameplay `frame` the playback stamped `started`
/// had consumed `ticks` clip ticks (before any character binding speed).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct ClipAnchor {
    /// The `started` of the playback this anchor belongs to.
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// The gameplay frame the clock was re-based at.
    #[schemars(with = "u64")]
    pub frame: FrameIndex,
    /// The clip ticks consumed at `frame`: each played tick counts its speed, so the clip time is
    /// `ticks` × seconds-per-tick (before any character binding speed).
    #[schemars(range(min = 0.0))]
    pub ticks: f64,
    /// The clip ticks the pose showed at the last grid tick before `frame`, under the speed before the
    /// change. Written only when `Animation.hold > 1` and `frame` is off the grid; default is none.
    /// Extract reads it for a held tick before `frame`, so a speed change between two steps leaves the
    /// pose alone until the next step. (A `hold` changed mid-period leaves it on the old grid until the
    /// next step.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_ticks: Option<f64>,
}

/// Switches `animation` to `clip` with a crossfade of `seconds` from what it was playing: the
/// current clip becomes the outgoing side, `started` becomes `now` (the gameplay frame), and
/// `speed` is kept. A `seconds` that is not a positive finite number is a hard cut and clears the
/// fade fields, so the component serializes exactly like a plain `clip`/`started` write.
///
/// Interrupting a fade in progress drops the older outgoing clip, which pops.
///
/// ```
/// use ludion_core::time::FrameIndex;
/// use ludion_components::{Animation, crossfade};
///
/// let mut animation = Animation { clip: "walk".into(), ..Animation::default() };
/// crossfade(&mut animation, "run", 0.25, FrameIndex(120));
/// assert_eq!((animation.clip.as_str(), animation.from.as_str()), ("run", "walk"));
/// assert_eq!(animation.fade, 0.25);
/// ```
pub fn crossfade(animation: &mut Animation, clip: &str, seconds: f32, now: FrameIndex) {
    if seconds > 0.0 && seconds.is_finite() {
        animation.from = std::mem::take(&mut animation.clip);
        animation.from_started = animation.started;
        animation.from_speed = animation.speed;
        animation.from_anchor = animation.anchor.filter(|a| a.started == animation.started);
        animation.fade_started = now;
        animation.fade = seconds;
    } else {
        animation.from = String::new();
        animation.from_started = FrameIndex::ZERO;
        animation.from_speed = 1.0;
        animation.from_anchor = None;
        animation.fade_started = FrameIndex::ZERO;
        animation.fade = 0.0;
    }
    animation.anchor = None;
    animation.clip = clip.to_string();
    animation.started = now;
}

fn is_frame_zero(frame: &FrameIndex) -> bool {
    frame.as_u64() == 0
}

#[allow(clippy::float_cmp)]
fn is_one(value: &f32) -> bool {
    *value == 1.0
}

#[allow(clippy::float_cmp)]
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

fn is_zero_params(value: &[f32; 2]) -> bool {
    is_zero(&value[0]) && is_zero(&value[1])
}

/// Drives a part entity's spawn pose from one rest joint of a skinned mesh: `skin` names the skinned
/// entity ([`Mesh`] `shape: asset` naming a mesh document with `joints:`), `bone` one of its joint
/// names. At every world build (`scene.load`, a district switch) a part carrying `SkinBone` and no
/// authored [`Transform`] of its own is spawned at `skin`'s world transform times `bone`'s rest model
/// transform (`crate::skin_bone::spawn_poses`); an authored `Transform` wins outright.
///
/// ```yaml
/// components:
///   SkinBone:
///     skin: { index: 0, generation: 0 }
///     bone: upper_arm_l
/// ```
///
/// Four load-time/check-time diagnostics refuse a bad link: the named entity not a skinned mesh
/// (`L1194`), `bone` naming no joint of it (`L1195`), two parts naming the same `(skin, bone)`
/// (`L1196`), and `skin` itself carrying an [`Animation`] — one pose source per skin (`L1197`).
///
/// **The part is the bone's pose source, every frame.** `extract` sets the named joint's
/// bone-palette entry from the part's live world transform each frame it draws
/// (`crate::extract::posed_palette`); a joint no part names keeps its rest pose relative to its
/// parent. Render-only: the part's own `Transform` is ordinary hashed sim state either way, nothing
/// is written back from the pose, and a stale link (an unknown skin, an unknown bone) degrades
/// silently to rest rather than raising a runtime error — only load-time/check-time catches it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SkinBone {
    /// The skinned mesh entity this part's bone belongs to.
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    pub skin: EntityId,
    /// The joint's name, as authored in the skin mesh document's `joints:`.
    pub bone: String,
}

/// Attaches an entity to one joint of its [`Parent`]'s skeleton: the parent must
/// be a skinned mesh ([`Mesh`] `shape: asset` naming a mesh document with `joints:`, or `shape:
/// character`, which resolves to one — the rule [`SkinBone`] uses). The entity's [`Transform`] is
/// local to the joint's model transform; sim reads compose the joint's rest pose into the chain.
///
/// ```yaml
/// components:
///   Parent: { index: 0, generation: 0 }
///   ParentJoint:
///     joint: hand_r
/// ```
///
/// Two load-time/check-time/prefab-spawn diagnostics refuse a bad link: the parent not a skinned
/// mesh, or no `Parent` at all (`L1318`), and `joint` naming no joint of it (`L1319`, the help names
/// the nearest). A runtime miss (a parent that lost its mesh, an unknown joint) composes as a plain
/// `Parent`, silently. Like any component it is hashed, snapshotted and saved on a sim entity.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ParentJoint {
    /// The joint's name, as authored in the parent mesh document's `joints:`.
    pub joint: String,
}

/// A looping sound placed in the world: audible within `range` of the listener, louder the nearer it
/// is. The sound analog of [`PointLight`] — position comes from the entity's [`Transform`], the
/// falloff is the same `clamp(1 − distance/range, 0, 1)` shape, and a scene may place any number.
///
/// **No system to register.** The engine syncs every emitter at the frame boundary, beside the audio
/// drain: it measures the distance from the listener (the lowest-`EntityId` entity carrying a
/// [`Camera`]; ground plane unless the game sets `audio_distance`), and keeps the loop at
/// `gain × falloff` under the emitter's own
/// entity identity. An emitter that goes inaudible — out of range, muted, despawned, or with the
/// component removed — is stopped on the tick it drops out. A scene with no camera syncs nothing that
/// tick.
///
/// Playback is out-of-sim, so this component is only ever *read* by the engine: it is ordinary hashed
/// sim state a game may write, but what the emitter sounds like never feeds back into a tick, and a
/// world with no `AudioEmitter` produces exactly the audio stream it did before the component existed.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AudioEmitter {
    /// The sound to loop, as a path relative to the content root (default `""`, which names no sound:
    /// `check-scene` refuses an empty `sound` with `L1101`, and at runtime the sink skips it with its
    /// usual once-per-path warning).
    #[serde(default)]
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub sound: String,
    /// Linear gain at the listener's own position, before falloff (default `1.0`). The mix is
    /// soft-limited above 0.8; headroom keeps dynamics.
    #[serde(default = "one")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub gain: f32,
    /// Distance at which the emitter falls silent (default `10.0`). Strictly positive and
    /// finite — zero, negative, or non-finite is `L1136`.
    #[serde(default = "ten")]
    pub range: f32,
    /// Which mixdown bus the loop belongs to (default `ambient` — an emitter is a sustained bed, so it
    /// follows the manual loop primitive rather than [`AudioCategory`]'s one-shot default).
    #[serde(default = "ambient_category")]
    #[schemars(schema_with = "audio_category_schema")]
    pub category: AudioCategory,
}

/// `AudioCategory`'s schema, hand-written because it lives in `ludion-core`, which has no `schemars`
/// dependency (the [`Parent`](ludion_ecs::Parent)/`EntityId` treatment). It must keep matching that
/// enum's serde shape — the snake_case variant name as a bare string — and the `oneOf`-of-`const`
/// form is what the editor inspector renders as a dropdown (see [`Anchor`]).
fn audio_category_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "description": "Which mixdown bus a sound belongs to.",
        "oneOf": [
            { "const": "sfx", "description": "A game sound effect." },
            { "const": "ambient", "description": "A sustained ambient bed: a drone, wind, a tolling bell." },
            { "const": "voice", "description": "Spoken dialogue and barks." },
            { "const": "ui", "description": "Interface sounds: clicks, confirmations, menu music stings." }
        ]
    })
}

impl Default for AudioEmitter {
    fn default() -> Self {
        AudioEmitter {
            sound: String::new(),
            gain: 1.0,
            range: 10.0,
            category: AudioCategory::Ambient,
        }
    }
}

impl SemanticValidate for AudioEmitter {
    /// Rejects a `range` that is zero, negative, or non-finite (`L1136`), pointer `/range` — the
    /// `Collider` extent case (`L1116`): JSON Schema's `range` keyword is inclusive, so it cannot
    /// express "strictly greater than zero". `gain` carries its own schemars lower bound, so a
    /// negative gain is the ordinary `L1014` at the field.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if let Some(range) = value.get("range").and_then(as_stored_f32)
            && !ludion_core::number::positive_f32(range)
        {
            out.push(audio_emitter_non_positive_range(range, "/range"));
        }
        out
    }
}

impl SemanticValidate for Animation {
    /// Rejects `hold: 0` (`L1317`), pointer `/hold`. The schema cannot say it: a schemars lower
    /// bound would surface on the wire as the generic `L1014` before this runs.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if value.get("hold").and_then(Value::as_u64) == Some(0) {
            out.push(animation_hold_zero("/hold"));
        }
        out
    }
}

/// `L1317` — an `Animation`'s `hold` is zero ticks.
fn animation_hold_zero(pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::ANIMATION_HOLD_ZERO,
        "Animation hold 0 is not valid; it is how many sim ticks each pose is shown, at least 1",
    )
    .with_path(pointer.to_string())
    .with_help("write a whole number of at least 1, or remove hold to pose every tick")
}

/// `L1136` — an `AudioEmitter`'s audible radius is not strictly positive and finite.
fn audio_emitter_non_positive_range(value: f64, pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::AUDIO_EMITTER_NON_POSITIVE_RANGE,
        format!(
            "AudioEmitter range {value} is not valid; the distance at which the sound falls silent \
             must be a strictly positive, finite number as an f32 (at least ~1.4e-45)"
        ),
    )
    .with_path(pointer.to_string())
    .with_help("give the emitter a range greater than 0, or set gain: 0.0 to silence it")
}

/// The volume a [`ParticleEmitter`] releases its particles inside, in the emitter's local frame.
///
/// **Every variant carries a doc comment** for [`Anchor`]'s reason: it is what makes `schemars` emit
/// the `oneOf`-of-`const` shape the editor inspector renders as a dropdown.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmitShape {
    /// Every particle is released from the entity's own origin; `extent` is unused.
    #[default]
    Point,
    /// Uniformly through the volume of a sphere of radius `extent.x`.
    Sphere,
    /// Uniformly through an axis-aligned box of half-extents `extent`.
    Box,
    /// Uniformly across a disc of radius `extent.x` lying in the local XZ plane.
    Disc,
}

/// How a particle's colour reaches the frame buffer.
///
/// Doc comments on both variants, for [`EmitShape`]'s reason.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParticleBlend {
    /// Ordinary alpha blending: the particle's alpha is its coverage, so a stack of them settles
    /// toward the topmost colour. Smoke, dust, debris.
    #[default]
    Alpha,
    /// Additive: the particle only ever brightens what is behind it, and a stack of them blows out.
    /// Sparks, embers, glows.
    Additive,
}

/// The most live particles one emitter may hold at once.
///
/// The live set is `rate × life[1] + burst` particles, and *all* of it is iterated on every
/// extracted frame — there is no pool to amortize the cost against. The cap is therefore a real
/// budget rather than a formality: a mis-typed `rate` must be an `L2107` at check time, never a
/// frozen machine at render time.
pub const PARTICLE_MAX_LIVE_PER_EMITTER: u32 = 4096;

/// A visual effect described by numbers rather than by entities: a plume, a burst, a spill of
/// sparks (`Plan/particles-engine-and-editor.md` decisions 1–4).
///
/// **Particles are a pure function of `(this component, sim time)`.** `extract()` computes the live
/// set analytically every frame — particle *i* of the continuous stream is released at
/// `started + i / rate`, a burst's *j* at `started`, and each one's position, size and colour follow
/// in closed form from its age. Nothing is spawned, pooled, or stepped, so a particle is never a
/// `WorldHash` term, never a replay record and never an inspector row; a paused engine renders the
/// same frame twice, and a restored snapshot renders what the recorded session rendered.
///
/// **Particles live in the emitter's frame.** Every offset is anchored at the entity's *current*
/// [`Transform`] translation, so a moving emitter carries its plume with it; `Transform.rotation`
/// orients the emission cone (whose axis is the emitter's local **+Y**) while `gravity` stays world
/// −Y; and the uniform `Transform.scale.x` multiplies the spawn extent, the speeds and both sizes —
/// the [`Sprite`]/[`GroundMarker`] "authored × entity scale" convention. A trail left behind by a
/// moving thing is therefore one short burst emitter spawned where the thing was, not a `space:`
/// option this design does not have.
///
/// **An endless emitter is in steady state at every instant, including frame 0.** With
/// `duration: 0` the live window admits negative indices — the emitter has "always been running" —
/// so an authored ambient plume is full in a gallery golden and in the editor's first frame with no
/// warm-up. `started` therefore matters only to bursts and to finite emitters: game code stamps it
/// when the *choice* happens, and re-stamping it every frame restarts the effect every frame
/// ([`Animation`]'s own warning).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ParticleEmitter {
    /// The volume particles are released inside (default `point`).
    #[serde(default)]
    pub shape: EmitShape,
    /// The spawn volume's size, read per [`EmitShape`] (default `[0.5, 0.5, 0.5]`): a sphere's
    /// radius and a disc's radius are `extent.x`, a box's half-extents are all three. Multiplied by
    /// `Transform.scale.x`. Every component must be finite and non-negative (`L2107`).
    #[serde(default = "particle_extent_default")]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub extent: [f32; 3],
    /// Particles released per second, continuously (default `40.0`). `0.0` is a burst-only emitter.
    /// Non-negative (`L1014`; non-finite `L2107`).
    #[serde(default = "particle_rate_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 200.0]))]
    pub rate: f32,
    /// How many particles are released all at once at `started` (default `0`). Independent of
    /// `rate`: an emitter may do both.
    #[serde(default)]
    #[schemars(extend("x-ui-range" = [0, 512]))]
    pub burst: u32,
    /// The frame the effect starts at (default frame `0`), the [`Animation::started`] convention.
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// Seconds of continuous emission after `started` (default `0.0` — endless, and therefore in
    /// steady state at every instant). Non-negative (`L1014`; non-finite `L2107`).
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 10.0]))]
    pub duration: f32,
    /// Particle lifetime in seconds as `[min, max]` (default `[0.8, 1.4]`), drawn uniformly. Both
    /// finite and non-negative, `min` no greater than `max` (`L2107`).
    #[serde(default = "particle_life_default")]
    #[schemars(extend("x-ui-range" = [0.0, 5.0]))]
    pub life: [f32; 2],
    /// Initial speed in units per second as `[min, max]` (default `[1.0, 2.5]`), drawn uniformly and
    /// multiplied by `Transform.scale.x`. Same validity rules as `life` (`L2107`).
    #[serde(default = "particle_speed_default")]
    #[schemars(extend("x-ui-range" = [0.0, 20.0]))]
    pub speed: [f32; 2],
    /// Half-angle of the emission cone about local +Y, in degrees (default `30.0`): `0` is a beam,
    /// `90` a hemisphere, `180` the whole sphere.
    #[serde(default = "particle_spread_default")]
    #[schemars(range(min = 0.0, max = 180.0))]
    #[schemars(extend("x-ui-range" = [0.0, 180.0]))]
    pub spread_deg: f32,
    /// World −Y acceleration in units per second squared (default `0.0`). A negative value lifts,
    /// which is how smoke rises.
    #[serde(default)]
    #[schemars(extend("x-ui-range" = [-20.0, 20.0]))]
    pub gravity: f32,
    /// Linear drag in inverse seconds (default `0.0` — none). Speed decays as `e^(−drag × age)`
    /// toward the terminal velocity `gravity / drag`. Non-negative (`L1014`; non-finite `L2107`).
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 10.0]))]
    pub drag: f32,
    /// World height (Y) below which a particle is simply not drawn (default `-f32::MAX` — no floor),
    /// the [`Fade::height_min`] shape. Cheaper and steadier than shortening `life` to keep debris
    /// off the ground: the particle still exists, it is just clipped.
    #[serde(default = "neg_f32_max")]
    #[schemars(extend("x-ui-range" = [-4.0, 4.0]))]
    pub floor: f32,
    /// Disc diameter in world units at age zero (default `0.25`), lerped to `end_size` over the
    /// particle's life and multiplied by `Transform.scale.x`. Non-negative (`L1014`; non-finite `L2107`).
    #[serde(default = "particle_start_size_default")]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub start_size: f32,
    /// Disc diameter at the end of the particle's life (default `0.0` — it shrinks away).
    /// Non-negative (`L1014`; non-finite `L2107`).
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-ui-range" = [0.0, 4.0]))]
    pub end_size: f32,
    /// Linear RGBA at age zero (default opaque white), lerped to `end_color` over the particle's
    /// life. HDR-capable: a value above `1.0` rides the bloom chain like an emissive mesh.
    #[serde(default = "white4")]
    pub start_color: [f32; 4],
    /// Linear RGBA at the end of the particle's life (default transparent white — it fades out).
    #[serde(default = "particle_end_color_default")]
    pub end_color: [f32; 4],
    /// How the disc is composited (default `alpha`).
    #[serde(default)]
    pub blend: ParticleBlend,
    /// Edge falloff of the analytic disc (default `0.5`): `0` is a hard anti-aliased circle, `1` a
    /// gaussian smudge.
    #[serde(default = "particle_softness_default")]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[schemars(extend("x-ui-range" = [0.0, 1.0]))]
    pub softness: f32,
    /// Lights the particles (default `false`: the authored colour is the look). `true`: a
    /// particle's colour is its albedo times the light reaching its centre, with the lit sprite's
    /// terms (ambient, point, spot, sun); shadow maps are ignored. Omitted from the text when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lit: bool,
    /// Names this emitter's random stream (default `0`). Two emitters sharing a seed draw the same
    /// lives, speeds and directions — which is a feature for a rank of torches and a mistake for a
    /// pair of impacts meant to look different.
    #[serde(default)]
    pub seed: u64,
    /// Scale in `[0, 1]` on the medium's optical depth along the view ray to this emitter's particles (default none,
    /// meaning `1.0`, today's look): `0` shows no air between the eye and the particles. Render-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub medium: Option<f32>,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        ParticleEmitter {
            shape: EmitShape::Point,
            extent: [0.5, 0.5, 0.5],
            rate: 40.0,
            burst: 0,
            started: FrameIndex::ZERO,
            duration: 0.0,
            life: [0.8, 1.4],
            speed: [1.0, 2.5],
            spread_deg: 30.0,
            gravity: 0.0,
            drag: 0.0,
            floor: -f32::MAX,
            start_size: 0.25,
            end_size: 0.0,
            start_color: [1.0, 1.0, 1.0, 1.0],
            end_color: [1.0, 1.0, 1.0, 0.0],
            blend: ParticleBlend::Alpha,
            softness: 0.5,
            lit: false,
            seed: 0,
            medium: None,
        }
    }
}

impl SemanticValidate for ParticleEmitter {
    /// Rejects exactly what the schema cannot: array *elements* that are negative or non-finite (the
    /// [`Environment`] case — schemars puts a `range` on a field, not on a member), a bad
    /// `start_color`/`end_color` (`L1263`: RGB finite and non-negative, alpha `>= 0`), an inverted
    /// `[min, max]` pair, non-finite scalars whose lower bound the schema does express (a `range`
    /// keyword cannot say "finite"), and a live population above
    /// [`PARTICLE_MAX_LIVE_PER_EMITTER`].
    ///
    /// `spread_deg` and `softness` carry closed schemars bounds, so an out-of-range value there is
    /// the ordinary `L1014` at the field and never reaches here.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();

        for (field, arity) in [("extent", 3), ("life", 2), ("speed", 2)] {
            for index in 0..arity {
                if let Some(component) = value.get(field).and_then(|v| v.get(index)) {
                    check_particle_dimension(
                        field,
                        as_stored_f32(component),
                        &format!("/{field}/{index}"),
                        &mut out,
                    );
                }
            }
        }
        for field in ["start_color", "end_color"] {
            out.extend(value_color_diagnostics_with(
                "ParticleEmitter",
                value,
                field,
                AlphaRule::Clamped,
            ));
        }
        for field in ["rate", "duration", "drag", "start_size", "end_size"] {
            check_particle_dimension(
                field,
                value.get(field).and_then(as_stored_f32),
                &format!("/{field}"),
                &mut out,
            );
        }
        if let Some(gravity) = value.get("gravity").and_then(as_stored_f32)
            && !gravity.is_finite()
        {
            out.push(particle_emitter_invalid(
                format!("ParticleEmitter gravity {gravity} is not a finite number"),
                "/gravity",
                "give gravity a finite acceleration (0.0 for none)",
            ));
        }

        for field in ["life", "speed"] {
            let pair = (
                value
                    .get(field)
                    .and_then(|v| v.get(0))
                    .and_then(as_stored_f32),
                value
                    .get(field)
                    .and_then(|v| v.get(1))
                    .and_then(as_stored_f32),
            );
            if let (Some(min), Some(max)) = pair
                && min.is_finite()
                && max.is_finite()
                && min > max
            {
                out.push(particle_emitter_invalid(
                    format!(
                        "ParticleEmitter {field} range [{min}, {max}] is inverted; a `[min, max]` \
                         pair is drawn from uniformly, so `min` must not exceed `max`"
                    ),
                    &format!("/{field}"),
                    "swap the two values, or widen the range",
                ));
            }
        }

        // The population the evaluator would iterate every frame. Read defensively — a document may
        // omit any field — so the check fires on the authored numbers rather than on nothing.
        let rate = value
            .get("rate")
            .and_then(as_stored_f32)
            .unwrap_or(f64::from(ParticleEmitter::default().rate));
        let life_max = value
            .get("life")
            .and_then(|v| v.get(1))
            .and_then(as_stored_f32)
            .unwrap_or(f64::from(ParticleEmitter::default().life[1]));
        let burst = value.get("burst").and_then(Value::as_f64).unwrap_or(0.0);
        let live = rate.max(0.0) * life_max.max(0.0) + burst.max(0.0);
        if live.is_finite() && live > f64::from(PARTICLE_MAX_LIVE_PER_EMITTER) {
            out.push(particle_emitter_invalid(
                format!(
                    "ParticleEmitter holds up to {live:.0} live particles (rate {rate} × life \
                     {life_max} s + burst {burst}), above the per-emitter cap of \
                     {PARTICLE_MAX_LIVE_PER_EMITTER}"
                ),
                "/rate",
                "lower `rate`, shorten `life`, or shrink `burst` until the live set fits the cap",
            ));
        }

        out
    }
}

/// Pushes an `L2107` if `value` is present and is not a finite, non-negative number.
fn check_particle_dimension(
    field: &str,
    value: Option<f64>,
    pointer: &str,
    out: &mut Vec<Diagnostic>,
) {
    if let Some(value) = value
        && (!value.is_finite() || value < 0.0)
    {
        out.push(particle_emitter_invalid(
            format!(
                "ParticleEmitter `{field}` {value} is not valid; it is a length, a duration or a \
                 rate, so it must be a finite, non-negative number"
            ),
            pointer,
            "give the field a value of 0 or more",
        ));
    }
}

/// `L2107` — a `ParticleEmitter` authors an effect the analytic evaluator cannot turn into
/// particles. One constructor for every form, because the code's four families share a shape: a
/// message naming the field and the value, a pointer at it, and the fix.
fn particle_emitter_invalid(message: String, pointer: &str, help: &str) -> Diagnostic {
    Diagnostic::new(code::PARTICLE_EMITTER_INVALID, message)
        .with_path(pointer.to_string())
        .with_help(help.to_string())
}

fn particle_extent_default() -> [f32; 3] {
    [0.5, 0.5, 0.5]
}
fn particle_rate_default() -> f32 {
    40.0
}
fn particle_life_default() -> [f32; 2] {
    [0.8, 1.4]
}
fn particle_speed_default() -> [f32; 2] {
    [1.0, 2.5]
}
fn particle_spread_default() -> f32 {
    30.0
}
fn particle_start_size_default() -> f32 {
    0.25
}
fn particle_end_color_default() -> [f32; 4] {
    [1.0, 1.0, 1.0, 0.0]
}
fn particle_softness_default() -> f32 {
    0.5
}

fn ambient_category() -> AudioCategory {
    AudioCategory::Ambient
}

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

fn one_u32() -> u32 {
    1
}

fn is_one_u32(value: &u32) -> bool {
    *value == 1
}

fn one() -> f32 {
    1.0
}
fn ten() -> f32 {
    10.0
}
fn near_default() -> f32 {
    0.1
}
fn far_default() -> f32 {
    1000.0
}
fn forty() -> f32 {
    40.0
}
fn two_hundred() -> f32 {
    200.0
}
fn shadow_bias_default() -> f32 {
    0.05
}
fn shadow_distance_default() -> f32 {
    100.0
}
#[allow(clippy::float_cmp)]
fn is_shadow_distance_default(value: &f32) -> bool {
    *value == shadow_distance_default()
}
fn local_shadow_bias_default() -> f32 {
    0.02
}
fn local_shadow_near_default() -> f32 {
    0.05
}
fn probe_blend() -> f32 {
    0.5
}
fn probe_extent() -> [f32; 3] {
    [2.0; 3]
}
fn probe_radius() -> f32 {
    2.0
}
fn spot_direction() -> [f32; 3] {
    [0.0, -1.0, 0.0]
}
fn spot_inner_angle() -> f32 {
    20.0
}
fn spot_outer_angle() -> f32 {
    30.0
}
fn sun_direction() -> [f32; 3] {
    [-0.3, -1.0, -0.2]
}
fn neg_f32_max() -> f32 {
    -f32::MAX
}
fn sky_zenith_default() -> [f32; 3] {
    [0.02, 0.03, 0.06]
}
fn sky_horizon_default() -> [f32; 3] {
    [0.05, 0.06, 0.10]
}
fn sky_ground_default() -> [f32; 3] {
    [0.01, 0.01, 0.02]
}
fn ambient_default() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}
fn bloom_strength_default() -> f32 {
    0.1
}
fn exposure_default() -> f32 {
    1.3
}
fn yes() -> bool {
    true
}
fn is_true(value: &bool) -> bool {
    *value
}
fn fog_color_default() -> [f32; 3] {
    [0.5, 0.5, 0.5]
}
fn fog_anisotropy_default() -> f32 {
    0.7
}
fn vignette_radius_default() -> f32 {
    0.5
}
fn gi_cell_size_default() -> f32 {
    0.5
}
fn gi_probe_spacing_default() -> f32 {
    1.0
}
fn gi_normal_bias_default() -> f32 {
    0.25
}
fn gi_sky_visibility_default() -> f32 {
    1.0
}
fn ssao_radius_default() -> f32 {
    0.5
}
fn ssao_bias_default() -> f32 {
    0.02
}
fn focus_distance_default() -> f32 {
    10.0
}
fn grain_size_default() -> f32 {
    0.002
}
fn ssr_fade_default() -> [f32; 2] {
    [0.45, 0.65]
}
fn ssr_max_distance_default() -> f32 {
    30.0
}
fn ssr_thickness_default() -> f32 {
    0.5
}
#[allow(clippy::float_cmp)]
fn is_ssr_fade_default(value: &[f32; 2]) -> bool {
    *value == ssr_fade_default()
}
#[allow(clippy::float_cmp)]
fn is_local_shadow_bias_default(value: &f32) -> bool {
    *value == local_shadow_bias_default()
}
#[allow(clippy::float_cmp)]
fn is_local_shadow_near_default(value: &f32) -> bool {
    *value == local_shadow_near_default()
}
#[allow(clippy::float_cmp)]
fn is_ssr_max_distance_default(value: &f32) -> bool {
    *value == ssr_max_distance_default()
}
#[allow(clippy::float_cmp)]
fn is_ssr_thickness_default(value: &f32) -> bool {
    *value == ssr_thickness_default()
}
fn one3() -> [f32; 3] {
    [1.0; 3]
}
fn white3() -> [f32; 3] {
    [1.0; 3]
}
fn white4() -> [f32; 4] {
    [1.0; 4]
}
fn one2() -> [f32; 2] {
    [1.0; 2]
}
fn hundred2() -> [f32; 2] {
    [100.0; 2]
}
fn twenty_four() -> f32 {
    24.0
}
fn sixteen() -> f32 {
    16.0
}
fn ui_list_selected_color_default() -> [f32; 4] {
    [1.0, 0.9, 0.4, 1.0]
}
fn ui_list_disabled_color_default() -> [f32; 4] {
    [0.4, 0.4, 0.4, 1.0]
}

/// The engine's built-in component registry, in canonical registration order.
///
/// This is the single source of truth `check-scene`, `sim-hash`, `render`, and `dump-graph` share
/// (the CLI calls this rather than defining its own). Order is load-bearing: it fixes the canonical
/// component key order for scene files and the world hash.
///
/// **Registration is append-only.** A new component goes at the very end, after every existing
/// entry, so no prior component's registration index shifts and every already-written scene and
/// recorded replay keeps its world hash byte-identical (an entity that does not carry the new type
/// is unaffected). Reordering or inserting would force a re-bless of every hash in the repo; the
/// order is pinned by `components_are_registered_in_a_pinned_append_only_order`.
///
/// # Example
/// ```
/// let registry = ludion_components::builtin_registry();
/// assert!(registry.get("Transform").is_some());
/// assert!(registry.get("Camera").is_some());
/// ```
pub fn builtin_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<Transform>("Transform").expect("unique");
    // `Sprite` carries a semantic validator (the `texture`/`atlas`/`frame` reference rules).
    registry
        .register_validated::<Sprite>("Sprite")
        .expect("unique");
    // `Mesh`, `PointLight` and `Text` carry a semantic validator (HDR colours, `L1263`).
    registry.register_validated::<Mesh>("Mesh").expect("unique");
    registry
        .register_validated::<PointLight>("PointLight")
        .expect("unique");
    registry
        .register_validated::<Camera>("Camera")
        .expect("unique");
    registry.register_validated::<Text>("Text").expect("unique");
    registry.register::<Parent>("Parent").expect("unique");
    // Reserved: authored through the entity `name:` field, never as a `components:` block.
    registry.register_reserved::<Name>("Name").expect("unique");
    // `Tilemap` carries a semantic validator (ragged rows / unknown glyphs).
    registry
        .register_validated::<Tilemap>("Tilemap")
        .expect("unique");
    // `Collider` (from `ludion-spatial`) carries a semantic validator (positive, finite extents).
    registry
        .register_validated::<Collider>("Collider")
        .expect("unique");
    // `UiRect`/`UiText` (screen-space UI primitives) carry a semantic validator (positive, finite
    // size).
    registry
        .register_validated::<UiRect>("UiRect")
        .expect("unique");
    registry
        .register_validated::<UiText>("UiText")
        .expect("unique");
    // `DirectionalLight` (the sun) carries a semantic validator (non-zero, finite direction; a
    // shadow depth range with near below far; an HDR colour).
    registry
        .register_validated::<DirectionalLight>("DirectionalLight")
        .expect("unique");
    // `Environment` (the opt-in sky/lighting-environment singleton) carries a semantic validator
    // (color components finite and non-negative — the array-element invariant schemars `range`
    // cannot reach); `Fade` (per-instance screen-door dither) validates entirely through its schemars
    // `range`, so it needs no semantic validator.
    registry
        .register_validated::<Environment>("Environment")
        .expect("unique");
    registry.register::<Fade>("Fade").expect("unique");
    // `PostProcess` (the opt-in imaging singleton) carries a semantic validator for exactly the
    // invariant its schema cannot express: the `gain`/`gamma` array elements (`Environment`'s case
    // again). Every scalar knob rides its own schemars `range`, and a non-finite value is
    // unrepresentable in both authoring paths.
    registry
        .register_validated::<PostProcess>("PostProcess")
        .expect("unique");
    // `GiVolume` (the opt-in GI-volume singleton) needs no semantic validator: its one scalar's
    // strictly-positive bound is an `exclusiveMinimum` in its schema (the `fog_anisotropy` case), so
    // a bad value is the ordinary `L1014` at the field, and a non-finite one is unrepresentable in
    // both authoring paths.
    registry.register::<GiVolume>("GiVolume").expect("unique");
    // `GiDynamic` (the per-entity dynamic-for-GI marker) needs no semantic validator: it has no
    // fields, so there is no value to be wrong, and an authored extra field is the ordinary
    // `L1014` at the empty object's own schema.
    registry.register::<GiDynamic>("GiDynamic").expect("unique");
    // `Residue` (the per-entity stays-behind marker) needs no semantic validator, for `GiDynamic`'s
    // reason exactly: no fields, so no value to be wrong, and an authored extra key is the ordinary
    // `L1014` at the empty object's own schema.
    registry.register::<Residue>("Residue").expect("unique");
    // `GroundMarker` (the ground-anchored telegraph) carries a semantic validator: its dimensions
    // live inside an enum variant, where a bound in the schema is reported as a failed `oneOf`
    // branch rather than at the offending field. `L2106` names the dimension and points at it.
    registry
        .register_validated::<GroundMarker>("GroundMarker")
        .expect("unique");
    // `UiList` (the selectable menu) carries a semantic validator for the one invariant its schema
    // cannot express, the `UiText` case exactly: a strictly positive, finite glyph `size` (`L1118`).
    // `line_spacing`/`padding` ride their own schemars lower bound, so a bad value there is the
    // ordinary `L1014` at the field.
    registry
        .register_validated::<UiList>("UiList")
        .expect("unique");
    // `Animation` (which assembly clip an entity plays) carries a semantic validator for one
    // invariant: `hold` is at least one tick (`L1317`). `hold` has no schemars lower bound on
    // purpose — the wire runs the schema check first, so an `L1014` would hide the specific code.
    // `clip` is a free-form short name resolved at render time (an unknown one is a documented
    // silent degrade, not an error), and `speed`'s lower bound is a schemars `range`.
    registry
        .register_validated::<Animation>("Animation")
        .expect("unique");
    // `AudioEmitter` (the positional looping sound) carries a semantic validator for the one
    // invariant its schema cannot express, the `Collider` case exactly: a strictly positive, finite
    // `range` (`L1136`). `gain` rides its own schemars lower bound, and `sound`/`category` cannot be
    // wrong in a way a document-time check can see.
    registry
        .register_validated::<AudioEmitter>("AudioEmitter")
        .expect("unique");
    // `ParticleEmitter` (the analytic visual effect) carries a semantic validator for the three
    // things its schema cannot express, the `Environment`/`GroundMarker` cases together: array
    // *elements* that must be finite and non-negative, an inverted `[min, max]` pair, and the live
    // population cap (`L2107`). `spread_deg` and `softness` ride closed schemars bounds, so a bad
    // value there is the ordinary `L1014` at the field.
    registry
        .register_validated::<ParticleEmitter>("ParticleEmitter")
        .expect("unique");
    // `Volley` (one fired, closed-form projectile volley) carries a semantic validator for the two
    // things its schema cannot express: a `count` past the kill bitset's width, and a non-finite
    // angle or speed — either of which would poison every bullet's position with a NaN (`L2108`).
    registry
        .register_validated::<Volley>("Volley")
        .expect("unique");
    // `PatternPlayer` (which pattern document an entity plays) needs no semantic validator: its
    // `pattern` is a doc key resolved at runtime — an unloaded one is the `L2108` degrade
    // `play_patterns` reports, not a document-time error — and `target` is an ordinary entity id.
    registry
        .register::<PatternPlayer>("PatternPlayer")
        .expect("unique");
    // `Velocity` (from `ludion-spatial`) needs no semantic validator: every field is an unconstrained
    // `f32` triple, so there is no invariant beyond what the schema already expresses.
    registry.register::<Velocity>("Velocity").expect("unique");
    // `Joint` (from `ludion-spatial`) carries a semantic validator: `a`/`b` equality, a non-finite or
    // zero-length axis, an inverted or non-finite limit, and a non-positive `break_impulse` (`L1183`)
    // — none of which JSON Schema can express. Its `a`/`b` entity references are additionally
    // structurally checked by `ludion-scene` (from the field's schema marker), the `Parent` case generalized.
    registry
        .register_validated::<Joint>("Joint")
        .expect("unique");
    // `JointMotor` (from `ludion-spatial`) carries a semantic validator: a `target` that is not 1 or 3
    // finite numbers and a negative or non-finite `stiffness`/`damping`/`max_force` (`L1188`); whether
    // its arity fits the carrier's `Joint` is the runtime `L3037`.
    registry
        .register_validated::<JointMotor>("JointMotor")
        .expect("unique");
    // `SkinBone` (a part that poses one joint of a skin, every frame) needs no semantic validator:
    // every check it needs — the skin entity's existence, its mesh document, the bone name — needs
    // the mesh document on disk, which a schema or `SemanticValidate` pass (document-value-only)
    // cannot read; `crate::skin_bone::scene_skin_bone_diagnostics` runs those checks with
    // filesystem access, at `scene.load` and under `check-scene`. Registered last: the
    // append-only order is pinned (`components_are_registered_in_a_pinned_append_only_order`), so
    // a new component always joins at the end regardless of which existing one it is thematically
    // closest to.
    registry.register::<SkinBone>("SkinBone").expect("unique");
    // `PathFollower` (an entity riding a `kind: path` document) validates `speed` and `phase` as
    // finite `f32`s (`L1269`); its `path` is a content-relative file read at runtime — an
    // unloadable one is the `L1215` degrade `path_follow` reports, not a document-time error.
    // Registered last, per the append-only order above.
    registry
        .register_validated::<PathFollower>("PathFollower")
        .expect("unique");
    // `PrefabInstance` is provenance `spawn_prefab` writes, never authored: its `root` is the one
    // entity reference, remapped on a scene load or restore like `Joint.a`. Registered last.
    registry
        .register::<PrefabInstance>("PrefabInstance")
        .expect("unique");
    // `MeshGrid` carries a semantic validator (ragged rows / unknown glyphs / orient shape).
    // Registered last, per the append-only order above.
    registry
        .register_validated::<MeshGrid>("MeshGrid")
        .expect("unique");
    // `SpotLight` carries a semantic validator (HDR colour, cone shape `L1272`). Registered last,
    // per the append-only order above.
    registry
        .register_validated::<SpotLight>("SpotLight")
        .expect("unique");
    // `ReflectionProbe` carries a semantic validator (volume bounds `L1274`). Registered last, per
    // the append-only order above.
    registry
        .register_validated::<ReflectionProbe>("ReflectionProbe")
        .expect("unique");
    // The character motor's authored component and its engine-written state (`ludion-physics`),
    // appended after `ReflectionProbe` per the append-only order above.
    registry
        .register_validated::<ludion_physics::CharacterMotor>("CharacterMotor")
        .expect("unique");
    registry
        .register::<ludion_physics::CharacterMotorState>("CharacterMotorState")
        .expect("unique");
    // The belt velocity a grounded motor is carried by, appended after the motor pair.
    registry
        .register_validated::<ludion_physics::SurfaceVelocity>("SurfaceVelocity")
        .expect("unique");
    // The lane a motor is held to, appended after the belt velocity.
    registry
        .register_validated::<LaneConstraint>("LaneConstraint")
        .expect("unique");
    // The camera rig, appended after the lane.
    registry
        .register_validated::<ludion_camera::CameraRig>("CameraRig")
        .expect("unique");
    // The projected drop shadow, appended after the rig; validated for its cross-field distance
    // rule and the shape dimensions inside the enum.
    registry
        .register_validated::<DropShadow>("DropShadow")
        .expect("unique");
    // The projected decal, appended after the drop shadow; validated for its box size, HDR tint
    // and angle fade.
    registry
        .register_validated::<Decal>("Decal")
        .expect("unique");
    // `ParentJoint` needs no semantic validator, like `SkinBone`: its checks (the parent's mesh
    // document, the joint name) need the document on disk, so
    // `crate::parent_joint::scene_parent_joint_diagnostics` runs them at load and under
    // `check-scene`. Registered last, per the append-only order above.
    registry
        .register::<ParentJoint>("ParentJoint")
        .expect("unique");
    // `Visibility` needs no semantic validator: two bools have no invalid value.
    registry
        .register::<Visibility>("Visibility")
        .expect("unique");
    // The presentation camera offset, appended after `Visibility`; validated for non-finite fields.
    registry
        .register_validated::<CameraOffset>("CameraOffset")
        .expect("unique");
    // The first-person viewmodel flag and its optional lens, appended after `CameraOffset`. The
    // flag has no invalid value; the lens is validated for out-of-range fields.
    registry.register::<Viewmodel>("Viewmodel").expect("unique");
    registry
        .register_validated::<ViewmodelLens>("ViewmodelLens")
        .expect("unique");
    // The 3D broadphase bound, appended last so no existing scene or hash moves.
    registry
        .register_validated::<SpatialVolume>("SpatialVolume")
        .expect("unique");
    // The navigation agent and its steering output, appended after `SpatialVolume`.
    registry
        .register_validated::<NavAgent>("NavAgent")
        .expect("unique");
    registry
        .register::<NavSteering>("NavSteering")
        .expect("unique");
    // The pose-blend pair, appended after `NavSteering`: per-part named drive poses (validated like
    // `JointMotor`) and the group root's active pose and weight, both read only by `pose_blend`.
    registry
        .register_validated::<JointPoses>("JointPoses")
        .expect("unique");
    registry.register::<PoseBlend>("PoseBlend").expect("unique");
    // The authored does-not-move marker, appended after `PoseBlend`.
    registry.register::<Static>("Static").expect("unique");
    // The routed-agent pair, appended after `Static`: the goal a game writes and the route `nav_steer` plans.
    registry
        .register_validated::<NavGoal>("NavGoal")
        .expect("unique");
    registry
        .register_validated::<NavRoute>("NavRoute")
        .expect("unique");
    // Query-only layer names, appended after `NavRoute`: read by physics queries, never the solver.
    registry
        .register_validated::<ludion_physics::QueryLayers>("QueryLayers")
        .expect("unique");
    // The HUD motion pair, appended after `QueryLayers`: closed-form tracks and a world-target pin,
    // both resolved by `extract` onto a screen-space element.
    registry
        .register_validated::<UiTween>("UiTween")
        .expect("unique");
    registry
        .register_validated::<UiWorldAnchor>("UiWorldAnchor")
        .expect("unique");
    // The patrol pair, appended after `UiWorldAnchor`: the route a guard walks and the state `patrol_update` keeps.
    registry
        .register_validated::<PatrolFollower>("PatrolFollower")
        .expect("unique");
    registry
        .register_validated::<PatrolState>("PatrolState")
        .expect("unique");
    // The sight pair, appended after `PatrolState`: who looks and what can be seen, read by `perception_update`.
    registry
        .register_validated::<Perceiver>("Perceiver")
        .expect("unique");
    registry
        .register_validated::<SightTarget>("SightTarget")
        .expect("unique");
    // The light-exposure subject, appended after `SightTarget`: read by `light_exposure_update`.
    registry
        .register_validated::<LightExposure>("LightExposure")
        .expect("unique");
    // The memory record, appended after `LightExposure`: one entity per remembered stimulus, written by `perception_update`.
    registry
        .register_validated::<StimulusRecord>("StimulusRecord")
        .expect("unique");
    // The camera zone, appended after `StimulusRecord`: a sensor volume that swaps the rig's config;
    // validated through its embedded `RigConfig`.
    registry
        .register_validated::<ludion_camera::CameraZone>("CameraZone")
        .expect("unique");
    // The animation layer stack, appended last: presentation-only, composed by `extract`.
    registry
        .register_validated::<AnimationLayers>("AnimationLayers")
        .expect("unique");
    // The surface tag, appended after `AnimationLayers`: read by physics hits, never the solver.
    registry
        .register_validated::<ludion_physics::SurfaceTag>("SurfaceTag")
        .expect("unique");
    // The audio listener, appended after `SurfaceTag`: where positional sound is measured from, read by the engine's audio path.
    registry
        .register::<AudioListener>("AudioListener")
        .expect("unique");
    // The surface overlay, appended after `AudioListener`: a sensor volume that overrides the surface a hit reports.
    registry
        .register_validated::<ludion_physics::SurfaceOverlay>("SurfaceOverlay")
        .expect("unique");
    // The hearing trio, appended after `SurfaceOverlay`: a noise-scaling volume, a queued noise and
    // the noise counter, read by `perception_update`.
    registry
        .register_validated::<NoiseZone>("NoiseZone")
        .expect("unique");
    registry
        .register::<PendingNoise>("PendingNoise")
        .expect("unique");
    registry
        .register::<PerceptionState>("PerceptionState")
        .expect("unique");
    // The blend-space phase, appended after `SurfaceOverlay`: written by `blend_clocks`, read by `animation_events` and `extract`.
    registry
        .register::<BlendClock>("BlendClock")
        .expect("unique");
    // Read only by a sim system: physics, path_follow, camera_rig, pose_blend.
    for name in SIM_ONLY_COMPONENTS {
        registry.mark_sim_only(name).expect("registered above");
    }
    registry
}

/// The built-ins whose engine-owned consumer is a sim-phase system.
const SIM_ONLY_COMPONENTS: [&str; 30] = [
    "Collider",
    "Velocity",
    "Joint",
    "JointMotor",
    "CharacterMotor",
    "CharacterMotorState",
    "SurfaceVelocity",
    "LaneConstraint",
    "PathFollower",
    "CameraRig",
    "SpatialVolume",
    "NavAgent",
    "NavSteering",
    "JointPoses",
    "PoseBlend",
    "NavGoal",
    "NavRoute",
    "QueryLayers",
    "PatrolFollower",
    "PatrolState",
    "Perceiver",
    "SightTarget",
    "LightExposure",
    "StimulusRecord",
    "SurfaceTag",
    "SurfaceOverlay",
    "NoiseZone",
    "PendingNoise",
    "PerceptionState",
    "BlendClock",
];

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_ecs::World;

    /// Exactly these built-ins are sim-only; a guard that the list above grows on purpose.
    #[test]
    fn the_sim_only_set_is_pinned() {
        let registry = builtin_registry();
        let mut marked: Vec<&str> = registry
            .iter()
            .filter(|info| info.sim_only())
            .map(|info| info.name())
            .collect();
        marked.sort_unstable();
        let mut expected = [
            "Collider",
            "Velocity",
            "Joint",
            "JointMotor",
            "CharacterMotor",
            "CharacterMotorState",
            "SurfaceVelocity",
            "LaneConstraint",
            "PathFollower",
            "CameraRig",
            "SpatialVolume",
            "NavAgent",
            "NavSteering",
            "JointPoses",
            "PoseBlend",
            "NavGoal",
            "NavRoute",
            "QueryLayers",
            "PatrolFollower",
            "PatrolState",
            "Perceiver",
            "SightTarget",
            "LightExposure",
            "StimulusRecord",
            "SurfaceTag",
            "SurfaceOverlay",
            "NoiseZone",
            "PendingNoise",
            "PerceptionState",
            "BlendClock",
        ];
        expected.sort_unstable();
        assert_eq!(marked, expected);
    }

    /// A default `Animation` keeps its three-key text; the crossfade fields are omitted at rest.
    #[test]
    fn animation_default_serializes_three_keys() {
        let json = serde_json::to_value(Animation::default()).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["clip", "speed", "started"]);
        assert!(!keys.contains(&"anchor") && !keys.contains(&"from_anchor"));
    }

    #[test]
    fn animation_crossfade_fields_round_trip() {
        let anchor = ClipAnchor {
            started: FrameIndex(30),
            frame: FrameIndex(50),
            ticks: 15.0,
            grid_ticks: None,
        };
        let animation = Animation {
            clip: "run".into(),
            started: FrameIndex(30),
            speed: 1.5,
            anchor: Some(anchor),
            from: "walk".into(),
            from_started: FrameIndex(10),
            from_speed: 0.5,
            from_anchor: Some(ClipAnchor {
                started: FrameIndex(10),
                ..anchor
            }),
            fade_started: FrameIndex(30),
            fade: 0.25,
            hold: 3,
            params: [2.5, -1.0],
        };
        let json = serde_json::to_value(&animation).unwrap();
        assert_eq!(
            serde_json::from_value::<Animation>(json).unwrap(),
            animation
        );
    }

    /// `params` is omitted at its default, so a default `Animation` keeps its old text.
    #[test]
    fn animation_params_are_omitted_at_zero() {
        let json = serde_json::to_value(Animation::default()).unwrap();
        assert!(json.get("params").is_none());
        let set = Animation {
            params: [0.0, 3.0],
            ..Animation::default()
        };
        assert_eq!(
            serde_json::to_value(&set).unwrap()["params"],
            serde_json::json!([0.0, 3.0])
        );
    }

    #[test]
    fn crossfade_of_zero_seconds_is_a_plain_write() {
        let mut animation = Animation {
            clip: "walk".into(),
            ..Animation::default()
        };
        crossfade(&mut animation, "run", 0.0, FrameIndex(7));
        let plain = Animation {
            clip: "run".into(),
            started: FrameIndex(7),
            ..Animation::default()
        };
        assert_eq!(animation, plain);
    }

    /// A primitive `shape` serializes as a bare string and round-trips.
    #[test]
    fn mesh_primitive_shape_round_trips() {
        let mesh = Mesh {
            shape: MeshSource::Plane,
            color: [0.5, 0.5, 0.5, 1.0],
            ..Mesh::default()
        };
        let json = serde_json::to_value(&mesh).unwrap();
        assert_eq!(json["shape"], serde_json::json!("plane"));
        assert_eq!(serde_json::from_value::<Mesh>(json).unwrap(), mesh);
    }

    /// An `asset` shape serializes as a one-key map (serde external tagging) and round-trips.
    #[test]
    fn mesh_asset_shape_round_trips() {
        let mesh = Mesh {
            shape: MeshSource::Asset("meshes/rock.mesh.yaml".to_string()),
            ..Mesh::default()
        };
        let json = serde_json::to_value(&mesh).unwrap();
        assert_eq!(
            json["shape"],
            serde_json::json!({ "asset": "meshes/rock.mesh.yaml" })
        );
        assert_eq!(serde_json::from_value::<Mesh>(json).unwrap(), mesh);
    }

    /// A `voxel` shape serializes as a one-key map (serde external tagging) beside `asset`, and
    /// round-trips — the Phase 10 addition-by-extension of `MeshSource`.
    #[test]
    fn mesh_voxel_shape_round_trips() {
        let mesh = Mesh {
            shape: MeshSource::Voxel("voxels/lantern.voxel.yaml".to_string()),
            ..Mesh::default()
        };
        let json = serde_json::to_value(&mesh).unwrap();
        assert_eq!(
            json["shape"],
            serde_json::json!({ "voxel": "voxels/lantern.voxel.yaml" })
        );
        assert_eq!(serde_json::from_value::<Mesh>(json).unwrap(), mesh);
    }

    /// An `assembly` shape serializes as a one-key map (serde external tagging) beside `asset` and
    /// `voxel`, and round-trips.
    #[test]
    fn mesh_assembly_shape_round_trips() {
        let mesh = Mesh {
            shape: MeshSource::Assembly("models/player.assembly.yaml".to_string()),
            ..Mesh::default()
        };
        let json = serde_json::to_value(&mesh).unwrap();
        assert_eq!(
            json["shape"],
            serde_json::json!({ "assembly": "models/player.assembly.yaml" })
        );
        assert_eq!(serde_json::from_value::<Mesh>(json).unwrap(), mesh);
    }

    /// A `sculpt` shape serializes as a one-key map (serde external tagging) beside the other
    /// document-backed sources, and round-trips.
    #[test]
    fn mesh_sculpt_shape_round_trips() {
        let mesh = Mesh {
            shape: MeshSource::Sculpt("sculpts/idol.sculpt.yaml".to_string()),
            ..Mesh::default()
        };
        let json = serde_json::to_value(&mesh).unwrap();
        assert_eq!(
            json["shape"],
            serde_json::json!({ "sculpt": "sculpts/idol.sculpt.yaml" })
        );
        assert_eq!(serde_json::from_value::<Mesh>(json).unwrap(), mesh);
    }

    /// A `sculpt` shape validates against the registry schema through the real scene checker.
    #[test]
    fn sculpt_shape_validates_in_a_scene() {
        let registry = builtin_registry();
        let sculpt = "format: 1\nentities:\n  - components:\n      Mesh:\n        shape:\n          \
                      sculpt: \"sculpts/idol.sculpt.yaml\"\n";
        ludion_scene::check(sculpt, &registry).expect("sculpt shape validates");
    }

    /// An `assembly` shape validates against the registry schema through the real scene checker —
    /// the registry picks the new variant up automatically (no schema hand-editing).
    #[test]
    fn assembly_shape_validates_in_a_scene() {
        let registry = builtin_registry();
        let assembly = "format: 1\nentities:\n  - components:\n      Mesh:\n        shape:\n          \
                        assembly: \"models/player.assembly.yaml\"\n";
        ludion_scene::check(assembly, &registry).expect("assembly shape validates");
    }

    /// A `voxel` shape validates against the registry schema through the real scene checker — the
    /// registry picks the new variant up automatically (no schema hand-editing).
    #[test]
    fn voxel_shape_validates_in_a_scene() {
        let registry = builtin_registry();
        let voxel = "format: 1\nentities:\n  - components:\n      Mesh:\n        shape:\n          \
                     voxel: \"voxels/lantern.voxel.yaml\"\n";
        ludion_scene::check(voxel, &registry).expect("voxel shape validates");
    }

    /// An empty `Mesh: {}` deserializes to the default (cube) — the drift-safe path the scene format
    /// relies on (`shape` and `color` both `#[serde(default)]`).
    #[test]
    fn empty_mesh_is_the_default() {
        let mesh: Mesh = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(mesh, Mesh::default());
        assert_eq!(mesh.shape, MeshSource::Cube);
    }

    /// Both authoring forms validate against the registry schema through the real scene checker.
    #[test]
    fn both_shape_forms_validate_in_a_scene() {
        let registry = builtin_registry();
        let primitive =
            "format: 1\nentities:\n  - components:\n      Mesh:\n        shape: sphere\n";
        ludion_scene::check(primitive, &registry).expect("primitive shape validates");

        let asset = "format: 1\nentities:\n  - components:\n      Mesh:\n        shape:\n          \
                     asset: \"meshes/rock.mesh.yaml\"\n";
        ludion_scene::check(asset, &registry).expect("asset shape validates");
    }

    /// A `shape` that is neither a known primitive string nor an `{asset: …}` map is an `L1014`
    /// schema mismatch — the enum is closed.
    #[test]
    fn unknown_shape_is_a_schema_error() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Mesh:\n        shape: pyramid\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(diagnostics.iter().any(|d| d.code.as_str() == "L1014"));
    }

    /// `roughness`/`metallic` out of `[0, 1]` and a negative `emissive` are each an `L1014` schema
    /// range violation, pointing at the exact field — proves the `#[schemars(range(...))]`
    /// annotations actually reach the scene validator (`check-scene`'s schema, not just docs).
    #[test]
    fn out_of_range_material_fields_are_schema_errors() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Mesh:\n        roughness: 1.5\n        \
                    metallic: -0.1\n        emissive: -1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        for field in ["roughness", "metallic", "emissive"] {
            assert!(
                diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                    && d.path.as_deref() == Some(&format!("/entities/0/components/Mesh/{field}"))),
                "expected an L1014 for Mesh/{field}: {diagnostics:?}"
            );
        }
    }

    /// `x-ui-range` (the field's *useful* travel) rides alongside `minimum`/`maximum` (the field's
    /// *legal* bounds) rather than replacing them: the schemars field extension is a post-mutator on
    /// the field's own subschema, so a field can carry both.
    #[test]
    fn x_ui_range_coexists_with_the_validated_range() {
        let schema = serde_json::to_value(schemars::schema_for!(PostProcess)).unwrap();
        let halation = &schema["properties"]["halation"];
        assert_eq!(halation["x-ui-range"], serde_json::json!([0.0, 0.5]));
        assert_eq!(halation["minimum"], serde_json::json!(0.0));

        // On a fixed numeric array the keyword lands on the array field, NOT on `items` — the
        // per-element contract every client reads it under. Pinned because the placement is
        // schemars' choice, not ours, and a move would silently strand every vector slider.
        let lift = &schema["properties"]["lift"];
        assert_eq!(lift["x-ui-range"], serde_json::json!([-0.5, 0.5]));
        assert!(lift["items"].get("x-ui-range").is_none());

        // Travel NARROWER than legal, which is not redundancy: `0.25` is where the gather's disc
        // stops being smooth, `0.15` is where it stops looking good. Dropping either loses a fact.
        let dof = &schema["properties"]["dof"];
        assert_eq!(dof["x-ui-range"], serde_json::json!([0.0, 0.15]));
        assert_eq!(dof["maximum"], serde_json::json!(0.25));

        // A colour triple states no travel at all: three narrow tracks are a worse control than the
        // colour picker that belongs there, and the absence is what keeps clients from drawing one.
        assert!(
            schema["properties"]["fog_color"]
                .get("x-ui-range")
                .is_none()
        );

        let gi_schema = serde_json::to_value(schemars::schema_for!(GiVolume)).unwrap();
        let normal_bias = &gi_schema["properties"]["normal_bias"];
        assert_eq!(normal_bias["x-ui-range"], serde_json::json!([0.0, 2.0]));
        assert_eq!(normal_bias["minimum"], serde_json::json!(0.0));

        let sun_schema = serde_json::to_value(schemars::schema_for!(DirectionalLight)).unwrap();
        let sun_intensity = &sun_schema["properties"]["intensity"];
        assert_eq!(sun_intensity["x-ui-range"], serde_json::json!([0.0, 4.0]));

        let shadow_bias = &sun_schema["properties"]["shadow_bias"];
        assert_eq!(shadow_bias["x-ui-range"], serde_json::json!([0.0, 0.5]));

        let spot_schema = serde_json::to_value(schemars::schema_for!(SpotLight)).unwrap();
        let spot_props = &spot_schema["properties"];
        assert_eq!(
            spot_props["intensity"]["x-ui-range"],
            serde_json::json!([0.0, 4.0])
        );
        assert_eq!(spot_props["intensity"]["minimum"], serde_json::json!(0.0));
        for angle in ["inner_angle", "outer_angle"] {
            assert_eq!(
                spot_props[angle]["x-ui-range"],
                serde_json::json!([0.0, 89.0])
            );
        }
    }

    /// Every register field of `ParticleEmitter` travels — the Effects view's emitter tab is the
    /// generic inspector, so a field without `x-ui-range` edits as a bare number box — while the two
    /// identity fields deliberately do not: `started` is the frame stamp a restart writes (a slider
    /// over it would be the scrubber the plan rules out) and `seed` names a random stream, where
    /// adjacent values mean nothing adjacent. The colours state no travel (the swatch rule above).
    #[test]
    fn particle_emitter_travel_covers_every_register_field() {
        let schema = serde_json::to_value(schemars::schema_for!(ParticleEmitter)).unwrap();
        let props = &schema["properties"];
        for field in [
            "extent",
            "rate",
            "burst",
            "duration",
            "life",
            "speed",
            "spread_deg",
            "gravity",
            "drag",
            "floor",
            "start_size",
            "end_size",
            "softness",
        ] {
            assert!(
                props[field]["x-ui-range"].is_array(),
                "`{field}` carries no x-ui-range"
            );
            assert!(
                props[field]["items"].get("x-ui-range").is_none(),
                "`{field}`: travel landed on `items`, not the field"
            );
        }
        for field in [
            "started",
            "seed",
            "start_color",
            "end_color",
            "shape",
            "blend",
            "lit",
        ] {
            assert!(
                props[field].get("x-ui-range").is_none(),
                "`{field}` must not travel"
            );
        }
        assert_eq!(props["burst"]["x-ui-range"], serde_json::json!([0, 512]));
        assert_eq!(props["floor"]["x-ui-range"], serde_json::json!([-4.0, 4.0]));
    }

    /// `ParticleEmitter` is registered **last** (after `AudioEmitter`), and the whole registration
    /// order is pinned. Guards the append-only contract: reordering any existing component would
    /// shift its index (and re-bless every world hash), so this list is a tripwire.
    #[test]
    fn components_are_registered_in_a_pinned_append_only_order() {
        let registry = builtin_registry();
        let names: Vec<&str> = registry.iter().map(|info| info.name()).collect();
        assert_eq!(
            names,
            [
                "Transform",
                "Sprite",
                "Mesh",
                "PointLight",
                "Camera",
                "Text",
                "Parent",
                "Name",
                "Tilemap",
                "Collider",
                "UiRect",
                "UiText",
                "DirectionalLight",
                "Environment",
                "Fade",
                "PostProcess",
                "GiVolume",
                "GiDynamic",
                "Residue",
                "GroundMarker",
                "UiList",
                "Animation",
                "AudioEmitter",
                "ParticleEmitter",
                "Volley",
                "PatternPlayer",
                "Velocity",
                "Joint",
                "JointMotor",
                "SkinBone",
                "PathFollower",
                "PrefabInstance",
                "MeshGrid",
                "SpotLight",
                "ReflectionProbe",
                "CharacterMotor",
                "CharacterMotorState",
                "SurfaceVelocity",
                "LaneConstraint",
                "CameraRig",
                "DropShadow",
                "Decal",
                "ParentJoint",
                "Visibility",
                "CameraOffset",
                "Viewmodel",
                "ViewmodelLens",
                "SpatialVolume",
                "NavAgent",
                "NavSteering",
                "JointPoses",
                "PoseBlend",
                "Static",
                "NavGoal",
                "NavRoute",
                "QueryLayers",
                "UiTween",
                "UiWorldAnchor",
                "PatrolFollower",
                "PatrolState",
                "Perceiver",
                "SightTarget",
                "LightExposure",
                "StimulusRecord",
                "CameraZone",
                "AnimationLayers",
                "SurfaceTag",
                "AudioListener",
                "SurfaceOverlay",
                "NoiseZone",
                "PendingNoise",
                "PerceptionState",
                "BlendClock",
            ]
        );
        assert_eq!(
            registry.len(),
            73,
            "appending `BlendClock` grows the registry to 73"
        );
    }

    /// An empty `Animation: {}` deserializes to the default — the drift-safe path the scene format
    /// relies on, and the documented meaning of "authored with no fields": no clip, from frame 0,
    /// at normal speed.
    #[test]
    fn empty_animation_is_the_default() {
        let animation: Animation = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(animation, Animation::default());
        assert_eq!(animation.clip, "");
        assert_eq!(animation.started, FrameIndex::ZERO);
        assert_eq!(animation.speed, 1.0);
    }

    /// `Animation` round-trips through serde with non-default fields, and `started` is a plain
    /// integer on the wire (a `FrameIndex` is a newtype over `u64`) — exact in the world hash, and
    /// exact in canonical YAML.
    #[test]
    fn animation_round_trips_with_an_integer_start_frame() {
        let animation = Animation {
            clip: "walk".to_string(),
            started: FrameIndex::new(120),
            speed: 0.5,
            ..Animation::default()
        };
        let json = serde_json::to_value(&animation).unwrap();
        assert_eq!(json["started"], serde_json::json!(120));
        assert_eq!(
            serde_json::from_value::<Animation>(json).unwrap(),
            animation
        );
    }

    /// A well-formed `Animation` validates through the real scene checker — the registry picks the
    /// new component up with no schema hand-editing.
    #[test]
    fn good_animation_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = "format: 1\nentities:\n  - components:\n      Animation:\n        \
                     clip: walk\n        started: 12\n        speed: 2.0\n";
        ludion_scene::check(scene, &registry).expect("a good animation validates");
    }

    /// A negative `speed` is out of the schema's range, so it is the ordinary `L1014` at the field
    /// — no semantic validator and no dedicated code (the `GiVolume` posture).
    #[test]
    fn negative_animation_speed_is_l1014() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Animation:\n        speed: -1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/Animation/speed")),
            "expected an L1014 at Animation/speed: {diagnostics:?}"
        );
    }

    /// `hold: 0` is `L1317` at the field through the real scene checker; a positive hold validates.
    #[test]
    fn zero_animation_hold_is_l1317() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Animation:\n        hold: 0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1317"
                && d.path.as_deref() == Some("/entities/0/components/Animation/hold")),
            "expected an L1317 at Animation/hold: {diagnostics:?}"
        );
        let good = "format: 1\nentities:\n  - components:\n      Animation:\n        hold: 5\n";
        ludion_scene::check(good, &registry).expect("a positive hold validates");
    }

    /// A default `Animation` omits `hold`; a non-default one round-trips.
    #[test]
    fn animation_hold_is_omitted_at_one_and_round_trips() {
        let json = serde_json::to_value(Animation::default()).unwrap();
        assert!(json.get("hold").is_none());
        let held = Animation {
            hold: 5,
            ..Animation::default()
        };
        let json = serde_json::to_value(&held).unwrap();
        assert_eq!(json["hold"], serde_json::json!(5));
        assert_eq!(serde_json::from_value::<Animation>(json).unwrap(), held);
    }

    // --- AudioEmitter, L1136 ---

    /// An empty `AudioEmitter: {}` deserializes to the documented default: no sound, full gain, a
    /// 10-unit radius, and the ambient bus (not [`AudioCategory`]'s own `Sfx` default — an emitter is
    /// a sustained bed).
    #[test]
    fn empty_audio_emitter_is_the_default() {
        let emitter: AudioEmitter = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(emitter, AudioEmitter::default());
        assert_eq!(emitter.gain, 1.0);
        assert_eq!(emitter.range, 10.0);
        assert_eq!(emitter.category, AudioCategory::Ambient);
    }

    /// A well-formed `AudioEmitter` validates through the real scene checker, category included.
    #[test]
    fn good_audio_emitter_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = "format: 1\nentities:\n  - components:\n      AudioEmitter:\n        \
                     sound: sounds/bell.wav\n        gain: 0.8\n        range: 24.0\n        \
                     category: ambient\n";
        ludion_scene::check(scene, &registry).expect("a good emitter validates");
    }

    /// A zero `range` is rejected as `L1136` through the real scene checker, with the JSON Pointer
    /// rebased onto the document position — the `Collider` `L1116` template, and the code explains
    /// (what `ludion explain L1136` prints).
    #[test]
    fn zero_audio_emitter_range_is_a_semantic_error_in_a_scene() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      AudioEmitter:\n        range: 0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1136"
                && d.path.as_deref() == Some("/entities/0/components/AudioEmitter/range")),
            "expected an L1136 at AudioEmitter/range: {diagnostics:?}"
        );
        assert!(
            ludion_core::codes::explain("L1136").is_some(),
            "L1136 must be registered in the code table so `ludion explain L1136` answers"
        );
    }

    /// The hand-written `AudioCategory` schema actually constrains: an unknown bus name is the
    /// ordinary `L1014` at the field, not a silently accepted string.
    #[test]
    fn an_unknown_audio_emitter_category_is_l1014() {
        let registry = builtin_registry();
        let bad =
            "format: 1\nentities:\n  - components:\n      AudioEmitter:\n        category: music\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/AudioEmitter/category")),
            "expected an L1014 at AudioEmitter/category: {diagnostics:?}"
        );
    }

    /// A negative `gain` rides the schema's own lower bound, so it is the ordinary `L1014` at the
    /// field — the semantic validator owns only `range` (the `UiList` division of labour).
    #[test]
    fn negative_audio_emitter_gain_is_l1014() {
        let registry = builtin_registry();
        let bad =
            "format: 1\nentities:\n  - components:\n      AudioEmitter:\n        gain: -1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/AudioEmitter/gain")),
            "expected an L1014 at AudioEmitter/gain: {diagnostics:?}"
        );
    }

    // --- ParticleEmitter, EmitShape, ParticleBlend, L2107 ---

    /// An empty `ParticleEmitter: {}` deserializes to the documented default: a point emitter at 40
    /// particles a second, drag-free, alpha-blended, shrinking and fading out over roughly a second.
    #[test]
    fn empty_particle_emitter_is_the_documented_default() {
        let emitter: ParticleEmitter = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(emitter, ParticleEmitter::default());
        assert_eq!(emitter.shape, EmitShape::Point);
        assert_eq!(emitter.extent, [0.5, 0.5, 0.5]);
        assert_eq!(emitter.rate, 40.0);
        assert_eq!(emitter.burst, 0);
        assert_eq!(emitter.started, FrameIndex::ZERO);
        assert_eq!(emitter.duration, 0.0);
        assert_eq!(emitter.life, [0.8, 1.4]);
        assert_eq!(emitter.speed, [1.0, 2.5]);
        assert_eq!(emitter.spread_deg, 30.0);
        assert_eq!(emitter.gravity, 0.0);
        assert_eq!(emitter.drag, 0.0);
        assert_eq!(emitter.floor, -f32::MAX);
        assert_eq!(emitter.start_size, 0.25);
        assert_eq!(emitter.end_size, 0.0);
        assert_eq!(emitter.start_color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(emitter.end_color, [1.0, 1.0, 1.0, 0.0]);
        assert_eq!(emitter.blend, ParticleBlend::Alpha);
        assert_eq!(emitter.softness, 0.5);
        assert_eq!(emitter.seed, 0);
    }

    /// `shape` and `blend` are unit enums on the wire: bare snake_case strings, which is both the
    /// canonical-YAML form and what the editor inspector renders as a dropdown.
    #[test]
    fn particle_enums_serialize_as_bare_strings() {
        let emitter = ParticleEmitter {
            shape: EmitShape::Disc,
            blend: ParticleBlend::Additive,
            ..ParticleEmitter::default()
        };
        let json = serde_json::to_value(&emitter).unwrap();
        assert_eq!(json["shape"], serde_json::json!("disc"));
        assert_eq!(json["blend"], serde_json::json!("additive"));
        assert_eq!(
            serde_json::from_value::<ParticleEmitter>(json).unwrap(),
            emitter
        );
    }

    /// `lit` is off by default and absent from the serialized form when off, so the hash, snapshots
    /// and canonical text of every unlit emitter are unmoved.
    #[test]
    fn particle_lit_is_omitted_when_false_and_round_trips_when_true() {
        let unlit = serde_json::to_value(ParticleEmitter::default()).unwrap();
        assert!(unlit.get("lit").is_none());
        let lit = ParticleEmitter {
            lit: true,
            ..ParticleEmitter::default()
        };
        let json = serde_json::to_value(&lit).unwrap();
        assert_eq!(json["lit"], serde_json::json!(true));
        assert_eq!(
            serde_json::from_value::<ParticleEmitter>(json).unwrap(),
            lit
        );
        assert!(
            !serde_json::from_value::<ParticleEmitter>(serde_json::json!({}))
                .unwrap()
                .lit
        );
    }

    /// A well-formed emitter validates clean through the real scene checker, every field authored.
    #[test]
    fn good_particle_emitter_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = "format: 1\nentities:\n  - components:\n      ParticleEmitter:\n        \
                     shape: sphere\n        extent:\n          - 0.4\n          - 0.4\n          \
                     - 0.4\n        rate: 60.0\n        burst: 12\n        started: 30\n        \
                     duration: 1.5\n        life:\n          - 0.5\n          - 1.0\n        \
                     speed:\n          - 1.0\n          - 4.0\n        spread_deg: 45.0\n        \
                     gravity: 9.8\n        drag: 1.2\n        floor: -2.0\n        \
                     start_size: 0.3\n        end_size: 0.05\n        blend: additive\n        \
                     softness: 0.75\n        seed: 7\n";
        ludion_scene::check(scene, &registry).expect("a good emitter validates");
    }

    /// A negative `extent` component is `L2107` at its own index — the array-element invariant no
    /// schemars `range` can express (the `Environment` colour case).
    #[test]
    fn a_negative_particle_extent_is_l2107() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      ParticleEmitter:\n        \
                   extent:\n          - 0.5\n          - -1.0\n          - 0.5\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L2107"
                && d.path.as_deref() == Some("/entities/0/components/ParticleEmitter/extent/1")),
            "expected an L2107 at ParticleEmitter/extent/1: {diagnostics:?}"
        );
        assert!(
            ludion_core::codes::explain("L2107").is_some(),
            "L2107 must be registered in the code table so `ludion explain L2107` answers"
        );
    }

    /// A negative lifetime is `L2107` at the offending element of the pair.
    #[test]
    fn a_negative_particle_life_is_l2107() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      ParticleEmitter:\n        \
                   life:\n          - -0.5\n          - 1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L2107"
                && d.path.as_deref() == Some("/entities/0/components/ParticleEmitter/life/0")),
            "expected an L2107 at ParticleEmitter/life/0: {diagnostics:?}"
        );
    }

    /// An inverted `[min, max]` pair is `L2107` at the pair itself — the fault is the ordering, not
    /// either value.
    #[test]
    fn an_inverted_particle_range_is_l2107() {
        let registry = builtin_registry();
        for (field, body) in [
            ("life", "life:\n          - 2.0\n          - 1.0\n"),
            ("speed", "speed:\n          - 9.0\n          - 3.0\n"),
        ] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      ParticleEmitter:\n        {body}"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = format!("/entities/0/components/ParticleEmitter/{field}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L2107" && d.path.as_deref() == Some(&pointer)),
                "expected an L2107 at {pointer}: {diagnostics:?}"
            );
        }
    }

    /// A live population past `PARTICLE_MAX_LIVE_PER_EMITTER` is `L2107` pointed at `rate` — the
    /// field a mis-typed effect almost always lands in, and the one the fix usually touches.
    #[test]
    fn an_over_cap_particle_population_is_l2107() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      ParticleEmitter:\n        \
                   rate: 2000.0\n        life:\n          - 3.0\n          - 3.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L2107"
                && d.path.as_deref() == Some("/entities/0/components/ParticleEmitter/rate")),
            "expected an L2107 at ParticleEmitter/rate: {diagnostics:?}"
        );

        // A burst alone can exceed it too, with `rate` at zero.
        let burst = format!(
            "format: 1\nentities:\n  - components:\n      ParticleEmitter:\n        \
             rate: 0.0\n        burst: {}\n",
            PARTICLE_MAX_LIVE_PER_EMITTER + 1
        );
        assert!(
            ludion_scene::check(&burst, &registry)
                .unwrap_err()
                .iter()
                .any(|d| d.code.as_str() == "L2107"),
            "an over-cap burst is over the cap too"
        );
    }

    /// The scalar half of the validator, called directly: every non-finite or negative scalar is its
    /// own `L2107` at its own field. Authoring cannot reach these (`rate`, `duration`, `drag` and the
    /// two sizes carry schemars lower bounds, and JSON holds no infinity), so the guard exists for
    /// the `Value` a client hands `component.set` — the `GroundMarker` posture.
    #[test]
    fn the_particle_validator_reports_every_bad_scalar() {
        let out = ParticleEmitter::validate_semantics(&serde_json::json!({
            "rate": -1.0,
            "duration": -2.0,
            "drag": -0.5,
            "start_size": -0.25,
            "end_size": -0.25,
        }));
        assert!(out.iter().all(|d| d.code.as_str() == "L2107"));
        let pointers: Vec<&str> = out.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(
            pointers,
            ["/rate", "/duration", "/drag", "/start_size", "/end_size"],
            "one diagnostic per offending scalar, in field order"
        );
    }

    /// `spread_deg` and `softness` carry closed schemars bounds, so an out-of-range value is the
    /// ordinary `L1014` at the field and the semantic validator never sees it — the `UiList`
    /// division of labour.
    #[test]
    fn an_out_of_range_particle_spread_or_softness_is_l1014() {
        let registry = builtin_registry();
        for (field, body) in [
            ("spread_deg", "spread_deg: 200.0\n"),
            ("softness", "softness: 1.5\n"),
        ] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      ParticleEmitter:\n        {body}"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = format!("/entities/0/components/ParticleEmitter/{field}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(&pointer)),
                "expected an L1014 at {pointer}: {diagnostics:?}"
            );
        }
    }

    /// An empty `Residue: {}` deserializes to the default (the drift-safe path the scene format
    /// relies on) and round-trips through serde.
    #[test]
    fn empty_residue_is_the_default() {
        let marker: Residue = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(marker, Residue::default());
        let json = serde_json::to_value(marker).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }

    /// `Residue` has no fields, so any authored key is unknown and the field-less struct's schema
    /// rejects it as the ordinary `L1014` — the `GiDynamic` posture.
    #[test]
    fn residue_rejects_an_unknown_field_as_l1014() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Residue:\n        keep: true\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/Residue/keep")),
            "expected an L1014 at Residue/keep: {diagnostics:?}"
        );
    }

    /// An empty `GiDynamic: {}` deserializes to the default (the drift-safe path the scene format
    /// relies on) and round-trips through serde.
    #[test]
    fn empty_gi_dynamic_is_the_default() {
        let marker: GiDynamic = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(marker, GiDynamic::default());
        let json = serde_json::to_value(marker).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }

    /// `GiDynamic` has no fields, so any authored key is unknown and the field-less struct's schema
    /// rejects it as the ordinary `L1014` — no dedicated code exists because there is no *value* to
    /// be wrong, only an extra key.
    #[test]
    fn gi_dynamic_rejects_an_unknown_field_as_l1014() {
        let registry = builtin_registry();
        let bad =
            "format: 1\nentities:\n  - components:\n      GiDynamic:\n        enabled: true\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/GiDynamic/enabled")),
            "expected an L1014 at GiDynamic/enabled: {diagnostics:?}"
        );
    }

    /// An empty `DirectionalLight: {}` deserializes to the default (every field has a serde
    /// default) — the drift-safe path the scene format relies on.
    #[test]
    fn empty_directional_light_is_the_default() {
        let sun: DirectionalLight = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(sun, DirectionalLight::default());
        assert_eq!(sun.direction, [-0.3, -1.0, -0.2]);
    }

    /// `DirectionalLight` round-trips through serde with non-default fields.
    #[test]
    fn directional_light_round_trips() {
        let sun = DirectionalLight {
            direction: [1.0, -2.0, 0.5],
            color: [0.9, 0.8, 0.7],
            intensity: 2.0,
            shadow_extent: 25.0,
            shadow_near: 0.5,
            shadow_far: 120.0,
            shadow_bias: 0.003,
            angular_radius: 0.27,
            cascades: Some(4),
            shadow_distance: 60.0,
            cascade_splits: Some(vec![0.06, 0.15, 0.4]),
        };
        let json = serde_json::to_value(&sun).unwrap();
        assert_eq!(
            serde_json::from_value::<DirectionalLight>(json).unwrap(),
            sun
        );
    }

    /// The cascade fields skip at their defaults, so a default sun serializes with exactly the keys
    /// it always had: WorldHash and saved scene text do not move.
    #[test]
    fn a_default_sun_serializes_without_the_cascade_fields() {
        let json = serde_json::to_value(DirectionalLight::default()).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "color",
                "direction",
                "intensity",
                "shadow_bias",
                "shadow_extent",
                "shadow_far",
                "shadow_near"
            ]
        );
    }

    /// A one-entity scene with `light` as the sun's fields. A `key: [a, b]` flow list is rewritten
    /// to the block style scene files require, so the cases below stay readable.
    fn sun_scene(light: &str) -> String {
        let mut body = String::new();
        for line in light.lines() {
            let Some((key, rest)) = line.split_once(": [") else {
                body.push_str(&format!("{line}\n"));
                continue;
            };
            body.push_str(&format!("{key}:\n"));
            let indent = &key[..key.len() - key.trim_start().len()];
            for item in rest.trim_end_matches(']').split(", ") {
                body.push_str(&format!("{indent}  - {item}\n"));
            }
        }
        format!("format: 1\nentities:\n  - components:\n      DirectionalLight:\n{body}")
    }

    /// A sun with cascades survives the canonical writer byte-for-byte; one without emits no
    /// cascade key.
    #[test]
    fn a_cascaded_sun_round_trips_through_the_canonical_writer() {
        let registry = builtin_registry();
        let plain = ludion_scene::check(&sun_scene("        intensity: 2.0\n"), &registry).unwrap();
        let plain_text = ludion_scene::emit(&plain, &registry);
        for key in ["cascades", "shadow_distance", "cascade_splits"] {
            assert!(!plain_text.contains(key), "{key} leaked: {plain_text}");
        }
        let source = sun_scene(
            "        cascades: 4\n        shadow_distance: 60.0\n        \
             cascade_splits: [0.06, 0.15, 0.4]\n",
        );
        let scene = ludion_scene::check(&source, &registry).expect("a valid cascaded sun");
        let once = ludion_scene::emit(&scene, &registry);
        let reparsed = ludion_scene::check(&once, &registry).expect("emitted scene re-parses");
        assert_eq!(once, ludion_scene::emit(&reparsed, &registry));
        assert!(
            once.contains("cascades: 4") && once.contains("shadow_distance: 60"),
            "{once}"
        );
    }

    fn sun_diagnostics(light: &str) -> Vec<Diagnostic> {
        match ludion_scene::check(&sun_scene(light), &builtin_registry()) {
            Ok(_) => Vec::new(),
            Err(found) => found,
        }
    }

    fn assert_sun_error(light: &str, code: &str, pointer: &str) {
        let found = sun_diagnostics(light);
        let at = format!("/entities/0/components/DirectionalLight{pointer}");
        assert!(
            found
                .iter()
                .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(at.as_str())),
            "expected {code} at {at} for {light:?}: {found:?}"
        );
    }

    #[test]
    fn cascades_and_shadow_distance_are_bounded_by_the_schema() {
        for bad in ["0", "5"] {
            assert_sun_error(&format!("        cascades: {bad}\n"), "L1014", "/cascades");
        }
        for bad in ["0.0", "-1.0"] {
            assert_sun_error(
                &format!("        shadow_distance: {bad}\n"),
                "L1014",
                "/shadow_distance",
            );
        }
    }

    #[test]
    fn bad_cascade_splits_are_l1123() {
        assert_sun_error(
            "        cascade_splits: [0.5]\n",
            "L1123",
            "/cascade_splits",
        );
        for splits in ["[0.5]", "[0.2, 0.4, 0.6]"] {
            assert_sun_error(
                &format!("        cascades: 3\n        cascade_splits: {splits}\n"),
                "L1123",
                "/cascade_splits",
            );
        }
        for (splits, index) in [
            ("[0.0, 0.5]", 0),
            ("[0.5, 1.0]", 1),
            ("[0.4, 0.2]", 1),
            ("[0.3, 0.3]", 1),
        ] {
            assert_sun_error(
                &format!("        cascades: 3\n        cascade_splits: {splits}\n"),
                "L1123",
                &format!("/cascade_splits/{index}"),
            );
        }
    }

    #[test]
    fn well_formed_cascades_are_clean() {
        for light in [
            "        cascades: 1\n",
            "        cascades: 4\n        cascade_splits: [0.06, 0.15, 0.4]\n",
            "        cascades: 2\n        shadow_distance: 60\n",
        ] {
            assert_eq!(sun_diagnostics(light), Vec::new(), "{light:?}");
        }
    }

    /// `angular_radius` is validated to `[0, 45]` degrees (`L1282`) and absent from the serialized
    /// default, which keeps WorldHash and scene text unchanged at 0.
    #[test]
    fn directional_light_angular_radius_is_validated_and_skipped_at_zero() {
        let registry = builtin_registry();
        let scene = |radius: &str| {
            format!(
                "format: 1\nentities:\n  - components:\n      DirectionalLight:\n        \
                 angular_radius: {radius}\n"
            )
        };
        ludion_scene::check(&scene("0.27"), &registry).expect("0.27 is clean");
        for bad in ["-1", "46"] {
            let diagnostics = ludion_scene::check(&scene(bad), &registry).unwrap_err();
            assert!(
                diagnostics.iter().any(|d| d.code.as_str() == "L1282"
                    && d.path.as_deref()
                        == Some("/entities/0/components/DirectionalLight/angular_radius")),
                "expected an L1282 for {bad}: {diagnostics:?}"
            );
        }
        let json = serde_json::to_value(DirectionalLight::default()).unwrap();
        assert!(json.get("angular_radius").is_none());
    }

    /// A well-formed `DirectionalLight` validates through the real scene checker — the registry
    /// picks the new component up automatically (no schema hand-editing).
    #[test]
    fn good_directional_light_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = "format: 1\nentities:\n  - components:\n      DirectionalLight:\n        \
                     direction:\n          - -0.3\n          - -1.0\n          - -0.2\n";
        ludion_scene::check(scene, &registry).expect("a good directional light validates");
    }

    /// Half an atlas reference on a `Sprite` is `L1151`, whichever half is missing, and the pointer
    /// names the field actually authored.
    #[test]
    fn a_half_written_sprite_atlas_reference_is_l1151() {
        let registry = builtin_registry();
        for (field, line) in [
            ("frame", "        frame: ship/idle\n"),
            ("atlas", "        atlas: atlases/stage.atlas.yaml\n"),
        ] {
            let scene = format!("format: 1\nentities:\n  - components:\n      Sprite:\n{line}");
            let diagnostics = ludion_scene::check(&scene, &registry).unwrap_err();
            let expected = format!("/entities/0/components/Sprite/{field}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1151" && d.path.as_deref() == Some(&expected[..])),
                "expected an L1151 at /{field}: {diagnostics:?}"
            );
        }
    }

    /// `texture` and `atlas` on one `Sprite` are two competing sources of pixels — `L1151`.
    #[test]
    fn a_sprite_with_both_texture_and_atlas_is_l1151() {
        let registry = builtin_registry();
        let scene = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      Sprite:\n",
            "        texture: assets/brick.png\n",
            "        atlas: atlases/stage.atlas.yaml\n",
            "        frame: wall\n",
        );
        let diagnostics = ludion_scene::check(scene, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1151"
                && d.path.as_deref() == Some("/entities/0/components/Sprite/atlas")),
            "expected an L1151 at /atlas: {diagnostics:?}"
        );
    }

    /// A legend entry answers to the same three rules as a `Sprite`, with the glyph in the pointer.
    #[test]
    fn a_half_written_legend_atlas_reference_is_l1151_at_its_glyph() {
        let registry = builtin_registry();
        let scene = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      Tilemap:\n",
            "        legend:\n",
            "          \"#\":\n",
            "            atlas: atlases/stage.atlas.yaml\n",
            "        rows:\n",
            "          - \"#\"\n",
        );
        let diagnostics = ludion_scene::check(scene, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1151"
                && d.path.as_deref() == Some("/entities/0/components/Tilemap/legend/#/atlas")),
            "expected an L1151 at the legend entry: {diagnostics:?}"
        );
    }

    /// A zero-length `direction` passes the schema (each component is a valid number) but is
    /// rejected by the semantic validator as `L1123`, pointer rebased onto the document position.
    #[test]
    fn zero_direction_is_a_semantic_error() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      DirectionalLight:\n        \
                   direction:\n          - 0.0\n          - 0.0\n          - 0.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1123"
                && d.path.as_deref() == Some("/entities/0/components/DirectionalLight/direction")),
            "expected an L1123 at the direction field: {diagnostics:?}"
        );
    }

    /// An empty `ReflectionProbe: {}` is the default box, and a sphere probe round-trips.
    #[test]
    fn reflection_probe_defaults_and_round_trips() {
        let probe: ReflectionProbe = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(probe, ReflectionProbe::default());
        assert_eq!(probe.shape, ProbeShape::Box { extent: [2.0; 3] });
        assert_eq!((probe.blend, probe.priority), (0.5, 0));
        let custom = ReflectionProbe {
            shape: ProbeShape::Sphere { radius: 3.5 },
            blend: 1.0,
            priority: -2,
        };
        let json = serde_json::to_value(custom).unwrap();
        assert_eq!(
            json["shape"],
            serde_json::json!({ "sphere": { "radius": 3.5 } })
        );
        assert_eq!(
            serde_json::from_value::<ReflectionProbe>(json).unwrap(),
            custom
        );
    }

    /// Each bad `ReflectionProbe` value is `L1274` at the offending field.
    #[test]
    fn a_bad_reflection_probe_is_l1274() {
        let extent = "        shape:\n          box:\n            extent:\n              - 4.0\n              - 0.0\n              - 3.0\n";
        refused_with("ReflectionProbe", extent, "L1274", "/shape/box/extent/1");
        let radius = "        shape:\n          sphere:\n            radius: -1.0\n";
        refused_with("ReflectionProbe", radius, "L1274", "/shape/sphere/radius");
        refused_with(
            "ReflectionProbe",
            "        blend: -0.5\n",
            "L1274",
            "/blend",
        );
        for clean in [
            "        blend: 0.0\n",
            "        shape:\n          sphere:\n            radius: 3.0\n",
        ] {
            assert!(refusals("ReflectionProbe", clean).is_empty(), "{clean:?}");
        }
    }

    /// An empty `SpotLight: {}` is the default, and a non-default one round-trips through serde.
    #[test]
    fn spot_light_defaults_and_round_trips() {
        let spot: SpotLight = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(spot, SpotLight::default());
        assert_eq!(spot.direction, [0.0, -1.0, 0.0]);
        assert_eq!((spot.inner_angle, spot.outer_angle), (20.0, 30.0));
        let custom = SpotLight {
            color: [1.0, 0.8, 0.5],
            intensity: 3.0,
            range: 25.0,
            direction: [1.0, -0.5, 0.0],
            inner_angle: 5.0,
            outer_angle: 12.5,
            cast_shadows: true,
            shadow_bias: 0.1,
            shadow_near: 0.2,
            shadow_distance: Some(30.0),
            shadow_fade: 8.0,
            scatter: 0.5,
        };
        let json = serde_json::to_value(custom).unwrap();
        assert_eq!(serde_json::from_value::<SpotLight>(json).unwrap(), custom);
        assert!(!spot.cast_shadows);
        assert_eq!((spot.shadow_bias, spot.shadow_near), (0.02, 0.05));
        // The hash-stability pin: a default-valued light serializes exactly its six old keys.
        assert_eq!(
            sorted_keys(&serde_json::to_value(SpotLight::default()).unwrap()),
            [
                "color",
                "direction",
                "inner_angle",
                "intensity",
                "outer_angle",
                "range"
            ]
        );
    }

    fn sorted_keys(value: &Value) -> Vec<String> {
        let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort_unstable();
        keys
    }

    /// An empty `PointLight: {}` is the default, a non-default one round-trips through serde, and a
    /// default-valued light serializes only its three old keys.
    #[test]
    fn point_light_defaults_and_round_trips() {
        let point: PointLight = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(point, PointLight::default());
        assert!(!point.cast_shadows);
        assert_eq!((point.shadow_bias, point.shadow_near), (0.02, 0.05));
        let custom = PointLight {
            color: [1.0, 0.8, 0.5],
            intensity: 3.0,
            range: 25.0,
            cast_shadows: true,
            shadow_bias: 0.1,
            shadow_near: 0.2,
            shadow_distance: Some(30.0),
            shadow_fade: 8.0,
            scatter: 0.5,
        };
        let json = serde_json::to_value(custom).unwrap();
        assert_eq!(serde_json::from_value::<PointLight>(json).unwrap(), custom);
        assert_eq!(
            sorted_keys(&serde_json::to_value(PointLight::default()).unwrap()),
            ["color", "intensity", "range"]
        );
    }

    /// Both local lights round-trip through the canonical writer and a `World`: absent shadow fields
    /// emit no new keys, authored ones emit byte-stably, and an old-shape value comes back
    /// key-identical.
    #[test]
    fn local_light_shadow_fields_round_trip_through_the_writer_and_a_world() {
        let registry = builtin_registry();
        for component in ["PointLight", "SpotLight"] {
            let old = format!(
                "format: 1\nentities:\n  - components:\n      {component}:\n        range: 7.0\n"
            );
            let scene = ludion_scene::check(&old, &registry).expect("an old-shape light");
            let text = ludion_scene::emit(&scene, &registry);
            for key in [
                "cast_shadows",
                "shadow_bias",
                "shadow_near",
                "shadow_distance",
                "shadow_fade",
            ] {
                assert!(!text.contains(key), "{component} emitted {key}: {text}");
            }

            let authored = format!(
                "format: 1\nentities:\n  - components:\n      {component}:\n        \
                 cast_shadows: true\n        shadow_bias: 0.1\n        shadow_near: 0.2\n        \
                 shadow_distance: 30.0\n        shadow_fade: 8.5\n"
            );
            let scene = ludion_scene::check(&authored, &registry).expect("a casting light");
            let once = ludion_scene::emit(&scene, &registry);
            let reparsed = ludion_scene::check(&once, &registry).expect("emitted scene re-parses");
            assert_eq!(once, ludion_scene::emit(&reparsed, &registry));
            assert!(once.contains("cast_shadows: true"), "{once}");
            assert!(once.contains("shadow_distance: 30.0"), "{once}");
            assert!(once.contains("shadow_fade: 8.5"), "{once}");

            let info = registry.get(component).unwrap();
            let mut world = World::new();
            let entity = world.spawn();
            info.deserialize_insert(&serde_json::json!({ "range": 7.0 }), &mut world, entity)
                .unwrap();
            let back = info.serialize(&world, entity).unwrap().unwrap();
            assert!(
                !sorted_keys(&back)
                    .iter()
                    .any(|k| k.starts_with("shadow") || k == "cast_shadows"),
                "{component} came back with {back}"
            );
        }
    }

    /// A bad `shadow_near` is `L1312` at `/shadow_near` on both local lights; a non-casting light
    /// with a tiny range and no shadow fields stays legal.
    #[test]
    fn a_bad_local_light_shadow_near_is_l1312() {
        for component in ["PointLight", "SpotLight"] {
            for near in ["0.0", "-1.0", "12.0"] {
                refused_with(
                    component,
                    &format!("        shadow_near: {near}\n"),
                    "L1312",
                    "/shadow_near",
                );
            }
            refused_with(
                component,
                "        cast_shadows: true\n        range: 0.04\n",
                "L1312",
                "/shadow_near",
            );
            for clean in [
                "        cast_shadows: true\n",
                "        shadow_near: 0.5\n        range: 9.0\n",
                "        range: 0.04\n",
            ] {
                assert!(
                    refusals(component, clean).is_empty(),
                    "{component} {clean:?}"
                );
            }
        }
    }

    /// A bad `shadow_distance` or `shadow_fade` is `L1322` on both local lights; `fade == distance`
    /// and an authored `shadow_fade: 0` alone are legal.
    #[test]
    fn a_bad_local_light_shadow_distance_is_l1322() {
        for component in ["PointLight", "SpotLight"] {
            for (fields, pointer) in [
                ("        shadow_distance: 0.0\n", "/shadow_distance"),
                ("        shadow_distance: -3.0\n", "/shadow_distance"),
                ("        shadow_fade: -1.0\n", "/shadow_fade"),
                ("        shadow_fade: 2.0\n", "/shadow_fade"),
                (
                    "        shadow_distance: 10.0\n        shadow_fade: 10.5\n",
                    "/shadow_fade",
                ),
            ] {
                refused_with(component, fields, "L1322", pointer);
            }
            for clean in [
                "        shadow_distance: 10.0\n        shadow_fade: 10.0\n",
                "        shadow_distance: 10.0\n",
                "        shadow_fade: 0.0\n",
                "        shadow_distance: null\n",
            ] {
                assert!(
                    refusals(component, clean).is_empty(),
                    "{component} {clean:?}"
                );
            }
        }
    }

    /// Each bad `SpotLight` cone shape is `L1272` at the offending field; `inner == outer` (a hard
    /// edge), the boundary `0`, and an absent angle compared against its default are legal or caught.
    #[test]
    fn a_bad_spot_light_cone_is_l1272() {
        let zero = "        direction:\n          - 0.0\n          - 0.0\n          - 0.0\n";
        refused_with("SpotLight", zero, "L1272", "/direction");
        refused_with(
            "SpotLight",
            "        inner_angle: 90.0\n        outer_angle: 90.0\n",
            "L1272",
            "/inner_angle",
        );
        refused_with(
            "SpotLight",
            "        outer_angle: 90.0\n",
            "L1272",
            "/outer_angle",
        );
        refused_with(
            "SpotLight",
            "        outer_angle: -1.0\n",
            "L1272",
            "/outer_angle",
        );
        refused_with(
            "SpotLight",
            "        inner_angle: -5.0\n",
            "L1272",
            "/inner_angle",
        );
        refused_with(
            "SpotLight",
            "        inner_angle: 40.0\n        outer_angle: 30.0\n",
            "L1272",
            "/inner_angle",
        );
        refused_with(
            "SpotLight",
            "        inner_angle: 40.0\n",
            "L1272",
            "/inner_angle",
        );
        for clean in [
            "        inner_angle: 30.0\n        outer_angle: 30.0\n",
            "        inner_angle: 0.0\n        outer_angle: 0.0\n",
            "        inner_angle: 10.0\n        outer_angle: 89.5\n",
            "        direction:\n          - 1.0\n          - 0.0\n          - 0.0\n",
        ] {
            assert!(refusals("SpotLight", clean).is_empty(), "{clean:?}");
        }
    }

    /// Checks one component authored as block-style YAML (`body` is indented eight spaces) and
    /// returns the `(code, path)` pairs it is refused with, paths relative to the component.
    fn refusals(component: &str, body: &str) -> Vec<(String, String)> {
        let scene = format!("format: 1\nentities:\n  - components:\n      {component}:\n{body}");
        let prefix = format!("/entities/0/components/{component}");
        match ludion_scene::check(&scene, &builtin_registry()) {
            Ok(_) => Vec::new(),
            Err(diagnostics) => diagnostics
                .iter()
                .map(|d| {
                    let path = d.path.as_deref().unwrap_or("");
                    (
                        d.code.as_str().to_string(),
                        path.strip_prefix(&prefix).unwrap_or(path).to_string(),
                    )
                })
                .collect(),
        }
    }

    fn refused_with(component: &str, body: &str, code: &str, pointer: &str) {
        let got = refusals(component, body);
        assert!(
            got.iter().any(|(c, p)| c == code && p == pointer),
            "{component} {body:?}: expected {code} at {pointer}, got {got:?}"
        );
    }

    /// A light typo is a readable refusal at the field, not a silent no-op or a NaN in the shader:
    /// negative intensity, a zero range or shadow extent, and a negative bias are the schema's `L1014`.
    #[test]
    fn a_negative_light_scalar_is_l1014() {
        refused_with(
            "PointLight",
            "        intensity: -2.0\n",
            "L1014",
            "/intensity",
        );
        refused_with("PointLight", "        range: 0.0\n", "L1014", "/range");
        refused_with(
            "SpotLight",
            "        intensity: -2.0\n",
            "L1014",
            "/intensity",
        );
        refused_with("SpotLight", "        range: 0.0\n", "L1014", "/range");
        for component in ["PointLight", "SpotLight"] {
            refused_with(
                component,
                "        shadow_bias: -0.1\n",
                "L1014",
                "/shadow_bias",
            );
        }
        refused_with(
            "DirectionalLight",
            "        intensity: -1.0\n",
            "L1014",
            "/intensity",
        );
        refused_with(
            "DirectionalLight",
            "        shadow_extent: 0.0\n",
            "L1014",
            "/shadow_extent",
        );
        refused_with(
            "DirectionalLight",
            "        shadow_bias: -0.1\n",
            "L1014",
            "/shadow_bias",
        );
    }

    /// An empty or inverted shadow depth range is `L1123` at `/shadow_far`; `shadow_near` alone is
    /// compared against the default far.
    #[test]
    fn an_inverted_shadow_depth_range_is_l1123() {
        refused_with(
            "DirectionalLight",
            "        shadow_near: 5.0\n        shadow_far: 5.0\n",
            "L1123",
            "/shadow_far",
        );
        refused_with(
            "DirectionalLight",
            "        shadow_near: 300.0\n",
            "L1123",
            "/shadow_near",
        );
        refused_with(
            "DirectionalLight",
            "        shadow_far: 0.5\n",
            "L1123",
            "/shadow_far",
        );
        assert!(
            refusals(
                "DirectionalLight",
                "        shadow_near: 1.0\n        shadow_far: 50.0\n"
            )
            .is_empty()
        );
    }

    /// `Mesh.wrap` rides its schema range; a default `Mesh` serializes neither wrap key.
    #[test]
    fn mesh_wrap_is_bounded_and_default_is_not_serialized() {
        refused_with("Mesh", "        wrap: 1.5\n", "L1014", "/wrap");
        refused_with("Mesh", "        wrap: -0.1\n", "L1014", "/wrap");
        assert!(refusals("Mesh", "        wrap: 0.5\n").is_empty());
        let value = serde_json::to_value(Mesh::default()).unwrap();
        assert!(value.get("wrap").is_none());
        assert!(value.get("scatter_color").is_none());
        let wrapped = serde_json::to_value(Mesh {
            wrap: 0.5,
            scatter_color: [1.0, 0.2, 0.2],
            ..Mesh::default()
        })
        .unwrap();
        assert_eq!(wrapped["wrap"], 0.5);
        assert!(wrapped.get("scatter_color").is_some());
    }

    /// `Mesh.rim` validates its colour at `/rim/color`, rides its schema bounds, and is absent when `None`.
    #[test]
    fn mesh_rim_is_validated_and_absent_when_none() {
        refused_with(
            "Mesh",
            "        rim:\n          color:\n            - 1.0\n            - -0.5\n            - 1.0\n",
            "L1263",
            "/rim/color/1",
        );
        refused_with(
            "Mesh",
            "        rim:\n          power: 0.0\n",
            "L1014",
            "/rim",
        );
        refused_with(
            "Mesh",
            "        rim:\n          strength: -1.0\n",
            "L1014",
            "/rim",
        );
        refused_with(
            "Mesh",
            "        rim:\n          power: 1.0e-50\n",
            "L1014",
            "/rim",
        );
        refused_with(
            "Mesh",
            "        rim:\n          strength: 1.0e+39\n",
            "L1014",
            "/rim",
        );
        assert!(refusals("Mesh", "        rim:\n          strength: 2.0\n").is_empty());
        assert!(refusals("Mesh", "        rim:\n          strength: 0.0\n").is_empty());
        assert!(refusals("Mesh", "        rim:\n          power: 0.5\n").is_empty());
        let value = serde_json::to_value(Mesh::default()).unwrap();
        assert!(value.get("rim").is_none());
    }

    /// `Mesh.medium` rides its schema range (out of range `L1014`, non-finite `L1009`) and is absent when `None`.
    #[test]
    fn mesh_medium_is_bounded_and_absent_when_none() {
        for bad in ["-0.1", "1.5", "1.0000001"] {
            refused_with(
                "Mesh",
                &format!("        medium: {bad}\n"),
                "L1014",
                "/medium",
            );
        }
        for bad in [".nan", ".inf", "-.inf"] {
            refused_with(
                "Mesh",
                &format!("        medium: {bad}\n"),
                "L1009",
                "/medium",
            );
        }
        for ok in ["0.0", "0.5", "1.0e-50"] {
            assert!(refusals("Mesh", &format!("        medium: {ok}\n")).is_empty());
        }
        let value = serde_json::to_value(Mesh::default()).unwrap();
        assert!(value.get("medium").is_none());
    }

    /// `Sprite.medium` rides its schema range (out of range `L1014`, non-finite `L1009`) and is absent when `None`.
    #[test]
    fn sprite_medium_is_bounded_and_absent_when_none() {
        for bad in ["-0.1", "1.5", "1.0000001"] {
            refused_with(
                "Sprite",
                &format!("        medium: {bad}\n"),
                "L1014",
                "/medium",
            );
        }
        for bad in [".nan", ".inf", "-.inf"] {
            refused_with(
                "Sprite",
                &format!("        medium: {bad}\n"),
                "L1009",
                "/medium",
            );
        }
        for ok in ["0.0", "0.5", "1.0e-50"] {
            assert!(refusals("Sprite", &format!("        medium: {ok}\n")).is_empty());
        }
        let value = serde_json::to_value(Sprite::default()).unwrap();
        assert!(value.get("medium").is_none());
    }

    /// `ParticleEmitter.medium` rides its schema range (out of range `L1014`, non-finite `L1009`) and is absent when `None`.
    #[test]
    fn particle_emitter_medium_is_bounded_and_absent_when_none() {
        for bad in ["-0.1", "1.5", "1.0000001"] {
            refused_with(
                "ParticleEmitter",
                &format!("        medium: {bad}\n"),
                "L1014",
                "/medium",
            );
        }
        for bad in [".nan", ".inf", "-.inf"] {
            refused_with(
                "ParticleEmitter",
                &format!("        medium: {bad}\n"),
                "L1009",
                "/medium",
            );
        }
        for ok in ["0.0", "0.5", "1.0e-50"] {
            assert!(refusals("ParticleEmitter", &format!("        medium: {ok}\n")).is_empty());
        }
        let value = serde_json::to_value(ParticleEmitter::default()).unwrap();
        assert!(value.get("medium").is_none());
    }

    /// A non-finite `wrap` (YAML `.nan`, `.inf`, `-.inf`) is refused as `L1009` at the field.
    #[test]
    fn mesh_wrap_non_finite_is_l1009() {
        for text in [".nan", ".inf", "-.inf"] {
            refused_with(
                "Mesh",
                &format!(
                    "        wrap: {text}
"
                ),
                "L1009",
                "/wrap",
            );
        }
    }

    /// Every HDR colour that reaches the scene target refuses a negative RGB element, at that element.
    #[test]
    fn a_negative_hdr_colour_component_is_l1263() {
        let rgb = "          - 1.0\n          - -0.5\n          - 1.0\n";
        let rgba = "          - 1.0\n          - 1.0\n          - -0.5\n          - 1.0\n";
        let cases: Vec<(&str, String, &str)> = vec![
            ("PointLight", format!("        color:\n{rgb}"), "/color/1"),
            ("SpotLight", format!("        color:\n{rgb}"), "/color/1"),
            (
                "DirectionalLight",
                format!("        color:\n{rgb}"),
                "/color/1",
            ),
            ("Sprite", format!("        color:\n{rgba}"), "/color/2"),
            (
                "Sprite",
                format!("        flash_color:\n{rgb}"),
                "/flash_color/1",
            ),
            ("Mesh", format!("        color:\n{rgba}"), "/color/2"),
            (
                "Mesh",
                format!("        flash_color:\n{rgb}"),
                "/flash_color/1",
            ),
            (
                "Mesh",
                format!("        scatter_color:\n{rgb}"),
                "/scatter_color/1",
            ),
            ("Text", format!("        color:\n{rgba}"), "/color/2"),
            (
                "GroundMarker",
                format!("        color:\n{rgba}"),
                "/color/2",
            ),
            (
                "ParticleEmitter",
                format!("        start_color:\n{rgba}"),
                "/start_color/2",
            ),
            (
                "ParticleEmitter",
                format!("        end_color:\n{rgba}"),
                "/end_color/2",
            ),
        ];
        for (component, body, pointer) in cases {
            refused_with(component, &body, "L1263", pointer);
        }
        let tilemap = format!(
            "        legend:\n          \"#\":\n            color:\n{}        rows:\n          - \"#\"\n",
            rgba.replace("          - ", "              - ")
        );
        refused_with("Tilemap", &tilemap, "L1263", "/legend/#/color/2");
        let mesh_grid = format!(
            "        legend:\n          \"#\":\n            color:\n{}        rows:\n          - \"#\"\n",
            "              - 1.0\n              - -1.0\n              - 1.0\n              - 1.0\n"
        );
        refused_with("MeshGrid", &mesh_grid, "L1263", "/legend/#/color/1");
    }

    /// Alpha is a coverage weight in `[0, 1]`: premultiplied `over` turns `a > 1` into a negative
    /// destination weight.
    #[test]
    fn an_alpha_outside_unit_is_l1263() {
        refused_with(
            "Sprite",
            "        color:\n          - 1.0\n          - 1.0\n          - 1.0\n          - 1.5\n",
            "L1263",
            "/color/3",
        );
        refused_with(
            "ParticleEmitter",
            "        end_color:\n          - 1.0\n          - 1.0\n          - 1.0\n          - -0.1\n",
            "L1263",
            "/end_color/3",
        );
        // A particle's alpha is clamped to 1 at draw, so a value above 1 only extends full opacity.
        assert!(
            refusals(
                "ParticleEmitter",
                "        start_color:\n          - 1.0\n          - 1.0\n          - 1.0\n          - 1.5\n"
            )
            .is_empty()
        );
        // HDR RGB above 1 with a legal alpha is fine.
        assert!(
            refusals(
                "Sprite",
                "        color:\n          - 8.0\n          - 4.0\n          - 2.0\n          - 1.0\n"
            )
            .is_empty()
        );
    }

    /// A camera depth range must be a real frustum: `near > 0` is the schema's `L1014`,
    /// `near < far` is `L1264` (an absent bound compares as its default).
    #[test]
    fn a_camera_depth_range_is_validated() {
        refused_with("Camera", "        near: 0.0\n", "L1014", "/near");
        refused_with(
            "Camera",
            "        near: 5.0\n        far: 5.0\n",
            "L1264",
            "/far",
        );
        refused_with("Camera", "        near: 2000.0\n", "L1264", "/near");
        assert!(refusals("Camera", "        near: 0.5\n        far: 50.0\n").is_empty());
        assert!(refusals("Camera", "        near: 0.1\n        far: 1000.0\n").is_empty());
    }

    /// The LOD knobs are range-checked, absent when unauthored, and round-trip when present.
    #[test]
    fn camera_lod_knobs_are_bounded_and_absent_when_none() {
        for bad in ["-4.5", "4.5"] {
            let found = refusals("Camera", &format!("        lod_bias: {bad}\n"));
            assert!(
                found.iter().any(|(_, p)| p == "/lod_bias"),
                "{bad}: {found:?}"
            );
        }
        let found = refusals("Camera", "        shadow_bias_levels: 4\n");
        assert!(
            found.iter().any(|(_, p)| p == "/shadow_bias_levels"),
            "{found:?}"
        );
        assert!(
            refusals(
                "Camera",
                "        lod_bias: -4.0\n        shadow_bias_levels: 3\n"
            )
            .is_empty()
        );
        let direct = Camera::validate_semantics(
            &serde_json::json!({"lod_bias": 9.0, "shadow_bias_levels": 7}),
        );
        assert_eq!(direct.len(), 2);
        assert!(direct.iter().all(|d| d.code == code::CAMERA_LOD_INVALID));
        let value = serde_json::to_value(Camera::default()).unwrap();
        assert!(value.get("lod_bias").is_none() && value.get("shadow_bias_levels").is_none());
        let set = Camera {
            lod_bias: Some(1.5),
            shadow_bias_levels: Some(2),
            ..Camera::default()
        };
        let back: Camera = serde_json::from_value(serde_json::to_value(set).unwrap()).unwrap();
        assert_eq!(back, set);
    }

    /// A `CameraOffset` field that narrows to a non-finite `f32` is `L1332` at its pointer; finite
    /// values and the identity are clean.
    #[test]
    fn a_non_finite_camera_offset_field_is_l1332_at_its_pointer() {
        let found = CameraOffset::validate_semantics(&serde_json::json!({
            "translation": [0.0, 1.0e300, 0.0],
            "rotation_deg": [0.0, 0.0, -1.0e300],
            "fov_delta": 1.0e300,
        }));
        let pointers: Vec<_> = found
            .iter()
            .map(|d| (d.code, d.path.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            pointers,
            vec![
                (code::CAMERA_OFFSET_INVALID, "/translation/1".to_string()),
                (code::CAMERA_OFFSET_INVALID, "/rotation_deg/2".to_string()),
                (code::CAMERA_OFFSET_INVALID, "/fov_delta".to_string()),
            ]
        );
        let finite = serde_json::json!({ "translation": [1.0, 2.0, 3.0], "fov_delta": -5.0 });
        assert!(CameraOffset::validate_semantics(&finite).is_empty());
    }

    /// `ViewmodelLens: {}` is the default, and the default serializes back to `{}`.
    #[test]
    fn empty_viewmodel_lens_is_the_default_and_round_trips() {
        let lens: ViewmodelLens = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(lens, ViewmodelLens::default());
        assert_eq!(serde_json::to_value(lens).unwrap(), serde_json::json!({}));
        let tuned = ViewmodelLens {
            fov_y_deg: 60.0,
            ..ViewmodelLens::default()
        };
        assert_eq!(
            serde_json::to_value(tuned).unwrap(),
            serde_json::json!({ "fov_y_deg": 60.0 })
        );
    }

    /// Each out-of-range lens field is `L1338` at its own pointer; the default is clean.
    #[test]
    fn an_out_of_range_viewmodel_lens_field_is_l1338_at_its_pointer() {
        for (field, value) in [
            ("fov_y_deg", 0.0),
            ("fov_y_deg", 180.0),
            ("near", -1.0),
            ("near", 0.0),
            ("depth_slice", 0.6),
            ("depth_slice", 0.0),
        ] {
            let found = ViewmodelLens::validate_semantics(&serde_json::json!({ field: value }));
            assert_eq!(found.len(), 1, "{field}={value}: {found:?}");
            assert_eq!(found[0].code, code::VIEWMODEL_LENS_INVALID);
            assert_eq!(found[0].path.as_deref(), Some(format!("/{field}").as_str()));
        }
        let default = serde_json::to_value(ViewmodelLens::default()).unwrap();
        assert!(ViewmodelLens::validate_semantics(&default).is_empty());
        let edge = serde_json::json!({ "fov_y_deg": 179.0, "near": 0.001, "depth_slice": 0.5 });
        assert!(ViewmodelLens::validate_semantics(&edge).is_empty());
    }

    /// An invalid lens in a scene is refused with `L1338` at the full pointer.
    #[test]
    fn a_scene_with_an_invalid_viewmodel_lens_is_refused() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      ViewmodelLens:\n        depth_slice: 0.6\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1338"
                && d.path.as_deref() == Some("/entities/0/components/ViewmodelLens/depth_slice")),
            "expected an L1338 at ViewmodelLens/depth_slice: {diagnostics:?}"
        );
    }

    /// `Viewmodel` is field-less, so an authored key is the ordinary `L1014` (the `GiDynamic`
    /// posture), and `Viewmodel: {}` is the default.
    #[test]
    fn viewmodel_is_a_field_less_marker_that_rejects_extras() {
        let marker: Viewmodel = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(marker, Viewmodel::default());
        assert_eq!(serde_json::to_value(marker).unwrap(), serde_json::json!({}));
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Viewmodel:\n        x: 1\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/Viewmodel/x")),
            "expected an L1014 at Viewmodel/x: {diagnostics:?}"
        );
    }

    /// `NavAgent: {}` is the default and the default serializes back to `{}`; each invalid field is
    /// `L1340` at its own pointer.
    #[test]
    fn nav_agent_defaults_round_trip_and_invalid_fields_report() {
        let agent: NavAgent = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(agent, NavAgent::default());
        assert_eq!(serde_json::to_value(agent).unwrap(), serde_json::json!({}));
        let steering = serde_json::to_value(NavSteering::default()).unwrap();
        assert_eq!(steering, serde_json::json!({}));
        let default = serde_json::to_value(NavAgent::default()).unwrap();
        assert!(NavAgent::validate_semantics(&default).is_empty());
        let cases = [
            ("speed", serde_json::json!(-1.0), "/speed"),
            ("arrive_radius", serde_json::json!(-0.1), "/arrive_radius"),
            (
                "separation_radius",
                serde_json::json!(1e40),
                "/separation_radius",
            ),
            (
                "separation_strength",
                serde_json::json!(-2.0),
                "/separation_strength",
            ),
            (
                "separation_neighbors",
                serde_json::json!(17),
                "/separation_neighbors",
            ),
            ("target", serde_json::json!([0.0, 1e40, 0.0]), "/target/1"),
        ];
        for (field, bad, pointer) in cases {
            let found = NavAgent::validate_semantics(&serde_json::json!({ field: bad }));
            assert_eq!(found.len(), 1, "{field}: {found:?}");
            assert_eq!(found[0].code, code::NAV_AGENT_INVALID);
            assert_eq!(found[0].path.as_deref(), Some(pointer));
        }
        let edge =
            serde_json::json!({ "speed": 0.0, "separation_neighbors": 16, "neighbor_layers": 0 });
        assert!(NavAgent::validate_semantics(&edge).is_empty());
    }

    /// A component authored with nothing but its defaults is never refused by the colour rules.
    #[test]
    fn default_colours_validate_clean() {
        fn clean<T: SemanticValidate + Default + Serialize>(name: &str) {
            let value = serde_json::to_value(T::default()).unwrap();
            assert!(
                T::validate_semantics(&value).is_empty(),
                "{name}: {:?}",
                T::validate_semantics(&value)
            );
        }
        clean::<PointLight>("PointLight");
        clean::<SpotLight>("SpotLight");
        clean::<CameraOffset>("CameraOffset");
        clean::<ViewmodelLens>("ViewmodelLens");
        clean::<SpatialVolume>("SpatialVolume");
        clean::<NavAgent>("NavAgent");
        clean::<ReflectionProbe>("ReflectionProbe");
        clean::<ludion_physics::CharacterMotor>("CharacterMotor");
        clean::<ludion_physics::SurfaceVelocity>("SurfaceVelocity");
        clean::<LaneConstraint>("LaneConstraint");
        clean::<ludion_camera::CameraRig>("CameraRig");
        clean::<DirectionalLight>("DirectionalLight");
        clean::<Sprite>("Sprite");
        clean::<Mesh>("Mesh");
        clean::<Text>("Text");
        clean::<GroundMarker>("GroundMarker");
        clean::<DropShadow>("DropShadow");
        clean::<Decal>("Decal");
        clean::<ParticleEmitter>("ParticleEmitter");
        clean::<Tilemap>("Tilemap");
    }

    /// A negative tile count is the schema's own `L1014`, pointed at the offending element: the
    /// `range` keyword rides the array's *items*, which is the one place it reaches inside a
    /// fixed-length numeric array.
    #[test]
    fn a_negative_uv_repeat_element_is_l1014_at_that_element() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Sprite:\n        \
                   uv_repeat:\n          - 4.0\n          - -1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/Sprite/uv_repeat/1")),
            "expected an L1014 at the negative element: {diagnostics:?}"
        );
    }

    /// Adding `UiRect`/`UiText` at the end must not change the world hash of a Ui-free world: the
    /// hash emits an entity's present components in registration order, and appending component
    /// types touches neither the set nor the relative order of what a Ui-free entity carries. This
    /// pinned value locks the emission order of the pre-existing components — reorder any of them
    /// and this fails. Appending a component *type* leaves it alone (it survived the `Collider`
    /// addition and every append since, `Animation` included); a new *field* on a component this
    /// world carries does move it, because the field is in the emitted bytes — `Sprite`'s
    /// `uv_repeat`/`uv_scroll` is what this value last moved for (the sprite/camera fields:
    /// `Sprite`'s `layer`/`blend`/`flip_x`/`flip_y`/`flash`/`flash_color` and `Mesh`'s
    /// `flash`/`flash_color`, moved it before that).
    #[test]
    fn appending_ui_components_keeps_a_ui_free_world_hash_byte_identical() {
        use ludion_ecs::World;
        use ludion_sim::WorldHash;

        let mut world = World::new();
        let a = world.spawn();
        world
            .insert(
                a,
                Transform {
                    translation: [1.0, 2.0, 3.0],
                    rotation: [0.0, 90.0, 0.0],
                    scale: [1.0, 1.0, 1.0],
                },
            )
            .unwrap();
        world.insert(a, Sprite::default()).unwrap();
        let b = world.spawn();
        world.insert(b, Mesh::default()).unwrap();
        world.insert(b, PointLight::default()).unwrap();

        let hash = WorldHash::compute(&world, &builtin_registry()).unwrap();
        assert_eq!(hash.to_string(), "3d0afd715f8a9e9e");
    }

    /// A well-formed `Collider` (both shapes, both bodies) validates through the real scene checker.
    #[test]
    fn good_collider_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      Collider:\n",
            "        shape:\n",
            "          circle:\n",
            "            radius: 0.75\n",
            "        body: kinematic\n",
            "  - components:\n",
            "      Collider:\n",
            "        shape:\n",
            "          aabb:\n",
            "            half_extents:\n",
            "              - 0.5\n",
            "              - 2.0\n",
            "        body: static\n",
        );
        ludion_scene::check(scene, &registry).expect("a good collider validates");
    }

    /// The contact-material map form validates beside the bare names; a negative restitution is the
    /// schema's `L1014` (reported at the `body` alternatives, the checker's `oneOf` granularity),
    /// and an unknown combine rule is a schema error there too.
    #[test]
    fn contact_material_bodies_validate_in_a_scene() {
        let registry = builtin_registry();
        let scene = |body: &str| {
            format!(
                "format: 1\nentities:\n  - components:\n      Collider:\n        shape:\n          circle:\n            radius: 0.5\n        body:\n{body}"
            )
        };
        let good = scene(
            "          static:\n            restitution: 0.3\n            restitution_combine: max\n",
        );
        ludion_scene::check(&good, &registry).expect("the map form validates");
        let bare = scene("          kinematic: {}\n");
        ludion_scene::check(&bare, &registry).expect("an empty material map validates");

        let negative = scene("          static:\n            restitution: -0.1\n");
        let diagnostics = ludion_scene::check(&negative, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/Collider/body")),
            "expected L1014 at body for a negative restitution: {diagnostics:?}"
        );

        let unknown = scene("          static:\n            restitution_combine: sum\n");
        let diagnostics = ludion_scene::check(&unknown, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d
                .path
                .as_deref()
                .is_some_and(|p| p.starts_with("/entities/0/components/Collider/body"))),
            "expected a schema error under Collider/body: {diagnostics:?}"
        );
    }

    /// A scene written before the `dynamic` → `kinematic` rename still validates through the real
    /// scene checker, not just through `Deserialize`: `ludion_scene::check` runs the generated
    /// schema first, so the schema itself — not only `Body`'s `#[serde(alias = "dynamic")]` — must
    /// accept the legacy spelling (`ludion-spatial`'s `BodySchema` proxy).
    #[test]
    fn a_legacy_dynamic_body_still_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      Collider:\n",
            "        shape:\n",
            "          circle:\n",
            "            radius: 0.75\n",
            "        body: dynamic\n",
        );
        ludion_scene::check(scene, &registry).expect("a legacy `dynamic` body still validates");
    }

    /// A negative `radius` is caught by the schema range as `L1014`. Because the shape is a closed
    /// (externally-tagged) enum, the checker reports the `oneOf` mismatch at `/shape` and carries the
    /// exact field failure ("must be at least 0") as a *related* diagnostic pointing at the radius.
    #[test]
    fn negative_radius_is_a_schema_error() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Collider:\n        shape:\n          \
                   circle:\n            radius: -1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        // Search the diagnostics and their related sub-diagnostics for the L1014 at the radius field.
        fn names_radius(d: &Diagnostic) -> bool {
            (d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/Collider/shape/circle/radius"))
                || d.related.iter().any(names_radius)
        }
        assert!(
            diagnostics.iter().any(names_radius),
            "expected an L1014 at the radius field: {diagnostics:?}"
        );
    }

    /// A zero `radius` passes the (inclusive) schema range but is rejected by the semantic validator
    /// as `L1116`, with the component-relative JSON Pointer rebased onto the document position.
    #[test]
    fn zero_radius_is_a_semantic_error() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Collider:\n        shape:\n          \
                   circle:\n            radius: 0.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1116"
                && d.path.as_deref()
                    == Some("/entities/0/components/Collider/shape/circle/radius")),
            "expected an L1116 at the radius field: {diagnostics:?}"
        );
    }

    /// A zero `half_extents` element is rejected as `L1116`, with the pointer naming the offending
    /// array element (the invariant the schema's `range` cannot reach).
    #[test]
    fn zero_half_extent_is_a_semantic_error() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Collider:\n        shape:\n          \
                   aabb:\n            half_extents:\n              - 1.0\n              - 0.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1116"
                && d.path.as_deref()
                    == Some("/entities/0/components/Collider/shape/aabb/half_extents/1")),
            "expected an L1116 at half_extents/1: {diagnostics:?}"
        );
    }

    /// An unknown shape key (neither `circle` nor `aabb`) is an `L1014` — the shape enum is closed.
    #[test]
    fn unknown_shape_key_is_a_schema_error() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Collider:\n        shape:\n          \
                   hexagon:\n            radius: 1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"),
            "expected an L1014 for the unknown shape key: {diagnostics:?}"
        );
    }

    // --- UiRect, UiText, Anchor, L1118 ---

    /// `UiRect` round-trips through serde with a non-default anchor, texture, and colors.
    #[test]
    fn ui_rect_round_trips() {
        let rect = UiRect {
            anchor: Anchor::BottomRight,
            offset: [-8.0, -8.0],
            size: [200.0, 40.0],
            texture: Some("textures/hp_bar.png".to_string()),
            atlas: None,
            frame: None,
            color: [1.0, 0.2, 0.2, 1.0],
            order: 5,
            visible: false,
            fill: true,
            uv_rect: Some([0.0, 0.25, 0.5, 1.0]),
            slice: Some(ui::UiSlice {
                border: [1.0, 2.0, 3.0, 4.0],
                scale: 2.0,
                center: ui::UiSliceCenter::Hidden,
            }),
        };
        let json = serde_json::to_value(&rect).unwrap();
        assert_eq!(json["uv_rect"], serde_json::json!([0.0, 0.25, 0.5, 1.0]));
        assert_eq!(json["slice"]["center"], serde_json::json!("hidden"));
        assert!(
            serde_json::to_value(UiRect::default())
                .unwrap()
                .get("slice")
                .is_none()
        );
        assert_eq!(json["anchor"], serde_json::json!("bottom_right"));
        assert_eq!(serde_json::from_value::<UiRect>(json).unwrap(), rect);
    }

    /// A `UiRect` referencing an atlas region round-trips through serde, carrying the `atlas`+`frame`
    /// pair and no `texture` — the [`Sprite`] atlas convention, on the HUD.
    #[test]
    fn ui_rect_atlas_reference_round_trips() {
        let rect = UiRect {
            anchor: Anchor::TopLeft,
            offset: [24.0, 24.0],
            size: [64.0, 64.0],
            texture: None,
            atlas: Some("sprites/hud.atlas.yaml".to_string()),
            frame: Some("icon/life".to_string()),
            color: [1.0; 4],
            order: 2,
            visible: true,
            fill: false,
            uv_rect: None,
            slice: None,
        };
        let json = serde_json::to_value(&rect).unwrap();
        assert_eq!(json["atlas"], serde_json::json!("sprites/hud.atlas.yaml"));
        assert_eq!(json["frame"], serde_json::json!("icon/life"));
        assert_eq!(serde_json::from_value::<UiRect>(json).unwrap(), rect);
    }

    /// An empty `UiRect: {}` deserializes to the documented default — anchor `TopLeft`, offset
    /// `[0, 0]`, size `[100, 100]`, no texture, opaque white, order `0`, visible (drift-safety: see
    /// `ludion_testkit::assert_components_drift_safe`).
    #[test]
    fn empty_ui_rect_is_the_documented_default() {
        let rect: UiRect = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(rect, UiRect::default());
        assert_eq!(rect.anchor, Anchor::TopLeft);
        assert_eq!(rect.offset, [0.0, 0.0]);
        assert_eq!(rect.size, [100.0, 100.0]);
        assert_eq!(rect.texture, None);
        assert_eq!(rect.atlas, None);
        assert_eq!(rect.frame, None);
        assert_eq!(rect.color, [1.0; 4]);
        assert_eq!(rect.order, 0);
        assert!(rect.visible);
        assert!(!rect.fill);
    }

    /// `fill: false` is never written, so a non-fill rect serializes exactly as before the field
    /// existed; `fill: true` round-trips.
    #[test]
    fn ui_rect_fill_is_omitted_when_false_and_round_trips_when_true() {
        let json = serde_json::to_value(UiRect::default()).unwrap();
        assert!(json.get("fill").is_none());
        let filled = UiRect {
            fill: true,
            ..UiRect::default()
        };
        let json = serde_json::to_value(&filled).unwrap();
        assert_eq!(json["fill"], serde_json::json!(true));
        assert_eq!(serde_json::from_value::<UiRect>(json).unwrap(), filled);
    }

    /// A filled rect that authors a non-default `offset` or `size` warns `L1316`, one per field
    /// with its pointer; defaults, and non-filled rects, do not.
    #[test]
    fn filled_ui_rect_warns_about_ignored_offset_and_size() {
        let warn = |value: serde_json::Value| UiRect::validate_semantics(&value);
        let both = warn(serde_json::json!({"fill": true, "offset": [4, 0], "size": [10, 10]}));
        let pointers: Vec<_> = both.iter().map(|d| d.path.clone().unwrap()).collect();
        assert_eq!(pointers, ["/offset", "/size"]);
        assert!(
            both.iter()
                .all(|d| d.code == code::UI_RECT_FILL_IGNORES_PLACEMENT)
        );
        assert!(warn(serde_json::json!({"fill": true})).is_empty());
        assert!(
            warn(serde_json::json!({"fill": true, "offset": [0, 0], "size": [100, 100]}))
                .is_empty()
        );
        assert!(warn(serde_json::json!({"offset": [4, 0], "size": [10, 10]})).is_empty());
    }

    /// `UiText` round-trips through serde with non-default fields.
    #[test]
    fn ui_text_round_trips() {
        let text = UiText {
            anchor: Anchor::Center,
            offset: [1.0, -2.0],
            text: "GAME OVER".to_string(),
            size: 48.0,
            color: [1.0, 1.0, 0.0, 1.0],
            order: 10,
            wrap_width: 320.0,
            visible: false,
            font: Some("title".to_string()),
        };
        let json = serde_json::to_value(&text).unwrap();
        assert_eq!(json["anchor"], serde_json::json!("center"));
        assert_eq!(serde_json::from_value::<UiText>(json).unwrap(), text);
    }

    /// An empty `UiText: {}` deserializes to the documented default — anchor `TopLeft`, empty text,
    /// size `24.0`, opaque white, order `0`, visible (drift-safety).
    #[test]
    fn empty_ui_text_is_the_documented_default() {
        let text: UiText = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(text, UiText::default());
        assert_eq!(text.anchor, Anchor::TopLeft);
        assert_eq!(text.offset, [0.0, 0.0]);
        assert_eq!(text.text, "");
        assert_eq!(text.size, 24.0);
        assert_eq!(text.color, [1.0; 4]);
        assert_eq!(text.order, 0);
        assert_eq!(text.wrap_width, 0.0, "no wrap unless asked for");
        assert!(text.visible);
    }

    /// A negative `UiText.wrap_width` is the ordinary `L1014` at the field — its lower bound lives in
    /// the schema (the `UiList::line_spacing` posture), so it needs no code of its own.
    #[test]
    fn negative_ui_text_wrap_width_is_l1014() {
        let registry = builtin_registry();
        let bad =
            "format: 1\nentities:\n  - components:\n      UiText:\n        wrap_width: -1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/UiText/wrap_width")),
            "expected an L1014 at UiText/wrap_width: {diagnostics:?}"
        );
    }

    /// `Anchor` serializes snake_case and defaults to `TopLeft`.
    #[test]
    fn anchor_serializes_snake_case_and_defaults_to_top_left() {
        assert_eq!(Anchor::default(), Anchor::TopLeft);
        for (variant, expected) in [
            (Anchor::TopLeft, "top_left"),
            (Anchor::TopCenter, "top_center"),
            (Anchor::TopRight, "top_right"),
            (Anchor::CenterLeft, "center_left"),
            (Anchor::Center, "center"),
            (Anchor::CenterRight, "center_right"),
            (Anchor::BottomLeft, "bottom_left"),
            (Anchor::BottomCenter, "bottom_center"),
            (Anchor::BottomRight, "bottom_right"),
        ] {
            assert_eq!(
                serde_json::to_value(variant).unwrap(),
                serde_json::json!(expected)
            );
        }
    }

    /// `Anchor`'s schema is `oneOf` of `const` branches — the shape the editor inspector's
    /// `deriveVariantOptions` needs to render a dropdown — never a bare `{"enum": [...]}`. This is
    /// exactly what makes every `Anchor` variant's doc comment load-bearing (see the type's own doc
    /// comment): drop one, and schemars collapses this to the un-widgetable bare-enum shape.
    #[test]
    fn anchor_schema_is_oneof_of_const_branches() {
        let schema = serde_json::to_value(schemars::schema_for!(Anchor)).unwrap();
        let one_of = schema
            .get("oneOf")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("expected a `oneOf` schema (every variant doc-commented), got {schema:#?}")
            });
        assert_eq!(
            one_of.len(),
            9,
            "expected all nine Anchor variants: {one_of:#?}"
        );
        for alt in one_of {
            assert!(
                alt.get("const").is_some(),
                "expected every Anchor alternative to be a `const` branch, got {alt:#?}"
            );
        }
        // Not a bare enum — schemars emits that instead when a variant lacks a doc comment.
        assert!(
            schema.get("enum").is_none(),
            "schema was a bare enum: {schema:#?}"
        );
    }

    /// `UiRect.size` semantic validation flags zero, negative, and non-finite axis values with
    /// `L1118` — the `Collider.shape` (`L1116`) template. Non-finite values cannot reach here
    /// through a scene or the wire (both reject them upstream); this unit-tests the guard directly.
    #[test]
    fn ui_rect_size_semantic_validation_flags_zero_negative_and_non_finite() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut out = Vec::new();
            require_positive_finite_ui_size(bad, "/size/0", &mut out);
            assert_eq!(out.len(), 1, "expected one diagnostic for {bad}");
            assert_eq!(out[0].code.as_str(), "L1118");
            assert_eq!(out[0].path.as_deref(), Some("/size/0"));
        }
    }

    /// `UiRect::validate_semantics` reports the offending axis (not the other, valid one), and adds
    /// nothing for a valid or absent `size` (the default `[100, 100]` is valid).
    #[test]
    fn ui_rect_validate_semantics_reports_the_offending_axis() {
        let bad = serde_json::json!({ "size": [10.0, 0.0] });
        let out = UiRect::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1118");
        assert_eq!(out[0].path.as_deref(), Some("/size/1"));

        assert!(
            UiRect::validate_semantics(&serde_json::json!({ "size": [10.0, 20.0] })).is_empty()
        );
        assert!(UiRect::validate_semantics(&serde_json::json!({})).is_empty());
    }

    /// `Text.size` is the world height of one line: zero and negative are `L1118` at `/size`.
    #[test]
    fn text_size_must_be_positive() {
        for bad in [0.0, -1.0] {
            let out = Text::validate_semantics(&serde_json::json!({ "size": bad }));
            assert_eq!(out.len(), 1, "expected one diagnostic for {bad}");
            assert_eq!(out[0].code.as_str(), "L1118");
            assert_eq!(out[0].path.as_deref(), Some("/size"));
        }
        assert!(Text::validate_semantics(&serde_json::json!({})).is_empty());
        assert!(Text::validate_semantics(&serde_json::json!({ "size": 1.0 })).is_empty());
    }

    /// A finite `f64` past `f32` range is `inf` in the field it deserializes into, so a validator
    /// reports it: the check runs on the stored number, not on the wider `f64`.
    #[test]
    fn a_number_past_f32_range_is_judged_as_the_infinity_it_stores() {
        let out = UiRect::validate_semantics(&serde_json::json!({ "size": [1e39, 10.0] }));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1118");
        assert_eq!(out[0].path.as_deref(), Some("/size/0"));

        let out = AudioEmitter::validate_semantics(&serde_json::json!({ "range": 1e39 }));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1136");
        assert_eq!(out[0].path.as_deref(), Some("/range"));
    }

    /// `UiText::validate_semantics` flags a non-positive `size`, pointer `/size`, and adds nothing
    /// for a valid or absent `size` (the default `24.0` is valid).
    #[test]
    fn ui_text_validate_semantics_flags_non_positive_size() {
        let bad = serde_json::json!({ "size": -5.0 });
        let out = UiText::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1118");
        assert_eq!(out[0].path.as_deref(), Some("/size"));

        assert!(UiText::validate_semantics(&serde_json::json!({ "size": 24.0 })).is_empty());
        assert!(UiText::validate_semantics(&serde_json::json!({})).is_empty());
    }

    /// Well-formed `UiRect` and `UiText` (non-default anchor, texture, order) validate through the
    /// real scene checker — the registry picks up both components generically, no special-casing.
    #[test]
    fn good_ui_components_validate_in_a_scene() {
        let registry = builtin_registry();
        let scene = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      UiRect:\n",
            "        anchor: bottom_right\n",
            "        offset:\n",
            "          - -10.0\n",
            "          - -10.0\n",
            "        size:\n",
            "          - 64.0\n",
            "          - 64.0\n",
            "        texture: \"textures/icon.png\"\n",
            "  - components:\n",
            "      UiText:\n",
            "        anchor: center\n",
            "        text: \"HELLO\"\n",
            "        size: 32.0\n",
        );
        ludion_scene::check(scene, &registry).expect("well-formed UI components validate");
    }

    /// A `UiRect` naming an atlas region validates through the real scene checker — the atlas
    /// reference obeys the same `L1151` rules a `Sprite` does, so a whole pair is clean (the atlas
    /// path need not resolve at check time; that is `extract`'s job, exactly as for a `Sprite`).
    #[test]
    fn a_ui_rect_atlas_reference_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      UiRect:\n",
            "        atlas: sprites/hud.atlas.yaml\n",
            "        frame: icon/life\n",
        );
        ludion_scene::check(scene, &registry).expect("a whole UiRect atlas reference validates");
    }

    /// A half-written `UiRect` atlas reference (`frame` without `atlas`, or `atlas` without `frame`)
    /// is `L1151`, pointer on the offending field — the shared `atlas_reference_diagnostics` rule.
    #[test]
    fn a_half_written_ui_rect_atlas_reference_is_l1151() {
        let registry = builtin_registry();
        for (field, line) in [
            ("frame", "        frame: icon/life\n"),
            ("atlas", "        atlas: sprites/hud.atlas.yaml\n"),
        ] {
            let scene = format!("format: 1\nentities:\n  - components:\n      UiRect:\n{line}");
            let diagnostics = ludion_scene::check(&scene, &registry).unwrap_err();
            let expected = format!("/entities/0/components/UiRect/{field}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1151" && d.path.as_deref() == Some(&expected[..])),
                "expected an L1151 at /{field}: {diagnostics:?}"
            );
        }
    }

    /// `texture` and `atlas` on one `UiRect` are two competing sources of pixels — `L1151` at
    /// `/atlas`, the same verdict a `Sprite` gives.
    #[test]
    fn a_ui_rect_with_both_texture_and_atlas_is_l1151() {
        let registry = builtin_registry();
        let scene = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      UiRect:\n",
            "        texture: assets/checker.png\n",
            "        atlas: sprites/hud.atlas.yaml\n",
            "        frame: icon/life\n",
        );
        let diagnostics = ludion_scene::check(scene, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1151"
                && d.path.as_deref() == Some("/entities/0/components/UiRect/atlas")),
            "expected an L1151 at /atlas: {diagnostics:?}"
        );
    }

    /// A zero `UiRect.size` axis is rejected as `L1118` through the real scene checker, with the
    /// JSON Pointer rebased onto the document position — mirrors how `Collider`'s `L1116` is
    /// covered (`zero_radius_is_a_semantic_error`).
    #[test]
    fn zero_ui_rect_size_is_a_semantic_error_in_a_scene() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      UiRect:\n        size:\n          \
                   - 100.0\n          - 0.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1118"
                && d.path.as_deref() == Some("/entities/0/components/UiRect/size/1")),
            "expected an L1118 at UiRect/size/1: {diagnostics:?}"
        );
    }

    /// A negative `UiText.size` is rejected as `L1118` through the real scene checker.
    #[test]
    fn negative_ui_text_size_is_a_semantic_error_in_a_scene() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      UiText:\n        size: -1.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1118"
                && d.path.as_deref() == Some("/entities/0/components/UiText/size")),
            "expected an L1118 at UiText/size: {diagnostics:?}"
        );
    }

    // --- Environment, Fade, L1128 ---

    /// An empty `Environment: {}` deserializes to the documented default — zero ambient
    /// and no disc (drift-safety: see `ludion_testkit::assert_components_drift_safe`).
    #[test]
    fn empty_environment_is_the_documented_default() {
        let env: Environment = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(env, Environment::default());
        assert_eq!(env.ambient, [0.0, 0.0, 0.0]);
        assert_eq!(env.sun_disc_size, 0.0);
        assert_eq!(env.ibl_intensity, 1.0);
    }

    /// `Environment` round-trips through serde with non-default fields.
    #[test]
    fn environment_round_trips() {
        let env = Environment {
            sky_zenith: [0.1, 0.2, 0.9],
            sky_horizon: [0.4, 0.4, 0.5],
            sky_ground: [0.0, 0.0, 0.0],
            sun_disc_size: 2.5,
            sun_disc_intensity: 8.0,
            ambient: [0.02, 0.02, 0.03],
            ibl_intensity: 1.5,
        };
        let json = serde_json::to_value(env).unwrap();
        assert_eq!(serde_json::from_value::<Environment>(json).unwrap(), env);
    }

    /// A well-formed `Environment` validates through the real scene checker — the registry picks the
    /// new component up automatically (no schema hand-editing).
    #[test]
    fn good_environment_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = "format: 1\nentities:\n  - components:\n      Environment:\n        \
                     ibl_intensity: 1.5\n        sun_disc_size: 4.0\n";
        ludion_scene::check(scene, &registry).expect("a good environment validates");
    }

    /// A negative sky color component passes the schema (each element is a valid number, and schemars
    /// `range` does not reach array elements) but is rejected by the semantic validator as `L1128`,
    /// with the pointer naming the offending array element. A valid or absent color adds nothing.
    #[test]
    fn negative_environment_color_is_a_semantic_error() {
        let bad = serde_json::json!({ "sky_zenith": [0.0, -0.5, 0.0] });
        let out = Environment::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1128");
        assert_eq!(out[0].path.as_deref(), Some("/sky_zenith/1"));

        // HDR (> 1.0) is fine; a valid or absent color adds nothing.
        assert!(
            Environment::validate_semantics(&serde_json::json!({ "sky_horizon": [4.0, 2.0, 1.0] }))
                .is_empty()
        );
        assert!(Environment::validate_semantics(&serde_json::json!({})).is_empty());
    }

    /// A negative color reaches `L1128` through the real scene checker, with the JSON Pointer rebased
    /// onto the document position (the `Collider.half_extents` template).
    #[test]
    fn negative_environment_color_is_a_semantic_error_in_a_scene() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Environment:\n        \
                   ambient:\n          - 0.0\n          - -0.1\n          - 0.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1128"
                && d.path.as_deref() == Some("/entities/0/components/Environment/ambient/1")),
            "expected an L1128 at Environment/ambient/1: {diagnostics:?}"
        );
    }

    /// A negative intensity scalar is caught by the schema `range(min = 0)` as `L1014` (not the
    /// semantic `L1128`, which owns only the color arrays), with the pointer naming the field.
    #[test]
    fn negative_environment_scalar_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Environment:\n        ibl_intensity: -0.5\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/Environment/ibl_intensity")),
            "expected an L1014 at Environment/ibl_intensity: {diagnostics:?}"
        );
    }

    /// An empty `Fade: {}` deserializes to the render-identical default `(1.0, -f32::MAX)`.
    #[test]
    fn empty_fade_is_the_documented_default() {
        let fade: Fade = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(fade, Fade::default());
        assert_eq!(fade.fade, 1.0);
        assert_eq!(fade.height_min, -f32::MAX);
    }

    /// `Fade` round-trips through serde with non-default fields.
    #[test]
    fn fade_round_trips() {
        let fade = Fade {
            fade: 0.25,
            height_min: 3.5,
        };
        let json = serde_json::to_value(fade).unwrap();
        assert_eq!(serde_json::from_value::<Fade>(json).unwrap(), fade);
    }

    /// A well-formed `Fade` validates through the real scene checker.
    #[test]
    fn good_fade_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = "format: 1\nentities:\n  - components:\n      Fade:\n        fade: 0.25\n        height_min: 3.5\n";
        ludion_scene::check(scene, &registry).expect("a good fade validates");
    }

    /// An out-of-`[0, 1]` `fade` is rejected through the real scene checker as the schema's `L1014`
    /// (the `range(min = 0, max = 1)` annotation), with the pointer naming the field — `fade`'s bound
    /// is fully expressible as a schemars range, so it needs no dedicated semantic code.
    #[test]
    fn out_of_range_fade_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      Fade:\n        fade: 1.5\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/Fade/fade")),
            "expected an L1014 at Fade/fade: {diagnostics:?}"
        );
    }

    // --- PostProcess: the opt-in imaging singleton ---

    /// An empty `PostProcess: {}` deserializes to the engine's shipped imaging — every knob at the
    /// value a scene with no `PostProcess` at all renders through, so adopting the component is a
    /// no-op until a field is re-authored (drift-safety: see
    /// `ludion_testkit::assert_components_drift_safe`). Spelled out field by field rather than only
    /// against `Default`, because *which* values those are is the claim: a default that quietly
    /// moved would still equal `PostProcess::default()`.
    #[test]
    fn empty_post_process_is_the_documented_default() {
        let post: PostProcess = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(post, PostProcess::default());
        assert_eq!(post.bloom_threshold, 1.0);
        // The imaging chain the engine ships: the pyramid at the strength that matches the energy the
        // old single-blur chain carried at 0.6, AgX with the exposure that holds mid-grey where ACES
        // put it, and the dither on because banding is a defect rather than a look.
        assert_eq!(post.bloom_strength, 0.1);
        assert_eq!(post.exposure, 1.3);
        assert_eq!(post.tonemap, TonemapOperator::Agx);
        assert!(post.dither);
        // The grade's identity: every knob at the value its own step's uniform branch skips on.
        assert_eq!(post.contrast, 1.0);
        assert_eq!(post.saturation, 1.0);
        assert_eq!(post.temperature, 0.0);
        assert_eq!(post.tint, 0.0);
        assert_eq!(post.lift, [0.0; 3]);
        assert_eq!(post.gamma, [1.0; 3]);
        assert_eq!(post.gain, [1.0; 3]);
        assert_eq!(post.fog_density, 0.0);
        assert_eq!(post.fog_start, 0.0);
        assert_eq!(post.fog_height, 0.0);
        assert_eq!(post.fog_height_falloff, 0.0);
        // The medium's albedo ships at pure scattering, which is what makes an unauthored fog term
        // byte-identical to the analytic fog that predates the albedo; `g` is the haze figure the
        // shaft march used as a hardcoded constant before it became authorable.
        assert_eq!(post.fog_albedo, 1.0);
        // The ambient in-scatter ships as the flat authored slab, so the sky term is a skip and no
        // scene moves when it becomes available.
        assert_eq!(post.fog_sky, 0.0);
        assert_eq!(post.fog_anisotropy, 0.7);
        assert_eq!(post.vignette, 0.0);
        // Ambient occlusion off, and the three shape knobs at values that are inert while it is —
        // `ssao_direct` at the ambient-only rule the engine has always applied.
        assert_eq!(post.ssao, 0.0);
        assert_eq!(post.ssao_radius, 0.5);
        assert_eq!(post.ssao_bias, 0.02);
        assert_eq!(post.ssao_direct, 0.0);
        // Light shafts off; the medium they march is `fog_density`, which is off too.
        assert_eq!(post.shafts, 0.0);
        // Depth of field off (every distance in focus), with the focal plane that would be used.
        assert_eq!(post.dof, 0.0);
        assert_eq!(post.focus_distance, 10.0);
        // Film emulation all off, with the grain cell that would be used if it were turned on.
        assert_eq!(post.chromatic_aberration, 0.0);
        assert_eq!(post.halation, 0.0);
        assert_eq!(post.grain, 0.0);
        assert_eq!(post.grain_size, 0.002);
        // Screen-space reflections off (the gate), with the look that would be used if turned on.
        assert_eq!(post.ssr, 0.0);
        assert_eq!(post.ssr_fade, [0.45, 0.65]);
        assert_eq!(post.ssr_max_distance, 30.0);
        assert_eq!(post.ssr_thickness, 0.5);
    }

    /// An unauthored SSR look serializes to none of its four keys, so the component's value in
    /// `WorldHash` (and every committed scene's canonical text) is what it was before the fields.
    #[test]
    fn an_unauthored_ssr_look_serializes_nothing() {
        let json = serde_json::to_value(PostProcess::default()).unwrap();
        for key in ["ssr", "ssr_fade", "ssr_max_distance", "ssr_thickness"] {
            assert!(json.get(key).is_none(), "`{key}` leaked into {json}");
        }
        let authored = serde_json::to_value(PostProcess {
            ssr: 0.5,
            ssr_fade: [0.3, 0.5],
            ssr_max_distance: 12.0,
            ssr_thickness: 0.25,
            ..PostProcess::default()
        })
        .unwrap();
        for key in ["ssr", "ssr_fade", "ssr_max_distance", "ssr_thickness"] {
            assert!(
                authored.get(key).is_some(),
                "`{key}` missing from {authored}"
            );
        }
    }

    /// `L1273` names the offending pointer for each rule, and a valid authored look passes.
    #[test]
    fn invalid_ssr_fields_reach_l1273() {
        for (bad, pointer) in [
            (
                serde_json::json!({ "ssr_fade": [-0.1, 0.5] }),
                "/ssr_fade/0",
            ),
            (serde_json::json!({ "ssr_fade": [0.2, 1.5] }), "/ssr_fade/1"),
            (serde_json::json!({ "ssr_fade": [0.6, 0.6] }), "/ssr_fade"),
            (serde_json::json!({ "ssr_fade": [0.7, 0.4] }), "/ssr_fade"),
            (
                serde_json::json!({ "ssr_max_distance": 0.0 }),
                "/ssr_max_distance",
            ),
            (
                serde_json::json!({ "ssr_thickness": 0.0 }),
                "/ssr_thickness",
            ),
        ] {
            let out = PostProcess::validate_semantics(&bad);
            assert!(
                out.iter()
                    .any(|d| d.code.as_str() == "L1273" && d.path.as_deref() == Some(pointer)),
                "expected an L1273 at {pointer} for {bad}: {out:?}"
            );
        }
        let good = serde_json::json!({
            "ssr": 0.8,
            "ssr_fade": [0.3, 0.5],
            "ssr_max_distance": 12.0,
            "ssr_thickness": 0.25,
        });
        assert!(PostProcess::validate_semantics(&good).is_empty());
        assert!(PostProcess::validate_semantics(&serde_json::json!({})).is_empty());
    }

    /// `PostProcess` round-trips through serde with non-default fields, and its closed enum spells
    /// its variants in snake_case on the wire and in a scene file.
    #[test]
    fn post_process_round_trips() {
        let post = PostProcess {
            bloom_threshold: 1.2,
            bloom_strength: 0.4,
            exposure: 1.6,
            tonemap: TonemapOperator::Aces,
            dither: true,
            contrast: 1.15,
            saturation: 0.85,
            temperature: -0.25,
            tint: 0.1,
            lift: [0.0, 0.01, 0.02],
            gamma: [1.2, 1.0, 0.85],
            gain: [1.0, 1.05, 1.1],
            fog_density: 0.03,
            fog_color: [0.08, 0.09, 0.14],
            fog_start: 4.0,
            fog_height: 2.0,
            fog_height_falloff: 0.15,
            fog_albedo: 0.8,
            fog_sky: 0.7,
            fog_anisotropy: -0.2,
            vignette: 0.35,
            vignette_radius: 0.25,
            ssao: 0.7,
            ssao_radius: 0.8,
            ssao_bias: 0.03,
            ssao_direct: 0.35,
            shafts: 0.5,
            dof: 0.02,
            focus_distance: 12.5,
            chromatic_aberration: 0.003,
            halation: 0.25,
            grain: 0.015,
            grain_size: 0.004,
            ssr: 0.8,
            ssr_fade: [0.3, 0.5],
            ssr_max_distance: 12.0,
            ssr_thickness: 0.25,
        };
        let json = serde_json::to_value(post).unwrap();
        assert_eq!(json["tonemap"], serde_json::json!("aces"));
        assert_eq!(serde_json::from_value::<PostProcess>(json).unwrap(), post);
        assert_eq!(
            serde_json::to_value(TonemapOperator::Agx).unwrap(),
            serde_json::json!("agx")
        );
        assert_eq!(
            serde_json::to_value(TonemapOperator::Neutral).unwrap(),
            serde_json::json!("neutral")
        );
        assert_eq!(
            serde_json::to_value(TonemapOperator::None).unwrap(),
            serde_json::json!("none")
        );
    }

    /// A well-formed `PostProcess` validates through the real scene checker — the registry picks the
    /// new component up automatically (no schema hand-editing).
    #[test]
    fn good_post_process_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = "format: 1\nentities:\n  - components:\n      PostProcess:\n        \
                     bloom_threshold: 1.2\n        bloom_strength: 0.4\n        exposure: 1.4\n     \
                     \x20  tonemap: agx\n        dither: true\n        contrast: 1.1\n        \
                     saturation: 0.9\n        temperature: -0.2\n        tint: 0.05\n        \
                     lift:\n          - 0.0\n          - 0.01\n          - 0.02\n        \
                     gamma:\n          - 1.1\n          - 1.0\n          - 0.95\n        \
                     gain:\n          - 1.0\n          - 1.0\n          - 1.05\n        \
                     fog_density: 0.02\n        fog_start: 8.0\n        fog_height: 2.0\n        \
                     fog_height_falloff: 0.2\n        fog_albedo: 0.6\n        \
                     fog_sky: 0.75\n        \
                     fog_anisotropy: -0.4\n        vignette: 0.4\n        \
                     vignette_radius: 0.35\n        ssao: 0.6\n        ssao_radius: 0.9\n        \
                     ssao_bias: 0.03\n        ssao_direct: 0.3\n        dof: 0.02\n        \
                     focus_distance: 14.0\n        \
                     chromatic_aberration: 0.003\n        halation: 0.3\n        grain: 0.02\n     \
                     \x20  grain_size: 0.004\n        \
                     fog_color:\n          - 0.05\n          - 0.06\n          - 0.09\n";
        ludion_scene::check(scene, &registry).expect("a good post-process validates");
    }

    /// An out-of-range scalar is caught by the schema's numeric bounds as `L1014`, with the pointer
    /// naming the field — the whole of `PostProcess`'s scalar validation, so no dedicated semantic
    /// code duplicates it. Covers all three bound shapes: the `>= 0` knobs, the two-sided inclusive
    /// boxes the grade's white-balance axes live in, and `fog_anisotropy`'s **exclusive** `(-1, 1)`,
    /// where the endpoints themselves are the rejection (the phase function is singular there).
    #[test]
    fn negative_post_process_scalar_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        for (field, literal) in [
            ("bloom_strength", "-0.5"),
            ("exposure", "-1.0"),
            ("contrast", "-0.1"),
            ("saturation", "-1.0"),
            ("temperature", "1.5"),
            ("temperature", "-1.5"),
            ("tint", "2.0"),
            ("fog_density", "-0.01"),
            ("fog_start", "-1.0"),
            ("fog_height_falloff", "-0.2"),
            ("fog_albedo", "-0.1"),
            ("fog_albedo", "1.5"),
            ("fog_sky", "-0.1"),
            ("fog_sky", "1.5"),
            ("fog_anisotropy", "1.0"),
            ("fog_anisotropy", "-1.0"),
            ("vignette", "-0.1"),
            ("vignette", "1.5"),
            ("vignette_radius", "-0.1"),
            ("vignette_radius", "1.1"),
            ("ssao", "-0.1"),
            ("ssao", "1.2"),
            ("ssao_radius", "-0.5"),
            ("ssao_bias", "-0.01"),
            ("ssao_direct", "-0.1"),
            ("ssao_direct", "1.2"),
            ("dof", "-0.01"),
            ("dof", "0.3"),
            ("focus_distance", "-1.0"),
            ("chromatic_aberration", "-0.001"),
            ("chromatic_aberration", "0.2"),
            ("halation", "-0.1"),
            ("grain", "-0.01"),
            ("grain", "1.5"),
            ("grain_size", "-0.001"),
            ("grain_size", "1.5"),
            ("ssr", "-0.1"),
            ("ssr", "1.5"),
            ("ssr_max_distance", "-1.0"),
            ("ssr_thickness", "-0.1"),
        ] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      PostProcess:\n        \
                 {field}: {literal}\n"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = format!("/entities/0/components/PostProcess/{field}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1014"
                        && d.path.as_deref() == Some(pointer.as_str())),
                "expected an L1014 at {pointer}: {diagnostics:?}"
            );
        }
    }

    /// A grade triple of the wrong arity is the schema's own array check — `L1014` at the **field**,
    /// not at an element, which is the division of labour with the semantic `L1130` above: the schema
    /// owns the triple's *shape*, the validator owns its *values*.
    #[test]
    fn a_wrong_arity_grade_triple_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        // Block style: the scene format's strict YAML subset allows only the empty `[]` in flow
        // style (`L1005` otherwise), which would answer before the schema ever saw the arity.
        for (field, elements) in [("lift", &["0.0", "0.1"][..]), ("gain", &["1.0"; 4][..])] {
            let rows: String = elements
                .iter()
                .map(|e| format!("          - {e}\n"))
                .collect();
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      PostProcess:\n        \
                 {field}:\n{rows}"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = format!("/entities/0/components/PostProcess/{field}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1014"
                        && d.path.as_deref() == Some(pointer.as_str())),
                "expected an L1014 at {pointer}: {diagnostics:?}"
            );
        }
    }

    /// An out-of-range triple element passes the schema (each element is a valid number, and
    /// schemars `range` does not reach array elements) but is rejected by the semantic validator as
    /// `L1130`, with the pointer naming the offending element. `gain` and `fog_color` ban only the
    /// negatives; `gamma`'s floor is open, because its reciprocal is what the shader applies. `lift`
    /// is unchecked at either sign — a negative offset crushes blacks, which is a grading move.
    #[test]
    fn out_of_range_triple_element_is_a_semantic_error() {
        let bad = PostProcess::validate_semantics(&serde_json::json!({
            "gain": [1.0, -0.5, 1.0],
            "gamma": [1.0, 1.0, 0.0],
            "fog_color": [-0.01, 0.2, 0.3],
        }));
        assert_eq!(bad.len(), 3, "{bad:?}");
        assert!(bad.iter().all(|d| d.code.as_str() == "L1130"));
        assert_eq!(bad[0].path.as_deref(), Some("/gain/1"));
        assert_eq!(bad[1].path.as_deref(), Some("/gamma/2"));
        assert_eq!(bad[2].path.as_deref(), Some("/fog_color/0"));

        // A `gain` of exactly zero is legal (it kills the channel, which is a look), and a fog colour
        // is HDR like every other colour in the engine; every valid, absent or negatively-lifted
        // triple adds nothing.
        for good in [
            serde_json::json!({ "gain": [0.0, 2.0, 1.0], "gamma": [0.5, 1.0, 2.5] }),
            serde_json::json!({ "fog_color": [0.0, 1.0, 4.0] }),
            serde_json::json!({ "lift": [-0.2, -0.2, -0.2] }),
            serde_json::json!({}),
        ] {
            assert!(
                PostProcess::validate_semantics(&good).is_empty(),
                "{good} must validate"
            );
        }
    }

    /// A non-positive `gamma` element and a negative `fog_color` component both reach `L1130`
    /// through the real scene checker, with the JSON Pointer rebased onto the document position (the
    /// `Environment.ambient` template).
    #[test]
    fn out_of_range_triple_element_is_a_semantic_error_in_a_scene() {
        let registry = builtin_registry();
        for (field, elements, element_pointer) in [
            ("gamma", ["1.0", "-2.0", "1.0"], "gamma/1"),
            ("fog_color", ["0.1", "0.1", "-0.5"], "fog_color/2"),
        ] {
            let rows: String = elements
                .iter()
                .map(|e| format!("          - {e}\n"))
                .collect();
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      PostProcess:\n        \
                 {field}:\n{rows}"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = format!("/entities/0/components/PostProcess/{element_pointer}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1130"
                        && d.path.as_deref() == Some(pointer.as_str())),
                "expected an L1130 at {pointer}: {diagnostics:?}"
            );
        }
    }

    /// A non-finite bloom knob is rejected by the scene format itself: YAML `.nan`/`.inf` are
    /// unrepresentable scalars (`L1009`), so they never reach the schema or a semantic validator.
    /// This is why `PostProcess` adds no finiteness code — the misshape is unreachable through both
    /// authoring paths (JSON has no spelling for a non-finite number either), and an unreachable
    /// diagnostic is worse than none.
    #[test]
    fn non_finite_post_process_scalar_is_rejected_by_the_scene_format() {
        let registry = builtin_registry();
        for literal in [".nan", ".inf", "-.inf"] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      PostProcess:\n        \
                 bloom_strength: {literal}\n"
            );
            let diagnostics = ludion_scene::check(&bad, &registry)
                .err()
                .unwrap_or_else(|| panic!("`{literal}` must not be accepted as a scene scalar"));
            assert!(
                diagnostics.iter().any(|d| d.code.as_str() == "L1009"),
                "expected an L1009 for `{literal}`: {diagnostics:?}"
            );
        }
    }

    /// A misspelled value of either closed enum is that enum's own `L1014`, at the field's JSON
    /// Pointer, saying how many shapes were allowed and naming one of them in a nested diagnostic.
    /// Pinned because that diagnostic *is* the field's whole validation — nothing semantic
    /// duplicates it. (The nested list stops at the first branch; that is the scene validator's
    /// general `oneOf` shape, shared with every other closed enum, not something these fields add.)
    #[test]
    fn misspelled_post_process_enum_is_a_schema_error_at_the_field() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      PostProcess:\n        \
                   tonemap: agex\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        let pointer = "/entities/0/components/PostProcess/tonemap";
        let top = diagnostics
            .iter()
            .find(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(pointer))
            .unwrap_or_else(|| panic!("expected an L1014 at {pointer}: {diagnostics:?}"));
        assert!(
            top.message.contains("4 allowed shapes"),
            "the enum is closed at 4 variants: {}",
            top.message
        );
        assert!(
            top.related
                .iter()
                .any(|d| d.message.contains("agx") || d.message.contains("aces")),
            "a nested diagnostic must name an allowed spelling: {top:?}"
        );
    }

    /// `TonemapOperator` has a `oneOf`-of-`const` schema — the inspector-dropdown shape every
    /// variant's doc comment buys (see `anchor_schema_is_oneof_of_const_branches`).
    #[test]
    fn post_process_enum_schema_is_oneof_of_const_branches() {
        let schema = serde_json::to_value(schemars::schema_for!(TonemapOperator)).unwrap();
        let one_of = schema
            .get("oneOf")
            .and_then(Value::as_array)
            .expect("expected a `oneOf` schema (every variant doc-commented)");
        assert_eq!(one_of.len(), 4, "TonemapOperator: variant count");
        assert!(
            schema.get("enum").is_none(),
            "TonemapOperator: schema was a bare enum"
        );
    }

    // --- GiVolume: the opt-in global-illumination singleton ---

    /// An empty `GiVolume: {}` deserializes to the five defaults — the drift-safe path the
    /// scene format and `ludion dev`'s snapshot/restore both rely on.
    #[test]
    fn empty_gi_volume_is_the_default() {
        let gi: GiVolume = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(gi, GiVolume::default());
        assert_eq!(gi.cell_size, 0.5);
        assert_eq!(gi.probe_spacing, 1.0);
        assert_eq!(gi.normal_bias, 0.25);
        assert_eq!(gi.sky_visibility, 1.0);
        assert_eq!(gi.bounce, 0.0);
    }

    /// `GiVolume` round-trips through serde with every field non-default.
    #[test]
    fn gi_volume_round_trips() {
        let gi = GiVolume {
            cell_size: 0.25,
            probe_spacing: 2.0,
            normal_bias: 0.125,
            sky_visibility: 0.5,
            bounce: 1.5,
        };
        let json = serde_json::to_value(gi).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "cell_size": 0.25,
                "probe_spacing": 2.0,
                "normal_bias": 0.125,
                "sky_visibility": 0.5,
                "bounce": 1.5,
            })
        );
        assert_eq!(serde_json::from_value::<GiVolume>(json).unwrap(), gi);
    }

    /// A well-formed, fully-authored `GiVolume` validates through the real scene checker — the
    /// registry picks the component up by name, so this is also the registration test.
    #[test]
    fn a_good_gi_volume_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = "format: 1\nentities:\n  - components:\n      GiVolume:\n        \
                      cell_size: 0.25\n        probe_spacing: 2.0\n        \
                      normal_bias: 0.1\n        sky_visibility: 0.5\n        bounce: 1.0\n";
        ludion_scene::check(scene, &registry).expect("a good gi volume validates");
    }

    /// The strictly-positive floor is the schema's own `exclusiveMinimum`, so zero and negative are
    /// both the ordinary `L1014` at the field — no dedicated semantic code duplicates a bound the
    /// schema can express (the `fog_anisotropy` precedent). Zero is the case an inclusive `min = 0`
    /// would have let through, which is why the bound is exclusive.
    #[test]
    fn a_non_positive_gi_cell_size_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        for literal in ["0.0", "-0.25"] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      GiVolume:\n        \
                 cell_size: {literal}\n"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = "/entities/0/components/GiVolume/cell_size";
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(pointer)),
                "expected an L1014 at {pointer} for `{literal}`: {diagnostics:?}"
            );
        }
    }

    /// A value that is positive as an `f64` but narrows to `0.0` as the `f32` the field stores is
    /// refused on the scene path and the wire path alike.
    #[test]
    fn an_f32_underflowing_cell_size_is_a_schema_error_on_both_paths() {
        let registry = builtin_registry();
        for (component, field) in [
            ("Tilemap", "cell_size"),
            ("MeshGrid", "cell_size"),
            ("GiVolume", "cell_size"),
            ("Camera", "near"),
        ] {
            let pointer = format!("/entities/0/components/{component}/{field}");
            let scene = format!(
                "format: 1
entities:
  - components:
      {component}:
                         {field}: 1.0e-46
"
            );
            let diagnostics = ludion_scene::check(&scene, &registry).unwrap_err();
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(&pointer)),
                "scene: expected an L1014 at {pointer}: {diagnostics:?}"
            );
            let wire = serde_json::json!({ field: 1.0e-46 });
            let diagnostics = ludion_scene::check_component(component, &wire, &registry);
            let pointer = format!("/{field}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(&pointer)),
                "wire: expected an L1014 at {pointer}: {diagnostics:?}"
            );
        }
    }

    /// The open-floor semantic checks judge the `f32` the field stores: `1e-46` is `0.0` there.
    #[test]
    fn an_f32_underflowing_open_floor_value_is_refused_by_the_semantic_validators() {
        let code_of =
            |diagnostics: Vec<Diagnostic>| diagnostics.first().map(|d| d.code.as_str().to_string());
        let tiny = 1.0e-46;
        assert_eq!(
            code_of(UiText::validate_semantics(
                &serde_json::json!({ "size": tiny })
            ))
            .as_deref(),
            Some("L1118")
        );
        assert_eq!(
            code_of(UiList::validate_semantics(
                &serde_json::json!({ "size": tiny })
            ))
            .as_deref(),
            Some("L1118")
        );
        assert_eq!(
            code_of(AudioEmitter::validate_semantics(
                &serde_json::json!({ "range": tiny })
            ))
            .as_deref(),
            Some("L1136")
        );
        assert_eq!(
            code_of(GroundMarker::validate_semantics(
                &serde_json::json!({ "shape": { "type": "circle", "radius": tiny } })
            ))
            .as_deref(),
            Some("L2106")
        );
        assert_eq!(
            code_of(PostProcess::validate_semantics(
                &serde_json::json!({ "gamma": [1.0, tiny, 1.0] })
            ))
            .as_deref(),
            Some("L1130")
        );
        // The smallest f32 that does not vanish stays valid.
        assert!(
            AudioEmitter::validate_semantics(&serde_json::json!({ "range": 1.0e-44 })).is_empty()
        );
    }

    /// The non-finite half of "positive and finite" is the scene format's own scalar resolver
    /// (`L1009`): `.nan`/`.inf` have no representation in the strict YAML subset, so the schema
    /// bound never has to carry them.
    #[test]
    fn a_non_finite_gi_cell_size_is_rejected_by_the_scene_format() {
        let registry = builtin_registry();
        for literal in [".nan", ".inf"] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      GiVolume:\n        \
                 cell_size: {literal}\n"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            assert!(
                !diagnostics.is_empty(),
                "`{literal}` must be rejected: {diagnostics:?}"
            );
        }
    }

    /// `probe_spacing` carries the same exclusive-positive floor as `cell_size`, at its own field
    /// pointer.
    #[test]
    fn a_non_positive_gi_probe_spacing_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        for literal in ["0.0", "-1.0"] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      GiVolume:\n        \
                 probe_spacing: {literal}\n"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = "/entities/0/components/GiVolume/probe_spacing";
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(pointer)),
                "expected an L1014 at {pointer} for `{literal}`: {diagnostics:?}"
            );
        }
    }

    /// `probe_spacing`'s non-finite half is rejected by the scene format's own scalar resolver, the
    /// `cell_size` precedent.
    #[test]
    fn a_non_finite_gi_probe_spacing_is_rejected_by_the_scene_format() {
        let registry = builtin_registry();
        for literal in [".nan", ".inf"] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      GiVolume:\n        \
                 probe_spacing: {literal}\n"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            assert!(
                !diagnostics.is_empty(),
                "`{literal}` must be rejected: {diagnostics:?}"
            );
        }
    }

    /// `normal_bias` rejects a negative value as the ordinary `L1014` at its own field pointer —
    /// its floor is inclusive (`0.0` is a legitimate "no bias" value), so only negatives are bad.
    #[test]
    fn a_negative_gi_normal_bias_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      GiVolume:\n        \
                    normal_bias: -0.1\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        let pointer = "/entities/0/components/GiVolume/normal_bias";
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(pointer)),
            "expected an L1014 at {pointer}: {diagnostics:?}"
        );
    }

    /// `sky_visibility` rejects both sides of its `[0, 1]` range as the ordinary `L1014` at its own
    /// field pointer.
    #[test]
    fn an_out_of_range_gi_sky_visibility_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        for literal in ["-0.1", "1.5"] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      GiVolume:\n        \
                 sky_visibility: {literal}\n"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = "/entities/0/components/GiVolume/sky_visibility";
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(pointer)),
                "expected an L1014 at {pointer} for `{literal}`: {diagnostics:?}"
            );
        }
    }

    /// `bounce` rejects a negative value as the ordinary `L1014` at its own field pointer — the
    /// floor is inclusive (`0.0` is the documented default, skipping the bounce bake entirely), so
    /// only negatives are bad. No dedicated component L-code: a bad value is the schema range's own
    /// diagnostic, the discipline `Plan/gi-wave2-emissive-bounce.md` §6 states explicitly.
    #[test]
    fn a_negative_gi_bounce_is_a_schema_error_in_a_scene() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      GiVolume:\n        \
                    bounce: -0.5\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        let pointer = "/entities/0/components/GiVolume/bounce";
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code.as_str() == "L1014" && d.path.as_deref() == Some(pointer)),
            "expected an L1014 at {pointer}: {diagnostics:?}"
        );
    }

    // --- GroundMarker, GroundMarkerShape, L2106 ---

    /// A `GroundMarker` shape serializes internally tagged — a `type` key beside its dimensions,
    /// the `Projection` spelling every struct-variant component enum uses — and round-trips, for
    /// both variants.
    #[test]
    fn ground_marker_shape_round_trips_internally_tagged() {
        let disc = GroundMarker {
            shape: GroundMarkerShape::Disc { radius: 3.0 },
            color: [1.6, 0.2, 0.1, 0.55],
        };
        let json = serde_json::to_value(disc).unwrap();
        assert_eq!(
            json["shape"],
            serde_json::json!({ "type": "disc", "radius": 3.0 })
        );
        assert_eq!(serde_json::from_value::<GroundMarker>(json).unwrap(), disc);

        let segment = GroundMarker {
            shape: GroundMarkerShape::Segment {
                length: 8.0,
                width: 2.0,
            },
            ..GroundMarker::default()
        };
        let json = serde_json::to_value(segment).unwrap();
        assert_eq!(
            json["shape"],
            serde_json::json!({ "type": "segment", "length": 8.0, "width": 2.0 })
        );
        assert_eq!(
            serde_json::from_value::<GroundMarker>(json).unwrap(),
            segment
        );
    }

    /// An empty `GroundMarker: {}` deserializes to the unit-radius white disc — the drift-safe
    /// all-defaults path the scene format relies on.
    #[test]
    fn empty_ground_marker_is_the_default() {
        let marker: GroundMarker = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(marker, GroundMarker::default());
        assert_eq!(marker.shape, GroundMarkerShape::Disc { radius: 1.0 });
        assert_eq!(marker.color, [1.0; 4]);
    }

    /// A well-formed marker of either shape validates through the real scene checker and survives
    /// the canonical writer: emission is idempotent over a scene carrying both.
    #[test]
    fn a_well_formed_ground_marker_round_trips_through_the_canonical_writer() {
        let registry = builtin_registry();
        let source = concat!(
            "format: 1\n",
            "entities:\n",
            "  - name: mortar\n",
            "    components:\n",
            "      GroundMarker:\n",
            "        shape:\n",
            "          type: disc\n",
            "          radius: 3.0\n",
            "        color:\n",
            "          - 1.6\n",
            "          - 0.2\n",
            "          - 0.1\n",
            "          - 0.55\n",
            "  - name: charge\n",
            "    components:\n",
            "      GroundMarker:\n",
            "        shape:\n",
            "          type: segment\n",
            "          length: 8.0\n",
            "          width: 2.0\n",
        );
        let scene = ludion_scene::check(source, &registry).expect("a valid marker scene");
        let once = ludion_scene::emit(&scene, &registry);
        let reparsed = ludion_scene::check(&once, &registry).expect("emitted scene re-parses");
        assert_eq!(once, ludion_scene::emit(&reparsed, &registry));
        assert!(
            once.contains("type: disc") && once.contains("type: segment"),
            "the canonical writer keeps the internally tagged shape: {once}"
        );
    }

    /// A zero `radius`, a negative `length`, and a zero `width` are each `L2106` at their own field
    /// pointer, through the real scene checker.
    #[test]
    fn a_non_positive_ground_marker_dimension_is_l2106() {
        let registry = builtin_registry();
        for (shape, field) in [
            ("type: disc\n          radius: 0.0", "radius"),
            (
                "type: segment\n          length: -4.0\n          width: 2.0",
                "length",
            ),
            (
                "type: segment\n          length: 4.0\n          width: 0.0",
                "width",
            ),
        ] {
            let bad = format!(
                "format: 1\nentities:\n  - components:\n      GroundMarker:\n        shape:\n          {shape}\n"
            );
            let diagnostics = ludion_scene::check(&bad, &registry).unwrap_err();
            let pointer = format!("/entities/0/components/GroundMarker/shape/{field}");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "L2106"
                        && d.path.as_deref() == Some(pointer.as_str())),
                "expected an L2106 at {pointer}: {diagnostics:?}"
            );
        }
    }

    /// A segment whose two dimensions are both bad reports both, in field order — one pass fixes
    /// the marker rather than one field per run.
    #[test]
    fn every_bad_ground_marker_dimension_is_reported() {
        let out = GroundMarker::validate_semantics(&serde_json::json!({
            "shape": { "type": "segment", "length": 0.0, "width": -1.0 },
        }));
        let pointers: Vec<&str> = out.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(pointers, ["/shape/length", "/shape/width"]);
        assert!(out.iter().all(|d| d.code.as_str() == "L2106"));
    }

    /// An absent `shape` (the valid unit-radius default) and a well-formed one each add nothing —
    /// the validator never invents a diagnostic for a field the author did not get wrong.
    #[test]
    fn a_default_ground_marker_shape_validates_clean() {
        assert!(GroundMarker::validate_semantics(&serde_json::json!({})).is_empty());
        assert!(
            GroundMarker::validate_semantics(&serde_json::json!({
                "shape": { "type": "disc", "radius": 3.0 },
            }))
            .is_empty()
        );
    }

    /// A wedge serializes internally tagged and round-trips; a minimal wedge takes the defaults.
    #[test]
    fn ground_marker_wedge_round_trips_and_defaults() {
        let wedge = GroundMarker {
            shape: GroundMarkerShape::Wedge {
                half_angle_deg: 30.0,
                outer_radius: 7.0,
                inner_radius: 3.0,
                fill: 0.5,
                grid_spacing: 2.0,
            },
            color: [0.25, 1.6, 0.6, 0.9],
        };
        let json = serde_json::to_value(wedge).unwrap();
        assert_eq!(
            json["shape"],
            serde_json::json!({
                "type": "wedge", "half_angle_deg": 30.0, "outer_radius": 7.0,
                "inner_radius": 3.0, "fill": 0.5, "grid_spacing": 2.0
            })
        );
        assert_eq!(serde_json::from_value::<GroundMarker>(json).unwrap(), wedge);

        let minimal: GroundMarker = serde_json::from_value(serde_json::json!({
            "shape": { "type": "wedge", "half_angle_deg": 30.0, "outer_radius": 7.0 },
        }))
        .unwrap();
        assert_eq!(
            minimal.shape,
            GroundMarkerShape::Wedge {
                half_angle_deg: 30.0,
                outer_radius: 7.0,
                inner_radius: 0.0,
                fill: 0.0,
                grid_spacing: 1.0,
            }
        );
    }

    /// A well-formed wedge validates clean and survives the canonical writer idempotently.
    #[test]
    fn a_well_formed_wedge_validates_and_round_trips_through_the_canonical_writer() {
        assert!(
            GroundMarker::validate_semantics(&serde_json::json!({
                "shape": { "type": "wedge", "half_angle_deg": 180.0, "outer_radius": 7.0,
                           "inner_radius": 3.0, "fill": 1.0 },
            }))
            .is_empty()
        );
        let registry = builtin_registry();
        let source = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      GroundMarker:\n",
            "        shape:\n",
            "          type: wedge\n",
            "          half_angle_deg: 30.0\n",
            "          outer_radius: 7.0\n",
            "          inner_radius: 3.0\n",
            "          fill: 0.35\n",
        );
        let scene = ludion_scene::check(source, &registry).expect("a valid wedge scene");
        let once = ludion_scene::emit(&scene, &registry);
        let reparsed = ludion_scene::check(&once, &registry).expect("emitted scene re-parses");
        assert_eq!(once, ludion_scene::emit(&reparsed, &registry));
        assert!(once.contains("type: wedge"), "{once}");
    }

    /// Each unusable wedge is `L2113` at its own field pointer.
    #[test]
    fn an_undrawable_wedge_is_l2113() {
        for (shape, field) in [
            (
                serde_json::json!({ "half_angle_deg": 0.0, "outer_radius": 7.0 }),
                "half_angle_deg",
            ),
            (
                serde_json::json!({ "half_angle_deg": 181.0, "outer_radius": 7.0 }),
                "half_angle_deg",
            ),
            (
                serde_json::json!({ "half_angle_deg": 30.0, "outer_radius": 7.0, "inner_radius": -1.0 }),
                "inner_radius",
            ),
            (
                serde_json::json!({ "half_angle_deg": 30.0, "outer_radius": 7.0, "inner_radius": 7.0 }),
                "inner_radius",
            ),
            (
                serde_json::json!({ "half_angle_deg": 30.0, "outer_radius": 7.0, "fill": 1.5 }),
                "fill",
            ),
            (
                serde_json::json!({ "half_angle_deg": 30.0, "outer_radius": 7.0, "fill": -0.1 }),
                "fill",
            ),
        ] {
            let mut shape = shape;
            shape["type"] = "wedge".into();
            let out = GroundMarker::validate_semantics(&serde_json::json!({ "shape": shape }));
            let pointer = format!("/shape/{field}");
            assert!(
                out.iter()
                    .any(|d| d.code.as_str() == "L2113"
                        && d.path.as_deref() == Some(pointer.as_str())),
                "expected an L2113 at {pointer}: {out:?}"
            );
        }
    }

    /// A zero `outer_radius` and a negative `grid_spacing` are `L2106`.
    #[test]
    fn a_non_positive_wedge_dimension_is_l2106() {
        let out = GroundMarker::validate_semantics(&serde_json::json!({
            "shape": { "type": "wedge", "half_angle_deg": 30.0, "outer_radius": 0.0,
                       "grid_spacing": -1.0 },
        }));
        let l2106: Vec<&str> = out
            .iter()
            .filter(|d| d.code.as_str() == "L2106")
            .filter_map(|d| d.path.as_deref())
            .collect();
        assert_eq!(l2106, ["/shape/outer_radius", "/shape/grid_spacing"]);
    }

    /// A `DropShadow` authoring a wedge is `L2113` at `/shape/type`.
    #[test]
    fn a_drop_shadow_wedge_is_l2113() {
        let out = DropShadow::validate_semantics(&serde_json::json!({
            "shape": { "type": "wedge", "half_angle_deg": 30.0, "outer_radius": 7.0 },
        }));
        assert!(
            out.iter()
                .any(|d| d.code.as_str() == "L2113" && d.path.as_deref() == Some("/shape/type")),
            "{out:?}"
        );
    }

    // --- DropShadow, L2106, L2110 ---

    /// An empty `DropShadow: {}` deserializes to the ruled defaults.
    #[test]
    fn empty_drop_shadow_is_the_default() {
        let shadow: DropShadow = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(shadow, DropShadow::default());
        assert_eq!(shadow.shape, GroundMarkerShape::Disc { radius: 0.5 });
        assert_eq!(shadow.opacity, 0.6);
        assert_eq!(shadow.softness, 0.5);
        assert_eq!(shadow.max_distance, 20.0);
        assert_eq!(shadow.fade_distance, 4.0);
    }

    /// A well-formed drop shadow of either shape survives the canonical writer idempotently.
    #[test]
    fn a_well_formed_drop_shadow_round_trips_through_the_canonical_writer() {
        let registry = builtin_registry();
        let source = concat!(
            "format: 1\n",
            "entities:\n",
            "  - name: blob\n",
            "    components:\n",
            "      DropShadow:\n",
            "        shape:\n",
            "          type: disc\n",
            "          radius: 0.75\n",
            "        opacity: 0.4\n",
            "  - name: streak\n",
            "    components:\n",
            "      DropShadow:\n",
            "        shape:\n",
            "          type: segment\n",
            "          length: 2.0\n",
            "          width: 0.5\n",
            "        max_distance: 10.0\n",
            "        fade_distance: 10.0\n",
        );
        let scene = ludion_scene::check(source, &registry).expect("a valid drop shadow scene");
        let once = ludion_scene::emit(&scene, &registry);
        let reparsed = ludion_scene::check(&once, &registry).expect("emitted scene re-parses");
        assert_eq!(once, ludion_scene::emit(&reparsed, &registry));
        assert!(
            once.contains("type: disc") && once.contains("type: segment"),
            "the canonical writer keeps the internally tagged shape: {once}"
        );
    }

    /// Checks `body` (a `DropShadow` mapping's fields) and returns the `(code, path)` pairs raised.
    fn drop_shadow_diagnostics(body: &str) -> Vec<(String, String)> {
        let registry = builtin_registry();
        let source = format!("format: 1\nentities:\n  - components:\n      DropShadow:\n{body}");
        match ludion_scene::check(&source, &registry) {
            Ok(_) => Vec::new(),
            Err(diagnostics) => diagnostics
                .iter()
                .map(|d| {
                    (
                        d.code.as_str().to_string(),
                        d.path.clone().unwrap_or_default(),
                    )
                })
                .collect(),
        }
    }

    fn drop_shadow_pointer(field: &str) -> String {
        format!("/entities/0/components/DropShadow/{field}")
    }

    /// A non-positive shape dimension is `L2106` at its own field, as for `GroundMarker`.
    #[test]
    fn a_non_positive_drop_shadow_dimension_is_l2106() {
        for (shape, field) in [
            ("type: disc\n          radius: 0.0", "radius"),
            ("type: disc\n          radius: -1.0", "radius"),
            (
                "type: segment\n          length: 2.0\n          width: 0.0",
                "width",
            ),
        ] {
            let found = drop_shadow_diagnostics(&format!("        shape:\n          {shape}\n"));
            assert!(
                found.contains(&(
                    "L2106".to_string(),
                    drop_shadow_pointer(&format!("shape/{field}"))
                )),
                "{found:?}"
            );
        }
    }

    /// An out-of-range opacity or softness is the schema's ordinary `L1014`.
    #[test]
    fn drop_shadow_opacity_and_softness_are_bounded_by_the_schema() {
        let found = drop_shadow_diagnostics("        opacity: 1.5\n");
        assert!(
            found.contains(&("L1014".to_string(), drop_shadow_pointer("opacity"))),
            "{found:?}"
        );
        let found = drop_shadow_diagnostics("        softness: -0.1\n");
        assert!(
            found.contains(&("L1014".to_string(), drop_shadow_pointer("softness"))),
            "{found:?}"
        );
    }

    /// The distance rules are `L2110`, an absent field read at its default.
    #[test]
    fn drop_shadow_distances_are_l2110() {
        let l2110 = |field: &str| ("L2110".to_string(), drop_shadow_pointer(field));
        let found = drop_shadow_diagnostics("        max_distance: 0.0\n");
        assert!(found.contains(&l2110("max_distance")), "{found:?}");
        let found = drop_shadow_diagnostics("        fade_distance: -1.0\n");
        assert!(found.contains(&l2110("fade_distance")), "{found:?}");
        // Against the default max of 20.
        let found = drop_shadow_diagnostics("        fade_distance: 30.0\n");
        assert_eq!(found, [l2110("fade_distance")]);
        let found =
            drop_shadow_diagnostics("        max_distance: 20.0\n        fade_distance: 20.0\n");
        assert!(found.is_empty(), "{found:?}");
        let found = drop_shadow_diagnostics("        max_distance: 10.0\n");
        assert!(found.is_empty(), "{found:?}");
        // A lone short max is the author's field to fix, against the default fade of 4.
        let found = drop_shadow_diagnostics("        max_distance: 2.0\n");
        assert_eq!(found, [l2110("max_distance")]);
        let registry = builtin_registry();
        let source =
            "format: 1\nentities:\n  - components:\n      DropShadow:\n        max_distance: 2.0\n";
        let help = ludion_scene::check(source, &registry)
            .expect_err("a lone short max_distance is refused")[0]
            .help
            .clone()
            .unwrap_or_default();
        assert!(
            help.contains("fade_distance") && help.contains("max_distance"),
            "{help}"
        );
        let found =
            drop_shadow_diagnostics("        max_distance: 2.0\n        fade_distance: 1.0\n");
        assert!(found.is_empty(), "{found:?}");
    }

    // --- Decal, L2106, L2112 ---

    /// An empty `Decal: {}` deserializes to the default and re-emits as exactly `Decal: {}`.
    #[test]
    fn empty_decal_is_the_default_and_re_emits_empty() {
        let decal: Decal = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(decal, Decal::default());
        let registry = builtin_registry();
        let source = "format: 1\nentities:\n  - components:\n      Decal: {}\n";
        let scene = ludion_scene::check(source, &registry).expect("an empty decal is valid");
        assert!(
            ludion_scene::emit(&scene, &registry).contains("      Decal: {}\n"),
            "the default decal re-emits as `Decal: {{}}`"
        );
    }

    /// A decal with every field off its default survives the canonical writer idempotently.
    #[test]
    fn a_fully_authored_decal_round_trips_through_the_canonical_writer() {
        let registry = builtin_registry();
        let source = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      Decal:\n",
            "        size:\n",
            "          - 2.0\n",
            "          - 0.5\n",
            "          - 3.0\n",
            "        base_color_texture:\n",
            "          path: decals/scorch.png\n",
            "          wrap: clamp\n",
            "        normal_texture:\n",
            "          path: decals/scorch_n.png\n",
            "          scale: 0.5\n",
            "        metallic_roughness_texture:\n",
            "          path: decals/scorch_mr.png\n",
            "          wrap: mirror\n",
            "        uv_rect:\n",
            "          - 0.25\n",
            "          - 0.0\n",
            "          - 0.75\n",
            "          - 0.5\n",
            "        color:\n",
            "          - 2.0\n",
            "          - 1.0\n",
            "          - 0.5\n",
            "          - 0.9\n",
            "        opacity: 0.8\n",
            "        roughness: 0.3\n",
            "        metallic: 0.5\n",
            "        blend: coat\n",
            "        angle_fade:\n",
            "          - 30.0\n",
            "          - 50.0\n",
            "        layer: -2\n",
            "        receivers: hierarchy\n",
        );
        let scene = ludion_scene::check(source, &registry).expect("a valid decal");
        let once = ludion_scene::emit(&scene, &registry);
        for field in [
            "size:",
            "base_color_texture:",
            "wrap: clamp",
            "scale: 0.5",
            "wrap: mirror",
            "uv_rect:",
            "color:",
            "opacity: 0.8",
            "roughness: 0.3",
            "metallic: 0.5",
            "blend: coat",
            "angle_fade:",
            "layer: -2",
            "receivers: hierarchy",
        ] {
            assert!(once.contains(field), "`{field}` missing from {once}");
        }
        let reparsed = ludion_scene::check(&once, &registry).expect("emitted scene re-parses");
        assert_eq!(once, ludion_scene::emit(&reparsed, &registry));
    }

    /// Checks `body` (a `Decal` mapping's fields) and returns the `(code, path)` pairs raised.
    fn decal_diagnostics(body: &str) -> Vec<(String, String)> {
        let registry = builtin_registry();
        let source = format!("format: 1\nentities:\n  - components:\n      Decal:\n{body}");
        match ludion_scene::check(&source, &registry) {
            Ok(_) => Vec::new(),
            Err(diagnostics) => diagnostics
                .iter()
                .map(|d| {
                    (
                        d.code.as_str().to_string(),
                        d.path.clone().unwrap_or_default(),
                    )
                })
                .collect(),
        }
    }

    fn decal_pointer(field: &str) -> String {
        format!("/entities/0/components/Decal/{field}")
    }

    /// A non-positive `size` element is `L2106` at that element.
    #[test]
    fn a_non_positive_decal_size_is_l2106_at_the_element() {
        let found = decal_diagnostics(
            "        size:\n          - 0.0\n          - 1.0\n          - -1.0\n",
        );
        for index in [0, 2] {
            assert!(
                found.contains(&("L2106".to_string(), decal_pointer(&format!("size/{index}")))),
                "{found:?}"
            );
        }
        assert_eq!(found.len(), 2, "{found:?}");
    }

    /// A negative tint is the HDR colour rule, `L1263`.
    #[test]
    fn a_negative_decal_color_is_l1263() {
        let found = decal_diagnostics(
            "        color:\n          - -1.0\n          - 0.0\n          - 0.0\n          - 1.0\n",
        );
        assert!(found.iter().any(|(code, _)| code == "L1263"), "{found:?}");
    }

    /// Out-of-range scalars are the schema's ordinary `L1014`, at the field or element.
    #[test]
    fn decal_scalars_are_bounded_by_the_schema() {
        for (body, pointer) in [
            ("        opacity: 1.5\n", "opacity"),
            ("        roughness: -0.1\n", "roughness"),
            ("        metallic: 2.0\n", "metallic"),
            (
                "        uv_rect:\n          - 0.0\n          - 0.0\n          - 1.5\n          - 1.0\n",
                "uv_rect/2",
            ),
        ] {
            let found = decal_diagnostics(body);
            assert!(
                found.contains(&("L1014".to_string(), decal_pointer(pointer))),
                "{body}: {found:?}"
            );
        }
    }

    /// The angle fade holds `0 <= start <= end <= 90` (`L2112`).
    #[test]
    fn decal_angle_fade_is_l2112() {
        let l2112 = |field: &str| ("L2112".to_string(), decal_pointer(field));
        assert_eq!(
            decal_diagnostics("        angle_fade:\n          - 80.0\n          - 60.0\n"),
            [l2112("angle_fade")]
        );
        assert_eq!(
            decal_diagnostics("        angle_fade:\n          - -1.0\n          - 80.0\n"),
            [l2112("angle_fade/0")]
        );
        assert_eq!(
            decal_diagnostics("        angle_fade:\n          - 10.0\n          - 95.0\n"),
            [l2112("angle_fade/1")]
        );
        assert!(
            decal_diagnostics("        angle_fade:\n          - 0.0\n          - 90.0\n")
                .is_empty()
        );
    }

    /// The three map paths are asset paths, so the registry rebases them document-relative.
    #[test]
    fn decal_map_paths_are_asset_paths() {
        let registry = builtin_registry();
        let paths = registry.get("Decal").expect("registered").asset_paths();
        assert_eq!(
            paths,
            [
                "/base_color_texture/path",
                "/metallic_roughness_texture/path",
                "/normal_texture/path",
            ]
        );
    }

    // --- UiList, UiListItem, L1118 ---

    /// `UiList` round-trips through serde with non-default fields, items included.
    #[test]
    fn ui_list_round_trips() {
        let list = UiList {
            anchor: Anchor::Center,
            offset: [10.0, -20.0],
            title: "WARES".to_string(),
            items: vec![
                UiListItem {
                    text: "SALVE".to_string(),
                    enabled: true,
                },
                UiListItem {
                    text: "RELIC".to_string(),
                    enabled: false,
                },
            ],
            selected: 1,
            size: 32.0,
            line_spacing: 4.0,
            padding: 24.0,
            color: [0.9, 0.9, 0.9, 1.0],
            selected_color: [1.0, 0.5, 0.0, 1.0],
            disabled_color: [0.2, 0.2, 0.2, 1.0],
            background: [0.0, 0.0, 0.0, 0.75],
            order: 7,
            max_visible: 4,
            visible: false,
            font: Some("body".to_string()),
        };
        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(serde_json::from_value::<UiList>(json).unwrap(), list);
    }

    /// An empty `UiList: {}` deserializes to the documented default — the drift-safe path the scene
    /// format relies on (every field carries a serde default).
    #[test]
    fn empty_ui_list_is_the_default() {
        let list: UiList = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(list, UiList::default());
        assert_eq!(list.anchor, Anchor::TopLeft);
        assert!(list.title.is_empty());
        assert!(list.items.is_empty());
        assert_eq!(list.selected, 0);
        assert_eq!(list.size, 24.0);
        assert_eq!(list.line_spacing, 0.0);
        assert_eq!(list.padding, 16.0);
        assert_eq!(list.color, [1.0; 4]);
        assert_eq!(list.background, [0.0; 4]);
        assert_eq!(list.order, 0);
        assert_eq!(list.max_visible, 0, "unlimited unless capped");
        assert!(list.visible);
    }

    /// An empty `UiListItem: {}` deserializes to an enabled, empty line — an authored item that says
    /// only `text:` is selectable, which is the common case.
    #[test]
    fn empty_ui_list_item_is_enabled_by_default() {
        let item: UiListItem = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(item, UiListItem::default());
        assert!(item.enabled);
        assert!(item.text.is_empty());

        let named: UiListItem =
            serde_json::from_value(serde_json::json!({ "text": "BUY" })).unwrap();
        assert!(
            named.enabled,
            "an item authored with only `text` is selectable"
        );

        let round_tripped = serde_json::to_value(&named).unwrap();
        assert_eq!(
            serde_json::from_value::<UiListItem>(round_tripped).unwrap(),
            named
        );
    }

    /// `UiList::validate_semantics` flags a non-positive `size` as `L1118`, pointer `/size`, and adds
    /// nothing for a valid or absent one. (Only zero and negatives are exercised through a JSON
    /// value: a non-finite `f64` has no JSON spelling, so it is unrepresentable in both authoring
    /// paths — the `UiText`/`UiRect` sibling tests take the same posture.)
    #[test]
    fn ui_list_size_semantics() {
        for bad in [0.0, -3.0] {
            let out = UiList::validate_semantics(&serde_json::json!({ "size": bad }));
            assert_eq!(out.len(), 1, "size {bad} must be one diagnostic: {out:?}");
            assert_eq!(out[0].code.as_str(), "L1118");
            assert_eq!(out[0].path.as_deref(), Some("/size"));
        }
        assert!(UiList::validate_semantics(&serde_json::json!({ "size": 24.0 })).is_empty());
        assert!(UiList::validate_semantics(&serde_json::json!({})).is_empty());
    }

    /// A well-formed `UiList` — title, two items, one of them disabled — validates through the real
    /// scene checker: the registry picks the new component up with no schema hand-editing.
    #[test]
    fn good_ui_list_validates_in_a_scene() {
        let registry = builtin_registry();
        let scene = concat!(
            "format: 1\n",
            "entities:\n",
            "  - components:\n",
            "      UiList:\n",
            "        anchor: center\n",
            "        title: WARES\n",
            "        items:\n",
            "          - text: SALVE\n",
            "          - text: RELIC\n",
            "            enabled: false\n",
            "        selected: 1\n",
            "        line_spacing: 4.0\n",
            "        background:\n",
            "          - 0.0\n",
            "          - 0.0\n",
            "          - 0.0\n",
            "          - 0.75\n",
        );
        ludion_scene::check(scene, &registry).expect("a good UiList validates");
    }

    /// A zero `UiList.size` is rejected as `L1118` through the real scene checker, with the pointer
    /// rebased onto the document position.
    #[test]
    fn zero_ui_list_size_is_a_semantic_error() {
        let registry = builtin_registry();
        let bad = "format: 1\nentities:\n  - components:\n      UiList:\n        size: 0.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1118"
                && d.path.as_deref() == Some("/entities/0/components/UiList/size")),
            "expected an L1118 at UiList/size: {diagnostics:?}"
        );
    }

    /// A negative `line_spacing` is the ordinary `L1014` at the field — its lower bound lives in the
    /// schema (the `GiVolume` posture), so it needs no code of its own.
    #[test]
    fn negative_ui_list_line_spacing_is_l1014() {
        let registry = builtin_registry();
        let bad =
            "format: 1\nentities:\n  - components:\n      UiList:\n        line_spacing: -2.0\n";
        let diagnostics = ludion_scene::check(bad, &registry).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1014"
                && d.path.as_deref() == Some("/entities/0/components/UiList/line_spacing")),
            "expected an L1014 at UiList/line_spacing: {diagnostics:?}"
        );
    }

    /// The four built-in entity-reference fields keep their semantics; that every such field is
    /// marked at all is `assert_entity_refs_marked`'s job (`ludion-testkit`).
    #[test]
    fn builtin_entity_references_keep_their_semantics() {
        let registry = builtin_registry();
        for (component, expected) in [
            ("Parent", vec![("", false, false, false)]),
            (
                "Joint",
                vec![("/a", true, false, false), ("/b", true, false, false)],
            ),
            ("PatternPlayer", vec![("/target", false, true, true)]),
            ("Volley", vec![("/source", true, true, true)]),
            ("UiWorldAnchor", vec![("/target", false, false, false)]),
        ] {
            let info = registry.get(component).expect("registered");
            let fields: Vec<(&str, bool, bool, bool)> = info
                .entity_refs()
                .iter()
                .map(|f| (f.suffix.as_str(), f.allow_self, f.optional, f.nullable))
                .collect();
            assert_eq!(
                fields, expected,
                "`{component}`'s entity references changed — did the schema shape change?"
            );
        }
    }

    /// Every registered component's asset-path fields, pinned: a new path field is a deliberate
    /// edit here, and a component not listed holds none.
    #[test]
    fn asset_path_fields_are_pinned() {
        const SHAPES: [&str; 6] = [
            "/shape/assembly",
            "/shape/assembly_part/assembly",
            "/shape/asset",
            "/shape/character",
            "/shape/sculpt",
            "/shape/voxel",
        ];
        let legend: Vec<String> = SHAPES.iter().map(|s| format!("/legend/*{s}")).collect();
        let pinned: Vec<(&str, Vec<String>)> = vec![
            ("Sprite", vec!["/atlas".into(), "/texture".into()]),
            ("Mesh", SHAPES.iter().map(|s| s.to_string()).collect()),
            ("MeshGrid", legend),
            (
                "Tilemap",
                vec!["/legend/*/atlas".into(), "/legend/*/texture".into()],
            ),
            ("UiRect", vec!["/atlas".into(), "/texture".into()]),
            ("AudioEmitter", vec!["/sound".into()]),
            ("PathFollower", vec!["/path".into()]),
            ("LaneConstraint", vec!["/path".into()]),
            ("PatrolFollower", vec!["/route".into()]),
            (
                "CameraRig",
                vec![
                    "/rig/mode/track/path".into(),
                    "/state/base/mode/track/path".into(),
                    "/state/last_applied/mode/track/path".into(),
                ],
            ),
            ("CameraZone", vec!["/rig/mode/track/path".into()]),
            ("PatternPlayer", vec!["/pattern".into()]),
            ("Volley", vec!["/pattern".into()]),
            (
                "Decal",
                vec![
                    "/base_color_texture/path".into(),
                    "/metallic_roughness_texture/path".into(),
                    "/normal_texture/path".into(),
                ],
            ),
            (
                "Collider",
                vec![
                    "/shape/compound/parts/*/shape/mesh/path".into(),
                    "/shape/mesh/path".into(),
                ],
            ),
        ];
        let registry = builtin_registry();
        for info in registry.iter() {
            let expected = pinned
                .iter()
                .find(|(name, _)| *name == info.name())
                .map_or(&[][..], |(_, fields)| fields.as_slice());
            assert_eq!(
                info.asset_paths(),
                expected,
                "`{}`'s asset-path fields changed — mark a new path field with \
                 ludion_ecs::AssetPathSchema and update this table",
                info.name()
            );
        }
        for (name, _) in &pinned {
            assert!(registry.get(name).is_some(), "`{name}` is not registered");
        }
    }

    #[test]
    fn the_asset_path_visitor_reaches_every_legend_entry() {
        let registry = builtin_registry();
        let info = registry.get("MeshGrid").expect("registered");
        let mut value = serde_json::json!({
            "legend": {
                "a": {"shape": {"asset": "a.mesh.yaml"}},
                "b": {"shape": {"voxel": "b.voxel.yaml"}}
            }
        });
        let mut seen = Vec::new();
        info.for_each_asset_path(&mut value, &mut |pointer, path| {
            seen.push((pointer.to_string(), path.clone()));
        });
        assert_eq!(
            seen,
            [
                (
                    "/legend/a/shape/asset".to_string(),
                    "a.mesh.yaml".to_string()
                ),
                (
                    "/legend/b/shape/voxel".to_string(),
                    "b.voxel.yaml".to_string()
                ),
            ]
        );
    }
}
