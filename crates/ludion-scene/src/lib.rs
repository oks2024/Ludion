#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-scene` — strict-subset YAML scene files: parse, validate, canonical emit.
//!
//! Scenes are text — the single source of truth (`engine-pillars.md`). This crate owns all YAML
//! knowledge in the engine: it parses a **strict subset of YAML 1.2** (no anchors/aliases, no
//! tags, no multiple documents, no non-empty flow collections, no block scalars — each rejection
//! is an L-coded [`Diagnostic`] with a JSON Pointer and a fix), validates component values
//! against the [`Registry`]'s per-component JSON Schemas, and writes scenes back through a
//! **canonical emitter** (stable key order, entity order preserved, ryu shortest-round-trip
//! floats, ambiguous scalars always quoted, LF endings). It also owns [`instantiate`] — scene →
//! [`World`] — because it is the one crate that knows both the scene document and the ECS.
//!
//! Boundary rules (`engine-architecture.md` §2): `ludion-ecs` never learns YAML exists — this
//! crate consumes the registry's schemas and erased serde functions, and never defines a
//! component.
//!
//! # The scene format (version 1)
//!
//! ```yaml
//! format: 1
//! name: "level-1"      # optional
//! seed: 42             # optional, seeds the sim's DeterministicRng
//! entities:
//!   - name: "paddle"   # optional
//!     components:
//!       Transform:
//!         x: 1.5
//!         y: 2.0
//!   - components: {}
//! ```
//!
//! # The prefab format (version 1)
//!
//! `kind: prefab` — a reusable group of named entities. The body is a scene's `entities:` list,
//! parsed and validated by the same code, with entity references ranged over the prefab's own parts;
//! every part is named, uniquely, with no `/`. A prefab carries no `seed` or `physics`. An entry with
//! a `prefab:` key is an [`InstanceEntry`] — a placed, overridable copy of another prefab — in a
//! scene or a prefab alike.
//! [`parse_prefab`], [`validate_prefab`], [`check_prefab`] and [`emit_prefab`] are the scene
//! functions' prefab twins.
//!
//! # The mesh asset format (version 1)
//!
//! `meshes/*.mesh.yaml` — a mesh document is not part of a scene; it is a separate strict-YAML
//! document a `Mesh` component's `asset` variant refers to by path. This crate parses, validates,
//! and emits it; `ludion-engine` resolves and loads it. `ludion check-scene` dispatches by the
//! top-level `kind: mesh` key ([`peek_kind`]); a scene document has no `kind` key at all.
//!
//! ```yaml
//! format: 1
//! kind: mesh
//! name: "rock"     # optional
//! positions:        # required, 3 floats per vertex
//!   - 0.0
//!   - 0.0
//!   - 0.0
//! normals:           # optional; omitted means "compute faceted normals at load"
//!   - 0.0
//!   - 1.0
//!   - 0.0
//! uvs: []            # optional, 2 floats per vertex, stored losslessly
//! indices:           # required, a flat triangle list, 3 indices per triangle
//!   - 0
//!   - 0
//!   - 0
//! ```
//!
//! # Example
//!
//! ```
//! use ludion_ecs::Registry;
//! use ludion_scene::{Scene, SceneEntity, check, emit, parse, validate};
//! use schemars::JsonSchema;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize, JsonSchema)]
//! struct Transform { x: f32, y: f32 }
//!
//! let mut registry = Registry::new();
//! registry.register::<Transform>("Transform").unwrap();
//!
//! // Parse + validate in one step (what `ludion check-scene` runs):
//! let source = "format: 1\nentities:\n  - components:\n      Transform:\n        x: 1.5\n        y: 2.0\n";
//! let scene = check(source, &registry).expect("the scene is valid");
//! assert_eq!(scene.entities.len(), 1);
//!
//! // The canonical emitter is idempotent: emitting what we parsed reproduces the bytes.
//! let canonical = emit(&scene, &registry);
//! assert_eq!(canonical, source);
//!
//! // A typo'd field comes back as an L1014 diagnostic with a JSON Pointer and a suggestion:
//! let broken = "format: 1\nentities:\n  - components:\n      Transform:\n        x: 1.5\n        yy: 2.0\n";
//! let diagnostics = check(broken, &registry).unwrap_err();
//! let field = diagnostics.iter().find(|d| d.message.contains("yy")).unwrap();
//! assert_eq!(field.code.as_str(), "L1014");
//! assert_eq!(field.path.as_deref(), Some("/entities/0/components/Transform/yy"));
//! assert_eq!(field.help.as_deref(), Some("nearest match is `y`"));
//! ```

use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Registry, World};
use serde_json::Value;

mod animation;
mod assembly;
mod atlas;
mod blend_space;
mod character;
mod clip_eval;
mod doc;
mod emit;
mod envelope;
mod expand;
mod fields;
mod font;
mod font_source;
mod fonts;
mod generation;
mod instance;
mod instantiate;
mod kinds;
mod lod;
mod map;
mod mesh;
mod mesh_edit;
mod overrides;
mod paired;
mod palette;
mod parse;
mod part;
mod path;
mod patrol;
mod physics;
mod physics_layers;
mod physics_surfaces;
mod prefab;
mod profile;
mod render_layers;
mod rig_blend;
mod rig_ik;
mod rig_mask;
mod rim;
mod scalar;
mod schema;
mod sculpt;
mod sprite;
mod validate;
mod voxel;
mod voxel_edit;
mod voxel_gen;
mod yaml;

pub use animation::{
    ANIMATION_FORMAT_VERSION, AnimationDocument, AnimationEvent, AnimationTrack, Ease, Keyframe,
    ROOT_TRACK,
};
pub use assembly::{
    ASSEMBLY_FORMAT_VERSION, AssembledGrid, AssemblyClip, AssemblyDocument, AssemblyJoint,
    AssemblyJointKind, AssemblyPartEntry, overlap_lint as assembly_overlap_lint,
};
pub use atlas::{
    ATLAS_FORMAT_VERSION, ATLAS_SOURCE_FORMAT_VERSION, AtlasDocument, AtlasFilter, AtlasRegion,
    AtlasSourceDocument, AtlasSourceInput, Flipbook,
};
pub use blend_space::{
    BlendWeights, MAX_BLEND_POINTS, blend_weights_1d, blend_weights_2d, triangulate,
};
pub use character::{CHARACTER_FORMAT_VERSION, CharacterClip, CharacterDocument};
pub use clip_eval::{
    ClipSampler, PartPose, clip_phase, flipbook_frame, joint_pose, part_pose, sample_channel,
    sample_rotation, slerp, track as clip_track,
};
pub use expand::{
    ExpandedInstance, ExpandedScene, identical, merge_partial, mixed_root_diagnostic, partial_diff,
};
pub use font::{FONT_FORMAT_VERSION, FontAtlas, FontDocument, FontGlyph, FontMode, FontNotdef};
pub use font_source::{
    CharsetRange, FONT_SOURCE_FORMAT_VERSION, FontCharset, FontSourceDocument, MAX_CHARSET_CHARS,
};
pub use fonts::{FONTS_FORMAT_VERSION, FontsDocument};
pub use generation::{
    GENERATION_DEFAULT_COUNT, GENERATION_FORMAT_VERSION, GENERATION_MAX_COUNT, GenerationDocument,
    InputValue, ORIGIN_FORMAT_VERSION, Origin, OriginImage, ScalarValue,
};
pub use instance::{InstanceEntry, InstanceTransform, normalize_prefab_path};
pub use instantiate::{InstanceHooks, InstantiateOptions};
pub use kinds::{KINDS, KindCheck, KindEntry, KindSniff, kind_entry};
pub use map::{
    ColliderSpec, KitPrimitive, MAP_FORMAT_VERSION, MapDoc, MapGrid, MapKit, MapMarker,
    MapOverrides, MapProbe, MarkerCell, MeshSpec, Mirror, box_collider_name, generate,
    generate_traced, parse_map, parse_map_kit,
};
pub use mesh::{
    AlphaMode, DEFAULT_ALPHA_CUTOFF, DEGENERATE_VECTOR_LENGTH_SQUARED, JointRest, MAX_MESH_LODS,
    MESH_FORMAT_VERSION, MESH_JOINT_LIMIT, MESH_VERTEX_BUDGET, MeshDocument, MeshJoint, MeshLodRef,
    MeshSkin, MeshSurfaces, MeshTextures, SKIN_WEIGHT_SUM_TOLERANCE, TextureRef, TextureWrap,
    check_lod_level, read_mesh_joints, rest_model_matrices,
};
pub use mesh_edit::{
    MESH_EDIT_FORMAT_VERSION, MeshEditDocument, MeshEditKey, MeshEditMap, MeshEditOp,
    MeshEditRegion, MeshEditRegionShape,
};
pub use overrides::{AppliedOverride, SceneOverride, apply_overrides, parse_sweep};
pub use paired::{
    PAIRED_FORMAT_VERSION, PairedAlign, PairedContact, PairedDocument, PairedEvent, PairedMoves,
    PairedOffset, PairedRole,
};
pub use palette::{PALETTE_FORMAT_VERSION, PaletteDocument};
pub use part::{PART_FORMAT_VERSION, PartDocument};
pub use path::{
    PATH_FORMAT_VERSION, PathDocument, PathFrame, PathInterpolation, PathMetric, PathSampler,
};
pub use patrol::{PATROL_FORMAT_VERSION, PatrolDocument, PatrolLook, PatrolMode, PatrolPoint};
pub use physics::{ScenePhysics, ScenePhysicsSpace, validate_physics};
pub use physics_layers::{DEFAULT_LAYER, LayerMatrix, MAX_LAYERS, SceneLayer};
pub use physics_surfaces::MAX_SURFACES;
pub use prefab::{PREFAB_FORMAT_VERSION, PrefabDocument};
pub use profile::ProfileEnvelope;
pub use render_layers::{DEFAULT_RENDER_LAYER, MAX_RENDER_LAYERS, SceneRenderLayer};
pub use rig_blend::{BlendAt, BlendPoint, BlendSpace};
pub use rig_ik::{
    DEFAULT_LOOK_FORWARD, DEFAULT_LOOK_LIMITS_DEG, IkEntry, IkGround, IkHinge, IkKind,
};
pub use rig_mask::RigMask;
pub use rim::DocRim;
pub use sculpt::{
    SCULPT_DEFAULT_RESOLUTION, SCULPT_DEFAULT_TRIANGLES, SCULPT_FORMAT_VERSION, SCULPT_MAX_REPEAT,
    SCULPT_MAX_RESOLUTION, SCULPT_MIN_RESOLUTION, SCULPT_MIN_TRIANGLES, SculptAxis, SculptBone,
    SculptDeform, SculptDocument, SculptNoise, SculptOp, SculptPart, SculptRepeat, SculptShape,
    SculptTaper, SculptTurn, apply_sculpt_overrides,
};
pub use sprite::{
    GradientBlend, GradientDirection, GradientMode, LightMode, OutlineSide, SPRITE_EMPTY_GLYPH,
    SPRITE_FORMAT_VERSION, SPRITE_GLYPH_BUDGET, SPRITE_MAX_SIZE, ShadeMode, SpriteDocument,
    SpriteEffect, SpriteEffectOp, SpriteFrame, SpriteFrameGrid, SpriteMirror, SpritePaletteEntry,
    SpritePaletteInclude, SpritePatch,
};
pub use validate::HierarchyCycle;
pub use voxel::{
    ConnectivityComponent, ConnectivityGap, EmissiveFaces, ExposureReport, ExposureView,
    GAME_VIEW_PITCH_DEG, GameViewFaces, GlyphExposure, MirrorRegion, PaletteEntry,
    VOXEL_CELL_BUDGET, VOXEL_FORMAT_VERSION, VOXEL_MAX_CELLS, VoxelDocument, VoxelLod,
    apply_assembly_palette_overrides, apply_palette_overrides,
};
pub use voxel_edit::{
    ExtractedComments, MirrorKeep, StrayComment, VOXEL_OPS_FORMAT_VERSION, VoxelAxis,
    VoxelComments, VoxelEdit, VoxelOp, VoxelUpscale, WriteMode,
};

/// The scene format version this crate reads and writes.
pub const FORMAT_VERSION: u64 = 1;

/// A parsed scene document: the in-memory form of one scene file.
///
/// Component values are held as [`serde_json::Value`]s plus registry names — the same erased
/// shape the wire protocol uses — so this crate never needs to know a concrete component type.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Scene {
    /// Optional human-readable scene name.
    pub name: Option<String>,
    /// Optional provenance marker: the map source file this scene was generated from (the
    /// `map.rs` compiler sets it to the `.map.yaml` file name). Emitted after `name:`, serde-default
    /// so hand-authored scenes omit it. It marks the file as **generated — edit the source, not
    /// this** for humans and agents, and doubles as the drift gate's discovery mechanism (CI
    /// regenerates every scene bearing the field and asserts byte-identity). Round-trips through
    /// parse→emit.
    pub generated_from: Option<String>,
    /// Optional seed for the sim's `DeterministicRng`.
    pub seed: Option<u64>,
    /// The scene's optional `physics:` block (`Plan/physics.md` W2). Present means every host
    /// that builds a world from the scene installs the rigid-body write/step/mirror triple;
    /// absent means exactly today — no physics, no hash moves. A rigid body that omits its own
    /// `sleep_after_ticks` resolves it against this block's value (else the pinned default) at
    /// body-creation time, in the solver, never by rewriting the authored component.
    pub physics: Option<ScenePhysics>,
    /// The scene's `render_layers:` block (`Plan/secondary-views.md` D14): the render layer names it
    /// authors, declared once. Validation only: `check-scene` reports `L2376` for an undeclared name,
    /// extract still interns names per frame. Emitted after `physics`, omitted when empty.
    pub render_layers: Vec<SceneRenderLayer>,
    /// Prefab paths (content-root relative) the engine readies at every load of this scene, so a
    /// later `spawn_prefab` of one reads no file. No meaning beyond that; document order is kept.
    pub preload: Vec<String>,
    /// The entities, in document order (entity order is data and is preserved by the emitter).
    pub entities: Vec<SceneEntity>,
}

/// One entity of a [`Scene`].
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SceneEntity {
    /// Optional human-readable entity name.
    pub name: Option<String>,
    /// Spawned from the presentation allocator: drawn, never hashed, invisible to sim systems;
    /// `Plan/presentation-state.md`. Emitted after `name:`, omitted when false.
    pub presentation: bool,
    /// `(component name, component value)` pairs, in document order. The canonical emitter
    /// reorders them into registry registration order on write.
    pub components: Vec<(String, Value)>,
    /// `Some` when the entry is a prefab instance (`prefab:` key): `components` is then empty and
    /// `presentation` false; the instance's changes live in the entry. The emitter still writes
    /// any components, so the parser refuses the text (`L1231`) instead of the data vanishing.
    pub instance: Option<Box<InstanceEntry>>,
}

/// Parses strict-subset YAML into a [`Scene`].
///
/// Syntax and strict-subset violations fail fast (one diagnostic); scene-structure problems are
/// collected, so every fixable error comes back in one pass.
///
/// # Errors
/// `L1001`–`L1012` per the crate docs; each diagnostic carries a JSON Pointer into the document
/// where applicable, plus a suggested fix.
///
/// # Example
/// ```
/// let scene = ludion_scene::parse("format: 1\nentities: []\n").unwrap();
/// assert!(scene.entities.is_empty());
///
/// let diagnostics = ludion_scene::parse("format: 1\nentites: []\n").unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L1010");
/// assert_eq!(diagnostics[0].help.as_deref(), Some("nearest match is `entities`"));
/// ```
pub fn parse(source: &str) -> Result<Scene, Vec<Diagnostic>> {
    parse::parse(source)
}

/// Validates every component in `scene` against `registry`'s per-component JSON Schemas.
///
/// Returns all diagnostics found; an empty vector means the scene is valid. Unknown component
/// names are `L1013`, schema mismatches are `L1014` — both point at the exact offending node.
///
/// # Example
/// ```
/// use ludion_ecs::Registry;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Health { hp: u32 }
///
/// let mut registry = Registry::new();
/// registry.register::<Health>("Health").unwrap();
///
/// let scene = ludion_scene::parse(
///     "format: 1\nentities:\n  - components:\n      Health:\n        hp: -3\n",
/// )
/// .unwrap();
/// let diagnostics = ludion_scene::validate(&scene, &registry);
/// assert_eq!(diagnostics[0].code.as_str(), "L1014");
/// assert_eq!(diagnostics[0].path.as_deref(), Some("/entities/0/components/Health/hp"));
/// ```
pub fn validate(scene: &Scene, registry: &Registry) -> Vec<Diagnostic> {
    let mut diagnostics = validate::validate(scene, registry);
    diagnostics.extend(validate::check_scene_asset_paths(scene, registry));
    diagnostics
}

/// Schema-checks one component value — the `L1014` check [`validate`] runs per scene component, for
/// a value that arrives outside a scene (`component.set`, `entity.spawn`).
///
/// Pointers are relative to the component (`/translaton`), there being no document position. An
/// unregistered `name` yields nothing (the insert reports `L0004`); semantic checks
/// (`ComponentInfo::validate_semantics`) are separate and run after this one is clean.
///
/// # Example
/// ```
/// use ludion_ecs::Registry;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Health { hp: u32 }
///
/// let mut registry = Registry::new();
/// registry.register::<Health>("Health").unwrap();
///
/// let diagnostics =
///     ludion_scene::check_component("Health", &serde_json::json!({ "hp": 3, "hpp": 3 }), &registry);
/// assert_eq!(diagnostics[0].code.as_str(), "L1014");
/// assert_eq!(diagnostics[0].path.as_deref(), Some("/hpp"));
/// ```
pub fn check_component(
    name: &str,
    value: &serde_json::Value,
    registry: &Registry,
) -> Vec<Diagnostic> {
    validate::check_component(name, value, registry)
}

/// Checks the entity references inside one component value that arrives outside a scene
/// (`component.set`, `entity.spawn`), by id rather than document position: `L1017` for a field that
/// forbids naming its carrier and does, `L1036` for a sim carrier naming a presentation entity.
///
/// `carrier` is the entity being written, or `None` for an `entity.spawn` (not yet alive).
/// `presentation_carrier` says whether the carrier is a presentation entity (for a spawn, whether
/// it is flagged `presentation`): only a presentation carrier may name a presentation entity.
/// Tree links (`Parent`) are not checked here: their rules need the world. A dead or
/// unparseable reference is not reported (`scene.save`'s `L1018` catches a dangling one). Pointers
/// are relative to the component, like [`check_component`]'s.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_ecs::{EntityRefSchema, Registry};
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct Leash {
///     #[schemars(with = "EntityRefSchema")]
///     holder: EntityId,
/// }
///
/// let mut registry = Registry::new();
/// registry.register::<Leash>("Leash").unwrap();
/// let me = EntityId::new(5, 0);
/// let value = serde_json::json!({ "holder": { "index": 5, "generation": 0 } });
/// let diagnostics = ludion_scene::check_component_refs("Leash", &value, &registry, Some(me), false);
/// assert_eq!(diagnostics[0].code.as_str(), "L1017");
/// assert_eq!(diagnostics[0].path.as_deref(), Some("/holder/index"));
/// ```
pub fn check_component_refs(
    name: &str,
    value: &serde_json::Value,
    registry: &Registry,
    carrier: Option<ludion_core::ids::EntityId>,
    presentation_carrier: bool,
) -> Vec<Diagnostic> {
    validate::check_component_refs(name, value, registry, carrier, presentation_carrier)
}

/// Finds the cycles in the scene's tree links (`Parent`), each once, starting at its lowest
/// member's document position — the same walk that gives [`validate`] its `L1035`.
///
/// Reads only well-formed links (an in-range `index` that is not the entity's own, `generation` 0);
/// which fields are tree links is the registry's, from their schema marker. `scene.save` calls this
/// on the scene it is about to write.
///
/// # Example
/// ```
/// use ludion_ecs::{Parent, Registry};
///
/// let mut registry = Registry::new();
/// registry.register::<Parent>("Parent").unwrap();
/// let link = |to: u32| format!("      Parent:\n        index: {to}\n        generation: 0\n");
/// let source = format!(
///     "format: 1\nentities:\n  - components:\n{}  - components:\n{}",
///     link(1),
///     link(0)
/// );
/// let scene = ludion_scene::parse(&source).unwrap();
/// let cycles = ludion_scene::hierarchy_cycles(&scene, &registry);
/// assert_eq!(cycles.len(), 1);
/// assert_eq!(cycles[0].members, [0, 1]);
/// ```
pub fn hierarchy_cycles(scene: &Scene, registry: &Registry) -> Vec<HierarchyCycle> {
    validate::hierarchy_cycles(scene, registry)
}

/// Parses and validates in one step: the engine's `scene.check` and the CLI's `check-scene`.
///
/// # Errors
/// Everything [`parse`] and [`validate`] can report.
///
/// # Example
/// ```
/// let registry = ludion_ecs::Registry::new();
/// assert!(ludion_scene::check("format: 1\nentities: []\n", &registry).is_ok());
/// assert!(ludion_scene::check("format: 1\nentities: {}\n", &registry).is_err());
/// ```
pub fn check(source: &str, registry: &Registry) -> Result<Scene, Vec<Diagnostic>> {
    let scene = parse(source)?;
    let diagnostics = validate(&scene, registry);
    if diagnostics.is_empty() {
        Ok(scene)
    } else {
        Err(diagnostics)
    }
}

/// Builds a fresh [`World`] from a checked [`Scene`]: one entity per scene entity, spawned in
/// document order (a `presentation: true` entity from the presentation allocator), each component
/// inserted via the registry's erased deserialization. Entity references are document positions in
/// the file and spawned indices in the world; [`instantiated_ids`] is the mapping.
///
/// Borrows the scene rather than consuming it — instantiation copies component values into the
/// world (the erased deserializer clones anyway), and callers routinely still need the scene
/// afterwards (its `seed` configures the sim, its text form is what gets re-emitted).
///
/// **Run [`check`] first.** This function deliberately does not re-validate: parse + validate +
/// instantiate is one pipeline, and each stage runs once (`ludion sim-hash` is exactly
/// `check` → `instantiate`). An unchecked scene still fails *safely* — the erased deserializer
/// enforces component shape — but with registry-level diagnostics (`L0004`/`L0006`) instead of
/// [`check`]'s richer schema diagnostics with suggestions.
///
/// Entity `name`s are documentation for humans and diffs at *this* layer: this function does not
/// consult them for component data. But they do not stay hash-invisible end to end — since Phase 6,
/// `ludion-engine`'s scene-loading path projects each name into a built-in `Name` component after
/// calling this function, and a component in the world is part of its `WorldHash`.
///
/// # Errors
/// All failures found, each an `L0004` (unknown component) or `L0006` (value does not
/// deserialize) diagnostic carrying a JSON Pointer to the offending component
/// (`/entities/3/components/Sprite`).
///
/// # Example
/// ```
/// use ludion_ecs::Registry;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug)]
/// struct Transform { x: f32, y: f32 }
///
/// let mut registry = Registry::new();
/// registry.register::<Transform>("Transform").unwrap();
///
/// let source = "format: 1\nentities:\n  - components:\n      Transform:\n        x: 1.5\n        y: 2.0\n";
/// let scene = ludion_scene::check(source, &registry).unwrap();
///
/// let world = ludion_scene::instantiate(&scene, &registry).unwrap();
/// assert_eq!(world.entity_count(), 1);
/// let entity = world.entities().next().unwrap();
/// assert_eq!(
///     world.get::<Transform>(entity).unwrap(),
///     Some(&Transform { x: 1.5, y: 2.0 })
/// );
/// ```
pub fn instantiate(scene: &Scene, registry: &Registry) -> Result<World, Vec<Diagnostic>> {
    instantiate::instantiate(scene, registry)
}

/// Spawns `entities` into an existing [`World`]: the one instantiation path ([`instantiate`] is its
/// fresh-world wrapper). Returns the new ids in document order.
///
/// Every marked entity reference is remapped from a document position to the spawned id, index and
/// generation. A root (a part holding no in-group `hierarchy` link, i.e. no `Parent`) takes
/// `options.placement` baked onto its pose, with no synthetic root; named parts are named
/// `<prefix>/<part>` with a prefix, bare without, through `options.hooks`.
///
/// All-or-nothing: every component is deserialized in a scratch world first, so a failure leaves
/// `world` untouched. Like [`instantiate`], this does not re-validate: run [`check`] first.
///
/// # Errors
/// Every component failure (`L0004`/`L0006`, pointer `/entities/3/components/Sprite`), and `L0028`
/// (pointer `/entities/3/presentation`) for a part of the wrong allocator for the world's current
/// phase — a sim system cannot spawn a presentation part, nor a presentation system a sim one.
///
/// # Example
/// ```
/// use ludion_ecs::Registry;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
/// use ludion_scene::{InstantiateOptions, InstanceHooks};
///
/// #[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug)]
/// struct Leash {
///     #[schemars(with = "ludion_ecs::EntityRefSchema")]
///     to: ludion_core::ids::EntityId,
/// }
///
/// struct NoOp;
/// impl InstanceHooks for NoOp {
///     fn name(&self, _: &mut ludion_ecs::World, _: ludion_core::ids::EntityId, _: &str) {}
///     fn place(&self, _: &mut ludion_ecs::World, _: ludion_core::ids::EntityId, _: &ludion_core::math::Trs) {}
/// }
///
/// let mut registry = Registry::new();
/// registry.register::<Leash>("Leash").unwrap();
/// let source = "format: 1\nentities:\n  - components: {}\n  - components:\n      Leash:\n        to:\n          index: 0\n          generation: 0\n";
/// let scene = ludion_scene::check(source, &registry).unwrap();
///
/// let mut world = ludion_ecs::World::new();
/// let options = InstantiateOptions { placement: None, name_prefix: None, hooks: &NoOp };
/// let first = ludion_scene::instantiate_into(&mut world, &scene.entities, &registry, &options).unwrap();
/// let second = ludion_scene::instantiate_into(&mut world, &scene.entities, &registry, &options).unwrap();
/// assert_eq!(second[0].index(), 2);
/// assert_eq!(world.get::<Leash>(first[1]).unwrap().unwrap().to, first[0]);
/// assert_eq!(world.get::<Leash>(second[1]).unwrap().unwrap().to, second[0]);
/// ```
pub fn instantiate_into(
    world: &mut World,
    entities: &[SceneEntity],
    registry: &Registry,
    options: &InstantiateOptions<'_>,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    instantiate::instantiate_into(world, entities, registry, options)
}

/// Expands a document's prefab-instance entries into a flat scene.
///
/// `resolve` maps a prefab path, as written in the entry, to that file's text (this crate reads no
/// file); it builds its own `L1230` for a missing one. Nested instances expand depth-first and parts
/// are named `<instance>/<part path>`. A reference to an instance entry lands on its first surviving
/// part; an entry's `overrides` merge per part (objects key by key, anything else replaces, a
/// missing component is added) and its `removed` drops parts. `scene` must have passed [`check`]; a
/// scene without instance entries expands to an equal clone.
///
/// # Errors
/// The resolver's `L1230`, the prefab's own codes (named `` in `<path>`: `` with one `related`
/// diagnostic per enclosing instance), `L1232` for a prefab loop, `L1233` for a bad part path in
/// `overrides`/`removed`, `L1014` for a merged override that breaks its component's schema, and
/// `L1035` for a parent loop closed across levels.
///
/// # Example
/// ```
/// use std::collections::BTreeMap;
/// use ludion_ecs::{Parent, Registry};
///
/// let mut registry = Registry::new();
/// registry.register::<Parent>("Parent").unwrap();
/// let files = BTreeMap::from([(
///     "lamp.prefab.yaml",
///     "format: 1\nkind: prefab\nentities:\n  - name: post\n    components: {}\n  - name: bulb\n    components:\n      Parent:\n        index: 0\n        generation: 0\n",
/// )]);
/// let source = "format: 1\nentities:\n  - name: plain\n    components: {}\n  - name: lamp-1\n    prefab: lamp.prefab.yaml\n  - name: ref\n    components:\n      Parent:\n        index: 1\n        generation: 0\n";
/// let scene = ludion_scene::check(source, &registry).unwrap();
/// let expanded = ludion_scene::expand(&scene, &registry, &mut |path| {
///     Ok(files[path].to_string())
/// })
/// .unwrap();
/// assert_eq!(expanded.scene.entities.len(), 4);
/// assert_eq!(expanded.scene.entities[2].name.as_deref(), Some("lamp-1/bulb"));
/// // The scene's reference to the entry lands on its first part.
/// assert_eq!(expanded.scene.entities[3].components[0].1["index"], 1);
/// ```
pub fn expand(
    scene: &Scene,
    registry: &Registry,
    resolve: &mut dyn FnMut(&str) -> Result<String, Diagnostic>,
) -> Result<ExpandedScene, Vec<Diagnostic>> {
    expand::expand(scene, registry, resolve)
}

/// Spawns an [`ExpandedScene`] into an existing [`World`], like [`instantiate_into`] on its flat
/// entities, then places each instance: a part with no `Parent` link inside its instance takes that
/// instance's `transform` through `options.hooks`, nested instances before the ones holding them, so
/// a nested root ends at `outer ∘ inner ∘ local`. `options.placement` then bakes onto the whole
/// group's roots. Provenance for scene-level instances is the caller's.
///
/// # Errors
/// As [`instantiate_into`]; all-or-nothing.
pub fn instantiate_expanded(
    world: &mut World,
    expanded: &ExpandedScene,
    registry: &Registry,
    options: &InstantiateOptions<'_>,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    expand::instantiate_expanded(world, expanded, registry, options)
}

/// A copy of `expanded` whose every part spawns from the presentation allocator, for a runtime
/// `presentation` spawn of the prefab named `prefab`.
///
/// # Errors
/// `L1320` for each sim part carrying a sim-only component (see `Registry::mark_sim_only`).
///
/// # Example
/// ```
/// let registry = ludion_ecs::Registry::new();
/// let scene = ludion_scene::parse("format: 1\nentities:\n  - components: {}\n").unwrap();
/// let expanded = ludion_scene::expand(&scene, &registry, &mut |_| unreachable!()).unwrap();
/// let copy = ludion_scene::presentation_copy(&expanded, &registry, "p.prefab.yaml").unwrap();
/// assert!(copy.scene.entities[0].presentation);
/// ```
pub fn presentation_copy(
    expanded: &ExpandedScene,
    registry: &Registry,
    prefab: &str,
) -> Result<ExpandedScene, Vec<Diagnostic>> {
    expand::presentation_copy(expanded, registry, prefab)
}

/// The [`EntityId`] [`instantiate`] gives each scene entity of a *fresh* world, indexed by document
/// position.
///
/// A sim entity takes the next sim slot and a `presentation: true` one the next slot of the
/// presentation range, so once the two kinds interleave a document position is no longer the entity
/// index. Anything mapping a document position to a live entity (names, spawn poses) goes through
/// this function.
///
/// # Example
/// ```
/// let scene = ludion_scene::parse(
///     "format: 1\nentities:\n  - components: {}\n  - presentation: true\n    components: {}\n  - components: {}\n",
/// )
/// .unwrap();
/// let ids = ludion_scene::instantiated_ids(&scene);
/// assert_eq!(ids[0].index(), 0);
/// assert_eq!(ids[1].index(), ludion_ecs::PRESENTATION_BASE);
/// assert_eq!(ids[2].index(), 1);
/// ```
pub fn instantiated_ids(scene: &Scene) -> Vec<EntityId> {
    instantiate::instantiated_ids(scene)
}

/// Writes `scene` in canonical form (see the crate docs for the canonical-form rules).
///
/// The registry supplies the canonical component order (registration order) and per-component
/// field order (schema/struct declaration order); names the registry does not know keep their
/// document order after the known ones. Emission never fails and never loses data — floats are
/// written in ryu shortest-round-trip form, never rounded.
///
/// # Example
/// ```
/// let registry = ludion_ecs::Registry::new();
/// let scene = ludion_scene::Scene { name: Some("demo".into()), ..Default::default() };
/// assert_eq!(
///     ludion_scene::emit(&scene, &registry),
///     "format: 1\nname: demo\nentities: []\n"
/// );
/// ```
pub fn emit(scene: &Scene, registry: &Registry) -> String {
    emit::emit(scene, registry)
}

// --- Mesh asset documents (`meshes/*.mesh.yaml`) -----------------------------------------------
//
// A mesh document has a fixed, small shape — unlike a scene, it is not validated against the
// component registry, so it has no `Registry` parameter. See the [`mesh`] module docs for the
// format and `Plan/phase-8-assets-and-sound.md` for the design decisions.

/// Sniffs a document's top-level `kind:` key, for `ludion check-scene`'s dispatch between scene
/// and mesh documents.
///
/// Returns `None` when the document has no `kind` key (a scene document — the scene format's own
/// `L1010` unknown-key rule keeps `kind` out of the scene skeleton) *or* when the document could
/// not even be sniffed (empty, not a mapping, a YAML syntax error). In every `None` case the
/// caller should fall through to [`parse`]/[`check`], whose diagnostics explain the real problem
/// when there is one.
///
/// # Example
/// ```
/// assert_eq!(ludion_scene::peek_kind("format: 1\nkind: mesh\n").as_deref(), Some("mesh"));
/// assert_eq!(ludion_scene::peek_kind("format: 1\nentities: []\n"), None);
/// ```
pub fn peek_kind(source: &str) -> Option<String> {
    kinds::peek_kind(source)
}

/// Sniffs a document's top-level `kind:` key like [`peek_kind`], but keeps a strict-YAML parse
/// failure observable instead of collapsing it into "no kind" — for a content-root walk
/// (`ludion-engine`'s `load_doc_stores`/`reload_doc_from_disk`), where a doc the reader rejects
/// (a syntax error, an anchor, a non-empty flow collection, ...) must be reported (`L100x`), never
/// silently absent from its `DocStore`.
///
/// `Ok(None)` for no `kind` key, an empty document, or a non-mapping document — every one of
/// those is legitimately "not this kind". `Err` carries the reader's own diagnostic when the
/// source does not parse as strict-subset YAML at all.
///
/// # Example
/// ```
/// assert_eq!(
///     ludion_scene::peek_kind_checked("format: 1\nkind: mesh\n")
///         .unwrap()
///         .as_deref(),
///     Some("mesh")
/// );
/// assert!(ludion_scene::peek_kind_checked("a: { b: 1 }\n").is_err());
/// ```
pub fn peek_kind_checked(source: &str) -> Result<Option<String>, Diagnostic> {
    kinds::peek_kind_checked(source)
}

/// A cheap textual read of a document's top-level `kind`, so a content-root walk can route a file
/// without the strict-YAML parse. Never changes an answer, only skips work: [`KindSniff::Absent`]
/// when no line could be a top-level `kind` key (`peek_kind_checked` is `Ok(None)` or `Err`),
/// [`KindSniff::Kind`] when exactly one is plainly decodable (`Ok(Some(k))` or `Err`), and
/// [`KindSniff::Undecided`] for anything else (an explicit `?` key, node properties, escapes,
/// multi-line or non-string values, a second `kind`): the caller full-parses.
///
/// A file a walk skips on `Absent` or an engine `Kind` is not strict-YAML-checked by it — the scene
/// loader, or the kind's own loader, reports it when it loads.
///
/// # Example
/// ```
/// use ludion_scene::{KindSniff, sniff_kind};
/// assert_eq!(sniff_kind("format: 1\nkind: mesh\n"), KindSniff::Kind("mesh"));
/// assert_eq!(sniff_kind("format: 1\nentities: []\n"), KindSniff::Absent);
/// assert_eq!(sniff_kind("light:\n  kind: point\n"), KindSniff::Absent); // nested, not top-level
/// assert_eq!(sniff_kind("kind: [mesh]\n"), KindSniff::Undecided);
/// ```
pub fn sniff_kind(source: &str) -> KindSniff<'_> {
    kinds::sniff_kind(source)
}

/// Parses strict-subset YAML into a [`MeshDocument`], checking structure/shape only.
///
/// # Errors
/// `L1110` for a missing/wrong-typed key, an unrecognized `kind`, or an unknown key; `L1190` for a
/// malformed skin (`joints`, `weights`, `bone_indices`) — each diagnostic carries a JSON Pointer
/// into the document where applicable.
pub fn parse_mesh(source: &str) -> Result<MeshDocument, Vec<Diagnostic>> {
    mesh::parse(source)
}

/// Validates data-consistency invariants of `doc`: flat-list counts are the right multiple,
/// cross-list counts agree, every index is in range, and every float is finite.
///
/// # Errors (returned, not `Result`)
/// `L1111` diagnostics, and `L1191` for an inconsistent skin; an empty vector means `doc` is
/// consistent.
pub fn validate_mesh(doc: &MeshDocument) -> Vec<Diagnostic> {
    mesh::validate(doc)
}

/// Parses and validates a mesh document in one step: the CLI's `check-scene` mesh path. The
/// document's `surfaces`/`triangle_surfaces` tags come back in [`MeshDocument::surfaces`], validated
/// (`L1696`).
///
/// # Errors
/// Everything [`parse_mesh`] and [`validate_mesh`] can report, plus `L1696` for a malformed tag list.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: mesh
/// positions:
///   - 0.0
///   - 0.0
///   - 0.0
///   - 1.0
///   - 0.0
///   - 0.0
///   - 0.0
///   - 1.0
///   - 0.0
/// surfaces:
///   - concrete
///   - metal_grate
/// triangle_surfaces:
///   - - 1
///     - 1
/// indices:
///   - 0
///   - 1
///   - 2
/// ";
/// let mesh = ludion_scene::check_mesh(source).expect("the mesh is valid");
/// let surfaces = mesh.surfaces.as_ref().expect("the mesh is tagged");
/// assert_eq!(surfaces.per_triangle(), vec![1]);
/// assert_eq!(ludion_scene::emit_mesh(&mesh), source);
/// ```
pub fn check_mesh(source: &str) -> Result<MeshDocument, Vec<Diagnostic>> {
    let doc = parse_mesh(source)?;
    let diagnostics = validate_mesh(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// The non-fatal style vertex-budget warning (`L1112`) for `doc`, above [`MESH_VERTEX_BUDGET`]
/// vertices. `None` when under budget. This never fails [`check_mesh`] — callers that want to
/// surface it (e.g. `ludion check-scene`) print it separately and still exit success.
pub fn mesh_budget_warning(doc: &MeshDocument) -> Option<Diagnostic> {
    mesh::budget_warning(doc)
}

/// Writes `doc` in canonical form (ryu shortest-round-trip floats, stable field order, LF
/// endings). Byte-idempotent: `emit_mesh(parse_mesh(emit_mesh(d)).unwrap())` reproduces the same
/// text (property-tested).
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: mesh
/// positions:
///   - 0.0
///   - 0.0
///   - 0.0
///   - 1.0
///   - 0.0
///   - 0.0
///   - 0.0
///   - 1.0
///   - 0.0
/// indices:
///   - 0
///   - 1
///   - 2
/// ";
/// let mesh = ludion_scene::check_mesh(source).expect("the mesh is valid");
/// assert_eq!(mesh.vertex_count(), 3);
/// assert_eq!(mesh.triangle_count(), 1);
/// assert!(ludion_scene::mesh_budget_warning(&mesh).is_none());
/// assert_eq!(ludion_scene::emit_mesh(&mesh), source); // canonical emit is a fixed point
/// ```
pub fn emit_mesh(doc: &MeshDocument) -> String {
    mesh::emit(doc)
}

// --- Mesh-edit documents (`meshes/*.mesh-edit.yaml`) --------------------------------------------
//
// An ordered, replayable list of authored edits to a mesh's texture maps, living beside the mesh it
// names (`Plan/mesh-texture-editing.md`). This crate owns the text and resolves no path: the source
// mesh, its maps and the derived tree are the consumer's business.

/// Parses strict-subset YAML into a [`MeshEditDocument`], checking structure only.
///
/// # Errors
/// `L1173` for a missing/wrong-typed key, an unrecognized `kind`/`op`/`map`/region shape, a region
/// naming other than exactly one shape, or a key the document or one of its ops does not have (with
/// a nearest match). Each diagnostic carries a JSON Pointer into the document.
pub fn parse_mesh_edit(source: &str) -> Result<MeshEditDocument, Vec<Diagnostic>> {
    mesh_edit::parse(source)
}

/// Validates the data invariants of `doc`: finite numbers, radii and box extents greater than 0, a
/// positive map `size`, colours and `strength` in `[0, 1]`, a non-negative `feather`, a non-empty
/// `source`, and an `out` that stays below this document's own directory.
///
/// Whether `source` exists on disk, carries `uvs`, or already has the map an op sizes is checked by
/// the consumer that resolves those paths, not here.
///
/// # Errors (returned, not `Result`)
/// `L1174`; an empty vector means `doc` is consistent.
pub fn validate_mesh_edit(doc: &MeshEditDocument) -> Vec<Diagnostic> {
    mesh_edit::validate(doc)
}

/// Parses and validates a mesh-edit document in one step.
///
/// # Errors
/// Everything [`parse_mesh_edit`] and [`validate_mesh_edit`] can report.
pub fn check_mesh_edit(source: &str) -> Result<MeshEditDocument, Vec<Diagnostic>> {
    let doc = parse_mesh_edit(source)?;
    let diagnostics = validate_mesh_edit(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `source`, `out` (optional),
/// `ops` in authored order, block style, ryu floats, LF endings. An absent `size`, `strength` or
/// `feather` is not written. Byte-idempotent:
/// `emit_mesh_edit(parse_mesh_edit(emit_mesh_edit(d)))` reproduces the same text.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: mesh-edit
/// name: idol-lit
/// source: mesh-0.mesh.yaml
/// out: idol-lit
/// ops:
///   - op: paint
///     map: emissive
///     size: 1024
///     color:
///       - 0.05
///       - 1.0
///       - 0.35
///     region:
///       sphere:
///         at:
///           - 0.118
///           - 1.342
///           - 0.203
///         radius: 0.035
///         feather: 0.012
/// ";
/// let edit = ludion_scene::check_mesh_edit(source).expect("the edit is valid");
/// assert_eq!(edit.source, "mesh-0.mesh.yaml");
/// assert_eq!(edit.out_dir("idol-lit"), "idol-lit");
/// assert_eq!(edit.ops.len(), 1);
/// assert_eq!(ludion_scene::emit_mesh_edit(&edit), source); // canonical emit is a fixed point
/// ```
pub fn emit_mesh_edit(doc: &MeshEditDocument) -> String {
    mesh_edit::emit(doc)
}

// --- Voxel model documents (`voxels/*.voxel.yaml`) ---------------------------------------------
//
// A voxel document has a fixed, small shape — like a mesh, it is not validated against the
// component registry, so it has no `Registry` parameter. It is a sibling of the [`mesh`] module.
// See the [`voxel`] module docs for the format and `Plan/phase-10-voxel-models.md` for
// the design decisions.

/// Parses strict-subset YAML into a [`VoxelDocument`], checking structure/shape only.
///
/// # Errors
/// `L1113` for a missing/wrong-typed key, an unrecognized `kind`, a malformed `size`/`palette`/
/// `slices`, or an unknown key — each diagnostic carries a JSON Pointer into the document where
/// applicable.
pub fn parse_voxel(source: &str) -> Result<VoxelDocument, Vec<Diagnostic>> {
    voxel::parse(source)
}

/// Validates data-consistency invariants of `doc`: palette keys are single printable-ASCII glyphs
/// and unique, colors are finite, the slice layers/rows/columns match `size`, and every slice
/// glyph is `.` or a palette entry.
///
/// # Errors (returned, not `Result`)
/// `L1114` diagnostics; an empty vector means `doc` is consistent.
pub fn validate_voxel(doc: &VoxelDocument) -> Vec<Diagnostic> {
    voxel::validate(doc)
}

/// Parses and validates a voxel document in one step: the CLI's `check-scene` voxel path.
///
/// # Errors
/// Everything [`parse_voxel`] and [`validate_voxel`] can report.
pub fn check_voxel(source: &str) -> Result<VoxelDocument, Vec<Diagnostic>> {
    let doc = parse_voxel(source)?;
    let diagnostics = validate_voxel(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// The non-fatal style cell-budget warning (`L1115`) for `doc`, above [`VOXEL_CELL_BUDGET`] cells.
/// `None` when under budget. This never fails [`check_voxel`]; callers that want to surface it
/// (e.g. `ludion check-scene`) print it separately and still exit success.
pub fn voxel_budget_warning(doc: &VoxelDocument) -> Option<Diagnostic> {
    voxel::budget_warning(doc)
}

/// The non-fatal connectivity warning (`L1129`) for `doc`, if its filled cells split into more than
/// one 6-connected component (a limb or accent that floats free of the main body). `None` for an
/// empty or fully-connected model. Like [`voxel_budget_warning`] this never fails [`check_voxel`];
/// callers that want to surface it (e.g. `ludion check-scene`) print it separately and still exit
/// success.
///
/// The message is multi-line: a count line, then one line per stray piece naming its size, a sample
/// cell, and the nearest pair of cells between it and the main body with the gap between them — so
/// the fix location is in the warning rather than a search away.
pub fn voxel_connectivity_warning(doc: &VoxelDocument) -> Option<Diagnostic> {
    voxel::connectivity_warning(doc)
}

/// Writes `doc` in canonical form (ryu shortest-round-trip floats, palette sorted by glyph, stable
/// field order, LF endings). Byte-idempotent: `emit_voxel(parse_voxel(emit_voxel(d)).unwrap())`
/// reproduces the same text (property-tested).
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: voxel
/// size:
///   - 1
///   - 1
///   - 1
/// palette:
///   A:
///     color:
///       - 1.0
///       - 1.0
///       - 1.0
///       - 1.0
/// slices:
///   -
///     - \"A\"
/// ";
/// let model = ludion_scene::check_voxel(source).expect("the model is valid");
/// assert_eq!(model.size, [1, 1, 1]);
/// assert_eq!(model.palette_index(0, 0, 0), Some(0));
/// assert!(ludion_scene::voxel_budget_warning(&model).is_none());
/// assert_eq!(ludion_scene::emit_voxel(&model), source); // canonical emit is a fixed point
/// ```
pub fn emit_voxel(doc: &VoxelDocument) -> String {
    voxel::emit(doc)
}

/// Writes `doc` in canonical form with authoring comments replayed into it: the header block
/// between the envelope keys and `size:`, and each layer's anatomy line above its `-`.
///
/// This is [`emit_voxel`] plus comments — with empty [`VoxelComments`] the two produce byte-identical
/// text, which is what lets a model keep exactly one canonical form while surviving a grid reshape
/// with its intent intact.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: voxel
/// ## A one-cell model.
/// size:
///   - 1
///   - 1
///   - 1
/// palette:
///   A:
///     color:
///       - 1.0
///       - 1.0
///       - 1.0
///       - 1.0
/// slices:
///   ## y0 - the only layer
///   -
///     - \"A\"
/// ";
/// let model = ludion_scene::check_voxel(source).expect("the model is valid");
/// let extracted = ludion_scene::extract_voxel_comments(source);
/// assert_eq!(extracted.comments.header, vec!["A one-cell model.".to_string()]);
/// assert_eq!(extracted.comments.layer(0), Some("y0 - the only layer"));
/// assert!(extracted.stray.is_empty());
/// // Comments replayed: the canonical text is the source again.
/// assert_eq!(
///     ludion_scene::emit_voxel_with_comments(&model, &extracted.comments),
///     source
/// );
/// // Without them, it is exactly `emit_voxel`.
/// assert_eq!(
///     ludion_scene::emit_voxel_with_comments(&model, &Default::default()),
///     ludion_scene::emit_voxel(&model)
/// );
/// ```
pub fn emit_voxel_with_comments(doc: &VoxelDocument, comments: &VoxelComments) -> String {
    voxel::emit_with_comments(doc, comments)
}

/// Extracts a voxel document's authoring comments from its raw source text: the header block above
/// `size:` and each slice layer's anatomy comment, plus every comment in a position the canonical
/// writer cannot reproduce (which a reshaping edit would drop — see [`ExtractedComments::stray`]).
///
/// Purely textual: it never parses the document, so it also works on a file that does not validate.
pub fn extract_voxel_comments(source: &str) -> ExtractedComments {
    voxel_edit::extract(source)
}

/// Parses a voxel ops document (`format: 1`, `kind: voxel-ops`, an `ops` sequence) — the input of
/// `ludion voxel apply`.
///
/// # Errors
/// `L1131` for a malformed envelope, an unknown op name or key, a missing required key, or a
/// coordinate token of the wrong shape; each diagnostic carries a JSON Pointer at the offending op
/// (`/ops/3/at`). Strict-YAML syntax violations surface the shared `L1001`–`L1009` codes.
pub fn parse_voxel_ops(source: &str) -> Result<Vec<VoxelOp>, Vec<Diagnostic>> {
    voxel_edit::parse_ops(source)
}

/// Applies `ops` to the voxel model in `source`, returning the new file text and a report of what
/// changed — the engine behind `ludion voxel apply`.
///
/// Cell-level edits are **surgical**: the returned text is the original bytes with only the changed
/// cell characters replaced, so the header docstring, layer comments, blank lines, and float
/// formatting all survive byte-identical. A grid reshape (`grow`/`crop`) re-emits canonically
/// instead, replaying the extracted comments with layer comments remapped by the reshape. Either
/// way the result is parsed and validated before it is returned, so a caller can write it as-is.
///
/// # Errors
/// `L1113`/`L1114` if the model does not validate (before or after the edit), `L1132` if an op
/// cannot be applied (an out-of-range `set`, a glyph with no palette entry, an empty `crop`).
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: voxel
/// size:
///   - 2
///   - 1
///   - 1
/// palette:
///   A:
///     color:
///       - 1.0
///       - 1.0
///       - 1.0
///       - 1.0
/// slices:
///   ## y0 - the base
///   -
///     - \"..\"
/// ";
/// let ops = ludion_scene::parse_voxel_ops(
///     "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"1,0,0\"\n    glyph: A\n",
/// )
/// .expect("the ops document is valid");
/// let edit = ludion_scene::apply_voxel_ops(source, &ops).expect("the op applies");
/// assert_eq!(edit.changed, vec![1]);
/// assert!(edit.surgical);
/// // Exactly one character differs from the input — the comment and everything else survives.
/// assert_eq!(edit.text, source.replace("\"..\"", "\".A\""));
/// ```
pub fn apply_voxel_ops(source: &str, ops: &[VoxelOp]) -> Result<VoxelEdit, Vec<Diagnostic>> {
    voxel_edit::apply(source, ops)
}

/// Upscales the voxel model in `source` by `factor`: nearest-neighbor, each source cell becoming a
/// `factor`^3 block — the engine behind `ludion voxel upscale`, and the highest-leverage authoring
/// verb the write side has (`Plan/voxel-authoring-tools.md`): a proven low-resolution model
/// becomes a correct, connected, palette-identical high-resolution base in one call.
///
/// `size` scales by `factor` (checked — an overflowing result is an error, not a silent wrap); a
/// declared `scale` divides by `factor` so the model's intended world footprint (grid height x
/// scale) is unchanged; `budget_exempt` carries over as-is; `generated_from` is dropped (the output
/// is not a product of the source's generator script) and reported back via
/// [`VoxelUpscale::dropped_generated_from`]. Comments carry down: the header verbatim plus one
/// appended `upscaled x<factor> from <origin_label>` line (omitted when `origin_label` is `None` —
/// this function is IO-free, so the caller, typically the CLI, supplies the source file's name
/// rather than this reading the filesystem), and each source layer's anatomy comment moves onto the
/// first of its `factor` output layers.
///
/// The result always re-emits canonically (every cell moves, so there is no surgical form), and is
/// re-parsed and validated before being returned — a caller can write [`VoxelUpscale::text`] as-is.
/// An upscale can legitimately cross the [`VOXEL_CELL_BUDGET`] style budget; that is reported by
/// [`voxel_budget_warning`] on [`VoxelUpscale::doc`] like any other model, never suppressed here.
///
/// # Errors
/// `L1132` when `factor` is outside `2..=8`, or scaling `size` by `factor` would overflow a 32-bit
/// dimension. Otherwise everything [`check_voxel`] can report on the source document.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: voxel
/// ## y0 - the only layer
/// size:
///   - 1
///   - 1
///   - 1
/// scale: 0.3
/// palette:
///   A:
///     color:
///       - 1.0
///       - 1.0
///       - 1.0
///       - 1.0
/// slices:
///   -
///     - A
/// ";
/// let up = ludion_scene::upscale_voxel(source, 2, Some("gem.voxel.yaml")).expect("2..=8 applies");
/// assert_eq!(up.doc.size, [2, 2, 2]);
/// assert_eq!(up.doc.scale, Some(0.15));
/// assert_eq!(up.doc.filled_count(), 8);
/// assert_eq!(up.doc.component_count(), 1);
/// assert!(up.doc.generated_from.is_none());
/// assert!(ludion_scene::check_voxel(&up.text).is_ok());
///
/// // Out of range is a plain `L1132`, the same "op inapplicable" family `voxel apply` uses.
/// let diagnostics = ludion_scene::upscale_voxel(source, 9, None).unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L1132");
/// ```
pub fn upscale_voxel(
    source: &str,
    factor: u32,
    origin_label: Option<&str>,
) -> Result<VoxelUpscale, Vec<Diagnostic>> {
    voxel_edit::upscale(source, factor, origin_label)
}

/// Turns a voxel generator intermediate — the `kind: voxel-gen` JSON a `*.voxel.gen.mjs` script
/// prints — into the model it describes and the authoring comments to write with it: the engine
/// behind `ludion voxel from-json`.
///
/// This is the from-scratch half of the write side (`Plan/voxel-authoring-tools.md`). The
/// script builds the grid over the committed `scripts/voxel/grid.mjs` library and prints one JSON
/// object; **it never writes YAML**, so canonical serialization stays in exactly one place. Pair the
/// result with [`emit_voxel_with_comments`] to get the file text. A model whose script is its source
/// of truth carries `generated_from:` and is kept honest by the repo's `voxel-drift` gate.
///
/// # Errors
/// `L1133` for anything wrong with the intermediate's own shape (not JSON, a missing or wrong-typed
/// key, an unknown key — with a nearest-match suggestion — a layer-comment key that is not a
/// number), each carrying a JSON Pointer into the intermediate. Once the shape checks out, the
/// described model faces [`validate_voxel`] unchanged, so a mis-sized row or an unknown glyph is the
/// usual `L1114`.
///
/// # Example
/// ```
/// let intermediate = r#"{
///   "format": 1, "kind": "voxel-gen", "name": "gem",
///   "size": [2, 1, 2],
///   "scale": 0.25,
///   "generated_from": "gem.voxel.gen.mjs",
///   "palette": {"A": {"color": [1.0, 0.0, 0.0, 1.0]}},
///   "header": ["A gem. Front = row z=0."],
///   "layer_comments": {"0": "the only layer"},
///   "layers": [["A.", ".A"]]
/// }"#;
/// let (model, comments) =
///     ludion_scene::voxel_from_json(intermediate).expect("the intermediate is well-formed");
/// assert_eq!(model.size, [2, 1, 2]);
/// assert_eq!(model.generated_from.as_deref(), Some("gem.voxel.gen.mjs"));
/// assert_eq!(comments.layer(0), Some("the only layer"));
///
/// // The canonical writer is the only thing that produces file text — never the script.
/// let text = ludion_scene::emit_voxel_with_comments(&model, &comments);
/// assert!(text.contains("generated_from: gem.voxel.gen.mjs"));
/// assert!(text.contains("  # the only layer\n  -\n"));
/// assert!(ludion_scene::check_voxel(&text).is_ok());
/// ```
pub fn voxel_from_json(source: &str) -> Result<(VoxelDocument, VoxelComments), Vec<Diagnostic>> {
    voxel_gen::from_json(source)
}

// --- Sculpt model documents (`sculpts/*.sculpt.yaml`) -------------------------------------------
//
// A sculpt is an implicit-surface model: named primitives with real dimensions folded in file order
// by union/subtract/intersect with a blend radius, each on a bone of the document's tree and colored
// by a voxel-shaped palette (`Plan/mesh-modeling.md`). This crate owns the text; meshing happens
// outside it.

/// Parses strict-subset YAML into a [`SculptDocument`], checking structure only.
///
/// # Errors
/// `L1156` for a missing/wrong-typed key, an unrecognized `kind`/`shape`/`op`/`mirror`, or a key no
/// part has (with a nearest match); `L1157` for a key that belongs to another shape than the part's
/// own (with that shape's key list). Each diagnostic carries a JSON Pointer into the document.
pub fn parse_sculpt(source: &str) -> Result<SculptDocument, Vec<Diagnostic>> {
    sculpt::parse(source)
}

/// Validates the data invariants of `doc`: finite numbers, `resolution` in
/// [`SCULPT_MIN_RESOLUTION`]`..=`[`SCULPT_MAX_RESOLUTION`], `triangles >=` [`SCULPT_MIN_TRIANGLES`],
/// the palette's voxel rules, uniquely named parts with positive dimensions, `blend >= 0`,
/// `repeat.count >= 2`, a `round` within half the box, and a `material` naming a palette glyph on
/// every `union` part and on no other; and the bone tree: unique names, the first bone the only root, every
/// `parent` and part `bone` naming a bone, no cycle.
///
/// # Errors (returned, not `Result`)
/// `L1157` for data, `L1160` for the bone tree; an empty vector means `doc` is consistent.
pub fn validate_sculpt(doc: &SculptDocument) -> Vec<Diagnostic> {
    sculpt::validate(doc)
}

/// Parses and validates a sculpt document in one step.
///
/// # Errors
/// Everything [`parse_sculpt`] and [`validate_sculpt`] can report.
pub fn check_sculpt(source: &str) -> Result<SculptDocument, Vec<Diagnostic>> {
    let doc = parse_sculpt(source)?;
    let diagnostics = validate_sculpt(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form: `format`, `kind`, `name`, `resolution`, `triangles`,
/// `budget_exempt`, `palette` (sorted by glyph), `bones` and `parts` in authored order, block style,
/// ryu floats, LF endings. A part default (`at` at the origin, `op: union`, `blend: 0`, a box's
/// `round: 0`, `sharp: false`) is not written. Byte-idempotent:
/// `emit_sculpt(parse_sculpt(emit_sculpt(d)))` reproduces the same text.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: sculpt
/// resolution: 32
/// triangles: 4000
/// palette:
///   S:
///     color:
///       - 1.0
///       - 1.0
///       - 1.0
///       - 1.0
/// bones:
///   - name: root
/// parts:
///   - name: ball
///     shape: sphere
///     radius: 0.5
///     material: S
/// ";
/// let model = ludion_scene::check_sculpt(source).expect("the sculpt is valid");
/// assert_eq!(model.parts[0].shape, ludion_scene::SculptShape::Sphere { radius: 0.5 });
/// assert_eq!(model.bone_index(&model.parts[0]), Some(0));
/// assert_eq!(ludion_scene::emit_sculpt(&model), source); // canonical emit is a fixed point
/// ```
pub fn emit_sculpt(doc: &SculptDocument) -> String {
    sculpt::emit(doc)
}

// --- Generation recipes (`kind: generation`) and provenance (`kind: origin`) --------------------
//
// A recipe names a committed image-generation workflow and overrides its leaf inputs by node title;
// an origin block is the provenance an accepted asset keeps, carried either by a mesh document's
// `origin:` key or by a `kind: origin` sidecar beside a generated image
// (`Plan/comfy-generation.md`). This crate owns the text and reads no file.

/// Parses strict-subset YAML into a [`GenerationDocument`], checking structure only.
///
/// # Errors
/// `L1161` for a missing/wrong-typed key, an unrecognized `kind`, or a key a recipe does not have
/// (with a nearest match); `L1162` for a `seed` outside `u64` and for an input value that is
/// neither a scalar nor the one `image:` reference form. Each diagnostic carries a JSON Pointer.
pub fn parse_generation(source: &str) -> Result<GenerationDocument, Vec<Diagnostic>> {
    generation::parse(source)
}

/// Validates the data invariants of `doc`: `count >= 1`, a non-empty `workflow` and `images` paths,
/// finite input numbers, and every `{image: x}` naming a declared `images` entry.
///
/// **Filesystem-agnostic**: whether `workflow` and the `images` paths exist is *not* checked here.
/// They are relative to the recipe document's own directory, which this crate does not know — the
/// caller resolves and checks them, and reports the same `L1162` at the same pointer.
///
/// # Errors (returned, not `Result`)
/// `L1162`; an empty vector means `doc` is consistent.
pub fn validate_generation(doc: &GenerationDocument) -> Vec<Diagnostic> {
    generation::validate(doc)
}

/// Parses and validates a generation recipe in one step.
///
/// # Errors
/// Everything [`parse_generation`] and [`validate_generation`] can report.
pub fn check_generation(source: &str) -> Result<GenerationDocument, Vec<Diagnostic>> {
    let doc = parse_generation(source)?;
    let diagnostics = validate_generation(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form: `format`, `kind`, `name`, `workflow`, `seed`, `count`, `images`,
/// `inputs`, block style, both `images` and `inputs` sorted by key, ryu floats, LF endings. `name`
/// is written only when authored, `count` only when it is not [`GENERATION_DEFAULT_COUNT`], and
/// `images`/`inputs` only when non-empty. Byte-idempotent:
/// `emit_generation(parse_generation(emit_generation(d)))` reproduces the same text.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: generation
/// workflow: \"../workflows/txt2img.json\"
/// seed: 7
/// count: 4
/// images:
///   subject: \"../renders/idol-front.png\"
/// inputs:
///   Load Subject:
///     image:
///       image: subject
/// ";
/// let recipe = ludion_scene::check_generation(source).expect("the recipe is valid");
/// assert_eq!(recipe.count, 4);
/// assert_eq!(ludion_scene::emit_generation(&recipe), source); // canonical emit is a fixed point
/// ```
pub fn emit_generation(doc: &GenerationDocument) -> String {
    generation::emit(doc)
}

/// Parses a `kind: origin` sidecar document — the provenance written beside a generated image,
/// which has no text place of its own to carry it.
///
/// # Errors
/// `L1161` for a missing/wrong-typed key or a key an origin block does not have (with a nearest
/// match); `L1162` for a non-finite number in its `inputs` snapshot.
pub fn parse_origin_document(source: &str) -> Result<Origin, Vec<Diagnostic>> {
    generation::parse_origin_document(source)
}

/// Validates a provenance block's data: the numbers of its `inputs` snapshot are finite and every
/// `images` entry is complete. An origin's image *references* name server-side uploads rather than a
/// declared `images:` map, so they are not resolved.
///
/// # Errors (returned, not `Result`)
/// `L1162`; an empty vector means `origin` is consistent.
pub fn validate_origin(origin: &Origin) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    generation::validate_origin_at(
        origin,
        "",
        ludion_core::codes::code::GENERATION_DATA,
        &mut diagnostics,
    );
    diagnostics
}

/// Parses and validates a `kind: origin` sidecar in one step.
///
/// # Errors
/// Everything [`parse_origin_document`] and [`validate_origin`] can report.
pub fn check_origin_document(source: &str) -> Result<Origin, Vec<Diagnostic>> {
    let origin = parse_origin_document(source)?;
    let diagnostics = validate_origin(&origin);
    if diagnostics.is_empty() {
        Ok(origin)
    } else {
        Err(diagnostics)
    }
}

/// Writes a `kind: origin` sidecar in canonical form: the envelope, then `tool`, `comfy_version`,
/// `recipe`, `recipe_hash`, `run_id`, `date`, `seed`, `workflow`, `workflow_hash`, `checkpoints`,
/// `images`, `prompt`, `inputs`. `checkpoints`, `images` and `inputs` are written only when
/// non-empty, `recipe_hash` and `seed` only when present.
/// Byte-idempotent.
///
/// The same block, indented, is what a [`MeshDocument`]'s `origin:` key emits — one struct, two
/// carriers, one writer.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: origin
/// tool: comfyui
/// comfy_version: \"0.3.61\"
/// recipe: generation/recipes/idol.gen.yaml
/// run_id: idol-0007
/// date: \"2026-09-17\"
/// seed: 9
/// workflow: generation/workflows/txt2img.json
/// workflow_hash: \"9f2c\"
/// prompt: a weathered stone idol
/// ";
/// let origin = ludion_scene::check_origin_document(source).expect("the sidecar is valid");
/// assert_eq!(origin.seed, Some(9));
/// assert_eq!(ludion_scene::emit_origin_document(&origin), source);
/// ```
pub fn emit_origin_document(origin: &Origin) -> String {
    generation::emit_origin_document(origin)
}

// --- Part documents (`parts/*.part.yaml`) -------------------------------------------------------
//
// A part is a lean voxel-slices-only document — one body part of a multi-part `kind: assembly`
// (`Plan/voxel-animation.md` decisions 3-4). It is a structural sibling of [`VoxelDocument`],
// trimmed to `size`/`slices` alone: a part has no palette of its own, so its glyphs resolve later
// against the owning assembly's palette (an unclaimed glyph is that assembly's lint to raise, not
// this crate's).

/// Parses strict-subset YAML into a [`PartDocument`], checking structure/shape only.
///
/// # Errors
/// `L1139` for a missing/wrong-typed key, an unrecognized `kind`, a malformed `size`/`slices`, or
/// an unknown key — each diagnostic carries a JSON Pointer into the document where applicable.
pub fn parse_part(source: &str) -> Result<PartDocument, Vec<Diagnostic>> {
    part::parse(source)
}

/// Validates data-consistency invariants of `doc`: the slice layers/rows/columns match `size`, and
/// every slice glyph is `.` or a printable-ASCII character (`0x21..=0x7E`) — a part has no palette,
/// so unlike [`validate_voxel`] no glyph is ever rejected as "unclaimed".
///
/// # Errors (returned, not `Result`)
/// `L1140` diagnostics; an empty vector means `doc` is consistent.
pub fn validate_part(doc: &PartDocument) -> Vec<Diagnostic> {
    part::validate(doc)
}

/// Parses and validates a part document in one step.
///
/// # Errors
/// Everything [`parse_part`] and [`validate_part`] can report.
pub fn check_part(source: &str) -> Result<PartDocument, Vec<Diagnostic>> {
    let doc = parse_part(source)?;
    let diagnostics = validate_part(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form (stable field order, LF endings). Byte-idempotent:
/// `emit_part(parse_part(emit_part(d)).unwrap())` reproduces the same text (property-tested).
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: part
/// size:
///   - 1
///   - 1
///   - 1
/// slices:
///   -
///     - A
/// ";
/// let part = ludion_scene::check_part(source).expect("the part is valid");
/// assert_eq!(part.size, [1, 1, 1]);
/// assert_eq!(ludion_scene::emit_part(&part), source); // canonical emit is a fixed point
/// ```
pub fn emit_part(doc: &PartDocument) -> String {
    part::emit(doc)
}

// --- Sprite documents (`sprites/*.sprite.yaml`) -------------------------------------------------
//
// A sprite is the 2D sibling of a voxel model (`Plan/sprite-engine.md`): one `size` in texels, a
// glyph palette, an ordered mapping of named frames of that one size, and a doc-level `effects:`
// list of enumerated ops the bake applies in order. Nothing here bakes — this crate owns the text
// and a separate crate turns it into pixels.

/// Parses strict-subset YAML into a [`SpriteDocument`], checking structure/shape only.
///
/// # Errors
/// `L1153` for a missing/wrong-typed key, an unrecognized `kind`, a malformed `size`/`palette`/
/// `frames`, or an unknown key; `L1154` for a malformed entry of `effects` (an unknown `op`, an
/// unknown key for that op, a wrong-typed parameter, a missing required one). Each diagnostic
/// carries a JSON Pointer into the document.
pub fn parse_sprite(source: &str) -> Result<SpriteDocument, Vec<Diagnostic>> {
    sprite::parse(source)
}

/// Validates semantic invariants of `doc`: `size` in `1..=`[`SPRITE_MAX_SIZE`], palette keys single
/// printable-ASCII glyphs with colors in `0..=1`, frame names unique and free of `/`, every frame's
/// grid matching `size` with glyphs the palette claims, pivots inside their frame, and every
/// effect's parameters in range and resolving against this document's own palette and frames.
///
/// # Errors (returned, not `Result`)
/// `L1153` for the document, `L1154` for anything under `/effects`; an empty vector means `doc` is
/// consistent.
pub fn validate_sprite(doc: &SpriteDocument) -> Vec<Diagnostic> {
    sprite::validate(doc)
}

/// Parses and validates a sprite document in one step.
///
/// # Errors
/// Everything [`parse_sprite`] and [`validate_sprite`] can report.
pub fn check_sprite(source: &str) -> Result<SpriteDocument, Vec<Diagnostic>> {
    let doc = parse_sprite(source)?;
    let diagnostics = validate_sprite(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form (stable field order, palette sorted by glyph, frames and effects
/// in authored order, ryu floats, LF endings).
///
/// **Canonical means minimal**: an effect parameter equal to its default is not written, and an
/// empty `effects` list is not written at all. So this is a fixed point on canonical text —
/// `emit_sprite(check_sprite(text)) == text` — and byte-idempotent on any document, while a
/// hand-written document that spells a default out loud re-emits shorter once and then holds.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: sprite
/// size:
///   - 2
///   - 1
/// palette:
///   A:
///     color: \"#ffffff\"
/// frames:
///   idle:
///     rows:
///       - A.
/// ";
/// let sprite = ludion_scene::check_sprite(source).expect("the sprite is valid");
/// assert_eq!(sprite.size, [2, 1]);
/// assert_eq!(sprite.default_pivot(), [1.0, 0.5]);
/// assert_eq!(ludion_scene::emit_sprite(&sprite), source); // canonical emit is a fixed point
/// ```
pub fn emit_sprite(doc: &SpriteDocument) -> String {
    sprite::emit(doc)
}

// --- Palette documents (`*.palette.yaml`) -------------------------------------------------------
//
// A palette is a standalone glyph palette (`format: 1`, `kind: palette`, a `palette:` map of the same
// `{color, emissive?}` entries a sprite document's inline palette uses) that many sprites can share
// instead of copying the same colours into each. A sprite references one with
// `palette: { from, overrides? }`; [`resolve_sprite_include`] merges the two into the concrete inline
// palette the bake consumes, so an inline sprite and an equivalent include bake byte-identically.

/// Parses strict-subset YAML into a [`PaletteDocument`], checking structure/shape only.
///
/// # Errors
/// `L1147` for a missing/wrong-typed key, an unrecognized `kind`, a malformed `palette`, or an
/// unknown key — each diagnostic carries a JSON Pointer into the document where applicable.
pub fn parse_palette(source: &str) -> Result<PaletteDocument, Vec<Diagnostic>> {
    palette::parse(source)
}

/// Validates semantic invariants of `doc`: a non-empty palette whose keys are single printable-ASCII
/// glyphs with colors in `0..=1` and finite non-negative emissives — the same rules a sprite's inline
/// palette obeys.
///
/// # Errors (returned, not `Result`)
/// `L1147` diagnostics; an empty vector means `doc` is consistent.
pub fn validate_palette(doc: &PaletteDocument) -> Vec<Diagnostic> {
    palette::validate(doc)
}

/// Parses and validates a palette document in one step: the CLI's `check-scene` palette path.
///
/// # Errors
/// Everything [`parse_palette`] and [`validate_palette`] can report.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: palette
/// palette:
///   A:
///     color: \"#ff0000\"
/// ";
/// let palette = ludion_scene::check_palette(source).expect("the palette is valid");
/// assert_eq!(palette.palette.len(), 1);
/// assert_eq!(ludion_scene::emit_palette(&palette), source); // canonical emit is a fixed point
/// ```
pub fn check_palette(source: &str) -> Result<PaletteDocument, Vec<Diagnostic>> {
    let doc = parse_palette(source)?;
    let diagnostics = validate_palette(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form (stable field order, palette sorted by glyph, ryu floats, LF
/// endings). Byte-idempotent and a fixed point on canonical input.
pub fn emit_palette(doc: &PaletteDocument) -> String {
    palette::emit(doc)
}

/// Resolves a sprite document's shared-palette include (`palette: { from, overrides }`) against the
/// referenced palette document's text `palette_source`, returning an ordinary inline sprite document
/// the bake consumes unchanged: the referenced palette with the sprite's `overrides` applied on top
/// (a glyph the palette defines is replaced, one it does not is added).
///
/// A sprite with no include is returned unchanged, so a caller may call this on every sprite it bakes
/// and only read the palette file when [`SpriteDocument::palette_include`] is `Some`. The result is
/// **not** re-validated: run [`validate_sprite`] (or bake, which does) on it — a glyph a frame uses
/// but the merged palette still lacks then surfaces as the ordinary `L1153`, exactly as it would in an
/// inline document.
///
/// # Errors
/// `L1147` if `palette_source` is not a well-formed `kind: palette` document.
///
/// # Example
/// ```
/// let sprite = ludion_scene::check_sprite(
///     "format: 1\nkind: sprite\nsize:\n  - 1\n  - 1\npalette:\n  from: world.palette.yaml\n\
///      frames:\n  idle:\n    rows:\n      - \"A\"\n",
/// )
/// .expect("a sprite with a palette include parses");
/// assert!(sprite.palette_include.is_some());
///
/// let palette = "format: 1\nkind: palette\npalette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n\
///                \x20     - 1.0\n      - 1.0\n";
/// let resolved = ludion_scene::resolve_sprite_include(&sprite, palette).expect("it resolves");
/// assert!(resolved.palette_include.is_none());
/// assert_eq!(resolved.palette.len(), 1);
/// assert!(ludion_scene::validate_sprite(&resolved).is_empty());
/// ```
pub fn resolve_sprite_include(
    doc: &SpriteDocument,
    palette_source: &str,
) -> Result<SpriteDocument, Vec<Diagnostic>> {
    palette::resolve(doc, palette_source)
}

/// Merges `overrides` into `palette` with the include's semantics: a glyph the palette defines is
/// replaced, one it does not is added, and the result is sorted by glyph. A sprite's `palette: {
/// from, overrides }` and an atlas-source input's `palette` both apply through this one function.
pub fn apply_sprite_palette_overrides(
    palette: &mut Vec<SpritePaletteEntry>,
    overrides: &[SpritePaletteEntry],
) {
    palette::apply_overrides(palette, overrides);
}

// --- Atlas documents (`*.atlas-source.yaml` / `*.atlas.yaml`) -----------------------------------
//
// An atlas source lists the sprite documents and loose PNGs one packed image is built from, plus
// the flipbooks it carries; `ludion generate-atlas` writes the `.png` and the derived `kind: atlas`
// beside it, and both are committed (nothing packs at runtime). The derived document is write-only
// for authors: edit the source and re-run the bake.

/// Parses strict-subset YAML into an [`AtlasSourceDocument`], checking structure/shape only.
///
/// # Errors
/// `L1153`, each diagnostic carrying a JSON Pointer into the document.
pub fn parse_atlas_source(source: &str) -> Result<AtlasSourceDocument, Vec<Diagnostic>> {
    atlas::parse_source(source)
}

/// Validates semantic invariants of `doc`: `pixels_per_unit` finite and positive, a non-empty
/// `inputs` list of non-empty paths whose `name` is one plain segment and whose `palette` is valid
/// (and absent on a loose PNG), and well-shaped flipbooks.
///
/// Flipbook frame names are deliberately **not** resolved: an atlas source has not read the sprite
/// documents its inputs name, so it cannot know which frames exist — that is the bake's check.
///
/// # Errors (returned, not `Result`)
/// `L1153` diagnostics; an empty vector means `doc` is consistent.
pub fn validate_atlas_source(doc: &AtlasSourceDocument) -> Vec<Diagnostic> {
    atlas::validate_source(doc)
}

/// Parses and validates an atlas source document in one step.
///
/// # Errors
/// Everything [`parse_atlas_source`] and [`validate_atlas_source`] can report.
pub fn check_atlas_source(source: &str) -> Result<AtlasSourceDocument, Vec<Diagnostic>> {
    let doc = parse_atlas_source(source)?;
    let diagnostics = validate_atlas_source(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form. Minimal like [`emit_sprite`]: a default `filter` or `loop` is
/// not written, and an empty `flipbooks` list is not written at all.
pub fn emit_atlas_source(doc: &AtlasSourceDocument) -> String {
    atlas::emit_source(doc)
}

/// Parses strict-subset YAML into an [`AtlasDocument`], checking structure/shape only.
///
/// # Errors
/// `L1153`, each diagnostic carrying a JSON Pointer into the document.
pub fn parse_atlas(source: &str) -> Result<AtlasDocument, Vec<Diagnostic>> {
    atlas::parse(source)
}

/// Validates semantic invariants of `doc`: a non-empty `generated_from` and colour map,
/// `pixels_per_unit` finite and positive, non-empty `regions` whose rects cover at least one texel
/// and whose pivots lie inside their own region, and flipbooks whose frames name regions this atlas
/// has.
///
/// Region **overlap** is deliberately not checked: the packer decides the layout, and one rect
/// aliased under two names is legal content.
///
/// # Errors (returned, not `Result`)
/// `L1153` diagnostics; an empty vector means `doc` is consistent.
pub fn validate_atlas(doc: &AtlasDocument) -> Vec<Diagnostic> {
    atlas::validate(doc)
}

/// Parses and validates an atlas document in one step.
///
/// # Errors
/// Everything [`parse_atlas`] and [`validate_atlas`] can report.
pub fn check_atlas(source: &str) -> Result<AtlasDocument, Vec<Diagnostic>> {
    let doc = parse_atlas(source)?;
    let diagnostics = validate_atlas(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form: regions in **name** order, so a packer change moves rects
/// without reshuffling the file. Byte-idempotent, and the byte comparison
/// `ludion generate-atlas --check` runs.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: atlas
/// generated_from: ship.atlas-source.yaml
/// pixels_per_unit: 16.0
/// maps:
///   color: ship.png
/// regions:
///   ship/idle:
///     rect:
///       - 0
///       - 0
///       - 16
///       - 12
///     pivot:
///       - 8.0
///       - 6.0
/// ";
/// let atlas = ludion_scene::check_atlas(source).expect("the atlas is valid");
/// assert_eq!(atlas.region("ship/idle").map(|r| r.rect), Some([0, 0, 16, 12]));
/// assert_eq!(ludion_scene::emit_atlas(&atlas), source); // canonical emit is a fixed point
/// ```
pub fn emit_atlas(doc: &AtlasDocument) -> String {
    atlas::emit(doc)
}

// --- Font documents (`*.font.yaml`) -------------------------------------------------------------
//
// A font document is the glyph table, metrics and atlas geometry of one face; the atlas PNG it names
// is resolved by the loader. This crate owns the text half only.

/// Parses strict-subset YAML into a [`FontDocument`], checking structure/shape only.
///
/// # Errors
/// `L1250` for a missing/wrong-typed key, an unrecognized `kind`, a malformed glyph, or an unknown
/// key — each diagnostic carries a JSON Pointer into the document where applicable.
pub fn parse_font(source: &str) -> Result<FontDocument, Vec<Diagnostic>> {
    font::parse(source)
}

/// Validates the semantic invariants of a parsed [`FontDocument`]: a positive em and line height,
/// glyphs strictly ascending by code point, rects inside the atlas, and aliases, kerning and
/// `missing` consistent with the glyph table.
///
/// An empty result means the document is valid; every diagnostic is `L1250`.
pub fn validate_font(doc: &FontDocument) -> Vec<Diagnostic> {
    font::validate(doc)
}

/// Parses and validates a font document in one step.
///
/// # Errors
/// Everything [`parse_font`] and [`validate_font`] can report.
pub fn check_font(source: &str) -> Result<FontDocument, Vec<Diagnostic>> {
    let doc = parse_font(source)?;
    let diagnostics = validate_font(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form: fixed key order, glyphs in code-point order, optional sections
/// only when non-empty. Byte-idempotent, and the comparison a drift gate runs.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: font
/// image: tiny.png
/// mode: bitmap
/// units_per_em: 8
/// ascender: 8
/// descender: 0
/// line_gap: 0
/// atlas:
///   width: 8
///   height: 8
///   em_px: 8
/// notdef:
///   advance: 8
/// glyphs:
///   - char: A
///     advance: 8
///     bounds:
///       - 0
///       - 0
///       - 8
///       - 8
///     rect:
///       - 0
///       - 0
///       - 8
///       - 8
/// ";
/// let font = ludion_scene::check_font(source).expect("the font is valid");
/// assert_eq!(font.glyphs[0].advance, 8);
/// assert_eq!(ludion_scene::emit_font(&font), source); // canonical emit is a fixed point
/// ```
pub fn emit_font(doc: &FontDocument) -> String {
    font::emit(doc)
}

/// Parses strict-subset YAML into a [`FontSourceDocument`], checking structure/shape only.
///
/// # Errors
/// `L1251` for a missing/wrong-typed key, an unrecognized `kind`, an unknown charset range keyword,
/// or an unknown key — each diagnostic carries a JSON Pointer into the document where applicable.
pub fn parse_font_source(source: &str) -> Result<FontSourceDocument, Vec<Diagnostic>> {
    font_source::parse(source)
}

/// Validates the semantic invariants of a parsed [`FontSourceDocument`]: a non-empty `file`,
/// `em_px` in `1..=256`, `spread_px` present and within `1..=em_px` exactly for an SDF face, and a
/// charset that resolves to at least one character.
///
/// An empty result means the document is valid; every diagnostic is `L1251`.
pub fn validate_font_source(doc: &FontSourceDocument) -> Vec<Diagnostic> {
    font_source::validate(doc)
}

/// Parses and validates a font source document in one step.
///
/// # Errors
/// Everything [`parse_font_source`] and [`validate_font_source`] can report.
pub fn check_font_source(source: &str) -> Result<FontSourceDocument, Vec<Diagnostic>> {
    let doc = parse_font_source(source)?;
    let diagnostics = validate_font_source(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form: fixed key order, charset ranges sorted by keyword and
/// deduplicated, `chars` as authored. Byte-idempotent, and the comparison a drift gate runs.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: font-source
/// file: Inter-Regular.ttf
/// mode: sdf
/// em_px: 56
/// spread_px: 6
/// charset:
///   ranges:
///     - ascii
/// ";
/// let doc = ludion_scene::check_font_source(source).expect("the source is valid");
/// assert_eq!(doc.em_px, 56);
/// assert_eq!(ludion_scene::emit_font_source(&doc), source); // canonical emit is a fixed point
/// ```
pub fn emit_font_source(doc: &FontSourceDocument) -> String {
    font_source::emit(doc)
}

// --- Font tables (`fonts.yaml`) -----------------------------------------------------------------
//
// A font table maps the logical face names a `font` field uses to `*.font.yaml` documents, with the
// reserved name `default` for text that names none. The engine's `FontLibrary` loads it.

/// Parses a `kind: fonts` document, checking structure and shape only.
///
/// # Errors
/// `L1253` for a missing/wrong-typed key, an unrecognized `kind` or an unknown key.
pub fn parse_fonts(source: &str) -> Result<FontsDocument, Vec<Diagnostic>> {
    fonts::parse(source)
}

/// Validates each name and path of a parsed [`FontsDocument`]: a name is non-empty without
/// whitespace or `/`; a path is root-relative, free of `..`, and ends `.font.yaml`.
///
/// An empty result means the document is valid; every diagnostic is `L1253`.
pub fn validate_fonts(doc: &FontsDocument) -> Vec<Diagnostic> {
    fonts::validate(doc)
}

/// Parses and validates a font table in one step.
///
/// # Errors
/// Everything [`parse_fonts`] and [`validate_fonts`] can report.
pub fn check_fonts(source: &str) -> Result<FontsDocument, Vec<Diagnostic>> {
    let doc = parse_fonts(source)?;
    let diagnostics = validate_fonts(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form: names sorted. Byte-idempotent.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: fonts
/// fonts:
///   body: fonts/inter.font.yaml
/// ";
/// let doc = ludion_scene::check_fonts(source).expect("the table is valid");
/// assert_eq!(doc.fonts[0].0, "body");
/// assert_eq!(ludion_scene::emit_fonts(&doc), source); // canonical emit is a fixed point
/// ```
pub fn emit_fonts(doc: &FontsDocument) -> String {
    fonts::emit(doc)
}

// --- Animation clip documents (`anims/*.anim.yaml`) ---------------------------------------------
//
// A clip animates named parts of a `kind: assembly` by keyframed rigid transforms
// (`Plan/voxel-animation.md` decision 8). The binding to real part names happens later, at
// assembly level; this crate validates a clip in isolation, the way it validates a part in
// isolation from its owning assembly.

/// Parses strict-subset YAML into an [`AnimationDocument`], checking structure/shape only.
///
/// # Errors
/// `L1141` for a missing/wrong-typed key, an unrecognized `kind`, a malformed track/keyframe, or
/// an unknown key — each diagnostic carries a JSON Pointer into the document where applicable.
pub fn parse_animation(source: &str) -> Result<AnimationDocument, Vec<Diagnostic>> {
    animation::parse(source)
}

/// Validates data-consistency invariants of `doc`: `duration` is finite and positive, `tracks` is
/// non-empty, every track animates at least one channel, every keyframe's `t`/`v` are finite with
/// `t` inside `[0, duration]`, each channel's keyframe times strictly increase, and no non-`step`
/// rotation segment turns 180 degrees or more on an Euler axis.
///
/// # Errors (returned, not `Result`)
/// `L1142` diagnostics; an empty vector means `doc` is consistent.
pub fn validate_animation(doc: &AnimationDocument) -> Vec<Diagnostic> {
    animation::validate(doc)
}

/// Parses and validates an animation clip document in one step.
///
/// # Errors
/// Everything [`parse_animation`] and [`validate_animation`] can report.
pub fn check_animation(source: &str) -> Result<AnimationDocument, Vec<Diagnostic>> {
    let doc = parse_animation(source)?;
    let diagnostics = validate_animation(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form (stable field order, LF endings, ryu shortest-round-trip floats,
/// `ease: linear` canonicalized to the key's absence). Byte-idempotent:
/// `emit_animation(parse_animation(emit_animation(d)).unwrap())` reproduces the same text
/// (property-tested).
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: animation
/// name: humanoid-walk
/// duration: 0.6
/// loop: true
/// tracks:
///   upper-leg-l:
///     rotation:
///       - t: 0.0
///         v:
///           - 25.0
///           - 0.0
///           - 0.0
///       - t: 0.3
///         v:
///           - -25.0
///           - 0.0
///           - 0.0
///         ease: smooth
///       - t: 0.6
///         v:
///           - 25.0
///           - 0.0
///           - 0.0
/// ";
/// let clip = ludion_scene::check_animation(source).expect("the clip is valid");
/// assert_eq!(clip.duration, 0.6);
/// assert!(clip.looped);
/// assert_eq!(clip.tracks.len(), 1);
/// assert_eq!(ludion_scene::emit_animation(&clip), source); // canonical emit is a fixed point
/// ```
pub fn emit_animation(doc: &AnimationDocument) -> String {
    animation::emit(doc)
}

// --- Assembly documents (`*.assembly.yaml`) -----------------------------------------------------
//
// An assembly is the model of record for a multi-part voxel model (`Plan/voxel-animation.md`
// decisions 3, 4 and 9): it owns the one palette every part's glyphs resolve against, places
// [`PartDocument`] files into one assembly-global grid, declares the hierarchy (pivots, mirroring),
// and binds [`AnimationDocument`] clips by short name. Part and clip paths are stored verbatim,
// doc-relative — this crate is IO-free, so the caller resolves them, and the checks that need a
// part's content take already-resolved documents ([`check_assembly_parts`]).

/// Parses strict-subset YAML into an [`AssemblyDocument`], checking structure/shape only.
///
/// # Errors
/// `L1137` for a missing/wrong-typed key at any level, an unrecognized `kind`, a malformed
/// `size`/`place`/`pivot`, a `mirror` other than `x`, or an unknown key — each diagnostic carries a
/// JSON Pointer into the document where applicable.
pub fn parse_assembly(source: &str) -> Result<AssemblyDocument, Vec<Diagnostic>> {
    assembly::parse(source)
}

/// Validates data-consistency invariants of `doc`: the palette's keys/colors/material scalars (a
/// voxel document's rules exactly), non-empty `parts` with unique non-empty names, `parent` links
/// that resolve without self-parenting or cycles, exactly one root, finite pivots, and `place`
/// components inside `size`.
///
/// Whether each part *fits* needs the part files themselves — that check is
/// [`check_assembly_parts`].
///
/// # Errors (returned, not `Result`)
/// `L1138` diagnostics; an empty vector means `doc` is consistent.
pub fn validate_assembly(doc: &AssemblyDocument) -> Vec<Diagnostic> {
    assembly::validate(doc)
}

/// Parses and validates an assembly document in one step.
///
/// # Errors
/// Everything [`parse_assembly`] and [`validate_assembly`] can report.
pub fn check_assembly(source: &str) -> Result<AssemblyDocument, Vec<Diagnostic>> {
    let doc = parse_assembly(source)?;
    let diagnostics = validate_assembly(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Checks an assembly against its resolved part documents: every glyph a part draws with is defined
/// by the assembly's palette (parts have no palette of their own), and every part fits the
/// assembled grid at its `place`.
///
/// `parts` is aligned by index with [`AssemblyDocument::parts`] — the caller resolved each entry's
/// `file` and checked it with [`check_part`]. Pure and IO-free: this is the half of assembly
/// validation that needs two files, split out so the crate never reads one.
///
/// # Errors (returned, not `Result`)
/// `L1138` diagnostics, each pointing into the *assembly* document (`/parts/<i>/...`) and naming
/// the part and its file; an empty vector means the composition holds.
pub fn check_assembly_parts(
    assembly: &AssemblyDocument,
    parts: &[PartDocument],
) -> Vec<Diagnostic> {
    assembly::check_parts(assembly, parts)
}

/// Checks an assembly's `blends:` against its resolved clip documents: every point clip is bound,
/// loops, and (with `sync`) has the sync event among its own `events` (an assembly has no overlay).
///
/// `clips` is aligned by index with [`AssemblyDocument::clips`], like [`check_assembly_parts`]. Pure
/// and IO-free.
///
/// # Errors (returned, not `Result`)
/// `L1138` diagnostics pointing into the *assembly* document (`/blends/<name>/...`); an empty vector
/// means every blend holds.
pub fn check_assembly_blends(
    assembly: &AssemblyDocument,
    clips: &[AnimationDocument],
) -> Vec<Diagnostic> {
    assembly::check_blends(assembly, clips)
}

/// The voxel document the engine meshes for one part of an assembly: the assembly's palette over
/// that part's own grid, named by the part entry.
///
/// A `mirror: x` entry has its slices mirrored here (each row string reversed — a pure grid
/// transform, so winding and normals stay correct with no negative scale downstream). The part
/// entry's `pivot` stays in unmirrored part-local coordinates;
/// [`AssemblyPartEntry::pivot_in_mesh_frame`] converts it into this document's frame.
///
/// # Panics
/// If `index` is not a valid index into [`AssemblyDocument::parts`].
pub fn assembly_part_voxel(
    assembly: &AssemblyDocument,
    index: usize,
    part: &PartDocument,
) -> VoxelDocument {
    assembly::part_voxel(assembly, index, part)
}

/// Each part's parent index, `None` for the root — the resolved form of the `parent` name links.
///
/// Assumes a [`validate_assembly`]-clean document: exactly one entry is then `None` and every other
/// is a real index. On an invalid document a `parent` naming no part also reads as `None` (that is
/// already an `L1138` of its own).
pub fn assembly_parent_indices(doc: &AssemblyDocument) -> Vec<Option<usize>> {
    assembly::parent_indices(doc)
}

/// Composes every part of `assembly` into one assembly-global grid at rest pose, plus per-cell part
/// attribution (`Plan/voxel-animation.md` decisions 10-11) — the substrate `--exposure`, `--mirror-x`,
/// and connectivity analysis run over for an assembly, the way [`VoxelDocument`]'s own methods run
/// over a plain voxel model.
///
/// `parts` is aligned by index with [`AssemblyDocument::parts`], [`check_assembly_parts`]'s
/// convention.
///
/// # Panics
/// If `parts` is not the same length as [`AssemblyDocument::parts`].
pub fn assembly_assembled_grid(
    assembly: &AssemblyDocument,
    parts: &[PartDocument],
) -> AssembledGrid {
    assembly::assembled_grid(assembly, parts)
}

/// Writes `doc` in canonical form (stable field order, LF endings, ryu shortest-round-trip floats;
/// parts in authored order, clips sorted by short name, an origin `pivot` and an unset `mirror`
/// canonicalized to their keys' absence). Byte-idempotent:
/// `emit_assembly(parse_assembly(emit_assembly(d)).unwrap())` reproduces the same text
/// (property-tested).
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: assembly
/// name: player
/// size:
///   - 12
///   - 24
///   - 8
/// palette:
///   S:
///     color:
///       - 0.3
///       - 0.22
///       - 0.18
///       - 1.0
/// parts:
///   - name: torso
///     file: parts/torso.part.yaml
///     place:
///       - 4
///       - 10
///       - 2
///   - name: upper-arm-r
///     file: parts/arm.part.yaml
///     parent: torso
///     place:
///       - 9
///       - 14
///       - 3
///     pivot:
///       - 1.0
///       - 5.0
///       - 1.0
///     mirror: x
/// clips:
///   walk: anims/humanoid-walk.anim.yaml
/// ";
/// let model = ludion_scene::check_assembly(source).expect("the assembly is valid");
/// assert_eq!(model.size, [12, 24, 8]);
/// assert_eq!(model.parts.len(), 2);
/// assert!(model.parts[1].mirror_x);
/// assert_eq!(ludion_scene::emit_assembly(&model), source); // canonical emit is a fixed point
/// ```
pub fn emit_assembly(doc: &AssemblyDocument) -> String {
    assembly::emit(doc)
}

// --- Character documents (`*.character.yaml`) ---------------------------------------------------
//
// A character binds a skinned mesh to named animation clips with a per-clip speed and events overlay
// (`Plan/animation-clips.md` decisions 10–12). The cross-file rules take already-resolved documents
// ([`check_character_clips`]); this crate never reads a file.

/// Parses strict-subset YAML into a [`CharacterDocument`], checking structure/shape only.
///
/// # Errors
/// `L1199` for a missing/wrong-typed key, an unrecognized `kind`, or an unknown key; `L1198` for
/// `events: []` in a clip entry.
pub fn parse_character(source: &str) -> Result<CharacterDocument, Vec<Diagnostic>> {
    character::parse(source)
}

/// Validates data-consistency invariants of `doc`: non-blank `mesh` and clip paths, a finite
/// positive `speed` (`L1200`), and well-formed overlay events (`L1198`, no duration bound — see
/// [`check_character_clips`]).
pub fn validate_character(doc: &CharacterDocument) -> Vec<Diagnostic> {
    character::validate(doc)
}

/// Parses and validates a character document in one step.
///
/// # Errors
/// Everything [`parse_character`] and [`validate_character`] can report.
pub fn check_character(source: &str) -> Result<CharacterDocument, Vec<Diagnostic>> {
    let doc = parse_character(source)?;
    let diagnostics = validate_character(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Checks a character against its resolved clip documents and its mesh's joint names: overlay
/// `events` on a clip that has its own (`L1200`), an overlay event past the clip's `duration`
/// or at it on a looping clip (`L1198`), and a clip track naming a joint the mesh lacks (`L1200`, naming the nearest joint).
///
/// `clips` is aligned by index with [`CharacterDocument::clips`]. Pure and IO-free; every
/// diagnostic points into the *character* document.
///
/// # Panics
/// If `clips` is not the same length as [`CharacterDocument::clips`].
pub fn check_character_clips(
    doc: &CharacterDocument,
    clips: &[AnimationDocument],
    joints: &[&str],
) -> Vec<Diagnostic> {
    character::check_clips(doc, clips, joints)
}

/// Resolves a rig mask to one weight per skin joint or assembly part. `joints` is `(name, parent
/// index)` in the rig's order (any order: ancestors are walked). A listed joint takes its weight
/// (1.0 without `weights`); with `descendants`, an unlisted joint inherits its nearest listed
/// ancestor's weight; the rest are 0.
///
/// # Example
/// ```
/// use ludion_scene::{RigMask, resolve_rig_mask};
/// let mask = RigMask {
///     name: "upper".into(),
///     joints: vec!["torso".into()],
///     descendants: true,
///     weights: vec![],
/// };
/// let joints = [("hips", None), ("torso", Some(0)), ("arm", Some(1)), ("leg", Some(0))];
/// assert_eq!(resolve_rig_mask(&mask, &joints), vec![0.0, 1.0, 1.0, 0.0]);
/// ```
#[must_use]
pub fn resolve_rig_mask(mask: &RigMask, joints: &[(&str, Option<u32>)]) -> Vec<f32> {
    rig_mask::resolve_mask(mask, joints)
}

/// Writes `doc` in canonical form (stable field order, LF endings, ryu shortest-round-trip floats;
/// clips sorted by short name, `speed: 1.0` canonicalized to the key's absence). Byte-idempotent.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: character
/// name: goblin
/// mesh: goblin.mesh.yaml
/// clips:
///   idle:
///     clip: clips/idle.anim.yaml
///   run:
///     clip: clips/run.anim.yaml
///     speed: 1.5
///     events:
///       - t: 0.2
///         name: footstep
/// ";
/// let model = ludion_scene::check_character(source).expect("the character is valid");
/// assert_eq!(model.clips.len(), 2);
/// assert_eq!(model.clips[1].speed, 1.5);
/// assert_eq!(ludion_scene::emit_character(&model), source); // canonical emit is a fixed point
/// ```
pub fn emit_character(doc: &CharacterDocument) -> String {
    character::emit(doc)
}

// --- Path documents (`*.path.yaml`) -------------------------------------------------------------
//
// A path is a list of 3D points read as a spline (`Plan/character-controller.md` decision 9);
// [`PathSampler`] turns a validated one into position and direction by arc length.

/// Parses strict-subset YAML into a [`PathDocument`], checking structure/shape only.
///
/// # Errors
/// `L1213` for a missing/wrong-typed key, an unrecognized `kind`, an unknown key or `interpolation`,
/// or a point that is not three numbers.
pub fn parse_path(source: &str) -> Result<PathDocument, Vec<Diagnostic>> {
    path::parse(source)
}

/// Validates data-consistency invariants of `doc`: enough points, finite f32-range components and no
/// coincident neighbours, and a relative, non-blank `generated_from` (`L1214`).
pub fn validate_path(doc: &PathDocument) -> Vec<Diagnostic> {
    path::validate(doc)
}

/// Parses and validates a path document in one step.
///
/// # Errors
/// Everything [`parse_path`] and [`validate_path`] can report.
pub fn check_path(source: &str) -> Result<PathDocument, Vec<Diagnostic>> {
    let doc = parse_path(source)?;
    let diagnostics = validate_path(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form (stable field order, LF endings, ryu shortest-round-trip floats;
/// `generated_from` after `name` when present; `interpolation` and `closed` always spelled).
/// Byte-idempotent.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: path
/// name: lift
/// interpolation: catmull_rom
/// closed: false
/// points:
///   - - 0.0
///     - 0.0
///     - 0.0
///   - - 4.0
///     - 0.5
///     - -6.0
/// ";
/// let path = ludion_scene::check_path(source).expect("the path is valid");
/// assert_eq!(path.points.len(), 2);
/// assert_eq!(ludion_scene::emit_path(&path), source); // canonical emit is a fixed point
/// let sampler = ludion_scene::PathSampler::new(&path);
/// assert_eq!(sampler.point_at(0.0), [0.0, 0.0, 0.0]);
/// ```
pub fn emit_path(doc: &PathDocument) -> String {
    path::emit(doc)
}

// --- Patrol documents (`*.patrol.yaml`) ---------------------------------------------------------
//
// A patrol is an ordered list of stops a guard walks (`Plan/nav.md` decision 13); the engine's
// `patrol_update` reads a validated one.

/// Parses strict-subset YAML into a [`PatrolDocument`], checking structure/shape only.
///
/// # Errors
/// `L1522` for a missing/wrong-typed key, an unrecognized `kind`, an unknown key or `mode`, or a
/// point whose `at` is not three numbers.
pub fn parse_patrol(source: &str) -> Result<PatrolDocument, Vec<Diagnostic>> {
    patrol::parse(source)
}

/// Validates data-consistency invariants of `doc`: at least one point, finite f32-range numbers,
/// non-negative `wait`, positive look `hold`, non-empty `tag` (`L1523`).
pub fn validate_patrol(doc: &PatrolDocument) -> Vec<Diagnostic> {
    patrol::validate(doc)
}

/// Parses and validates a patrol document in one step.
///
/// # Errors
/// Everything [`parse_patrol`] and [`validate_patrol`] can report.
pub fn check_patrol(source: &str) -> Result<PatrolDocument, Vec<Diagnostic>> {
    let doc = parse_patrol(source)?;
    let diagnostics = validate_patrol(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form (stable field order, LF endings, ryu shortest-round-trip floats;
/// `mode` always spelled, default point fields omitted). Byte-idempotent.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: patrol
/// name: yard
/// mode: loop
/// points:
///   - at:
///       - 0.0
///       - 0.0
///       - 0.0
///     wait: 1.5
///     look:
///       - yaw_deg: 45.0
///         hold: 0.5
///   - at:
///       - 4.0
///       - 0.0
///       - -3.0
/// ";
/// let patrol = ludion_scene::check_patrol(source).expect("the patrol is valid");
/// assert_eq!(patrol.points.len(), 2);
/// assert_eq!(ludion_scene::emit_patrol(&patrol), source); // canonical emit is a fixed point
/// ```
pub fn emit_patrol(doc: &PatrolDocument) -> String {
    patrol::emit(doc)
}

// --- Paired documents (`*.paired.yaml`) ---------------------------------------------------------
//
// A paired action aligns two or more voxel-assembly roles and plays a clip on each
// (`Plan/paired-animations.md` decisions 15–21); the engine's `check_paired_files` checks one
// against its role assemblies.

/// Parses strict-subset YAML into a [`PairedDocument`], checking structure/shape only.
///
/// # Errors
/// `L1752` for a missing/wrong-typed key, an unrecognized `kind`, an unknown key, or a role, offset,
/// event or contact of the wrong shape.
pub fn parse_paired(source: &str) -> Result<PairedDocument, Vec<Diagnostic>> {
    paired::parse(source)
}

/// Validates the data rules of `doc` that need no other file: role references, offsets, `ticks`,
/// `scale`, event order and contact sides (`L1753`).
pub fn validate_paired(doc: &PairedDocument) -> Vec<Diagnostic> {
    paired::validate(doc)
}

/// Parses and validates a paired document in one step.
///
/// # Errors
/// Everything [`parse_paired`] and [`validate_paired`] can report.
pub fn check_paired(source: &str) -> Result<PairedDocument, Vec<Diagnostic>> {
    let doc = parse_paired(source)?;
    let diagnostics = validate_paired(&doc);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form (stable field order, roles and offsets sorted by name, LF endings,
/// ryu shortest-round-trip floats). Byte-idempotent.
///
/// # Example
/// ```
/// let source = "\
/// format: 1
/// kind: paired
/// name: choke
/// roles:
///   instigator:
///     assembly: humanoid.assembly.yaml
///     clip: carry
///   target:
///     assembly: humanoid.assembly.yaml
///     clip: idle
/// align:
///   reference: instigator
///   moves: instigator
///   ticks: 12
///   offsets:
///     target:
///       translation:
///         - 0.0
///         - 0.0
///         - -0.6
///       yaw_deg: 180.0
/// events:
///   - name: grab
///     t: 0.25
/// ";
/// let paired = ludion_scene::check_paired(source).expect("the document is valid");
/// assert_eq!(paired.roles.len(), 2);
/// assert_eq!(ludion_scene::emit_paired(&paired), source); // canonical emit is a fixed point
/// ```
pub fn emit_paired(doc: &PairedDocument) -> String {
    paired::emit(doc)
}

// --- Prefab documents (`*.prefab.yaml`) ---------------------------------------------------------
//
// A prefab is a scene's `entities:` list under a `kind: prefab` envelope; see the [`prefab`] module.

/// Parses strict-subset YAML into a [`PrefabDocument`], checking structure and the part-name rules.
///
/// # Errors
/// `L1226` for an envelope problem (including a scene-only `seed`/`physics` key), `L1227` for a
/// part with no usable name, `L1228` for a duplicate part name, plus the scene's entity-structure
/// codes (`L1009`, `L1010`).
///
/// # Example
/// ```
/// let source = "format: 1\nkind: prefab\nentities:\n  - name: root\n    components: {}\n";
/// let doc = ludion_scene::parse_prefab(source).unwrap();
/// assert_eq!(doc.entities.len(), 1);
///
/// let unnamed = "format: 1\nkind: prefab\nentities:\n  - components: {}\n";
/// let diagnostics = ludion_scene::parse_prefab(unnamed).unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L1227");
/// ```
pub fn parse_prefab(source: &str) -> Result<PrefabDocument, Vec<Diagnostic>> {
    prefab::parse(source)
}

/// Validates every component of the prefab's parts against `registry`, as [`validate`] does a
/// scene's; an entity reference must name a part of the same prefab.
///
/// # Example
/// ```
/// let registry = ludion_ecs::Registry::new();
/// let source = "format: 1\nkind: prefab\nentities:\n  - name: root\n    components:\n      Nope: {}\n";
/// let doc = ludion_scene::parse_prefab(source).unwrap();
/// let diagnostics = ludion_scene::validate_prefab(&doc, &registry);
/// assert_eq!(diagnostics[0].code.as_str(), "L1013");
/// ```
pub fn validate_prefab(doc: &PrefabDocument, registry: &Registry) -> Vec<Diagnostic> {
    prefab::validate(doc, registry)
}

/// Parses and validates a prefab document in one step.
///
/// # Errors
/// Everything [`parse_prefab`] and [`validate_prefab`] can report.
///
/// # Example
/// ```
/// let registry = ludion_ecs::Registry::new();
/// let source = "format: 1\nkind: prefab\nentities:\n  - name: root\n    components: {}\n";
/// assert!(ludion_scene::check_prefab(source, &registry).is_ok());
/// let seeded = "format: 1\nkind: prefab\nseed: 3\nentities: []\n";
/// assert!(ludion_scene::check_prefab(seeded, &registry).is_err());
/// ```
pub fn check_prefab(source: &str, registry: &Registry) -> Result<PrefabDocument, Vec<Diagnostic>> {
    let doc = parse_prefab(source)?;
    let diagnostics = validate_prefab(&doc, registry);
    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Writes `doc` in canonical form: the `format`/`kind` envelope, `name`, then the parts as a scene
/// writes its entities. Byte-idempotent.
///
/// # Example
/// ```
/// let registry = ludion_ecs::Registry::new();
/// let source = "format: 1\nkind: prefab\nname: goblin\nentities:\n  - name: pelvis\n    components: {}\n";
/// let doc = ludion_scene::check_prefab(source, &registry).unwrap();
/// assert_eq!(ludion_scene::emit_prefab(&doc, &registry), source);
/// ```
pub fn emit_prefab(doc: &PrefabDocument, registry: &Registry) -> String {
    prefab::emit(doc, registry)
}

// --- Game data-doc documents (a game-registered `kind:` + its schema) --------------------------
//
// A game registers a doc kind (a `kind` string + its `schemars` JSON Schema) and gets the same
// schema-driven validation and canonical emit scene components enjoy, for a type this crate never
// names. Unlike mesh/voxel (fixed, hand-parsed shapes), a data-doc is generic: parse → the shared
// schema [`validate`] checker → schema-ordered [`emit`]. See the [`doc`] module docs and
// `Plan/phase-15-data-docs.md` for the design.

/// Parses and validates a game data-doc against `kind`'s JSON `schema`, returning the doc's field
/// map with the `format`/`kind` envelope stripped — ready for [`deserialize_doc`] into the
/// registered type (or use [`parse_doc`], which does both).
///
/// The document is `format: 1` + `kind: <kind>` at the top, then the type's own fields flattened at
/// the top level. The envelope is validated (`L1119`), then the remaining fields are checked against
/// `schema` with the same checker scene components use (`L1120`, JSON Pointer + nearest-match).
/// Strict-YAML syntax errors surface the shared `L1001`–`L1009` codes.
///
/// # Errors
/// `L1119` for a malformed envelope (missing/wrong `format`, missing/wrong/mismatched `kind`, or a
/// top-level key that is a near-miss of an envelope key); `L1120` for fields that do not match the
/// schema; `L1001`–`L1009` for strict-YAML violations. Non-syntax diagnostics are collected in one
/// pass.
///
/// # Example
/// ```
/// use serde_json::json;
///
/// // A hand-built schema for a `weapon` kind (what `schema_for!(WeaponDoc)` would produce).
/// let schema = json!({
///     "type": "object",
///     "properties": { "name": { "type": "string" }, "damage": { "type": "number" } },
///     "required": ["name", "damage"],
/// });
///
/// let source = "format: 1\nkind: weapon\nname: crossbow\ndamage: 12.0\n";
/// let fields = ludion_scene::check_doc(source, "weapon", &schema).expect("valid weapon doc");
/// assert_eq!(fields["name"], json!("crossbow"));
/// assert_eq!(fields["damage"], json!(12.0));
///
/// // A wrong-typed field is an L1120 with a JSON Pointer to the field.
/// let broken = "format: 1\nkind: weapon\nname: crossbow\ndamage: fast\n";
/// let diagnostics = ludion_scene::check_doc(broken, "weapon", &schema).unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L1120");
/// assert_eq!(diagnostics[0].path.as_deref(), Some("/damage"));
/// ```
pub fn check_doc(source: &str, kind: &str, schema: &Value) -> Result<Value, Vec<Diagnostic>> {
    doc::check_doc(source, kind, schema)
}

/// Writes a game data-doc in canonical form: the `format`/`kind` envelope, then `value`'s fields in
/// `schema`-declaration order (ryu shortest-round-trip floats, LF endings) — the doc analog of
/// [`emit`]. `value` is the type's object form (`serde_json::to_value(&doc)`).
///
/// Byte-idempotent for a value the schema accepts: `emit_doc → check_doc → emit_doc` reproduces the
/// same text (property-tested).
///
/// # Example
/// ```
/// use serde_json::json;
///
/// let schema = json!({
///     "type": "object",
///     "properties": { "name": { "type": "string" }, "damage": { "type": "number" } },
/// });
/// let value = json!({ "name": "crossbow", "damage": 12.0 });
/// assert_eq!(
///     ludion_scene::emit_doc("weapon", &value, &schema),
///     "format: 1\nkind: weapon\nname: crossbow\ndamage: 12.0\n"
/// );
/// ```
pub fn emit_doc(kind: &str, value: &Value, schema: &Value) -> String {
    doc::emit_doc(kind, value, schema)
}

/// Checks that a registered data-doc or profile type can round-trip through the doc envelope: its
/// schema must not declare a top-level `format` or `kind` field, which would collide with the
/// envelope's own keys. A nested field of either name is fine.
///
/// # Errors
/// `L1257` naming the offending field, with a rename suggestion.
///
/// # Example
/// ```
/// use serde_json::json;
///
/// let tagged = json!({ "type": "object", "properties": { "kind": { "type": "string" } } });
/// let error = ludion_scene::check_doc_type("weapon", &tagged).unwrap_err();
/// assert_eq!(error.code.as_str(), "L1257");
///
/// let nested = json!({
///     "type": "object",
///     "properties": {
///         "stats": { "type": "object", "properties": { "kind": { "type": "string" } } },
///     },
/// });
/// assert!(ludion_scene::check_doc_type("weapon", &nested).is_ok());
/// ```
pub fn check_doc_type(kind: &str, schema: &Value) -> Result<(), Diagnostic> {
    doc::check_doc_type(kind, schema)
}

/// Reads a game data-doc into `T` with the boot loader's steps: the reserved-field check (`L1257`),
/// the envelope (`L1119`) and schema (`L1120`) check over strict YAML (`L100x`), then the
/// deserialize (`L1120`).
///
/// It enforces no count and does not run the game's `validate_docs`; `kind` is not checked against
/// the engine's kinds. A test parse and the boot parse give the same value.
///
/// # Errors
/// The diagnostics above, collected for a schema mismatch.
///
/// # Example
/// ```
/// use schemars::JsonSchema;
/// use serde::Deserialize;
///
/// #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
/// struct Weapon {
///     name: String,
///     damage: f32,
/// }
///
/// let source = "format: 1\nkind: weapon\nname: crossbow\ndamage: 12.0\n";
/// let weapon: Weapon = ludion_scene::parse_doc(source, "weapon").expect("valid weapon doc");
/// assert_eq!(weapon, Weapon { name: "crossbow".into(), damage: 12.0 });
///
/// let broken = "format: 1\nkind: weapon\nname: crossbow\ndamage: fast\n";
/// let diagnostics = ludion_scene::parse_doc::<Weapon>(broken, "weapon").unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L1120");
/// assert_eq!(diagnostics[0].path.as_deref(), Some("/damage"));
/// ```
pub fn parse_doc<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    source: &str,
    kind: &str,
) -> Result<T, Vec<Diagnostic>> {
    doc::parse_doc(source, kind)
}

/// The step after [`check_doc`]: deserializes a schema-validated field map into `T`. The boot
/// loader uses it; prefer [`parse_doc`].
///
/// # Errors
/// One `L1120` when serde refuses a map the schema accepted.
///
/// # Example
/// ```
/// use serde::Deserialize;
/// use serde_json::json;
///
/// #[derive(Debug, Deserialize)]
/// struct Weapon {
///     damage: f32,
/// }
///
/// let error = ludion_scene::deserialize_doc::<Weapon>(json!({ "damage": "x" })).unwrap_err();
/// assert_eq!(error.code.as_str(), "L1120");
/// ```
pub fn deserialize_doc<T: serde::de::DeserializeOwned>(fields: Value) -> Result<T, Diagnostic> {
    doc::deserialize_doc(fields)
}

// --- Player-profile documents (`save/profile.yaml`, `kind: profile`) ---------------------------
//
// A profile is a game's persistent save state. It reuses the data-doc envelope + [`emit_doc`]
// verbatim (`kind: profile`) on the *write* side, but its *load* is lenient: `open_profile`
// validates only the envelope and never runs the strict field checker, so a player's accumulated
// save migrates instead of hard-failing on schema drift. See the [`profile`] module docs and
// `Plan/phase-16-profile.md` for the design.

/// Opens a player-profile document leniently: validates and strips its `format`/`kind` envelope and
/// returns the remaining field map plus the declared `format` version, **without** running the
/// strict field checker.
///
/// Unlike [`check_doc`], `open_profile` never rejects an unknown, missing, or retyped field — a
/// profile is a player's accumulated save that must *migrate*, so field-level drift is the engine's
/// lenient migration's job, not a parse error. The only errors are a malformed envelope (surfaced as
/// `L1121`) or a strict-YAML syntax violation (the shared `L100x` codes). The canonical *write* is
/// the existing [`emit_doc`] with `kind: profile`.
///
/// # Errors
/// `L1121` for a malformed envelope — a missing/non-integer `format`, a missing/non-string/
/// non-`profile` `kind`, or a non-mapping document (with a JSON Pointer at the offending key where
/// one exists); `L1001`–`L1009` for strict-YAML violations.
///
/// # Example
/// ```
/// let source = "format: 1\nkind: profile\nunlocked_crossbow: true\nruns: 7\n";
/// let profile = ludion_scene::open_profile(source).expect("a well-formed profile envelope");
/// assert_eq!(profile.format, 1);
/// // The envelope keys are stripped; the game's own fields remain, unvalidated.
/// assert_eq!(profile.fields["unlocked_crossbow"], serde_json::json!(true));
/// assert_eq!(profile.fields["runs"], serde_json::json!(7));
/// assert!(profile.fields.get("format").is_none());
///
/// // A missing `kind` is an L1121 — the engine backs the file up and starts a default profile.
/// let broken = "format: 1\nunlocked_crossbow: true\n";
/// let diagnostics = ludion_scene::open_profile(broken).unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L1121");
/// ```
pub fn open_profile(source: &str) -> Result<ProfileEnvelope, Vec<Diagnostic>> {
    profile::open_profile(source)
}

// --- Player-settings documents (`save/settings.yaml`, `kind: settings`) ------------------------
//
// The per-machine settings file shares the profile's lenient envelope open; the engine's
// `settings` module owns the field-level leniency (`Plan/settings.md`).

/// Opens a player-settings document leniently: validates and strips its `format`/`kind` envelope
/// (`kind: settings`) and returns the remaining field map plus the declared `format`, **without**
/// running the strict field checker. The canonical *write* is [`emit_doc`] with `kind: settings`.
///
/// # Errors
/// `L1700` for a malformed envelope (a missing/non-integer `format`, a missing/non-`settings`
/// `kind`, a non-mapping document); `L1001`–`L1009` for strict-YAML violations.
///
/// # Example
/// ```
/// let source = "format: 1\nkind: settings\naudio:\n  master: 0.5\n";
/// let settings = ludion_scene::open_settings(source).expect("a well-formed settings envelope");
/// assert_eq!(settings.format, 1);
/// assert_eq!(settings.fields["audio"]["master"], serde_json::json!(0.5));
///
/// // A `kind: profile` file is an L1700 — the engine backs it up and starts from defaults.
/// let wrong = "format: 1\nkind: profile\n";
/// let diagnostics = ludion_scene::open_settings(wrong).unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L1700");
/// ```
pub fn open_settings(source: &str) -> Result<ProfileEnvelope, Vec<Diagnostic>> {
    profile::open_envelope(
        source,
        "settings",
        ludion_core::codes::code::SETTINGS_ENVELOPE,
    )
}

// --- Input-bindings documents (`save/input-bindings.yaml`, `kind: input-bindings`) --------------
//
// The player's rebinds share the same lenient envelope open; the engine's `input_bindings`
// module owns the per-action leniency (`Plan/input-actions.md`).

/// Opens an input-bindings document leniently: validates and strips its `format`/`kind` envelope
/// (`kind: input-bindings`) and returns the remaining field map plus the declared `format`,
/// **without** compiling any binding. The canonical *write* is [`emit_doc`] with
/// `kind: input-bindings`.
///
/// # Errors
/// `L1636` for a malformed envelope (a missing/non-integer `format`, a missing/non-`input-bindings`
/// `kind`, a non-mapping document); `L1001`–`L1009` for strict-YAML violations.
///
/// # Example
/// ```
/// let source = "format: 1\nkind: input-bindings\nactions:\n  jump:\n    bindings: []\n";
/// let doc = ludion_scene::open_input_bindings(source).expect("a well-formed envelope");
/// assert_eq!(doc.format, 1);
/// assert!(doc.fields["actions"]["jump"].is_object());
///
/// // A `kind: settings` file is an L1636 — the engine backs it up and starts from the defaults.
/// let wrong = "format: 1\nkind: settings\n";
/// let diagnostics = ludion_scene::open_input_bindings(wrong).unwrap_err();
/// assert_eq!(diagnostics[0].code.as_str(), "L1636");
/// ```
pub fn open_input_bindings(source: &str) -> Result<ProfileEnvelope, Vec<Diagnostic>> {
    profile::open_envelope(
        source,
        "input-bindings",
        ludion_core::codes::code::INPUT_BINDINGS_ENVELOPE,
    )
}
