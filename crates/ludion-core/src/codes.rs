//! The central diagnostic-code table.
//!
//! Every `L####` code in the engine is registered here, in one grep-able const array — not scattered
//! across crates via `inventory`/`linkme` collection magic. `ludion explain L####` is a lookup in
//! this table ([`explain`]).
//!
//! Codes are partitioned by domain on their first digit (architecture doc §2 "Diagnostics"):
//!
//! | Range   | Domain           |
//! |---------|------------------|
//! | `L0xxx` | core / ecs       |
//! | `L1xxx` | scene / assets   |
//! | `L2xxx` | render           |
//! | `L3xxx` | protocol / server|
//! | `L4xxx` | sim              |
//! | `L5xxx` | audio            |
//!
//! Only codes actually constructible by code that exists *today* are registered; future crates add
//! their own rows as they come online. A unit test asserts the table is internally consistent and
//! that every declared [`DiagnosticCode`] constant appears in it.
//!
//! Each row also carries the code's [`Severity`], and that row is the source of truth:
//! [`Diagnostic::new`](crate::diagnostics::Diagnostic::new) reads it, so whether a code is fatal is
//! decided once, here, instead of by whichever container a call site drops the diagnostic into.
//!
//! # Example
//!
//! ```
//! use ludion_core::codes;
//! use ludion_core::diagnostics::Severity;
//!
//! let entry = codes::explain("L0001").expect("L0001 is registered");
//! assert_eq!(entry.title, "unknown diagnostic code");
//! assert_eq!(entry.severity, Severity::Error);
//! assert!(codes::explain("L9999").is_none());
//! ```

use crate::diagnostics::{DiagnosticCode, Severity};

/// The canonical `L####` code constants owned by `ludion-core`.
///
/// Later crates declare their own constants in the same style; all of them must be registered in
/// [`CODES`].
pub mod code {
    use crate::diagnostics::DiagnosticCode;

    /// `L0001` — the code `ludion explain` itself raises when asked about an unregistered code.
    pub const UNKNOWN_DIAGNOSTIC_CODE: DiagnosticCode = DiagnosticCode::new("L0001");

    /// `L0002` — an operation named an entity that is not alive (never existed, already despawned,
    /// or a stale handle whose slot was recycled). Raised by `ludion-ecs`.
    pub const DEAD_ENTITY: DiagnosticCode = DiagnosticCode::new("L0002");

    /// `L0004` — a lookup named a component that is not registered in the component registry.
    /// Raised by `ludion-ecs`.
    pub const UNKNOWN_COMPONENT: DiagnosticCode = DiagnosticCode::new("L0004");

    /// `L0005` — a component registration collided with an existing one (same name, or the same Rust
    /// type under a different name). Raised by `ludion-ecs`.
    pub const DUPLICATE_COMPONENT_REGISTRATION: DiagnosticCode = DiagnosticCode::new("L0005");

    /// `L0006` — a component value failed to deserialize from JSON while being inserted into the
    /// world. Raised by `ludion-ecs`.
    pub const COMPONENT_DESERIALIZE_FAILED: DiagnosticCode = DiagnosticCode::new("L0006");

    /// `L0007` — a component value failed to serialize to JSON while being read out of the world.
    /// Raised by `ludion-ecs`.
    pub const COMPONENT_SERIALIZE_FAILED: DiagnosticCode = DiagnosticCode::new("L0007");

    /// `L0008` — the agent-docs directory (`docs/` at the repo root) could not be located.
    /// Raised by `ludion-cli` (`ludion docs`).
    pub const DOCS_DIR_NOT_FOUND: DiagnosticCode = DiagnosticCode::new("L0008");

    /// `L0009` — `ludion docs` was asked for a topic that has no doc file. Raised by
    /// `ludion-cli`.
    pub const DOCS_UNKNOWN_TOPIC: DiagnosticCode = DiagnosticCode::new("L0009");

    /// `L0010` — a topic doc file has missing or malformed frontmatter. Raised by `ludion-cli`.
    pub const DOCS_MALFORMED: DiagnosticCode = DiagnosticCode::new("L0010");

    /// `L0011` — `ludion new` was asked to scaffold a project inside the engine workspace's member
    /// globs (`crates/*` or `games/*`), which would make the project a member of two workspaces.
    /// Raised by `ludion-cli`.
    pub const NEW_TARGET_IN_WORKSPACE: DiagnosticCode = DiagnosticCode::new("L0011");

    /// `L4390` — a scaffolded project's `.ludion/engine` link is missing, dangling, or not an engine
    /// checkout. Raised by the project's `scripts/ludion.mjs`.
    pub const ENGINE_LINK_MISSING: DiagnosticCode = DiagnosticCode::new("L4390");

    /// `L4391` — `link-engine` was given a path that is not an engine checkout. Raised by the
    /// project's `scripts/ludion.mjs`.
    pub const ENGINE_LINK_TARGET_INVALID: DiagnosticCode = DiagnosticCode::new("L4391");

    /// `L4392` — `.ludion/engine` exists and is not a link, so `link-engine` refuses to replace it.
    /// Raised by the project's `scripts/ludion.mjs`.
    pub const ENGINE_LINK_OCCUPIED: DiagnosticCode = DiagnosticCode::new("L4392");

    /// `L4393` — `ludion new` scaffolded the files but could not create the `.ludion/engine` link.
    /// Raised by `ludion-cli`.
    pub const NEW_ENGINE_LINK_FAILED: DiagnosticCode = DiagnosticCode::new("L4393");

    /// `L0012` — `ludion dev` was run from a directory that is not a runnable game project (no
    /// `Cargo.toml`, or one with no binary target to host). Raised by `ludion-cli` (`ludion dev`).
    pub const DEV_NOT_A_GAME: DiagnosticCode = DiagnosticCode::new("L0012");

    /// `L0013` — `ludion dev`'s game child process failed: it exited before it began listening,
    /// closed its output without a listening line, printed none within the 20 s listen window,
    /// could not be started, or exited with a non-zero status. Carries the child's captured output. Raised by `ludion-cli`
    /// (`ludion dev`).
    pub const DEV_CHILD_FAILED: DiagnosticCode = DiagnosticCode::new("L0013");

    /// `L0014` — the `LUDION_THREADS` environment variable holds something that is neither a
    /// positive integer nor `auto`. Raised by any host resolving its sim worker count at startup
    /// (`ludion::run`, `ludion-cli`).
    pub const THREADS_ENV_INVALID: DiagnosticCode = DiagnosticCode::new("L0014");

    /// `L0015` — `ludion docs <topic> --section <heading>` found no ATX heading in the topic whose
    /// trimmed text matches (case-insensitively) the requested one. Raised by `ludion-cli`.
    pub const DOCS_UNKNOWN_SECTION: DiagnosticCode = DiagnosticCode::new("L0015");

    /// `L0016` — a render quality setting (a `--quality` flag, or `LUDION_QUALITY`) is neither a
    /// preset name nor a usable override file: an unknown preset, an unreadable path, malformed
    /// TOML, an unknown key or section, or a value of the wrong type. Raised by any host resolving
    /// its quality at startup (`ludion-engine`'s `resolve_startup_quality`).
    pub const QUALITY_SETTING_INVALID: DiagnosticCode = DiagnosticCode::new("L0016");

    /// `L0017` — a render extent (a `--render-size` flag, or `LUDION_RENDER_SIZE`) is not a
    /// `WIDTHxHEIGHT` pixel pair inside the supported range: a missing or wrong separator, a
    /// non-numeric axis, a zero axis, or one past the 7680×4320 ceiling. Raised by any host
    /// resolving its render extent at startup (`ludion-engine`'s `resolve_startup_render_size`).
    pub const RENDER_SIZE_INVALID: DiagnosticCode = DiagnosticCode::new("L0017");

    /// `L0018` — a host was asked for `--window` in a build compiled without the `ludion` facade's
    /// `window` cargo feature, so there is no native viewer to open. Raised by `ludion`'s
    /// `host_scene` before anything binds.
    pub const WINDOW_FEATURE_OFF: DiagnosticCode = DiagnosticCode::new("L0018");

    /// `L0019` — a host was given no scene: `ludion::run` with neither `--scene` nor a `default_scene`,
    /// or a host with no scene argument (`ludion::launch`, the embedding core) handed an engine
    /// builder without one, so there is nothing to host. Raised by `ludion`'s hosts before anything
    /// is built.
    pub const NO_DEFAULT_SCENE: DiagnosticCode = DiagnosticCode::new("L0019");

    /// `L0020` — the embedding core's C ABI was handed an argument it cannot use: a null or
    /// non-UTF-8 root, an unknown surface kind, a null view, or an unknown pad button, axis or touch
    /// phase. Raised by `ludion`'s `ffi` module.
    pub const EMBED_ARGUMENT_INVALID: DiagnosticCode = DiagnosticCode::new("L0020");

    /// `L0021` — a touch layout (`touch.yaml`, `kind: touch-layout`) names a control the mapper
    /// cannot use. Raised by `ludion`'s embedding core when it starts.
    pub const TOUCH_LAYOUT_INVALID: DiagnosticCode = DiagnosticCode::new("L0021");

    /// `L0022` — `ludion export` for iOS cannot find its toolchain: Xcode's SDK for the target, or
    /// the rustup target itself. Raised by `ludion-cli`'s `export` verb before any build.
    pub const EXPORT_TOOLCHAIN_UNAVAILABLE: DiagnosticCode = DiagnosticCode::new("L0022");

    /// `L0023` — the project manifest (`ludion.toml`) is missing, unreadable, malformed, or lacks a
    /// key the verb needs. Raised by `ludion-engine`'s `manifest` reader for every consumer (`export`,
    /// `sync`, every game host), and by `ludion-cli`'s `new --refresh-guide` when no manifest is found.
    pub const MANIFEST_INVALID: DiagnosticCode = DiagnosticCode::new("L0023");

    /// `L0024` — `ludion export` was given a flag combination it cannot honour: `--run` on a
    /// device, `--replay` or arguments after `--` without the `--debug` (and `--run`) that read
    /// them, or a `--replay` that is not a file. Raised by `ludion-cli`'s `export` verb before
    /// anything is read or built.
    pub const EXPORT_FLAGS_INVALID: DiagnosticCode = DiagnosticCode::new("L0024");

    /// `L0025` — an entity allocator state or live set is inconsistent: a duplicate or
    /// out-of-range free-list index, a duplicate live slot index, or more slots than the engine's
    /// entity limit (`MAX_ENTITY_SLOTS`, 16,777,216 concurrent entities). Raised by `ludion-ecs`
    /// when a world is restored from it.
    pub const ENTITY_ALLOCATOR_STATE_INVALID: DiagnosticCode = DiagnosticCode::new("L0025");

    /// `L0026` — a component read through `ctx.world` inside a `par_for_each` region whose
    /// exclusive column the region lifted out of the world. Raised by `ludion-ecs`.
    pub const LIFTED_COMPONENT_READ: DiagnosticCode = DiagnosticCode::new("L0026");

    /// `L0027` — a component type has values in the world but is absent from the registry, so the
    /// world hash, snapshots, saves and the wire cannot see it. A warning raised by `ludion-engine`.
    pub const COMPONENT_NOT_REGISTERED: DiagnosticCode = DiagnosticCode::new("L0027");

    /// `L0028` — a system crossed the sim/presentation boundary: a sim system touched a presentation
    /// entity, or a presentation system wrote sim state. Raised by `ludion-ecs`.
    pub const PRESENTATION_BOUNDARY: DiagnosticCode = DiagnosticCode::new("L0028");

    /// `L0029` — an engine operation failed but produced no diagnostic to report — an engine bug.
    /// Raised by `ludion-engine`.
    pub const OPERATION_FAILED_SILENTLY: DiagnosticCode = DiagnosticCode::new("L0029");

    /// `L0030` — a create verb (`ludion new`, `voxel new`, `voxel upscale`, `sculpt new`,
    /// `ludion import <png>` onto an existing sprite document, or a glTF import whose `--out` holds
    /// outputs the new import would not write) found its target already present and refuses to
    /// overwrite it. Raised by `ludion-cli`.
    pub const CREATE_TARGET_EXISTS: DiagnosticCode = DiagnosticCode::new("L0030");

    /// `L0031` — `ludion check-scene` was handed a document whose `kind:` another verb validates.
    /// Raised by `ludion-cli`.
    pub const KIND_CHECKED_ELSEWHERE: DiagnosticCode = DiagnosticCode::new("L0031");

    /// `L0032` — `ludion dev` could not spawn or wait on `cargo build`. Raised by `ludion-cli`.
    pub const DEV_CARGO_FAILED: DiagnosticCode = DiagnosticCode::new("L0032");

    /// `L0033` — `ludion dev` continues with reduced function (status file, kept snapshot or hot
    /// reload fingerprints unavailable). A warning. Raised by `ludion-cli`.
    pub const DEV_DEGRADED: DiagnosticCode = DiagnosticCode::new("L0033");

    /// `L0034` — `ludion export` failed resolving its output, building the staticlib or running the
    /// simulator. Raised by `ludion-cli`.
    pub const EXPORT_FAILED: DiagnosticCode = DiagnosticCode::new("L0034");

    /// `L0035` — `ludion new` was given a name that would scaffold a project that does not build.
    /// Raised by `ludion-cli`.
    pub const NEW_NAME_INVALID: DiagnosticCode = DiagnosticCode::new("L0035");

    /// `L0036` — a project's `LUDION-GUIDE.md` differs from the CLI's engine guide. Raised by
    /// `ludion-cli`.
    pub const GUIDE_STALE: DiagnosticCode = DiagnosticCode::new("L0036");

    /// `L0037` — `LUDION_FULLSCREEN` is neither `1` nor `0`. Raised by the window hosts.
    pub const FULLSCREEN_SETTING_INVALID: DiagnosticCode = DiagnosticCode::new("L0037");

    /// `L0038` — the engine builder's default scene and the project manifest's `default_scene` name
    /// different files. Raised by the game hosts and by `ludion export`.
    pub const DEFAULT_SCENE_MISMATCH: DiagnosticCode = DiagnosticCode::new("L0038");

    // --- L1xxx: scene / assets (raised by `ludion-scene`, `ludion-assets`) ---

    /// `L1001` — a scene file is not well-formed YAML at all (unbalanced structure, bad
    /// indentation, an unterminated quote…). Raised by `ludion-scene`.
    pub const SCENE_YAML_SYNTAX: DiagnosticCode = DiagnosticCode::new("L1001");

    /// `L1002` — a scene file used a YAML anchor (`&name`) or alias (`*name`), which the strict
    /// subset forbids. Raised by `ludion-scene`.
    pub const SCENE_YAML_ANCHOR: DiagnosticCode = DiagnosticCode::new("L1002");

    /// `L1003` — a scene file used a YAML tag (`!!int`, `!custom`), which the strict subset
    /// forbids. Raised by `ludion-scene`.
    pub const SCENE_YAML_TAG: DiagnosticCode = DiagnosticCode::new("L1003");

    /// `L1004` — a scene file contains more than one YAML document (`---` / `...`), which the
    /// strict subset forbids. Raised by `ludion-scene`.
    pub const SCENE_YAML_MULTIPLE_DOCUMENTS: DiagnosticCode = DiagnosticCode::new("L1004");

    /// `L1005` — a scene file used a non-empty flow collection (`[1, 2]`, `{a: 1}`); the strict
    /// subset allows flow style only for the empty `[]` and `{}`. Raised by `ludion-scene`.
    pub const SCENE_YAML_FLOW_COLLECTION: DiagnosticCode = DiagnosticCode::new("L1005");

    /// `L1006` — a scene file used a literal (`|`) or folded (`>`) block scalar, which the strict
    /// subset forbids. Raised by `ludion-scene`.
    pub const SCENE_YAML_BLOCK_SCALAR: DiagnosticCode = DiagnosticCode::new("L1006");

    /// `L1007` — a mapping in a scene file repeats a key. Raised by `ludion-scene`.
    pub const SCENE_YAML_DUPLICATE_KEY: DiagnosticCode = DiagnosticCode::new("L1007");

    /// `L1008` — a mapping in a scene file used a non-scalar (sequence or mapping) key, which the
    /// strict subset forbids. Raised by `ludion-scene`.
    pub const SCENE_YAML_COMPLEX_KEY: DiagnosticCode = DiagnosticCode::new("L1008");

    /// `L1009` — a scene document node has the wrong type or shape for its position (e.g.
    /// `entities` is not a sequence, `seed` is not a non-negative integer, a numeric literal
    /// overflows or is non-finite). Raised by `ludion-scene`.
    pub const SCENE_TYPE_MISMATCH: DiagnosticCode = DiagnosticCode::new("L1009");

    /// `L1010` — a scene document mapping contains a key the format does not define (often a
    /// typo; the diagnostic suggests the nearest valid key). Raised by `ludion-scene`.
    pub const SCENE_UNKNOWN_KEY: DiagnosticCode = DiagnosticCode::new("L1010");

    /// `L1011` — a scene document is missing a required key (e.g. the top-level `format`).
    /// Raised by `ludion-scene`.
    pub const SCENE_MISSING_KEY: DiagnosticCode = DiagnosticCode::new("L1011");

    /// `L1012` — a scene file declares a `format` version this engine build does not support.
    /// Raised by `ludion-scene`.
    pub const SCENE_UNSUPPORTED_FORMAT: DiagnosticCode = DiagnosticCode::new("L1012");

    /// `L1013` — a scene entity names a component that is not in the component registry (the
    /// diagnostic suggests the nearest registered name). Raised by `ludion-scene`.
    pub const SCENE_UNKNOWN_COMPONENT: DiagnosticCode = DiagnosticCode::new("L1013");

    /// `L1014` — a component value in a scene does not match that component's JSON Schema
    /// (wrong field type, missing required field, unknown field…). Raised by `ludion-scene`.
    pub const SCENE_COMPONENT_MISMATCH: DiagnosticCode = DiagnosticCode::new("L1014");

    /// `L1015` — a file (or directory listing) a command reads as input could not be read: a scene,
    /// a model/sculpt/mesh/atlas/font/map document, a profile or sweep source, a golden suite
    /// directory or its manifest. Raised by `ludion-cli`, `ludion-engine`, `ludion-mesh-edit`,
    /// `ludion-testkit`. The name predates its widening to every input file.
    pub const SCENE_IO: DiagnosticCode = DiagnosticCode::new("L1015");

    /// `L1016` — a scene entity declared a **reserved** component as a `components:` block. Reserved
    /// built-ins are field-backed (e.g. `Name`, projected from an entity's `name:`), so they have
    /// exactly one authoring channel — the field — and may not also be written as a component.
    /// Raised by `ludion-scene`.
    pub const SCENE_RESERVED_COMPONENT: DiagnosticCode = DiagnosticCode::new("L1016");

    /// `L1017` — a scene entity's component carries an entity reference (`Parent`'s implicit link,
    /// or `Joint`'s `a`/`b`) that is structurally invalid: its `index` is not a valid document
    /// position (out of range), its `generation` is nonzero, or the reference names an entity that
    /// must not name itself (`Parent` always; `Joint`'s `a`/`b` only when `a == b`). Raised by
    /// `ludion-scene` (the JSON Pointer names the offending value). Also raised on the wire by
    /// `component.set` and `entity.spawn` for a self reference the field forbids, by id rather than
    /// document position. Inside a map fragment the references are local to the fragment and rebased
    /// on composition; the pointer is `/fragments/<i>`.
    pub const SCENE_INVALID_ENTITY_REF: DiagnosticCode = DiagnosticCode::new("L1017");

    /// `L1018` — a `scene.save` refused because a live entity's component carries an entity
    /// reference (any field marked as one — `Parent`'s implicit link, `Joint`'s `a`/`b`, …) that
    /// points at an entity not in the save (dead, or otherwise absent), or at the carrier itself
    /// where the field forbids that, so the reference cannot be expressed as a valid scene. Raised
    /// by `ludion-engine` while serializing the world to a scene.
    pub const SCENE_SAVE_DANGLING_REF: DiagnosticCode = DiagnosticCode::new("L1018");

    /// `L1019` — a `Tilemap` component's `rows` are ragged: not every row string has the same glyph
    /// count, so the grid is not rectangular. Raised by the `Tilemap` component's semantic validator
    /// (invoked by scene validation and `component.set`); the JSON Pointer names the first offending
    /// row.
    pub const TILEMAP_RAGGED_ROWS: DiagnosticCode = DiagnosticCode::new("L1019");

    /// `L1020` — a `Tilemap` glyph is invalid: a row uses a glyph that has no entry in the `legend`,
    /// or a `legend` key is not exactly one Unicode scalar (a glyph is one `char`). Raised by the
    /// `Tilemap` component's semantic validator (invoked by scene validation and `component.set`); the
    /// JSON Pointer names the offending row or legend key. A map-kit `tiles` key may not be the space,
    /// the reserved void glyph.
    pub const TILEMAP_INVALID_GLYPH: DiagnosticCode = DiagnosticCode::new("L1020");

    /// `L1021` — a map source (`kind: map`) or map-kit (`kind: map-kit`) document is structurally
    /// malformed: a required top-level key is missing or has the wrong type, an unrecognized `kind`
    /// value was found, a marker/primitive field has the wrong shape, or the document has a key the
    /// map format does not define. The structural analog of `L1110`/`L1113` for the two map document
    /// kinds. `bounds` is exclusive with `rows`/`base`, and a fragment's `physics`/`seed` is refused
    /// (declare it on the map). Raised by `ludion-scene` (`parse_map`/`parse_map_kit`).
    pub const MAP_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1021");

    /// `L1022` — a map-kit entry, or a reference to one, is semantically invalid: a glyph/kind with
    /// an empty `emit` list, a primitive naming zero or more than one mesh source, a `footprint` that
    /// is not two positive integers, a primitive that sets both `merge` and `footprint`, a non-finite
    /// `y`/`scale`/`color`/collider size, a `roughness`/`metallic`/`emissive` out of the `Mesh`
    /// range, a mesh-only key on a collider-only primitive, a non-positive `probe` radius or speed, or a marker
    /// `variant` index with no such candidate in the referenced kit entry. Raised by
    /// `ludion-scene`'s map parser and generator; the JSON Pointer names the offending kit
    /// primitive, `probe` field or marker field. A merged primitive needs a cell-sized `aabb`,
    /// never a `circle`.
    pub const MAP_KIT_INVALID: DiagnosticCode = DiagnosticCode::new("L1022");

    /// `L1023` — a map marker's world position (`x`, `z`) falls outside the grid's XZ extent
    /// (`origin` + `cell_size` × grid dimensions), so it names no cell and cannot be placed; also a
    /// map with markers and no frame at all (empty `rows`, no `bounds`). Raised by `ludion-scene`'s
    /// map generator; the JSON Pointer names the marker, or `/rows` for the missing frame.
    pub const MAP_MARKER_OUT_OF_BOUNDS: DiagnosticCode = DiagnosticCode::new("L1023");

    /// `L1024` — a map marker names a `kind` that has no entry in the referenced map-kit's `kinds`
    /// mapping (the diagnostic suggests the nearest kit kind). Raised by `ludion-scene`'s map
    /// generator; the JSON Pointer names the marker's `kind`.
    pub const MAP_UNKNOWN_KIND: DiagnosticCode = DiagnosticCode::new("L1024");

    /// `L1025` — the map generator would emit two entities with the same `name`. Generated names are
    /// coordinate-derived and must be globally unique across the fragment, grid, and marker entities,
    /// because gate anchors resolve by name — a collision is a real bug, not a style issue. Raised by
    /// `ludion-scene`'s map generator; the message names the colliding entity name and both origins
    /// with their JSON Pointers.
    pub const MAP_NAME_COLLISION: DiagnosticCode = DiagnosticCode::new("L1025");

    /// `L1026` — a run of a footprint-tiled glyph does not divide evenly into whole footprint spans
    /// (e.g. a 6-cell run with a `footprint: [4, 1]` piece leaves a 2-cell remainder), so the glyph's
    /// cells cannot be tiled by that modular piece. Raised by `ludion-scene`'s map generator; the JSON
    /// Pointer names the offending grid row.
    pub const MAP_FOOTPRINT_MISFIT: DiagnosticCode = DiagnosticCode::new("L1026");

    /// `L1399` — `generate-map --mirror` was asked of a map that carries `fragments:`. The mirror
    /// reflects the generated grid and markers; fragments are hand-authored scenes whose transforms
    /// (parents, pitch/roll) the compiler does not reflect. Raised by `ludion-scene`'s map generator;
    /// the JSON Pointer is `/fragments`.
    pub const MAP_MIRROR_FRAGMENTS: DiagnosticCode = DiagnosticCode::new("L1399");
    /// `L1406` — `spawn_map` could not read or accept the map it was asked to stamp: a path that is
    /// absolute, escapes the content root, is unreadable or not recorded under replay; a file that is
    /// not a `kind: map` (or its kit not a `kind: map-kit`); a map carrying `physics:`; a room root
    /// carrying `Animation`; or a world with no prefab library. Raised by `ludion-engine`'s `spawn_map`.
    pub const MAP_SPAWN_INVALID: DiagnosticCode = DiagnosticCode::new("L1406");
    /// `L1400` — the suspended run (`save/suspend.json`) could not be written or removed.
    pub const RUN_SAVE_WRITE_FAILED: DiagnosticCode = DiagnosticCode::new("L1400");
    /// `L1401` — the suspended run was refused on resume; nothing was restored.
    pub const RUN_SAVE_RESUME_REFUSED: DiagnosticCode = DiagnosticCode::new("L1401");
    /// `L1402` — an in-game run recording could not start, be written or be finalized.
    pub const RUN_RECORDING_FAILED: DiagnosticCode = DiagnosticCode::new("L1402");

    /// `L1560` — an `EngineBuilder::checkpoints` config is invalid: `max_slots` is 0 or above 64. An
    /// **error** at `build`.
    pub const CHECKPOINT_CONFIG_INVALID: DiagnosticCode = DiagnosticCode::new("L1560");
    /// `L4200` — a checkpoint restore named a slot that does not exist; the world is untouched and
    /// `Checkpoints::last_restore()` is `Refused`. A warning.
    pub const CHECKPOINT_NO_SLOT: DiagnosticCode = DiagnosticCode::new("L4200");
    /// `L4201` — a checkpoint capture of a new name would pass `max_slots`; nothing is captured and no
    /// slot is evicted. A warning.
    pub const CHECKPOINT_SLOTS_FULL: DiagnosticCode = DiagnosticCode::new("L4201");
    /// `L4202` — a checkpoint name is empty, longer than 64 characters or outside `[A-Za-z0-9_.-]`;
    /// the request is ignored. A warning.
    pub const CHECKPOINT_NAME_INVALID: DiagnosticCode = DiagnosticCode::new("L4202");
    /// `L4203` — a checkpoint capture failed (for example a non-finite value, `L0007`); the previous
    /// slot of that name, if any, is kept. A warning.
    pub const CHECKPOINT_CAPTURE_FAILED: DiagnosticCode = DiagnosticCode::new("L4203");
    /// `L4204` — a checkpoint restore failed while decoding or validating the slot; the world is
    /// untouched. A warning.
    pub const CHECKPOINT_RESTORE_FAILED: DiagnosticCode = DiagnosticCode::new("L4204");
    /// `L4205` — a scene switch requested the same frame as a checkpoint restore was dropped; the
    /// restore wins. A warning.
    pub const CHECKPOINT_SWITCH_DROPPED: DiagnosticCode = DiagnosticCode::new("L4205");
    /// `L4206` — a replay keyframe was skipped because checkpoint slots are held; a seek lands on the
    /// last keyframe before the first capture. Reported once. A warning.
    pub const CHECKPOINT_KEYFRAME_SKIP: DiagnosticCode = DiagnosticCode::new("L4206");
    /// `L4207` — a run recording was not armed because checkpoint slots are held. A warning.
    pub const CHECKPOINT_RECORDING_ARM_REFUSED: DiagnosticCode = DiagnosticCode::new("L4207");
    /// `L4208` — the checkpoint disk slot could not be written or removed. The previous whole file
    /// stays and the next request retries. An error.
    pub const CHECKPOINT_DISK_WRITE_FAILED: DiagnosticCode = DiagnosticCode::new("L4208");
    /// `L4209` — a disk checkpoint was not loaded; the file was backed up (or kept, while a recording
    /// is open) and the world is untouched. A warning.
    pub const CHECKPOINT_DISK_LOAD_REFUSED: DiagnosticCode = DiagnosticCode::new("L4209");
    /// `L4260` — a `GameClock::scale` value was non-finite, `<= 0` or `> 1` and was ignored. A warning.
    pub const GAME_SCALE_INVALID: DiagnosticCode = DiagnosticCode::new("L4260");
    /// `L1403` — the `run_recordings` builder config is invalid (zero keyframe interval, bad dir).
    pub const RUN_RECORDING_CONFIG_INVALID: DiagnosticCode = DiagnosticCode::new("L1403");
    /// `L1404` — a replay was recorded by a different game build (schema or version) and was refused.
    pub const REPLAY_GAME_MISMATCH: DiagnosticCode = DiagnosticCode::new("L1404");
    /// `L1405` — in-window replay playback could not start, run to the end, or be listed or deleted.
    pub const REPLAY_PLAYBACK_FAILED: DiagnosticCode = DiagnosticCode::new("L1405");

    /// `L1027` — a map marker's cross-district gate (`Gate.to` + `anchor`) names a spawn anchor that
    /// resolves against no entity in the sibling district's generated scene, so the gate would fail
    /// only at runtime (as `L3019`). **Reserved:** registered here for the map format, but the check
    /// itself (which must resolve and generate the sibling map) lives in Wave 2's `ludion map-check`,
    /// not in the pure, IO-free generator. Raised by `ludion-cli` (`ludion map-check`).
    pub const MAP_DANGLING_GATE_ANCHOR: DiagnosticCode = DiagnosticCode::new("L1027");

    /// `L1028` — a map marker carrying a `Gate` component sits sealed behind a solid boundary wall:
    /// neither its own cell nor any orthogonally-adjacent cell is both walkable and on the grid
    /// perimeter, so there is no gap for a player to pass through. This is the city-scale bug (a
    /// perimeter gap authored on the wrong side of the wall). Raised as a **warning** by `ludion
    /// map-check` — a heuristic that does not fail the command, unlike the provable `L1027`.
    pub const MAP_GATE_SEALED: DiagnosticCode = DiagnosticCode::new("L1028");

    /// `L1029` — a map marker's point has another prop's collider within the probe radius (it would
    /// spawn blocked) or its cell is walkable but unreachable from the reachability anchor (a sealed
    /// interior pocket). The marker's own collider is excluded, so a passthrough building that blocks
    /// its own cell by design does not fire. Raised as a **warning** by `ludion map-check`.
    pub const MAP_MARKER_BLOCKED: DiagnosticCode = DiagnosticCode::new("L1029");

    /// `L1030` — a mapping key in a scene file is too long once spelled in canonical form: the
    /// canonical writer would emit more characters than the YAML scanner accepts for a simple key,
    /// so the emitted document would no longer re-parse. Control characters cost six characters each
    /// (`\u00XX`), so a key well under the limit in source can cross it once escaped. Raised by
    /// `ludion-scene`.
    pub const SCENE_YAML_KEY_TOO_LONG: DiagnosticCode = DiagnosticCode::new("L1030");

    /// `L1031` — two `Gate` markers on one map have no walkable path between them: the district is
    /// bisected, so a player entering by one gate can never reach the other. Where `L1028` catches a
    /// single gate sealed at the wall, this catches the interior split. Raised as a **warning** by
    /// `ludion map-check`, for the same reason as `L1028`: it reads the heuristic occupancy probe.
    pub const MAP_GATES_DISCONNECTED: DiagnosticCode = DiagnosticCode::new("L1031");

    /// `L1032` — a map marker whose `components:` bag authors a `Residue` component has no explicit
    /// `name:`, so its `Name` is index-derived (`MapMarker::derived_name`). Scene residue keys a
    /// remembered entity by `Name`, so an unnamed marker's identity shifts whenever a marker is
    /// inserted or reordered above it. Raised as a **warning** by `ludion map-check`.
    pub const MAP_RESIDUE_MARKER_UNNAMED: DiagnosticCode = DiagnosticCode::new("L1032");

    /// `L1033` — a scene's `physics:` block carries an invalid value: an unrecognized `space`
    /// string, a `solver_iterations` or `stabilization_iterations` outside `1..=64`, a
    /// `sleep_after_ticks` above `u32::MAX`, or a `gravity` that is not exactly three numbers each
    /// representable as `f32`. Raised by `ludion-scene`'s block parser, with a JSON Pointer into
    /// the block and, for an unrecognized `space`, the nearest-match suggestion.
    pub const SCENE_PHYSICS_INVALID: DiagnosticCode = DiagnosticCode::new("L1033");

    /// `L1034` — a scene document nests collections (sequences or mappings, block or flow) more
    /// than 128 levels deep. Every reader of the parsed tree is recursive, so the strict subset
    /// caps depth at a fixed limit rather than let a hostile file overflow the stack; real scenes
    /// nest about ten levels. Raised by `ludion-scene`.
    pub const SCENE_YAML_TOO_DEEP: DiagnosticCode = DiagnosticCode::new("L1034");

    /// `L1035` — a scene's `Parent` links form a cycle (entity A's parent chain leads back to A),
    /// so the hierarchy is not a tree. Raised by `ludion-scene`'s validation, one per cycle.
    pub const SCENE_PARENT_CYCLE: DiagnosticCode = DiagnosticCode::new("L1035");

    /// `L1036` — a sim entity's entity-reference field names a presentation entity
    /// (`presentation: true`), which would let the sim hash a presentation id. Raised by
    /// `ludion-scene`'s validation, and on the wire by `component.set` and `entity.spawn` (the id is
    /// named, not a document position).
    pub const SCENE_PRESENTATION_REF: DiagnosticCode = DiagnosticCode::new("L1036");

    /// `L1037` — a scene or doc file starts with a YAML directive other than `%YAML 1.2` (`%YAML
    /// 1.1`, `%TAG`, an unknown `%NAME`). Files are always read as YAML 1.2 core, so the directive
    /// would lie about how the text is read. Raised by `ludion-scene`'s YAML layer.
    pub const SCENE_YAML_DIRECTIVE: DiagnosticCode = DiagnosticCode::new("L1037");

    /// `L1038` — an output file could not be written: a directory could not be created, a file could
    /// not be written, renamed or removed, or its bytes could not be encoded. Raised by `ludion-cli`,
    /// the `ludion` facade (`--trace` to stdout), `ludion-engine` (`scene.save`, the replay
    /// recorder), `ludion-mesh-edit`, `ludion-gen` (run folders, `run.json`, the contact sheet, an
    /// accepted file).
    pub const OUTPUT_IO: DiagnosticCode = DiagnosticCode::new("L1038");

    /// `L1039` — a generated scene differs from what its map source generates (`generate-map
    /// --check`): a stale commit or a hand edit. Raised by `ludion-cli`.
    pub const MAP_SCENE_DRIFT: DiagnosticCode = DiagnosticCode::new("L1039");

    /// `L1040` — `map-check`'s sound seal flood found a walker-wide way from the map's seed to its
    /// edge that never enters a gate's trigger. Raised by `ludion-cli`.
    pub const MAP_EDGE_LEAK: DiagnosticCode = DiagnosticCode::new("L1040");

    /// `L1041` — `map-drift` found a map-generated scene or prefab that fails the drift gate for a
    /// reason other than byte drift (L1039): a missing output, a moved copy, a bad recorded flag, a
    /// leaking seal, a denied warning, an empty scan. Raised by `ludion-cli`.
    pub const MAP_DRIFT_FAULT: DiagnosticCode = DiagnosticCode::new("L1041");

    /// `L1101` — an asset file referenced by a scene could not be read (missing, a directory, or
    /// permission-denied). Raised by `ludion-assets` (wrapped with the offending component path by
    /// `ludion-engine`'s extract). The diagnostic suggests the nearest filename in the assets dir.
    pub const ASSET_FILE_IO: DiagnosticCode = DiagnosticCode::new("L1101");

    /// `L1102` — an asset file was read but could not be decoded as a PNG image (corrupt data, an
    /// unsupported color type, or truncated bytes), or its header declares more than 8192×8192
    /// pixels or a side over 8192 (the decode budget). Raised by `ludion-assets`.
    pub const ASSET_PNG_DECODE: DiagnosticCode = DiagnosticCode::new("L1102");

    /// `L1103` — a filesystem watcher could not be started: the OS watcher could not be created, or
    /// it could not begin watching the requested directory (the path is absent, or the process is out
    /// of watch descriptors). Raised by `ludion-assets` (`watch`); consumed by `ludion dev`.
    pub const WATCHER_START_FAILED: DiagnosticCode = DiagnosticCode::new("L1103");

    /// `L1104` — an asset file was read but could not be decoded as a WAV audio clip: not a
    /// RIFF/WAVE container, a non-PCM or non-16-bit format, an unsupported channel count (only mono
    /// and stereo are decoded), a sample rate below 8000 Hz, or truncated data. Raised by
    /// `ludion-assets` (`load_wav`).
    pub const ASSET_WAV_DECODE: DiagnosticCode = DiagnosticCode::new("L1104");

    /// `L1110` — a mesh asset document (`meshes/*.mesh.yaml`) is structurally malformed: a
    /// required top-level key is missing or has the wrong type, an unrecognized `kind` value was
    /// found, or the document has a key the mesh format does not define. Raised by
    /// `ludion-scene`.
    pub const MESH_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1110");

    /// `L1111` — a mesh asset document parsed cleanly but its vertex/index data does not hang
    /// together: a flat list's length is not the required multiple, two lists that must describe
    /// the same vertices disagree on the count, an index names a vertex that does not exist, or a
    /// float is not finite. Raised by `ludion-scene`; `ludion import` raises it too for a mesh it
    /// compiled, to be fixed in the source and re-exported.
    pub const MESH_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1111");

    /// `L1112` — a mesh asset document has more vertices than the HD-2D style budget expects. A
    /// **warning**, not a validation failure — `ludion check-scene` still exits success. Raised
    /// by `ludion-scene`.
    pub const MESH_VERTEX_BUDGET_EXCEEDED: DiagnosticCode = DiagnosticCode::new("L1112");

    /// `L1113` — a voxel model document (`voxels/*.voxel.yaml`) is structurally malformed: a
    /// required top-level key is missing or has the wrong type, an unrecognized `kind` value was
    /// found, `size` is not a sequence of three positive integers, a palette entry is not a
    /// `{color: [...]}` mapping, `slices` is not a sequence of row-string layers, or the document
    /// has a key the voxel format does not define. Raised by `ludion-scene`.
    pub const VOXEL_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1113");

    /// `L1114` — a voxel model document parsed cleanly but its data does not hang together: the
    /// slice layers/rows/columns do not match the declared `size`, a slice glyph is not `.` and
    /// names no palette entry, a palette key is not a single printable-ASCII glyph (multi-character,
    /// non-ASCII, or the reserved `.`), two palette keys name the same glyph, or a palette color
    /// component is not finite. Raised by `ludion-scene`.
    pub const VOXEL_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1114");

    /// `L1115` — a voxel model document declares more cells (`size.x * size.y * size.z`) than the
    /// style budget expects. A **warning**, not a validation failure — `ludion check-scene` still
    /// exits success. Raised by `ludion-scene`.
    pub const VOXEL_SIZE_BUDGET_EXCEEDED: DiagnosticCode = DiagnosticCode::new("L1115");

    /// `L1116` — a `Collider` component's shape has a size (a circle `radius`, an `aabb`
    /// `half_extents` element) or a `rigid` body's `mass` that is not a strictly positive, finite
    /// number — it is zero, negative, or non-finite. Raised by the `Collider` component's semantic
    /// validator (invoked by scene validation and `component.set`); the JSON Pointer names the
    /// offending `radius`, `half_extents` element, or `/body/rigid/mass`. (A negative `radius` is
    /// additionally rejected by the schema as `L1014`; this code covers the zero and non-finite
    /// cases the schema cannot express.) A `rigid` body's `gravity_scale` is the one exception to
    /// "positive": it must only be finite (zero floats, a negative value rises), pointer
    /// `/body/rigid/gravity_scale`. A `cylinder`'s sizes and a `compound` part's sizes are covered
    /// too, the latter at `/shape/compound/parts/<i>/shape/...`.
    pub const COLLIDER_NON_POSITIVE_EXTENT: DiagnosticCode = DiagnosticCode::new("L1116");

    /// `L1117` — the model view harness (`render --model`) was given an input it cannot render as a
    /// model: the file could not be read, it has no top-level `kind:` key (so it is a scene, not a
    /// model), or its `kind:` is neither `mesh` nor `voxel`. It also covers a `--rig` scene that
    /// could not be read or that holds none of the four liftable component types. The message names
    /// the path, what was found, and the fix. Raised by `ludion-engine`'s `render_model_sheet`. (A
    /// `mesh`/`voxel` document that *is* recognized but invalid fails with its own
    /// `L1110`/`L1111`/`L1113`/`L1114`; an invalid `--rig` scene fails with its own scene
    /// diagnostics.)
    pub const MODEL_SHEET_INPUT: DiagnosticCode = DiagnosticCode::new("L1117");

    /// `L1118` — a `UiRect`, `UiText`, `UiList` or `Text` component's size is not a strictly
    /// positive, finite number — it is zero, negative, or non-finite. For `UiRect` this is either
    /// axis of `size`; for `UiText` and `UiList` it is `size` (the glyph cell height); for `Text`
    /// it is `size` (the world height of one line). Raised by the components' semantic validators
    /// (invoked by scene validation and `component.set`); the JSON Pointer names the offending
    /// field (`/size/0`, `/size/1` for `UiRect`; `/size` for the others).
    pub const UI_ELEMENT_NON_POSITIVE_SIZE: DiagnosticCode = DiagnosticCode::new("L1118");

    /// `L1119` — a game data-doc's envelope is malformed: the `format` key is missing or is not
    /// version 1, the `kind` key is missing / not a string / does not match the kind the doc was
    /// loaded as, or a top-level key is a near-miss of an envelope key (`format`/`kind`). The
    /// message names the kind; the JSON Pointer names the offending key (empty for a missing
    /// required key). Raised by `ludion-scene`'s `check_doc`, by `ludion-engine`'s content-root
    /// walk when a `*.yaml` declares a `kind:` that is neither an engine kind nor a registered
    /// data-doc kind (with a nearest-match over the registered kinds), and by `ludion check-scene`
    /// for a `kind:` that names no engine kind. (Field-level shape problems
    /// are the separate `L1120`.)
    pub const DOC_ENVELOPE_INVALID: DiagnosticCode = DiagnosticCode::new("L1119");

    /// `L1120` — a game data-doc's envelope is well-formed but its fields do not match the kind's
    /// registered JSON Schema: a wrong-typed or out-of-range value, a missing required field, or a
    /// field the kind does not declare. The message names the kind; the JSON Pointer names the
    /// offending field, with a nearest-match suggestion for a typo. Raised by `ludion-scene`'s
    /// `check_doc` (the same schema checker scene components use, `L1014`).
    pub const DOC_SCHEMA_MISMATCH: DiagnosticCode = DiagnosticCode::new("L1120");

    /// `L1121` — a saved player profile could not be loaded as authored, so a fresh default
    /// profile started — a persisting host first renames the file aside as a backup, and any other
    /// engine, or a failed rename, leaves it in place. The game still boots and the player's bytes are
    /// never destroyed. Raised for a malformed profile envelope (a missing/wrong `format`,
    /// a missing/wrong/non-`profile` `kind`, a non-mapping or unparseable document) by
    /// `ludion-scene`'s `open_profile`, and for a value that no longer deserializes into the game's
    /// current profile type (a retyped field) or a newer-than-engine `format` by the engine's
    /// lenient profile migration. A **warning**, not a hard failure — boot never blocks on it.
    pub const PROFILE_LOAD_FAILED: DiagnosticCode = DiagnosticCode::new("L1121");

    /// `L1700` — a player settings file (`save/settings.yaml`) has a malformed `format`/`kind`
    /// envelope, so the engine backs it up and starts from defaults.
    pub const SETTINGS_ENVELOPE: DiagnosticCode = DiagnosticCode::new("L1700");
    /// `L1710` — the `Engine::builder().dialogue` config is invalid: an empty `default_locale`, or a
    /// reading-time constant that is negative or not finite.
    pub const DIALOGUE_CONFIG_INVALID: DiagnosticCode = DiagnosticCode::new("L1710");
    /// `L1711` — a string key is defined twice for one locale across the `kind: strings` docs.
    pub const STRINGS_DUPLICATE_KEY: DiagnosticCode = DiagnosticCode::new("L1711");
    /// `L1712` — a dialogue line id is defined in more than one `kind: dialogue` doc.
    pub const DIALOGUE_DUPLICATE_LINE: DiagnosticCode = DiagnosticCode::new("L1712");
    /// `L1713` — a speaker id is defined in more than one `kind: dialogue` doc.
    pub const DIALOGUE_DUPLICATE_SPEAKER: DiagnosticCode = DiagnosticCode::new("L1713");
    /// `L1714` — a dialogue line names a speaker no `kind: dialogue` doc defines.
    pub const DIALOGUE_UNKNOWN_SPEAKER: DiagnosticCode = DiagnosticCode::new("L1714");
    /// `L1715` — a line `text` or speaker `name` key is missing from the default locale's strings.
    pub const STRING_KEY_MISSING_DEFAULT: DiagnosticCode = DiagnosticCode::new("L1715");
    /// `L1716` — a translation uses different `{placeholders}` than the default locale's text. A
    /// warning.
    pub const STRINGS_PLACEHOLDER_MISMATCH: DiagnosticCode = DiagnosticCode::new("L1716");
    /// `L1717` — an authored line is invalid: `duration_ticks: 0`, or a cue at or past its duration.
    pub const DIALOGUE_LINE_INVALID: DiagnosticCode = DiagnosticCode::new("L1717");
    /// `L1718` — a `kind: strings` doc has an empty `locale`.
    pub const STRINGS_LOCALE_INVALID: DiagnosticCode = DiagnosticCode::new("L1718");
    /// `L1730` — a `kind: sequence` doc is structurally invalid: `length_ticks` 0, a key past the
    /// length or out of order, a camera key with neither or both of `rig`/`restore`, or a
    /// non-finite number.
    pub const SEQUENCE_INVALID: DiagnosticCode = DiagnosticCode::new("L1730");
    /// `L1731` — a sequence key names a role the doc does not declare, a role is declared twice, or a
    /// camera, animation or ui key has no role.
    pub const SEQUENCE_UNKNOWN_ROLE: DiagnosticCode = DiagnosticCode::new("L1731");
    /// `L1732` — a sequence `event` key has an empty name or the reserved name `end`.
    pub const SEQUENCE_EVENT_NAME: DiagnosticCode = DiagnosticCode::new("L1732");
    /// `L1733` — a sequence `dialogue` key names a line no loaded `kind: dialogue` doc defines, or
    /// the game did not opt in with `.dialogue()`.
    pub const SEQUENCE_UNKNOWN_LINE: DiagnosticCode = DiagnosticCode::new("L1733");
    /// `L1734` — a sequence sets `holds_gameplay: true` with `clock: game`; a held game clock would
    /// never advance it.
    pub const SEQUENCE_HOLD_CLOCK: DiagnosticCode = DiagnosticCode::new("L1734");
    /// `L1735` — two sequence `wait` keys share one `at`.
    pub const SEQUENCE_WAIT_CONFLICT: DiagnosticCode = DiagnosticCode::new("L1735");
    /// `L4340` — the settings file could not be loaded, so the engine backed it up and started from
    /// defaults.
    pub const SETTINGS_RESET: DiagnosticCode = DiagnosticCode::new("L4340");
    /// `L4341` — the settings file has a key the engine does not know; dropped, or an unknown bus
    /// name kept.
    pub const SETTINGS_KEY_DROPPED: DiagnosticCode = DiagnosticCode::new("L4341");
    /// `L4342` — a settings value was out of range, not a number or not a known name, and was
    /// replaced.
    pub const SETTINGS_VALUE_CLAMPED: DiagnosticCode = DiagnosticCode::new("L4342");
    /// `L4343` — the settings file could not be written at a frame boundary.
    pub const SETTINGS_SAVE_FAILED: DiagnosticCode = DiagnosticCode::new("L4343");
    /// `L4350` — `sequences::play` named a document no loaded `kind: sequence` defines.
    pub const SEQUENCE_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L4350");
    /// `L4351` — `sequences::play` bound a role the document does not declare, or bound one twice.
    pub const SEQUENCE_BIND_INVALID: DiagnosticCode = DiagnosticCode::new("L4351");
    /// `L4352` — a declared role was left unbound or its entity is dead, so its keys are inert. A
    /// warning, once per player and role.
    pub const SEQUENCE_ROLE_MISSING: DiagnosticCode = DiagnosticCode::new("L4352");
    /// `L4353` — a sequence key degraded at run time: the role entity lacks the component the key
    /// writes, a dialogue line was refused, or the document vanished mid-play. A warning, once per
    /// player and reason.
    pub const SEQUENCE_KEY_DEGRADED: DiagnosticCode = DiagnosticCode::new("L4353");
    /// `L4354` — a `sequences::` call reached an engine that did not opt in with `.sequences()`.
    pub const SEQUENCES_NOT_ENABLED: DiagnosticCode = DiagnosticCode::new("L4354");
    /// `L4370` — `paired_fit` found no `kind: paired` document at the path it was given: the engine
    /// has no project, or the document fails to load (the loader's diagnostics follow).
    pub const PAIRED_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L4370");
    /// `L4371` — a paired cast names an unknown, repeated or missing role, or an entity that cannot
    /// play its role: dead, no `Transform`, parented, or not the role's assembly.
    pub const PAIRED_CAST_INVALID: DiagnosticCode = DiagnosticCode::new("L4371");
    /// `L4372` — a cast entity's scale is not uniform or differs from its role's `scale`.
    pub const PAIRED_SCALE_MISMATCH: DiagnosticCode = DiagnosticCode::new("L4372");
    /// `L4373` — a paired document has a shape the runtime does not play yet: more than two roles.
    pub const PAIRED_UNSUPPORTED: DiagnosticCode = DiagnosticCode::new("L4373");
    /// `L4374` — warning: a game write to a playing paired role's `Animation` (`clip`, `started` or
    /// `speed`) broke lockstep, so the whole action was aborted.
    pub const PAIRED_LOCKSTEP_BROKEN: DiagnosticCode = DiagnosticCode::new("L4374");
    /// `L4375` — `abort_paired` was given an entity that is not in a paired action.
    pub const PAIRED_NOT_PLAYING: DiagnosticCode = DiagnosticCode::new("L4375");
    /// `L4290` — `AnimGraph::step` found no `AnimGraphState`, a state name the graph does not declare,
    /// no `Animation`, or a layer the entity's `AnimationLayers` lacks. Nothing was written.
    pub const ANIM_GRAPH_STATE_INVALID: DiagnosticCode = DiagnosticCode::new("L4290");
    /// `L4291` — an `AnimGraph` table has duplicate state names, or an edge names an undeclared state.
    pub const ANIM_GRAPH_INVALID: DiagnosticCode = DiagnosticCode::new("L4291");

    /// `L1122` — a saved player profile could not be written to disk at a frame boundary. The engine
    /// stages the canonical text to a hidden `.tmp` sibling, syncs it to disk and renames it over
    /// `save/profile.yaml`, so a whole save stays on disk — this diagnostic means that staged write,
    /// rename or directory sync failed (a missing/unwritable save directory, a permissions error, a
    /// full disk). A **warning**, never a hard failure — the sim keeps running and the next `profile.save()`
    /// retries. Raised by the engine's frame-boundary profile save drain.
    pub const PROFILE_SAVE_FAILED: DiagnosticCode = DiagnosticCode::new("L1122");

    /// `L1123` — a `DirectionalLight` cannot form a shadow frustum: its `direction` is zero-length
    /// (all components zero) or has a non-finite (`NaN`/`±Infinity`) component, or `shadow_near` is
    /// not less than `shadow_far`. A directional light's direction is normalized at extract; a zero
    /// or non-finite vector has no direction to normalize and would collapse the shadow frustum, and
    /// an inverted depth range leaves it no depth to cover. Bad `cascade_splits` (without `cascades`,
    /// the wrong length for it, not strictly ascending, or outside (0, 1)) are refused here too.
    /// These invariants live in the component's
    /// semantic validator rather than its schema (the same split the `Collider` validator, `L1116`,
    /// uses). Raised by the `DirectionalLight` semantic validator (invoked by scene validation and
    /// `component.set`); the JSON Pointer names `/direction`, `/shadow_far`, `/shadow_near`,
    /// `/cascade_splits` or `/cascade_splits/{i}`.
    pub const DIRECTIONAL_LIGHT_INVALID_FRUSTUM: DiagnosticCode = DiagnosticCode::new("L1123");

    /// `L1124` — a single definition *module* concentrates more than
    /// `SYSTEMS_PER_MODULE_WARN_THRESHOLD` registered schedule systems — the signal that a game is
    /// regrowing a catch-all schedule module. Grouped at engine build time by each system's
    /// `std::any::type_name` (generics stripped, the final `::segment` dropped), it is a **warning**,
    /// never a build failure, stored on the engine and printed by the host (`ludion::run`). It never
    /// touches sim state, the `WorldHash`, or replay. Raised by `ludion-engine`'s engine build.
    pub const SCHEDULE_MODULE_CONCENTRATION: DiagnosticCode = DiagnosticCode::new("L1124");

    /// `L1125` — a system's declared query (`EngineBuilder::system(...).query::<...>()`) names a
    /// component type that is not in the component registry, so the declaration cannot resolve to a
    /// component name. A **build error**: the declaration is verified against the registry when the
    /// engine is built, before any world exists. The message names the system, the declared query's
    /// index, and the Rust type; the fix is to register the component or correct the declaration.
    /// Raised by `ludion-engine`'s engine build.
    pub const QUERY_DECL_UNKNOWN_COMPONENT: DiagnosticCode = DiagnosticCode::new("L1125");

    /// `L1393` — `EngineBuilder::query::<...>()` was called before any `.system(...)`, so the
    /// declaration describes no system. A **build error** raised by `ludion-engine`'s engine build;
    /// the message names the call site's file and line.
    pub const QUERY_DECL_NO_SYSTEM: DiagnosticCode = DiagnosticCode::new("L1393");

    /// `L1126` — a system ran a query whose component shape it never declared (observed-but-undeclared).
    /// An **error** raised by the query-drift verifier (`ludion_engine::check_query_drift_*`) after an
    /// instrumented replay/driver run: the system's declared `.query::<...>()` set does not cover a
    /// shape its body actually iterated. The message names the system, the observed component set, and
    /// the declared shapes; the fix is to add the missing `.query` declaration.
    pub const QUERY_DRIFT_UNDECLARED: DiagnosticCode = DiagnosticCode::new("L1126");

    /// `L1127` — a system declared a query shape that was never observed during the verified run
    /// (declared-but-never-observed). A **warning** only, raised by the query-drift verifier: a
    /// conditional branch legitimately may not fire in every replay, so a declaration with no
    /// observation is reported, not failed. The message names the system and the unobserved shape.
    pub const QUERY_DRIFT_UNOBSERVED: DiagnosticCode = DiagnosticCode::new("L1127");

    /// `L1128` — an `Environment` component has a sky/ambient color component that is negative or
    /// non-finite. The four color fields (`sky_zenith`/`sky_horizon`/`sky_ground`/`ambient`) are HDR
    /// (they may exceed 1) but must be finite and non-negative — a negative radiance is meaningless.
    /// JSON Schema's `range` keyword does not reach array elements, so this invariant lives in the
    /// component's semantic validator rather than its schema — the same split the `Collider` (`L1116`)
    /// validator uses for its `half_extents`. (The scalar intensity/size/bloom fields carry their own
    /// schemars `range(min = 0)`, so a negative scalar is the schema's `L1014`, not this code.) Raised
    /// by the `Environment` semantic validator (invoked by scene validation and `component.set`); the
    /// JSON Pointer names the offending element (`/sky_zenith/0`, `/ambient/2`, …).
    pub const ENVIRONMENT_INVALID_FIELD: DiagnosticCode = DiagnosticCode::new("L1128");

    /// `L1129` — a voxel model document's filled cells split into more than one 6-connected component,
    /// so part of the model floats free of the main body (the classic "disconnected limb" that only a
    /// GPU render used to reveal). A **warning**, not a validation failure — `ludion check-scene` still
    /// exits success, and the model meshes normally (a deliberately detached accent is legal). The
    /// message's first line names the component count and the main body's size; one indented line per
    /// stray piece follows, smallest first (the smallest piece is the likeliest mistake), carrying its
    /// cell count, a sample cell `(x, y, z)`, and the **nearest pair of cells** between it and the main
    /// body with the gap in steps and the bridge-cell count that would close it — so the message names
    /// the place to fix, not only the piece to look for. Raised by `ludion-scene`.
    pub const VOXEL_DISCONNECTED_COMPONENTS: DiagnosticCode = DiagnosticCode::new("L1129");

    /// `L1130` — a `PostProcess` component has a triple element outside its valid range: a negative
    /// `gain` (the ASC CDL slope, which must be `>= 0`), a `gamma` element that is not strictly
    /// positive (the CDL power is applied as its reciprocal, so `0` is a division by zero), or a
    /// negative `fog_color` component (a colour is HDR above 1.0 but never negative — the
    /// `Environment` colour rule, `L1128`). `lift` is deliberately unchecked — it is an offset, and a
    /// negative one legitimately crushes blacks. JSON Schema's `range` keyword does not reach array
    /// elements, so this invariant lives in the component's semantic validator rather than its
    /// schema — the `Environment` (`L1128`) and `Collider` (`L1116`) split. (Every scalar field
    /// carries its own schemars `range`, so an out-of-range scalar is the schema's `L1014`, not this
    /// code; a non-finite element is unrepresentable in the scene format and unspellable in JSON, so
    /// it is nobody's code.) Raised by the `PostProcess` semantic validator (invoked by scene
    /// validation and `component.set`); the JSON Pointer names the offending element (`/gain/0`,
    /// `/gamma/1`, `/fog_color/2`, …).
    pub const POST_PROCESS_INVALID_TRIPLE: DiagnosticCode = DiagnosticCode::new("L1130");

    /// `L1131` — a voxel ops document (`kind: voxel-ops`, the input of `ludion voxel apply`) is
    /// malformed: a missing/wrong-typed envelope key, an `ops` entry that is not a mapping, an
    /// unknown op name or op key, a missing required key for the named op, or a coordinate token
    /// that is not the comma-joined shape the op expects. The JSON Pointer names the offending op
    /// and key (`/ops/3/at`); unknown names carry a nearest-match suggestion. Raised by
    /// `ludion-scene`. The *model* the ops apply to keeps its own codes (`L1113`/`L1114`).
    pub const VOXEL_OPS_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1131");

    /// `L1132` — a well-formed voxel op could not be applied to the model: a point op (`set`)
    /// addresses a cell outside the grid, a glyph names no palette entry, or a reshape op names an
    /// empty or inverted range, or a `grow` or `upscale` would make a grid past the cell ceiling.
    /// `voxel apply` also fires it on a model carrying `generated_from`, which is write-only.
    /// Volume ops (`box`, `ellipsoid`, `cylinder`, `line`) clip to the grid silently and never raise
    /// this, except a `line` spanning more than the ceiling's cells. The message names the op index
    /// and the offending coordinate or glyph. Raised by `ludion-scene`; nothing is written when it
    /// fires.
    pub const VOXEL_OP_INAPPLICABLE: DiagnosticCode = DiagnosticCode::new("L1132");

    /// `L1133` — the voxel generator intermediate (`kind: voxel-gen`, the JSON a
    /// `*.voxel.gen.mjs` script prints and `ludion voxel from-json` consumes) is malformed: it is
    /// not JSON at all, its envelope is missing or wrong, a required key is absent or wrong-typed,
    /// or it carries a key the intermediate does not define. The JSON Pointer names the offending
    /// value (`/palette/H/color/2`, `/layers/7`); unknown keys carry a nearest-match suggestion.
    /// Raised by `ludion-scene`. The *model* the intermediate describes keeps its own codes — a
    /// structurally valid intermediate whose grid does not hang together is `L1114`.
    ///
    /// This code has a second, CLI-only use: `ludion voxel from-json --check` regenerates the
    /// intermediate in memory and byte-compares it against the committed model, and reports a
    /// mismatch as this same `L1133` with a `DRIFT` message naming the first differing line — the
    /// intermediate is not malformed, but the committed file no longer matches what it now
    /// produces, which is the drift gate's job to catch. `--validate` runs the same in-memory
    /// generation and validation with no committed file to compare against, so it never raises this
    /// drift form.
    pub const VOXEL_GEN_INTERMEDIATE: DiagnosticCode = DiagnosticCode::new("L1133");

    /// `L1134` — a `mirror` op in a `voxel apply` ops document discarded cells that an earlier op in
    /// the same run deliberately wrote. A **warning**, not a validation failure — the mirror already
    /// won, so the model is well-formed; this only flags the likely-accidental ordering (author the
    /// discarded cells after the mirror, or in the kept half, and the overwrite disappears). One
    /// warning per *writer* op, tallying how many of its cells the mirror erased. Raised by
    /// `ludion-scene`.
    pub const VOXEL_OPS_SHADOWED_BY_MIRROR: DiagnosticCode = DiagnosticCode::new("L1134");

    /// `L1135` — the voxel generator intermediate reaching `ludion voxel from-json` carries text
    /// that was encoded as UTF-8 twice: a shell decoded the generator's bytes as Latin-1/CP1252 and
    /// re-encoded them, so `—` arrived as `â€"`. A **warning** — the JSON still parses and the model
    /// is still written, but its header and layer comments carry mojibake. Raised by `ludion-cli`
    /// over the raw input, before parsing, for both the `--in` and stdin forms.
    pub const VOXEL_GEN_DOUBLE_ENCODED: DiagnosticCode = DiagnosticCode::new("L1135");

    /// `L1136` — an `AudioEmitter` component's `range` is not a strictly positive, finite number.
    /// The audible radius divides the listener distance, so zero, negative, or non-finite has no
    /// meaningful falloff. The `COLLIDER_NON_POSITIVE_EXTENT` analog for the positional-audio
    /// component; raised by `ludion-engine`'s semantic validator, pointer `/range`.
    pub const AUDIO_EMITTER_NON_POSITIVE_RANGE: DiagnosticCode = DiagnosticCode::new("L1136");

    /// `L1650` — more than one entity carries an `AudioListener`. Raised by `check-scene` and once at
    /// runtime; only the lowest entity id is heard from.
    pub const AUDIO_LISTENER_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L1650");

    /// `L1651` — an `AudioZone` field is invalid: a non-finite or non-positive extent or radius, a
    /// negative or non-finite blend or gain, more than eight beds, or a reverb field out of range.
    pub const AUDIO_ZONE_INVALID: DiagnosticCode = DiagnosticCode::new("L1651");

    /// `L1137` — an assembly document (`*.assembly.yaml`, `kind: assembly`) is malformed: a
    /// required key is missing or has the wrong type at any level, the document (or a part entry)
    /// has a key the assembly format does not define, `size`/`place`/`pivot` does not have exactly
    /// three components, `mirror` names something other than the one legal axis `x`, or a `clips`
    /// name/path is not a string. The `L1113`/`L1139` analog for the assembly family
    /// (`Plan/voxel-animation.md` decision 9). Raised by `ludion-scene`.
    pub const ASSEMBLY_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1137");

    /// `L1138` — an assembly document parsed structurally cleanly, but its data does not hang
    /// together: a palette glyph is not a single printable-ASCII character, is the reserved `.`, is
    /// defined twice, or carries a non-finite color or an out-of-range material scalar; `parts` is
    /// empty; two parts share a name; a `parent` names no part in the assembly; the hierarchy has
    /// zero roots, more than one root, or a cycle; a `pivot` component is not finite; a `place`
    /// component falls outside the assembly's own `size`; or — across files — a part's glyph names
    /// no palette entry, or a placed part overflows the assembled grid. The `L1114`/`L1140` analog
    /// for the assembly family. Raised by `ludion-scene`.
    pub const ASSEMBLY_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1138");

    /// `L1139` — a part document (`parts/*.part.yaml`, `kind: part`) is malformed: a required
    /// top-level key is missing or has the wrong type, an unrecognized `kind` value was found, or
    /// the document has a key the part format does not define. The `L1113` analog for the part
    /// family (`Plan/voxel-animation.md`). Raised by `ludion-scene`.
    pub const PART_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1139");

    /// `L1140` — a part document parsed structurally cleanly, but its grid data does not hang
    /// together: `slices` has a layer count other than `size.y`, a layer has a row count other than
    /// `size.z`, a row has a character count other than `size.x`, or a slice character is neither
    /// `.` (empty) nor printable ASCII (`0x21..=0x7E`). A part has no palette, so unlike `L1114`
    /// there is no "names no palette entry" case — a printable-ASCII glyph is always structurally
    /// legal here; whether the owning assembly's palette actually claims it is that assembly's
    /// concern. The `L1114` analog for the part family. Raised by `ludion-scene`.
    pub const PART_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1140");

    /// `L1141` — an animation clip document (`anims/*.anim.yaml`, `kind: animation`) is malformed:
    /// a required top-level key is missing or has the wrong type, an unrecognized `kind` value was
    /// found, the document (or a track, a keyframe, or an event) has a key the animation format
    /// does not define, `generated_from` or an event's `t`/`name` is missing or mistyped, `events`
    /// is not a list of mappings, a keyframe's `v` does not have exactly three components, or a
    /// keyframe's `ease` names no recognized easing. The `L1113`/`L1139` analog for the animation family
    /// (`Plan/voxel-animation.md` decision 8). Raised by `ludion-scene`.
    pub const ANIMATION_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1141");

    /// `L1142` — an animation clip document parsed structurally cleanly, but its data does not hang
    /// together: `duration` is not finite, not greater than zero or over 3600 seconds, `tracks` is
    /// empty, a track animates no channel, a declared channel has zero keyframes, a keyframe's `t` or
    /// `v` is not finite, a keyframe's `t` falls outside `[0, duration]`, a channel's keyframe times
    /// are not strictly increasing, a `scale` component is not greater than zero, or a non-`step`
    /// `rotation` segment turns 180 degrees or more on an Euler axis or combined across axes. The
    /// `L1114`/`L1140` analog for the animation family. Raised by `ludion-scene`.
    pub const ANIMATION_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1142");

    /// `L1143` — two parts of an assembly overlap in the composed rest-pose grid in a way the
    /// hierarchy and the declared pivots do not explain: parts with no parent/child relation share
    /// cells, or a part overlaps its own parent far from that child's `pivot` (usually a wrong
    /// pivot). A **warning**, never an error — overlap is legal and expected at a joint (the
    /// joint-plug technique), and this is a heuristic like `L1028`/`L1029`
    /// (`Plan/voxel-animation.md` decision 10). Raised by `ludion-scene`.
    pub const ASSEMBLY_UNEXPECTED_OVERLAP: DiagnosticCode = DiagnosticCode::new("L1143");

    /// `L1144` — a game data-doc names an id, district, kind or doc that does not exist: a
    /// cross-reference between two docs (or from a doc into the scene/component vocabulary) that
    /// resolves to nothing. Schema validation cannot see this — the field is a well-typed string —
    /// so it is raised by the game's own doc-validation pass
    /// (`EngineBuilder::validate_docs`), which owns every rule; the engine runs the pass at host
    /// start, on a doc reload, and under `--check-docs`.
    pub const DOC_REFERENCE_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L1144");

    /// `L1145` — two game data-docs of one kind carry the same `name`. A doc's `name` is the
    /// identity every other doc, every save and every system resolves it by, so a shared one makes
    /// the later doc unreachable while both still load. Raised by the game's own doc-validation
    /// pass (`EngineBuilder::validate_docs`); the sibling of `L1144`, which is the same identity
    /// failing from the other side.
    pub const DOC_NAME_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L1145");

    /// `L1146` — more game data-docs (or doc entries) exist than the surface that shows them can
    /// display, so the overflow is silently invisible in game: more quests than the tracker has
    /// rows, more wares than the menu lists. A **warning** — the content is valid and the game
    /// runs; only its visibility is short. Raised by the game's own doc-validation pass
    /// (`EngineBuilder::validate_docs`).
    pub const DOC_SET_OVER_CAPACITY: DiagnosticCode = DiagnosticCode::new("L1146");

    /// `L1147` — a `kind: palette` document is malformed, or a `kind: sprite` document's shared-palette
    /// include does not resolve: the standalone palette document's structure is wrong (an unknown or
    /// wrong-typed key, a bad palette entry), or a sprite's `palette: { from }` names a file that
    /// cannot be read or that is not a `kind: palette` document. The `L1153` sibling for the shared
    /// palette a sprite includes rather than inlines (`Plan/sprite-engine.md`, "Data and bake"). A
    /// glyph a frame uses that is still undefined *after* the include resolves is the ordinary `L1153`,
    /// because by then the sprite has a concrete palette like any inline one. Raised by `ludion-scene`
    /// for the palette document's text and by the bake/`check-scene`/`generate-atlas` IO seam for an
    /// unreadable or wrong-kind reference.
    pub const SPRITE_PALETTE_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1147");

    /// `L1148` — a `kind: pattern` document's timeline entry names a bullet kind that is not in
    /// the document's own `bullets:` map. The diagnostic points at `/timeline/<i>/bullet` and
    /// carries a nearest-match suggestion over the document's own bullet names, the same
    /// did-you-mean shape as `L1152`. Raised by `ludion-engine`'s `patterns` (`Plan/projectile-patterns.md`
    /// decision 4).
    pub const PATTERN_BULLET_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1148");

    /// `L1149` — a `kind: pattern` document authors a bound the schema cannot express: a bullet
    /// kind's `motion` list over four segments, a segment carrying both `ramp` and `turn` (no
    /// elementary closed form), a volley `count` over 256 (the kill bitset's width), an `every: 0`
    /// paired with `repeat > 1`, or a non-finite or negative speed, frame count, or `life`. The
    /// pointer names the offending field. Raised by `ludion-engine`'s `patterns` (`Plan/projectile-patterns.md`
    /// decisions 1 and 4).
    pub const PATTERN_ENTRY_INVALID: DiagnosticCode = DiagnosticCode::new("L1149");

    /// `L1150` — a timeline entry's `play: <name>` does not resolve to a playable pattern: the
    /// named document does not exist, the named document itself contains a `play:` entry (nesting
    /// is one level deep), or the chain cycles back to the playing document. The pointer is
    /// `/timeline/<i>/play`. Raised by `ludion-engine`'s `patterns` (`Plan/projectile-patterns.md` decision 4).
    pub const PATTERN_PLAY_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L1150");

    /// `L1151` — a `Sprite` or `Tilemap` legend entry's atlas reference does not resolve: the named
    /// atlas document cannot be read or parsed, `frame` is set without `atlas` (or `atlas` without
    /// `frame`), or both `texture` and `atlas` are set on one entry. Raised where the reference is
    /// resolved against the atlas on disk, not by `ludion-scene`'s document parse.
    pub const SPRITE_ATLAS_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L1151");

    /// `L1152` — the atlas loaded, but has no region with the requested `frame` name. The message
    /// carries a nearest-match suggestion over the atlas's own region names
    /// (`AtlasDocument::nearest_region_name`). The sibling of [`SPRITE_ATLAS_UNRESOLVED`], which is
    /// the same reference failing one level earlier.
    pub const SPRITE_FRAME_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1152");

    /// `L1153` — a `kind: sprite`, `kind: atlas-source` or `kind: atlas` document is malformed:
    /// an unknown or wrong-typed key, rows that do not match `size`, a glyph naming no palette
    /// entry, a mirrored frame's rows not matching its kept half, a patch outside its frame, a
    /// `base` naming no `rows:` frame, `mirror` on a patched frame, a duplicate frame or region
    /// name, a region rect or pivot outside its image, a missing input file, or `generate-atlas
    /// --check` drift. Raised by `ludion-scene` for the text-level defects and by the bake for the
    /// ones only it can see.
    pub const SPRITE_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1153");

    /// `L1250` — a `kind: font` document is malformed: an unknown or wrong-typed key, glyphs out
    /// of code-point order, a glyph `rect` outside the atlas, an alias or kerning pair naming a
    /// char with no glyph, metrics that leave no line height or exceed their caps, an `image`
    /// path that escapes the document's directory, or `generate-atlas --check` drift of a font pair.
    /// Raised by `ludion-scene` and the bake.
    pub const FONT_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1250");

    /// `L1251` — a `kind: font-source` document is malformed: an unknown or wrong-typed key, a
    /// missing `file`, an `em_px` out of range, a `spread_px` on a bitmap face or missing on an SDF
    /// one, a `line_height` outside `1..=65535` or one that leaves the ascender at or below zero
    /// (raised by the bake), an atlas over 4096 on a side, a `name` that is not a plain file stem,
    /// an unknown charset range keyword, or a charset that resolves to nothing. Raised by
    /// `ludion-scene` and the bake.
    pub const FONT_SOURCE_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1251");

    /// `L1252` — a font file cannot be baked: unreadable, not a single TrueType/OpenType face (a
    /// `.ttc` collection included), or carrying no usable `cmap`. Raised by `ludion-font`'s bake.
    pub const FONT_FILE: DiagnosticCode = DiagnosticCode::new("L1252");

    /// `L1253` — a `kind: fonts` table is malformed: an unknown or wrong-typed key, a face name
    /// that is empty or holds whitespace or `/`, or a path that is absolute, climbs with `..`,
    /// is not in canonical form or does not end `.font.yaml`. Raised by `ludion-scene`.
    pub const FONTS_TABLE_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1253");

    /// `L1254` — a `font` field names a face the content root's `fonts.yaml` does not define, or
    /// the root has no table. The text is drawn and measured with the default face.
    pub const FONT_NAME_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1254");

    /// `L1255` — a face in the font table cannot load: its `kind: font` document or atlas image is
    /// unreadable or invalid, its atlas PNG's size disagrees with the document, or the table itself
    /// cannot be read; also `asset.reload` of a font while a recording is armed.
    pub const FONT_LOAD: DiagnosticCode = DiagnosticCode::new("L1255");

    /// `L1256` — authored text uses characters the resolved table face has no glyph for; they
    /// draw as the face's `.notdef` glyph (the built-in 8x8 face draws them blank). A warning.
    pub const FONT_GLYPH_MISSING: DiagnosticCode = DiagnosticCode::new("L1256");

    /// `L1257` — a type registered as a data-doc kind or as the player profile declares a
    /// top-level `format` or `kind` field, which collides with the doc envelope's own keys. The
    /// engine refuses it when built; rename the field. Raised by `ludion-scene`'s
    /// `check_doc_type`.
    pub const DOC_TYPE_RESERVED_FIELD: DiagnosticCode = DiagnosticCode::new("L1257");

    /// `L1258` — `ludion voxel apply` or `upscale` dropped a source comment that had no place in
    /// the re-emitted model. A warning. Raised by `ludion-cli`.
    pub const VOXEL_COMMENT_DROPPED: DiagnosticCode = DiagnosticCode::new("L1258");

    /// `L1259` — the ComfyUI server's `/object_info` reply is not an object of node classes.
    /// Raised by `ludion-cli`.
    pub const COMFY_REPLY_MALFORMED: DiagnosticCode = DiagnosticCode::new("L1259");

    /// `L1154` — one entry of a sprite document's `effects:` list is invalid: an unknown `op`, a
    /// missing or out-of-range parameter, a `glyphs:` entry naming no palette glyph, a `frames:`
    /// entry naming no frame, or a `dither` without exactly two glyphs. Split from
    /// [`SPRITE_DOCUMENT_STRUCTURE`] because an effect is the one part of the document an author
    /// edits by sweep, so its failures need their own code to match on.
    pub const SPRITE_EFFECT_INVALID: DiagnosticCode = DiagnosticCode::new("L1154");

    /// `L1155` — the content-root walk of an engine that registered **only** engine-owned doc kinds
    /// (the built-in engine behind `ludion serve`/`ludion sim-hash`) found a `*.yaml` declaring a
    /// `kind:` it does not know, so the file was skipped. A warning, not the `L1119` failure, because
    /// such an engine hosts anyone's scene and cannot know a game's kinds. Raised by `ludion-engine`.
    pub const DOC_KIND_NOT_REGISTERED: DiagnosticCode = DiagnosticCode::new("L1155");

    /// `L1156` — a `kind: sculpt` document's structure is wrong: a missing or wrong-typed key, a
    /// non-mapping root or part, an unrecognized `kind`, `shape`, `op` or `mirror` value, or a key no
    /// part, bone or `repeat` has (with a nearest match). Raised by `ludion-scene`
    /// (`Plan/mesh-modeling.md`, "The document").
    pub const SCULPT_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1156");

    /// `L1157` — a structurally sound `kind: sculpt` document's data does not hang together: a
    /// non-finite float, `resolution` outside `4..=128`, `triangles` below 12, a duplicate or empty
    /// part name, a non-positive dimension, a `round` past half the box, a negative `blend`, a
    /// `repeat` `count` below 2, a `material` naming no palette glyph, missing on a `union` part or
    /// authored on a `subtract`/`intersect` part, an empty `deform`, a `taper` scale that is not
    /// positive, a negative `noise` `amplitude` or a `noise` `scale` that is not positive, or a key
    /// that belongs to another shape (with that shape's key list). Raised by `ludion-scene`.
    pub const SCULPT_DOCUMENT_DATA: DiagnosticCode = DiagnosticCode::new("L1157");

    /// `L1158` — a sculpt part cannot be seen in the meshed result: it is thinner than two grid cells
    /// at the document's `resolution`, or contributes no surface vertex. A **warning** — authored and
    /// never seen, the mesh analogue of a wrong pivot. Raised by the sculpt mesher.
    pub const SCULPT_PART_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L1158");

    /// `L1159` — a sculpt's meshed surface splits into more than one connected component. A
    /// **warning** carrying the nearest pair of vertices between each stray piece and the main body,
    /// the `L1129` shape. Raised by the sculpt mesher.
    pub const SCULPT_DISCONNECTED: DiagnosticCode = DiagnosticCode::new("L1159");

    /// `L1160` — a sculpt document's bone tree is invalid: an empty or duplicate bone name, a
    /// `parent` or part `bone` naming no bone, a first bone that has a parent, a second root, a
    /// `parent` chain that loops, or a bone position that is half-authored (`head` without `tail`,
    /// or the reverse) or non-finite. Raised by `ludion-scene`.
    pub const SCULPT_BONE_TREE: DiagnosticCode = DiagnosticCode::new("L1160");

    /// `L1161` — a `kind: generation` document's structure is wrong: a missing or wrong-typed key, a
    /// non-mapping root, `images` or `inputs`, an unrecognized `kind`, or a key a generation recipe
    /// does not have (with a nearest match). Raised by `ludion-scene`
    /// (`Plan/comfy-generation.md`, "The recipe document").
    pub const GENERATION_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1161");

    /// `L1162` — a structurally sound `kind: generation` document's data does not hang together: a
    /// `count` below 1, a negative or non-integer `seed`, a non-finite number, an empty `workflow`
    /// or `images` path, an `{ image: x }` naming no declared `images` entry, a structured input
    /// value that is not that one image-reference form, a `name` that is not a plain file stem, or
    /// a `workflow` that resolves outside the project root. Raised by `ludion-scene` and the
    /// generation client.
    pub const GENERATION_DATA: DiagnosticCode = DiagnosticCode::new("L1162");

    /// `L1163` — the workflow a recipe names is not in API format: a UI/litegraph export, whose
    /// `nodes`/`links` arrays the server's `/prompt` endpoint does not accept. Names the
    /// **File → Export (API)** menu item and the expected flat `id -> {class_type, inputs}` shape.
    /// Raised by the generation client.
    pub const WORKFLOW_NOT_API: DiagnosticCode = DiagnosticCode::new("L1163");

    /// `L1164` — a workflow node's `class_type` is absent from the server's `/object_info`: a custom
    /// node the server does not have installed, or a typo. Names the class and the nearest installed
    /// match. Raised by the generation client.
    pub const WORKFLOW_UNKNOWN_NODE: DiagnosticCode = DiagnosticCode::new("L1164");

    /// `L1165` — a workflow input is invalid: a required input the graph never supplies, or an
    /// override whose value does not match the input's declared type or enum. Names the node title,
    /// the input and the expected type. Raised by the generation client.
    pub const WORKFLOW_INPUT_INVALID: DiagnosticCode = DiagnosticCode::new("L1165");

    /// `L1166` — a recipe override names a node `_meta.title` that the workflow does not have, or
    /// that two of its nodes share, so the override addresses nothing or is ambiguous. Lists every
    /// title in the graph. Raised by the generation client.
    pub const WORKFLOW_TITLE_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L1166");

    /// `L1167` — nothing answers at the resolved ComfyUI address, or the address is refused before
    /// anything is tried (an `https://` URL, which this client has no TLS for, or one that is not
    /// `http://`). Names both the `LUDION_COMFY_URL` environment property and the `--url` flag, and
    /// never writes the address into a file. Raised by the generation client.
    pub const COMFY_UNREACHABLE: DiagnosticCode = DiagnosticCode::new("L1167");

    /// `L1168` — the ComfyUI server rejected a request: a `/prompt` 400 with its `node_errors`
    /// reprinted structurally, any other endpoint's HTTP error, or a reply the server began and the
    /// connection could not finish. Raised by the generation client.
    pub const COMFY_REJECTED: DiagnosticCode = DiagnosticCode::new("L1168");

    /// `L1169` — a submitted job failed on the server (an `execution_error`, an error status in
    /// history), is lost (no history entry, absent from the queue) or finished and wrote no file
    /// (`empty`). One per job; a partial run keeps the rest. Names the node, its class and the
    /// server's message. Raised by the run pipeline.
    pub const GENERATION_RUN_FAILED: DiagnosticCode = DiagnosticCode::new("L1169");

    /// `L1170` — a run did not finish within `--timeout`, or `gen fetch` found a job still queued.
    /// Names the job id and the `gen status` / `gen fetch` recovery, since the server may still be
    /// working. Raised by the generation client and the run pipeline.
    pub const GENERATION_RUN_TIMED_OUT: DiagnosticCode = DiagnosticCode::new("L1170");

    /// `L1171` — a run's output kind is unusable: the graph has no output node, its only output is
    /// a preview, it produces images and a mesh at once, or `gen sheet` is asked for a mesh run or
    /// a run with no candidate. Raised by the workflow check and `gen sheet`.
    pub const GENERATION_OUTPUT_KIND: DiagnosticCode = DiagnosticCode::new("L1171");

    /// `L1172` — an accept was refused: a candidate index out of range, a destination that already
    /// exists (a file for an image, a directory holding anything for a mesh import), a missing or
    /// incomplete run folder, a run that failed or is not finalized, an older report that cannot
    /// be re-read faithfully (recipe or workflow drifted, recipe unresolvable, overrides no longer
    /// apply, unreadable inputs snapshot, a missing or mismatched `inputs/` image copy). Raised by
    /// the generation client.
    pub const GENERATION_ACCEPT_REFUSED: DiagnosticCode = DiagnosticCode::new("L1172");

    /// `L1173` — a `kind: mesh-edit` document's structure is wrong: a missing or wrong-typed key, a
    /// key a mesh-edit document does not have (with a nearest match), or an unrecognized `op`, `map`
    /// or region shape. Also carries the derived tree's drift report (the `generate-atlas`
    /// precedent). Raised by `ludion-scene` (`Plan/mesh-texture-editing.md`, "The edit document").
    pub const MESH_EDIT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1173");

    /// `L1174` — a structurally sound `kind: mesh-edit` document's data does not hang together: a
    /// non-finite number, a non-positive radius, extent or map size, a colour or strength outside
    /// `[0, 1]`, a negative feather, colour tolerance or colour feather, a capsule with fewer than
    /// two points, an empty `source`, or an `out` that escapes the document's own directory
    /// (`..`, an absolute path, a `:` or a Windows device name), or an absolute or `:`-prefixed
    /// `source`. Raised by `ludion-scene`.
    pub const MESH_EDIT_DATA: DiagnosticCode = DiagnosticCode::new("L1174");

    /// `L1175` — a pick resolved to no surface: no triangle under that pixel, an unknown view name,
    /// or a pixel outside the assumed extent. Names the view's axis and the extent it assumed, since
    /// a pick is read against a rendered sheet and the two must agree. Raised by the mesh-edit pick.
    pub const MESH_EDIT_PICK_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L1175");

    /// `L1176` — an edit cannot apply to the source it names: the source mesh has no `uvs`, a target
    /// map cannot be created for want of a `size`, the derived directory holds files the derive
    /// did not write, or the source's UVs or map extent cannot be rasterised. Raised by the
    /// mesh-edit derive.
    pub const MESH_EDIT_INAPPLICABLE: DiagnosticCode = DiagnosticCode::new("L1176");

    /// `L1177` — an op painted nothing: its region covers no texel of the source's surface. Names
    /// the op index and the region's world box against the mesh's, because a region that misses the
    /// surface is the likeliest failure for an author who cannot see the model. Raised by the
    /// mesh-edit derive.
    pub const MESH_EDIT_OP_PAINTED_NOTHING: DiagnosticCode = DiagnosticCode::new("L1177");

    /// `L1178` — a colour match failed: a `key` filter rejected every texel its region reaches
    /// (the message names the texels reached, the nearest distance and that texel's colour), or a
    /// `fill` region's seed is not on the surface or lies outside its `max_radius`. Raised by the
    /// mesh-edit derive.
    pub const MESH_EDIT_MATCH_FAILED: DiagnosticCode = DiagnosticCode::new("L1178");

    /// `L1181` — a `Collider` component's `convex` shape's `points` enclose no volume: fewer than
    /// four points, or all of them coincident, collinear, or coplanar. Raised by the `Collider`
    /// component's semantic validator on load; the JSON Pointer names `/shape/convex/points`.
    /// Points that pass this check but the solver's own hull builder still rejects (nearly
    /// coplanar or nearly coincident, within its own tolerance) raise it again at runtime, named by
    /// entity instead of a pointer.
    pub const COLLIDER_DEGENERATE_HULL: DiagnosticCode = DiagnosticCode::new("L1181");

    /// `L1182` — a `Collider` component's `heightfield` shape has a malformed grid: `rows` or
    /// `cols` below 2, or `heights.len()` not equal to `rows * cols`. Raised by the `Collider`
    /// component's semantic validator; the JSON Pointer names `/shape/heightfield/rows`,
    /// `/shape/heightfield/cols`, or `/shape/heightfield/heights`.
    pub const COLLIDER_HEIGHTFIELD_GRID: DiagnosticCode = DiagnosticCode::new("L1182");

    /// `L1183` — a `Joint` component is structurally invalid: `a` and `b` name the same entity, an
    /// `anchor` or a kind's `axis` is not finite, an axis is the zero vector, a `[min, max]` limit
    /// (hinge, slider, or one of a ball's three) is non-finite or inverted, or `break_impulse` is
    /// set but not strictly positive and finite. Raised by the `Joint` component's semantic
    /// validator; the JSON Pointer names the offending field.
    pub const JOINT_INVALID: DiagnosticCode = DiagnosticCode::new("L1183");

    /// `L1184` — an assembly document's `joints:` entry is structurally invalid: it names a part the
    /// assembly does not have, names the assembly's root part (a joint is carried by a non-root part
    /// and joins it to its `parent`, which the root does not have), an `axis` is non-finite or the
    /// zero vector, a `[min, max]` limit (hinge, slider, or one of a ball's three) is non-finite or
    /// inverted, or `break_impulse` is set but not strictly positive and finite. Raised by
    /// `ludion-scene`'s assembly validator (`L1138`'s data-consistency family, its own code since it
    /// mirrors `Joint`'s `L1183` one for one); the JSON Pointer names the offending field under
    /// `/joints/<part>`.
    pub const ASSEMBLY_JOINT_INVALID: DiagnosticCode = DiagnosticCode::new("L1184");

    /// `L1185` — a `Mesh` component's `shape: {assembly_part: {assembly, part}}` names a `part` the
    /// named assembly does not have. Raised by `ludion-engine`'s mesh resolution (`extract.rs`); the
    /// JSON Pointer is `/entities/<index>/components/Mesh/shape/assembly_part/part`. A failure to
    /// load the named assembly itself (the same failure a whole-assembly `MeshSource::Assembly`
    /// reports) keeps that diagnostic's own code, pointed at `.../assembly_part/assembly` instead.
    pub const MESH_PART_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1185");

    /// `L1186` — a `Collider`'s `layer`, or an entry of a `QueryLayers`, names no layer the scene's
    /// `physics.layers` declares. The JSON Pointer is `/entities/<index>/components/Collider/layer`
    /// or `/entities/<index>/components/QueryLayers/layers/<j>`.
    pub const COLLIDER_LAYER_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1186");

    /// `L1187` — a `Collider` carries a `layer` the 3D solver does not claim: a 2D `circle`/`aabb`
    /// that is not `rigid`, whose tier has no filtering. The JSON Pointer is
    /// `/entities/<index>/components/Collider/layer`.
    pub const COLLIDER_LAYER_UNCLAIMED: DiagnosticCode = DiagnosticCode::new("L1187");

    /// `L1500` — a `QueryLayers` entry is empty or repeated, or (`check-scene`) names the entity's
    /// own body layer. The JSON Pointer is `.../QueryLayers/layers/<j>`.
    pub const QUERY_LAYERS_INVALID: DiagnosticCode = DiagnosticCode::new("L1500");

    /// `L1501` — a `QueryLayers` on an entity with neither a solver-claimed `Collider` nor a
    /// `CharacterMotor`, so no query can ever see its extra layers. Raised by `check-scene`.
    pub const QUERY_LAYERS_UNCLAIMED: DiagnosticCode = DiagnosticCode::new("L1501");

    /// `L1690` — a `SurfaceTag.surface` or the scene's `physics.default_surface` names no surface
    /// that `physics.surfaces` declares. The JSON Pointer is
    /// `/entities/<i>/components/SurfaceTag/surface` or `/physics/default_surface`.
    pub const SURFACE_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1690");

    /// `L1691` — a `physics.surfaces` entry is empty or repeated, the list holds more than 256
    /// names, or a `SurfaceTag.surface` is empty.
    pub const SURFACES_INVALID: DiagnosticCode = DiagnosticCode::new("L1691");

    /// `L1692` — a `SurfaceTag` on an entity with neither a solver-claimed `Collider` nor a
    /// `CharacterMotor`, so no hit can ever report it. Raised by `check-scene`.
    pub const SURFACE_TAG_UNCLAIMED: DiagnosticCode = DiagnosticCode::new("L1692");

    /// `L1693` — a `SurfaceOverlay` on an entity without a solver-claimed `Collider` with
    /// `sensor: true`, so it has no volume to contain a hit point. Raised by `check-scene`.
    pub const SURFACE_OVERLAY_NO_SENSOR: DiagnosticCode = DiagnosticCode::new("L1693");

    /// `L1694` — a `SurfaceTag.parts` entry whose key is empty or repeated, whose integer key is
    /// not a part of the entity's `compound` `Collider`, or whose string key is on an entity whose
    /// `Mesh` is not an `assembly`.
    pub const SURFACE_PART_INVALID: DiagnosticCode = DiagnosticCode::new("L1694");

    /// `L1695` — a `SurfaceTag.parts` string key names no part of the assembly `switch_to_ragdoll`
    /// switched; every part took the root surface. A warning.
    pub const SURFACE_PART_UNMATCHED: DiagnosticCode = DiagnosticCode::new("L1695");

    /// `L1696` — a mesh document's `surfaces`/`triangle_surfaces` keys or a `SurfaceTag.cells`
    /// table are malformed (one key without the other, an empty or repeated name, a zero run, an
    /// index out of range, or run counts that miss the triangle or cell count).
    pub const MESH_SURFACES_INVALID: DiagnosticCode = DiagnosticCode::new("L1696");

    /// `L1697` — a mesh document's `surfaces` names a surface the scene's `physics.surfaces`
    /// does not declare; those triangles fall through to part, entity and default. A warning.
    pub const MESH_SURFACE_UNDECLARED: DiagnosticCode = DiagnosticCode::new("L1697");

    /// `L1510` — a `Machine`'s `parent` fn does not form a tree under `ROOT`: `ROOT` has a parent,
    /// or some state's parent chain never reaches `ROOT`. Raised by `ludion-ai`'s `register_machine`.
    pub const AI_MACHINE_HIERARCHY: DiagnosticCode = DiagnosticCode::new("L1510");

    /// `L1511` — a `Machine`'s `initial(s)` names a state whose `parent` is not `s`. Raised by
    /// `ludion-ai`'s `register_machine`.
    pub const AI_MACHINE_INITIAL: DiagnosticCode = DiagnosticCode::new("L1511");

    /// `L1512` — a `Machine`'s `ALL` is empty, omits `ROOT`, or lists a variant twice. Raised by
    /// `ludion-ai`'s `register_machine`.
    pub const AI_MACHINE_VARIANTS: DiagnosticCode = DiagnosticCode::new("L1512");

    /// `L1513` — a `Machine`'s `Event` type holds an entity reference that is not nullable. Raised by
    /// `ludion-ai`'s `register_machine`.
    pub const AI_EVENT_REF_NOT_NULLABLE: DiagnosticCode = DiagnosticCode::new("L1513");

    /// `L1188` — a `JointMotor` is structurally invalid: its `target` is not 1 or 3 numbers or holds
    /// a non-finite one, or its `stiffness`, `damping` or `max_force` is negative or non-finite. The
    /// JSON Pointer names the offending field.
    pub const JOINT_MOTOR_INVALID: DiagnosticCode = DiagnosticCode::new("L1188");

    /// `L1750` — a `Joint`'s `zero` is structurally invalid: it holds a non-finite element or has a
    /// length under 1e-6, so it is not a quaternion. The JSON Pointer is `/zero`.
    pub const JOINT_ZERO_INVALID: DiagnosticCode = DiagnosticCode::new("L1750");

    /// `L1751` — a `PoseSnapshot` is structurally invalid: a part's `rotation` or `translation`
    /// holds a non-finite element, or a `rotation` has a squared length under 1e-12. The JSON
    /// Pointer names the part's field.
    pub const POSE_SNAPSHOT_INVALID: DiagnosticCode = DiagnosticCode::new("L1751");

    /// `L1752` — a `kind: paired` document is structurally invalid: a missing or wrong-typed key,
    /// an unknown key, or a role, offset, event or contact of the wrong shape.
    pub const PAIRED_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1752");

    /// `L1753` — a well-shaped `kind: paired` document breaks a data rule inside the file: an
    /// unknown role reference, a missing offset, events out of order, a bad contact.
    pub const PAIRED_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1753");

    /// `L1754` — a `kind: paired` document disagrees with its role assemblies: one does not load, a
    /// clip or contact part is unknown, or an event lies past a clip's duration.
    pub const PAIRED_ROLE_MISMATCH: DiagnosticCode = DiagnosticCode::new("L1754");

    /// `L1755` — warning: a paired contact's measured gap exceeds its `max_gap`.
    pub const PAIRED_CONTACT_GAP: DiagnosticCode = DiagnosticCode::new("L1755");

    /// `L1189` — a `Collider` whose shape is a `compound` is structurally invalid: `parts` is
    /// missing or empty, a part's `shape` is a `compound`, `heightfield`, `circle` or `aabb`, or a
    /// part `translation`/`rotation` element is beyond `f32` range. The JSON Pointer names the
    /// offending `parts`, part `shape` or element.
    pub const COLLIDER_COMPOUND_INVALID: DiagnosticCode = DiagnosticCode::new("L1189");

    /// `L1190` — the skin keys of a `kind: mesh` document are structurally malformed: only some of
    /// `joints`, `weights` and `bone_indices` are present, or a joint is not a mapping, lacks `name`,
    /// `rest` (`t`, `r`, `s`) or `inverse_bind`, holds an unknown key, or a list has the wrong type
    /// or length (`t` 3, `r` 4, `s` 3, `inverse_bind` 16). The JSON Pointer names the offending
    /// key or element. Raised by `ludion-scene`.
    pub const MESH_SKIN_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1190");

    /// `L1191` — the skin data of a `kind: mesh` document is inconsistent: `joints` is empty or
    /// past 256, a joint name is empty or repeated, a `parent` does not come earlier in `joints`, a
    /// rest or bind value is non-finite or a rest rotation is not a unit quaternion, `weights` or
    /// `bone_indices` is not 4 per vertex, a weight is negative or a vertex's weights do not sum to
    /// 1, or a bone index names no joint. The JSON Pointer names the offending element. Raised by
    /// `ludion-scene`.
    pub const MESH_SKIN_DATA: DiagnosticCode = DiagnosticCode::new("L1191");

    /// `L1192` — a rescue quest's safe zone is too small to hold both the player and a trailing
    /// escortee: a `kind: quest` document's `archetype: rescue` authors a `safe_radius` under the
    /// game's own floor (twice its escortee trail distance). Schema-valid — the field is a plain
    /// float — so the constraint is a game rule, raised by the game's own doc-validation pass at host
    /// start, on a doc reload and under `--check-docs`. The JSON Pointer names the offending
    /// `safe_radius` field.
    pub const QUEST_SAFE_ZONE_TOO_SMALL: DiagnosticCode = DiagnosticCode::new("L1192");

    /// `L1194` — a `SkinBone`'s `skin` names an entity that is not a skinned mesh: it carries no
    /// `Mesh`, its `shape` is not `asset`, or the mesh document it names has no `joints:`. The JSON
    /// Pointer is `/entities/<index>/components/SkinBone/skin`.
    pub const SKIN_BONE_NOT_SKINNED: DiagnosticCode = DiagnosticCode::new("L1194");

    /// `L1195` — a `SkinBone`'s `bone` names no joint of its skin's mesh document. The JSON Pointer is
    /// `/entities/<index>/components/SkinBone/bone`; the help names the nearest joint name, or lists
    /// every joint name when none is close.
    pub const SKIN_BONE_UNKNOWN_BONE: DiagnosticCode = DiagnosticCode::new("L1195");

    /// `L1196` — two `SkinBone` parts name the same `(skin, bone)` pair. The JSON Pointer names the
    /// later part; the message names the earlier one.
    pub const SKIN_BONE_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L1196");

    /// `L1197` — a `SkinBone`'s `skin` entity carries an `Animation`: one pose source per skin, so a
    /// skin that plays a clip cannot also be driven by its parts' rest poses. Raised once per skin,
    /// pointed at `/entities/<skin index>/components/Animation`.
    pub const SKIN_BONE_ANIMATED_SKIN: DiagnosticCode = DiagnosticCode::new("L1197");

    /// `L1198` — an animation clip's `events` are invalid: `events:` is declared with no entries, an
    /// event's `t` is not finite or falls outside `[0, duration]`, event times are not strictly
    /// increasing, an event `name` is empty, or a clip with `generated_from` carries events. Pointed
    /// at `/events` or `/events/<i>/t|name`. Raised by `ludion-scene`.
    pub const ANIMATION_EVENT_INVALID: DiagnosticCode = DiagnosticCode::new("L1198");

    /// `L1199` — a `kind: character` document is structurally invalid: an empty or non-mapping root,
    /// a wrong `format`/`kind`, a missing `mesh` or clip `clip`, a wrong-typed value, or an unknown
    /// key. Pointed at the offending key. Raised by `ludion-scene`.
    pub const CHARACTER_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1199");

    /// `L1200` — a well-shaped `kind: character` document is inconsistent: a blank `mesh`/`clip`
    /// path, a non-finite or non-positive `speed`, overlay `events` on a clip that has its own, a
    /// clip track naming a joint the mesh lacks, or a mesh with no skin. Raised by `ludion-scene`
    /// and the engine's character loader.
    pub const CHARACTER_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1200");

    /// `L1670` — an `AnimationLayers` component holds more than four layers. Pointed at `/layers`.
    pub const ANIMATION_LAYERS_TOO_MANY: DiagnosticCode = DiagnosticCode::new("L1670");

    /// `L1671` — an `AnimationLayers` layer is invalid: a duplicate or empty `name`, a `weight` or
    /// `from_weight` outside `[0, 1]`, or a negative or non-finite `fade` or `speed`. Pointed at
    /// `/layers/<i>/<field>`.
    pub const ANIMATION_LAYER_INVALID: DiagnosticCode = DiagnosticCode::new("L1671");

    /// `L1672` — a `SkinBone`-driven skin carries an `AnimationLayers`: its pose comes from the
    /// parts, so layers have nothing to compose over. Raised once per skin.
    pub const ANIMATION_LAYERS_ON_SKIN_BONE_SKIN: DiagnosticCode = DiagnosticCode::new("L1672");

    /// `L1673` — an `IkTargets` target is invalid: not exactly one of `point` or
    /// `direction`, a `weight` or `from_weight` outside `[0, 1]`, a negative or non-finite `fade`, or
    /// a non-finite number. Pointed at `/targets/<name>/<field>`.
    pub const IK_TARGET_INVALID: DiagnosticCode = DiagnosticCode::new("L1673");

    /// `L1201` — a glTF input could not be read or parsed: the file is unreadable, or its bytes are
    /// not a valid `.gltf`/`.glb` document (bad JSON, a failed structural validation, or a buffer/
    /// image source that could not be resolved), or its data is malformed (a non-finite number, an
    /// accessor that does not fit its buffer view), or its node graph is not a forest (a node listed
    /// as a child twice, or its own ancestor). Raised by `ludion-import` (`ludion import`).
    pub const IMPORT_INVALID_GLTF: DiagnosticCode = DiagnosticCode::new("L1201");

    /// `L1202` — a glTF document uses a feature the importer does not map onto a Ludion component
    /// (a texture on a second UV set, a `BLEND` alpha mode, any camera — a prefab carries no `Camera`, a directional light, a morph target,
    /// a morph-weights channel, an animation channel on a node under a skin joint, or one mixing skin
    /// joints and prefab parts, a sparse or non-triangle primitive, a JPEG image, any other extension). A hard error by default — the offending node/mesh/accessor is named — downgraded
    /// to a printed warning by `--allow-lossy`. Raised by `ludion-import`.
    pub const IMPORT_UNSUPPORTED_FEATURE: DiagnosticCode = DiagnosticCode::new("L1202");

    /// `L1203` — the imported output tree could not be written: a directory could not be created or
    /// a file (`model.prefab.yaml`, a mesh doc, a texture) could not be written to disk; or the
    /// importer's own PNG re-encode failed in memory (an importer bug). Raised by `ludion-import`
    /// (`ludion import`).
    pub const IMPORT_OUTPUT_IO: DiagnosticCode = DiagnosticCode::new("L1203");

    /// `L1204` — a glTF external buffer or image URI is not readable from where the import allows:
    /// it leaves the `.gltf`'s directory, names a non-regular or oversized file, or the caller
    /// accepts embedded sources only. Raised by `ludion-import` (`ludion import`, `ludion gen accept`).
    pub const IMPORT_URI_REFUSED: DiagnosticCode = DiagnosticCode::new("L1204");

    /// `L1205` — a skinned vertex's four glTF weights sum to zero, so the importer bound it fully to
    /// the skin's root joint. A warning: the import succeeds. Raised by `ludion-import`.
    pub const IMPORT_SKIN_UNWEIGHTED: DiagnosticCode = DiagnosticCode::new("L1205");

    /// `L1239` — a glTF's optional `extensionsUsed` entry has no Ludion mapping, so what it adds is
    /// left out. A warning: the import succeeds. Raised by `ludion-import`.
    pub const IMPORT_EXTENSION_IGNORED: DiagnosticCode = DiagnosticCode::new("L1239");

    /// `L1240` — a placed prop floats: it is above the ground and touches no other prop. A warning
    /// from `ludion check-scene --placement`.
    pub const PLACEMENT_FLOATING: DiagnosticCode = DiagnosticCode::new("L1240");

    /// `L1241` — a placed prop is sunk below the ground by more than the tolerance. A warning from
    /// `ludion check-scene --placement`.
    pub const PLACEMENT_SUNK: DiagnosticCode = DiagnosticCode::new("L1241");

    /// `L1242` — two props' static colliders overlap. A warning from `ludion check-scene
    /// --placement`.
    pub const PLACEMENT_OVERLAP: DiagnosticCode = DiagnosticCode::new("L1242");

    /// `L1243` — a `MeshGrid`'s `orient` does not match its `rows` in shape, or holds a glyph that is
    /// not an orientation.
    pub const MESH_GRID_ORIENT: DiagnosticCode = DiagnosticCode::new("L1243");

    /// `L1244` — a `MeshGrid` legend entry names a `character` shape, which cannot pose per cell.
    pub const MESH_GRID_ANIMATED_SHAPE: DiagnosticCode = DiagnosticCode::new("L1244");
    /// `L1245` — `ludion sync` refused an `[[import]]` table: a `from` that matches nothing, two sources
    /// sharing a stem, or a `to` holding files it does not own.
    pub const IMPORT_SYNC_REFUSED: DiagnosticCode = DiagnosticCode::new("L1245");

    /// `L1206` — an imported animation clip, after key reduction, strays from the source motion by
    /// more than the model-space error bound on some joint. The clip is refused (never downgraded
    /// by `--allow-lossy`). Raised by `ludion-import`.
    pub const IMPORT_CLIP_ERROR_BOUND: DiagnosticCode = DiagnosticCode::new("L1206");

    /// `L1207` — a glTF animation's `extras` carries an unknown key or a tolerance that is not a
    /// finite positive number. Raised by `ludion-import`.
    pub const IMPORT_CLIP_EXTRAS_INVALID: DiagnosticCode = DiagnosticCode::new("L1207");

    /// `L1208` — every channel of an imported glTF animation stays within the key-reduction tolerance
    /// of the rest pose, so the clip would have no tracks. Refused in both modes (never downgraded by
    /// `--allow-lossy`). Raised by `ludion-import`.
    pub const IMPORT_CLIP_AT_REST: DiagnosticCode = DiagnosticCode::new("L1208");

    /// `L1209` — a `Sprite` drawn from an atlas carries an `Animation` with `fade` above zero; a
    /// flipbook hard-cuts, so the crossfade fields are ignored. Raised by `ludion-engine`'s scene
    /// check.
    pub const ANIMATION_SPRITE_CROSSFADE: DiagnosticCode = DiagnosticCode::new("L1209");

    /// `L1210` — an entity's `Animation` sits on a `Mesh` whose assembly or character failed to load
    /// at runtime, so `animation_events` fires nothing for it. Raised once per entity by
    /// `ludion-engine`.
    pub const ANIMATION_MODEL_UNLOADED: DiagnosticCode = DiagnosticCode::new("L1210");

    /// `L1211` — a `Collider` with `shape: { mesh: … }` sits on a `body: rigid`. A trimesh is
    /// hollow, so a dynamic body cannot be simulated against it. Raised by `ludion-spatial`'s
    /// collider validation and by `ludion-physics` at runtime.
    pub const COLLIDER_MESH_ON_RIGID: DiagnosticCode = DiagnosticCode::new("L1211");

    /// `L1212` — a mesh collider's document cannot be built: a blank path, a missing or invalid
    /// `kind: mesh` document, a trimesh the physics engine refuses, or no loader installed. Raised by
    /// `ludion-spatial`'s collider validation and by `ludion-physics` at runtime.
    pub const COLLIDER_MESH_UNAVAILABLE: DiagnosticCode = DiagnosticCode::new("L1212");

    /// `L1213` — a `kind: path` document is structurally invalid: an empty or non-mapping root, an
    /// envelope error, a wrong type, an unknown key, a point that is not three numbers, or an unknown
    /// `interpolation`. Raised by `ludion-scene`.
    pub const PATH_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1213");

    /// `L1214` — a well-shaped `kind: path` document is inconsistent: too few points, a non-finite
    /// or out-of-f32-range component, or coincident neighbouring points. Raised by `ludion-scene`.
    pub const PATH_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1214");

    /// `L1215` — an entity's `PathFollower.path` names a `kind: path` document that is missing or
    /// invalid, so the entity does not move. Raised once per entity by `ludion-engine`'s
    /// `path_follow`.
    pub const PATH_FOLLOWER_UNLOADED: DiagnosticCode = DiagnosticCode::new("L1215");

    /// `L1217` — a `kind: generation` recipe declares an `images:` entry that no `inputs:` override
    /// references as `{ image: <name> }`, so the workflow would run its own baked-in image. Raised
    /// by the generation client, offline, before anything is uploaded.
    pub const GENERATION_IMAGE_UNBOUND: DiagnosticCode = DiagnosticCode::new("L1217");

    /// `L1218` — an entity's `Parent` chain loops at runtime (a raw component insert; scenes refuse
    /// it as `L1035`). The entity is treated as a root. Raised once per loop by `ludion-engine`.
    pub const HIERARCHY_CYCLE: DiagnosticCode = DiagnosticCode::new("L1218");

    /// `L1219` — a `Parent` names a dead entity, or a sim entity names a presentation parent. The
    /// chain stops there and the entity acts as a root. Raised once per entity by `ludion-engine`.
    pub const HIERARCHY_PARENT_INVALID: DiagnosticCode = DiagnosticCode::new("L1219");

    /// `L1220` — an entity has more ancestors than `MAX_HIERARCHY_DEPTH` (64); it is treated as a
    /// root. Raised once per entity by `ludion-engine`.
    pub const HIERARCHY_TOO_DEEP: DiagnosticCode = DiagnosticCode::new("L1220");

    /// `L1221` — a rigid body sits under a `Parent`; it gets no physics body. Raised by
    /// `check-scene` and once per entity at runtime; `entity.reparent`, `component.set Parent` and
    /// `entity.spawn` refuse to create it.
    pub const HIERARCHY_RIGID_BODY: DiagnosticCode = DiagnosticCode::new("L1221");

    /// `L1222` — a static solver collider sits under a `Parent`; it gets no physics body. Raised by
    /// `check-scene` and once per entity at runtime; `entity.reparent`, `component.set Parent` and
    /// `entity.spawn` refuse to create it.
    pub const HIERARCHY_STATIC_COLLIDER: DiagnosticCode = DiagnosticCode::new("L1222");

    /// `L1223` — a kinematic body under a sheared parent chain; the solver gets the lossy pose.
    /// Raised once per entity by `ludion-engine`.
    pub const HIERARCHY_KINEMATIC_SHEAR: DiagnosticCode = DiagnosticCode::new("L1223");

    /// `L1224` — a reparent would make an entity its own ancestor (the new parent is the entity or
    /// one of its descendants). Refused by `entity.reparent` and `component.set Parent`, and by
    /// `entity.spawn` when the named parent's ancestry already loops; nothing changes.
    pub const HIERARCHY_REPARENT_CYCLE: DiagnosticCode = DiagnosticCode::new("L1224");

    /// `L1225` — a reparent would put a sim entity under a presentation entity. Refused by
    /// `entity.reparent`, `component.set Parent` and `entity.spawn`; nothing changes.
    pub const HIERARCHY_REPARENT_PRESENTATION: DiagnosticCode = DiagnosticCode::new("L1225");

    /// `L1260` — a reparent would give an entity more than `MAX_HIERARCHY_DEPTH` (64) ancestors.
    /// Refused by `entity.reparent`, `component.set Parent` and `entity.spawn`; nothing changes.
    pub const HIERARCHY_REPARENT_TOO_DEEP: DiagnosticCode = DiagnosticCode::new("L1260");

    /// `L1226` — a `kind: prefab` document is malformed at the top level: empty, bad `format` or
    /// `kind`, an unknown key, a scene-only key (`seed`, `physics`, `generated_from`), or a `clips`
    /// map that is not a mapping of short name to clip file (or has an empty name or file).
    pub const PREFAB_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1226");

    /// `L1227` — a prefab part has no `name`, an empty one, or one containing `/`.
    pub const PREFAB_PART_NAME: DiagnosticCode = DiagnosticCode::new("L1227");

    /// `L1228` — two parts of a prefab share a name.
    pub const PREFAB_PART_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L1228");

    /// `L1229` — a prefab part carries a scene-wide singleton component (`Camera`,
    /// `DirectionalLight`, `Environment`, `PostProcess`, `GiVolume`).
    pub const PREFAB_SINGLETON: DiagnosticCode = DiagnosticCode::new("L1229");

    /// `L1230` — the prefab text could not be resolved (an unreadable file, or no prefab library).
    pub const PREFAB_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L1230");

    /// `L1231` — a prefab-instance entry is malformed: its shape, its name or its keys.
    pub const PREFAB_INSTANCE_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1231");

    /// `L1232` — a prefab instances itself, directly or through nesting.
    pub const PREFAB_INSTANCE_CYCLE: DiagnosticCode = DiagnosticCode::new("L1232");

    /// `L1233` — an instance's `overrides` or `removed` names no part, or leaves the instance
    /// inconsistent.
    pub const PREFAB_INSTANCE_PART: DiagnosticCode = DiagnosticCode::new("L1233");

    /// `L1234` — a live prefab instance cannot be saved as an instance entry.
    pub const PREFAB_INSTANCE_SAVE: DiagnosticCode = DiagnosticCode::new("L1234");

    /// `L1235` — the entity is not part of a prefab instance.
    pub const PREFAB_NOT_INSTANCE: DiagnosticCode = DiagnosticCode::new("L1235");

    /// `L1236` — a prefab cannot be created from the given entities.
    pub const PREFAB_CREATE: DiagnosticCode = DiagnosticCode::new("L1236");

    /// `L1237` — an instance's changes cannot be applied to its prefab.
    pub const PREFAB_APPLY: DiagnosticCode = DiagnosticCode::new("L1237");

    /// `L1238` — a prefab instance cannot be reverted as asked.
    pub const PREFAB_REVERT: DiagnosticCode = DiagnosticCode::new("L1238");

    /// `L1261` — a solver collider (a 3D shape or a rigid body) exists in a world with no `physics:`
    /// block, so no solver simulates it. Raised by `check-scene` and, once, at runtime.
    pub const COLLIDER_NO_PHYSICS_SCENE: DiagnosticCode = DiagnosticCode::new("L1261");

    /// `L1262` — a 2D collider exists in a world whose schedule never builds a `SpatialIndex`, so no
    /// query or `move_and_slide` sees it. Raised once, at runtime.
    pub const COLLIDER_NO_SPATIAL_INDEX: DiagnosticCode = DiagnosticCode::new("L1262");

    /// `L1263` — a light or drawn colour has a negative or non-finite RGB component, or an alpha
    /// outside `[0, 1]` (a particle's alpha: below 0 or non-finite). RGB is HDR (it may exceed 1)
    /// but a negative value subtracts radiance in the scene target, and alpha is a coverage weight
    /// that premultiplied `over` turns into a negative destination weight past 1. Raised by the
    /// semantic validators of `PointLight`, `SpotLight`, `DirectionalLight`, `Sprite`, `Mesh`,
    /// `Text`, `GroundMarker`, a `Tilemap` or `MeshGrid` legend entry and `ParticleEmitter`, and by a
    /// pattern document's bullet kinds; the JSON Pointer names the element.
    pub const COLOR_COMPONENT_INVALID: DiagnosticCode = DiagnosticCode::new("L1263");

    /// `L1264` — a `Camera`'s `near` is not less than its `far`, so the projection has no depth
    /// (and a perspective one is non-finite). `near <= 0` is the schema's `L1014`.
    pub const CAMERA_INVALID_DEPTH_RANGE: DiagnosticCode = DiagnosticCode::new("L1264");

    /// `L1265` — a pattern document validates but misleads (a warning): a full-turn `arc`, which
    /// overlaps itself (at exactly 360 its ends coincide), or a `loop` on a pattern that another
    /// pattern `play`s, which runs once and ignores it.
    pub const PATTERN_AUTHORING_LINT: DiagnosticCode = DiagnosticCode::new("L1265");
    /// `L1266` — a generation run would submit identical graphs (a warning): `count` above 1 but
    /// no seed input takes the run's seed, so every candidate is the same job.
    pub const GENERATION_IDENTICAL_CANDIDATES: DiagnosticCode = DiagnosticCode::new("L1266");
    /// `L1267` — `scene.save` refused: the scene bears `generated_from:`, so it is write-only
    /// output of `ludion generate-map`; the destination file bearing it is refused the same way.
    pub const SCENE_SAVE_GENERATED: DiagnosticCode = DiagnosticCode::new("L1267");
    /// `L1268` — a voxel view request (`ludion check-scene --slice`/`--column` on a voxel model)
    /// names a `y`, `x` or `z` outside the model's size; raised by `ludion-scene`'s `render_slice`
    /// and `render_column`.
    pub const VOXEL_VIEW_OUT_OF_RANGE: DiagnosticCode = DiagnosticCode::new("L1268");
    /// `L1269` — a `PathFollower`'s `speed` or `phase` is not a finite `f32` (non-finite, or a
    /// number outside `f32` range). A validation error from text; a follower built in Rust is held
    /// where it is and reported once per entity by `ludion-engine`'s `path_follow`.
    pub const PATH_FOLLOWER_INVALID: DiagnosticCode = DiagnosticCode::new("L1269");
    /// `L1270` — `ludion sync`'s cook stage could not encode a texture map into BC7 or ASTC; the
    /// import tree is unaffected and the map stays usable from its source PNG.
    pub const COOK_ENCODE_FAILED: DiagnosticCode = DiagnosticCode::new("L1270");
    /// `L1324` — a `[cook] roots` directory is absent, not a directory, behind a symbolic link or
    /// junction, or holds a link; the sync refuses rather than read it as empty and sweep the store.
    pub const COOK_ROOT_INVALID: DiagnosticCode = DiagnosticCode::new("L1324");
    /// `L1325` — a mesh under a `[cook] roots` directory references a map outside that root; the
    /// map is not cooked and loads from its PNG. A warning, one per sync.
    pub const COOK_MAP_LEAVES_ROOT: DiagnosticCode = DiagnosticCode::new("L1325");
    /// `L1271` — a scene load under `textures: cooked` read some mesh maps from their PNG sources
    /// because the cooked file was missing, corrupt or unusable on this GPU; one warning per load.
    pub const TEXTURE_NOT_COOKED: DiagnosticCode = DiagnosticCode::new("L1271");
    /// `L1272` — a `SpotLight` cannot form a cone: its `direction` is zero-length or non-finite, an
    /// angle is non-finite or outside `[0, 90)`, or `inner_angle` exceeds `outer_angle`. Raised by
    /// `SpotLight`'s semantic validator.
    pub const SPOT_LIGHT_INVALID_CONE: DiagnosticCode = DiagnosticCode::new("L1272");
    /// `L1273` — a `PostProcess` screen-space reflection field is out of range: an `ssr_fade` end
    /// outside `[0, 1]`, a band whose start is not below its end, or a zero `ssr_max_distance` or
    /// `ssr_thickness`.
    pub const POST_PROCESS_INVALID_SSR: DiagnosticCode = DiagnosticCode::new("L1273");
    /// `L1274` — a `ReflectionProbe` has a non-positive or non-finite `extent` component or
    /// `radius`, or a negative or non-finite `blend`. Raised by `ReflectionProbe`'s semantic
    /// validator.
    pub const REFLECTION_PROBE_INVALID: DiagnosticCode = DiagnosticCode::new("L1274");
    /// `L1275` — two reflection probes have equal priority and volume and overlap, their `blend`
    /// bands counted, so their order falls to entity id. Raised by the reflection-probe scene check.
    pub const REFLECTION_PROBE_EQUAL_RANK: DiagnosticCode = DiagnosticCode::new("L1275");
    /// `L1276` — a reflection probe lies entirely inside a higher-ranked probe's volume, where
    /// its weight is 1, so it never contributes. Raised by the reflection-probe scene check.
    pub const REFLECTION_PROBE_SHADOWED: DiagnosticCode = DiagnosticCode::new("L1276");
    /// `L1277` — two reflection probes have the same shape, size and pose, almost always a
    /// copy-paste. Raised by the reflection-probe scene check.
    pub const REFLECTION_PROBE_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L1277");
    /// `L1278` — a scene has more reflection probes than the 32 the renderer binds. Raised by the
    /// reflection-probe scene check.
    pub const REFLECTION_PROBE_OVER_CAP: DiagnosticCode = DiagnosticCode::new("L1278");
    /// `L1279` — a host serving the probe store had to capture some reflection probes because the
    /// store held no entry for them, or `Engine::probe_coverage` finds missing; one warning per resolve.
    pub const REFLECTION_PROBE_NOT_STORED: DiagnosticCode = DiagnosticCode::new("L1279");
    /// `L1280` — `Engine::bake_probes` was asked to force a probe by a name no probe in the scene
    /// carries.
    pub const REFLECTION_PROBE_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1280");
    /// `L1281` — `ludion export` baked a scene's reflection probes but an entry is missing from the
    /// bundle's probe store.
    pub const REFLECTION_PROBE_NOT_BUNDLED: DiagnosticCode = DiagnosticCode::new("L1281");
    /// `L1282` — a `DirectionalLight`'s `angular_radius` is non-finite or outside `[0, 45]` degrees.
    pub const DIRECTIONAL_LIGHT_INVALID_ANGULAR_RADIUS: DiagnosticCode =
        DiagnosticCode::new("L1282");

    /// `L1283` — a `kind: texture` recipe's structure is wrong: a missing or wrong-typed key, an
    /// unknown op, field or output name (with a nearest match). Raised by `ludion-texgen`.
    pub const TEXTURE_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1283");
    /// `L1284` — a structurally sound `kind: texture` recipe holds a value that does not hang
    /// together: out of range, a bad hex colour, unsorted stops or points, a gray/color mismatch,
    /// an undefined or wrong-typed `$param`, a bad size. Raised by `ludion-texgen`.
    pub const TEXTURE_DATA: DiagnosticCode = DiagnosticCode::new("L1284");
    /// `L1285` — a `kind: texture` recipe's step graph cannot be resolved: an unknown step
    /// reference, a cycle, an unresolvable or cyclic sub-recipe, or an unavailable `std:` recipe.
    /// Raised by `ludion-texgen`.
    pub const TEXTURE_GRAPH: DiagnosticCode = DiagnosticCode::new("L1285");
    /// `L1286` — a `kind: texture` recipe's committed map set differs from what the recipe builds:
    /// a map's bytes changed, one is missing or stray, or the set was never built. Raised by
    /// `ludion texture check`.
    pub const TEXTURE_DRIFT: DiagnosticCode = DiagnosticCode::new("L1286");
    /// `L1287` — a built texture map set has a validator outside its bounds at level `warn`.
    /// Raised by `ludion texture build` and `check`.
    pub const TEXTURE_CHECK_WARNING: DiagnosticCode = DiagnosticCode::new("L1287");
    /// `L1288` — a built texture map set has a validator outside its bounds at level `error`.
    /// Raised by `ludion texture build` and `check`.
    pub const TEXTURE_CHECK_FAILED: DiagnosticCode = DiagnosticCode::new("L1288");
    /// `L1289` — a texture rules file cannot be used: unreadable, invalid TOML, a wrong-typed
    /// value or an unknown validator name. Raised by `ludion texture`.
    pub const TEXTURE_RULES_INVALID: DiagnosticCode = DiagnosticCode::new("L1289");

    /// `L1290` — a solver-owned collider whose world scale (own times its ancestors') is not
    /// `[1, 1, 1]`; collision ignores scale. Raised once per entity by `ludion-engine`.
    pub const COLLIDER_SCALE_IGNORED: DiagnosticCode = DiagnosticCode::new("L1290");

    /// `L1291` — a `Collider` with `sensor: true` sits on a `body: rigid`. A sensor detects
    /// overlaps and never collides, so it cannot be simulated. Raised by `ludion-spatial`'s
    /// collider validation and by `ludion-physics` at runtime.
    pub const COLLIDER_SENSOR_ON_RIGID: DiagnosticCode = DiagnosticCode::new("L1291");

    /// `L1292` — a `Collider` with `sensor: true` has a `mesh` or `heightfield` shape. Neither has
    /// an interior, so the sensor would fire only on a surface crossing. Raised by
    /// `ludion-spatial`'s collider validation and by `ludion-physics` at runtime.
    pub const COLLIDER_SENSOR_NO_INTERIOR: DiagnosticCode = DiagnosticCode::new("L1292");

    /// `L1293` — a `Collider` with `sensor: true` has a 2D `circle` or `aabb` shape the physics
    /// solver does not claim. Raised by `ludion-engine`'s `check-scene`.
    pub const COLLIDER_SENSOR_UNCLAIMED: DiagnosticCode = DiagnosticCode::new("L1293");

    /// `L1294` — a `CharacterMotor` field is out of range: a length is not finite and positive, the
    /// slope limit is outside (0, 90), the step is not below the height, the capsule is shorter than
    /// two radii, or `max_slides` is outside 1..=16. Raised by the component's validator and, for a
    /// motor built in Rust, by `ludion-physics` at runtime.
    pub const CHARACTER_MOTOR_INVALID: DiagnosticCode = DiagnosticCode::new("L1294");

    /// `L1295` — an entity carries both a `CharacterMotor` and a `Collider`. The motor owns its
    /// capsule, so the `Collider` is ignored. Raised by `check-scene` and by `ludion-physics`.
    pub const CHARACTER_MOTOR_WITH_COLLIDER: DiagnosticCode = DiagnosticCode::new("L1295");

    /// `L1296` — a `CharacterMotor` exists in a world with no `physics:` block, so its capsule body
    /// is never built. Raised by `check-scene` and once at runtime by `ludion-engine`.
    pub const CHARACTER_MOTOR_NO_PHYSICS_SCENE: DiagnosticCode = DiagnosticCode::new("L1296");

    /// `L1297` — a `CharacterMotor` entity has a `Parent`. A motor writes world-space feet, so it
    /// must be a root entity. Raised by `check-scene` and once at runtime by `ludion-engine`.
    pub const CHARACTER_MOTOR_PARENTED: DiagnosticCode = DiagnosticCode::new("L1297");

    /// `L1540` — an authored `CharacterMotorState.posture` names a posture the entity's
    /// `CharacterMotor` does not declare. Raised by `check-scene`.
    pub const CHARACTER_MOTOR_POSTURE_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1540");

    /// `L4170` — a motor's state names a posture its `CharacterMotor` does not declare (a component
    /// edited live, or the posture removed). The motor runs as its base shape. Raised once per
    /// entity at runtime.
    pub const CHARACTER_MOTOR_POSTURE_FALLBACK: DiagnosticCode = DiagnosticCode::new("L4170");

    /// `L1298` — a `SurfaceVelocity` has a non-finite component. Raised by the component's validator.
    pub const SURFACE_VELOCITY_INVALID: DiagnosticCode = DiagnosticCode::new("L1298");

    /// `L1299` — a `SurfaceVelocity` entity has no `Collider`, or a rigid one, so nothing is ever
    /// carried by it. Raised by `check-scene`.
    pub const SURFACE_VELOCITY_NO_SURFACE: DiagnosticCode = DiagnosticCode::new("L1299");

    /// `L1300` — a `LaneConstraint` field is out of range: a time, `pull_speed` or `search_window`
    /// is negative or not finite, `offset` or `s` is not finite, or `blend` is outside [0, 1].
    /// Raised by the component's validator and, for a lane built in Rust, once per entity at runtime.
    pub const LANE_CONSTRAINT_INVALID: DiagnosticCode = DiagnosticCode::new("L1300");

    /// `L1301` — the `kind: path` document named by `LaneConstraint.path` failed to load. Raised
    /// once per entity at runtime; the lane stays inert.
    pub const LANE_PATH_UNLOADED: DiagnosticCode = DiagnosticCode::new("L1301");

    /// `L1302` — a `LaneConstraint` entity has no `CharacterMotor`, so nothing reads the lane.
    /// Raised by `check-scene` and once per entity at runtime.
    pub const LANE_WITHOUT_MOTOR: DiagnosticCode = DiagnosticCode::new("L1302");

    /// `L1303` — a lane path has a near-vertical span, where the horizontal frame is undefined from
    /// the tangent. A warning, raised when a lane first loads the path.
    pub const LANE_PATH_VERTICAL: DiagnosticCode = DiagnosticCode::new("L1303");

    /// `L1304` — a `CameraRig` field is out of range: a half-life, slack, distance, decay or limit is
    /// negative or not finite, a window, speed, frequency or exponent is not positive, an offset is
    /// not finite, `pitch_limit_deg` is outside (0, 90), a `chase` or `follow` `distance` is not
    /// positive, their pitches are outside (-90, 90) or `pitch_max_deg` is below `pitch_min_deg`,
    /// `enter_pitch_deg` or a `chase` or `follow` `yaw_deg` or `pitch_deg` is not finite, `look_deg`,
    /// `look_deg_per_count`, `kick_deg`, `fov_kick_deg`, `roll_deg` or `fov_offset_deg` is not
    /// finite, a `kick` half-life is negative, `fov_y_deg` is outside (0, 180), `trauma` is
    /// outside [0, 1], `collision.radius` or `framing.distance` is not positive, another
    /// `collision`, `recenter` or `framing` field is negative or not finite. Raised by the component's validator and, for a
    /// rig built in Rust, once per entity at runtime.
    pub const CAMERA_RIG_INVALID: DiagnosticCode = DiagnosticCode::new("L1304");

    /// `L1305` — a `CameraRig` `target` names an entity that no longer exists. A warning raised once
    /// per rig at runtime; the camera holds its last pose.
    pub const CAMERA_RIG_TARGET_MISSING: DiagnosticCode = DiagnosticCode::new("L1305");

    /// `L1306` — the `kind: path` document named by a rig's track `path` failed to load. Raised once
    /// per rig at runtime; the camera holds its last pose.
    pub const CAMERA_RIG_PATH_UNLOADED: DiagnosticCode = DiagnosticCode::new("L1306");

    /// `L1307` — a `CameraRig` entity has no `Camera`, so there is no camera to pose. Raised by
    /// `check-scene` and once per entity at runtime.
    pub const CAMERA_RIG_WITHOUT_CAMERA: DiagnosticCode = DiagnosticCode::new("L1307");

    /// `L1308` — more than one entity carries a `CameraRig`. Raised by `check-scene` and once at
    /// runtime; only the lowest entity id runs.
    pub const CAMERA_RIG_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L1308");

    /// `L1309` — a `CameraRig` entity has a `Parent`. A rig writes a world-space pose, so the camera
    /// must be a root entity. Raised by `check-scene` and once per entity at runtime.
    pub const CAMERA_RIG_PARENTED: DiagnosticCode = DiagnosticCode::new("L1309");

    /// `L1550` — the physics world rejected a `CameraRig` collision cast, for example an undeclared
    /// layer name in `include_layers` or `ignore_layers`. Raised once per rig; the collision stage is
    /// skipped.
    pub const CAMERA_RIG_COLLISION_QUERY: DiagnosticCode = DiagnosticCode::new("L1550");

    /// `L1551` — a `CameraRig` has `collision.enabled` but the world has no `PhysicsWorld`. A
    /// warning raised once per rig; the stage is skipped.
    pub const CAMERA_RIG_COLLISION_NO_PHYSICS: DiagnosticCode = DiagnosticCode::new("L1551");

    /// `L1552` — a `CameraRig`'s pivot and target both start inside geometry, so the boom is held at
    /// `min_distance`. A warning raised once per rig.
    pub const CAMERA_RIG_COLLISION_STUCK: DiagnosticCode = DiagnosticCode::new("L1552");

    /// `L1553` — a `CameraZone` exists but the world has no `PhysicsWorld`, so no zone can contain
    /// the target. A warning raised once; zones are skipped.
    pub const CAMERA_ZONE_NO_PHYSICS: DiagnosticCode = DiagnosticCode::new("L1553");

    /// `L1554` — a `CameraZone` entity has no `Collider` with `sensor: true`, so nothing can enter
    /// it. Raised by `check-scene`.
    pub const CAMERA_ZONE_WITHOUT_SENSOR: DiagnosticCode = DiagnosticCode::new("L1554");

    /// `L1555` — the target stands in two `CameraZone`s of the same top priority; the lower entity
    /// id wins. A warning raised once per pair.
    pub const CAMERA_ZONE_EQUAL_PRIORITY: DiagnosticCode = DiagnosticCode::new("L1555");

    /// `L1311` — an asset path inside a `kind: prefab` document, or a nested `prefab:` entry, is not
    /// a relative path (absolute, drive or scheme prefix, NUL, or a malformed `..` segment). Raised
    /// by prefab validation.
    pub const PREFAB_ASSET_PATH: DiagnosticCode = DiagnosticCode::new("L1311");

    /// `L1312` — a `PointLight`/`SpotLight` `shadow_near` is non-finite, ≤ 0, or not below `range`;
    /// raised by the light's semantic validator; pointer `/shadow_near`.
    pub const LOCAL_LIGHT_INVALID_SHADOW: DiagnosticCode = DiagnosticCode::new("L1312");

    /// `L1313` — a `DirectionalLight` sets `cascades` and a non-default `shadow_extent`. Under
    /// cascades each frustum is fitted to the camera, so the extent and the `Transform` anchor are
    /// unused by the camera's shadows; reflection-probe captures still use the anchored box. A
    /// **warning**, raised by the engine's scene check (`scene_sun_diagnostics`); pointer
    /// `/entities/{i}/components/DirectionalLight/shadow_extent`.
    pub const DIRECTIONAL_LIGHT_EXTENT_UNUSED: DiagnosticCode = DiagnosticCode::new("L1313");

    /// `L1314` — a clip a `kind: prefab` document binds under `clips:` has a track whose `target`
    /// names no part of the prefab; playback would silently draw that part at rest. Raised by
    /// `check-scene <prefab>`; pointer `/clips/<name>`.
    pub const PREFAB_CLIP_TARGET: DiagnosticCode = DiagnosticCode::new("L1314");

    /// `L1315` — a kind registered with `EngineBuilder::doc_singleton` has zero, or two or more,
    /// docs; the kind must hold exactly one. Raised by `ludion-engine` wherever the doc validators
    /// run; pointer `<kind>`.
    pub const DOC_SINGLETON_COUNT: DiagnosticCode = DiagnosticCode::new("L1315");

    /// `L1316` — a `UiRect` with `fill: true` also authors a non-default `offset` or `size`; a fill
    /// rect covers the whole render target, so both are ignored. Warning, raised by `UiRect`'s
    /// semantic validator; the JSON Pointer names the ignored field (`/offset`, `/size`).
    pub const UI_RECT_FILL_IGNORES_PLACEMENT: DiagnosticCode = DiagnosticCode::new("L1316");

    /// `L1317` — an `Animation` component's `hold` is `0`; it must be at least one tick. Raised by
    /// `Animation`'s semantic validator, in files and on the wire; pointer `/hold`.
    pub const ANIMATION_HOLD_ZERO: DiagnosticCode = DiagnosticCode::new("L1317");

    /// `L1318` — a `ParentJoint` component's `Parent` is not a skinned mesh or an assembly, or the
    /// entity has no `Parent`. Raised at load, `check-scene` and prefab spawn; pointer
    /// `/entities/{i}/components/ParentJoint`.
    pub const PARENT_JOINT_NOT_SKINNED: DiagnosticCode = DiagnosticCode::new("L1318");

    /// `L1319` — a `ParentJoint`'s `joint` names no joint of its parent's mesh document, or no part
    /// of its parent's assembly. Pointer
    /// `/entities/{i}/components/ParentJoint/joint`.
    pub const PARENT_JOINT_UNKNOWN_JOINT: DiagnosticCode = DiagnosticCode::new("L1319");

    /// `L1320` — a prefab spawned as presentation has a part carrying a sim-only component. Raised
    /// at expansion (load, `check-scene`; pointer `/entities/{i}/presentation`) and at a runtime
    /// presentation spawn (`spawn_prefab(.., true)`, `prefab.spawn` with `presentation: true`;
    /// pointer `/entities/{k}/components/{Component}`).
    pub const PRESENTATION_INSTANCE_SIM_COMPONENT: DiagnosticCode = DiagnosticCode::new("L1320");

    /// `L1321` — the general rule for a content or payload path leaving the content root: absolute,
    /// prefixed, reserved, or climbing above the project root (the nearest directory holding
    /// `ludion.toml`, else the content root) once its `..` segments fold. Raised by
    /// `ludion_assets::resolve`, so every asset read shares it; the per-kind path codes stay.
    pub const CONTENT_PATH_OUTSIDE_ROOT: DiagnosticCode = DiagnosticCode::new("L1321");

    /// `L1322` — a `PointLight`/`SpotLight` `shadow_distance` is non-finite or ≤ 0, or its
    /// `shadow_fade` is non-finite, negative, set without a distance, or longer than the distance.
    pub const LOCAL_LIGHT_INVALID_SHADOW_DISTANCE: DiagnosticCode = DiagnosticCode::new("L1322");

    /// `L1323` — a `PointLight`/`SpotLight` sets `shadow_distance` or `shadow_fade` without
    /// `cast_shadows: true`, so the field does nothing.
    pub const LOCAL_LIGHT_SHADOW_FADE_UNUSED: DiagnosticCode = DiagnosticCode::new("L1323");

    /// `L1330` — a `PostProcess` enables depth of field (`dof > 0`) with an authored `focus_distance`
    /// of 0, which focuses on the lens itself and blurs the whole scene.
    pub const CAMERA_DOF_FOCUS_ZERO: DiagnosticCode = DiagnosticCode::new("L1330");

    /// `L1331` — `Environment.sun_disc_size` and `DirectionalLight.angular_radius` are both
    /// authored and positive but the disc is not twice the radius, so the sky disc and the sun's
    /// specular highlight disagree.
    pub const SUN_DISC_SIZE_MISMATCH: DiagnosticCode = DiagnosticCode::new("L1331");

    /// `L1332` — a `CameraOffset` field is non-finite (a scene is refused, a runtime offset is
    /// ignored).
    pub const CAMERA_OFFSET_INVALID: DiagnosticCode = DiagnosticCode::new("L1332");

    /// `L1333` — a `CameraOffset` is on a sim entity, so it is hashed and never applied.
    pub const CAMERA_OFFSET_ON_SIM_ENTITY: DiagnosticCode = DiagnosticCode::new("L1333");

    /// `L1334` — more than one `CameraOffset` on presentation entities; the lowest id is applied
    /// and the rest are ignored.
    pub const CAMERA_OFFSET_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L1334");

    /// `L1335` — a `CameraOffset.fov_delta` would take a perspective camera's field of view outside
    /// 1 to 179 degrees; the result is clamped.
    pub const CAMERA_OFFSET_FOV_CLAMPED: DiagnosticCode = DiagnosticCode::new("L1335");

    /// `L1336` — a `Viewmodel` mesh exists but the scene has no single camera that is both
    /// perspective and a first-person `CameraRig`; the mesh draws as an ordinary world mesh.
    pub const VIEWMODEL_NO_CAMERA: DiagnosticCode = DiagnosticCode::new("L1336");

    /// `L1337` — a `ViewmodelLens` is on an entity without a `Camera`; it is ignored.
    pub const VIEWMODEL_LENS_NOT_ON_CAMERA: DiagnosticCode = DiagnosticCode::new("L1337");

    /// `L1338` — a `ViewmodelLens` field is out of range.
    pub const VIEWMODEL_LENS_INVALID: DiagnosticCode = DiagnosticCode::new("L1338");

    /// `L1339` — a `SpatialVolume` has a non-positive or non-finite size, or no layer bit.
    pub const SPATIAL_VOLUME_INVALID: DiagnosticCode = DiagnosticCode::new("L1339");
    /// `L1340` — a `NavAgent` has a non-finite or negative field, or too many separation neighbours.
    pub const NAV_AGENT_INVALID: DiagnosticCode = DiagnosticCode::new("L1340");

    /// `L1520` — a `NavRoute` has a non-finite goal, waypoint or cost, or an `index` past its waypoints.
    pub const NAV_ROUTE_INVALID: DiagnosticCode = DiagnosticCode::new("L1520");
    /// `L1521` — a `NavGoal` has a non-finite `at`.
    pub const NAV_GOAL_INVALID: DiagnosticCode = DiagnosticCode::new("L1521");
    /// `L1522` — a `kind: patrol` document is structurally invalid: an empty or non-mapping root, a
    /// missing or unknown key, or a value of the wrong type.
    pub const PATROL_DOCUMENT_STRUCTURE: DiagnosticCode = DiagnosticCode::new("L1522");
    /// `L1523` — a well-shaped `kind: patrol` document is inconsistent: no points, a non-finite or
    /// out-of-range number, a negative `wait` or `hold`, or an empty `tag`.
    pub const PATROL_DATA_CONSISTENCY: DiagnosticCode = DiagnosticCode::new("L1523");
    /// `L1524` — a `PatrolFollower` field is invalid.
    pub const PATROL_FOLLOWER_INVALID: DiagnosticCode = DiagnosticCode::new("L1524");
    /// `L1525` — a `PatrolState` has a non-finite or negative timer, or a non-finite `look_deg`.
    pub const PATROL_STATE_INVALID: DiagnosticCode = DiagnosticCode::new("L1525");
    /// `L1526` — a `NavLink` has a non-finite `from` or `to`, a negative or non-finite `cost`, or a
    /// non-positive or non-finite `snap_radius`.
    pub const NAV_LINK_INVALID: DiagnosticCode = DiagnosticCode::new("L1526");
    /// `L1527` — a `NavBlocker` has a non-finite or non-positive half-extent, a negative or non-finite
    /// `cost`, or an empty `classes` entry.
    pub const NAV_BLOCKER_INVALID: DiagnosticCode = DiagnosticCode::new("L1527");
    /// `L1528` — a nav avoidance value is invalid: a `NavAgent.avoidance` with a non-finite or non-positive
    /// `radius` or `horizon_s`, a `max_neighbors` outside 1..=16, or `avoidance` on a flying agent.
    pub const NAV_AVOIDANCE_INVALID: DiagnosticCode = DiagnosticCode::new("L1528");
    /// `L1570` — a `Perceiver` field is invalid: no or too many sight zones, a bad range or angle, a
    /// duplicate zone name, a bad sweep or a bad visibility clamp.
    pub const PERCEIVER_INVALID: DiagnosticCode = DiagnosticCode::new("L1570");
    /// `L1571` — a `SightTarget` field is invalid: an empty class, no or too many sample points, a
    /// non-finite offset or visibility, or a `fraction` outside (0, 1].
    pub const SIGHT_TARGET_INVALID: DiagnosticCode = DiagnosticCode::new("L1571");
    /// `L1572` — perception named a class or a layer the world does not declare, so the zone or the
    /// sample point detects or sees nothing. A warning, once per name.
    pub const PERCEPTION_NAME_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1572");
    /// `L1573` — a `Perceiver` `memory` field is invalid: a non-finite or negative forget time, or a
    /// `max_records` of 0 or above 64.
    pub const PERCEIVER_MEMORY_INVALID: DiagnosticCode = DiagnosticCode::new("L1573");
    /// `L1574` — `report_stimulus` dropped a stimulus: the perceiver is dead or has no `memory`, or
    /// `at` or `strength` is not finite. A warning, once per cause.
    pub const STIMULUS_REPORT_INVALID: DiagnosticCode = DiagnosticCode::new("L1574");
    /// `L1575` — a `NoiseZone` field is invalid: a non-finite or non-positive extent or radius, a
    /// negative or non-finite multiplier, or an empty or repeated `kinds` entry.
    pub const NOISE_ZONE_INVALID: DiagnosticCode = DiagnosticCode::new("L1575");
    /// `L1576` — a `Perceiver` `hearing` field is invalid: a non-finite ear, a negative or
    /// non-finite sensitivity, or an empty, empty-named or repeated `hears` list.
    pub const HEARING_INVALID: DiagnosticCode = DiagnosticCode::new("L1576");
    /// `L1577` — `emit_noise` dropped a noise: a non-finite or negative `at`, `radius` or
    /// `loudness`. A warning, once per cause.
    pub const NOISE_INVALID: DiagnosticCode = DiagnosticCode::new("L1577");
    /// `L4220` — the `Engine::builder().perception` config is invalid: no more than 32 classes, each
    /// non-empty and unique.
    pub const PERCEPTION_CONFIG_INVALID: DiagnosticCode = DiagnosticCode::new("L4220");
    /// `L4221` — a due `Perceiver` has gone unserved for more than four of its own periods because
    /// the `max_sight_rays_per_tick` budget is too small for the load. A warning, once per run.
    pub const PERCEPTION_DEFERRED: DiagnosticCode = DiagnosticCode::new("L4221");
    /// `L1630` — a binding has zero or several sources.
    pub const INPUT_BINDING_SOURCES: DiagnosticCode = DiagnosticCode::new("L1630");
    /// `L1631` — a binding source does not fit its action kind.
    pub const INPUT_BINDING_SOURCE_KIND: DiagnosticCode = DiagnosticCode::new("L1631");
    /// `L1632` — a binding modifier does not fit its source.
    pub const INPUT_BINDING_MODIFIER: DiagnosticCode = DiagnosticCode::new("L1632");
    /// `L1633` — a binding value is out of range.
    pub const INPUT_BINDING_RANGE: DiagnosticCode = DiagnosticCode::new("L1633");
    /// `L1634` — two bindings share a source and gesture.
    pub const INPUT_BINDING_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L1634");
    /// `L1635` — an action is empty or badly named.
    pub const INPUT_ACTION_INVALID: DiagnosticCode = DiagnosticCode::new("L1635");
    /// `L4270` — a game read an action name the loaded input map does not define.
    pub const INPUT_ACTION_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L4270");
    /// `L4271` — a restored input-action state is inconsistent with the loaded map.
    pub const INPUT_STATE_INVALID: DiagnosticCode = DiagnosticCode::new("L4271");
    /// `L4272` — a rebind, capture or unbind named a `locked` input action.
    pub const INPUT_ACTION_LOCKED: DiagnosticCode = DiagnosticCode::new("L4272");
    /// `L4273` — an input-binding override names an action the map does not define; it is dropped.
    pub const INPUT_OVERRIDE_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L4273");
    /// `L4274` — an input-binding override does not compile; that action keeps its default bindings.
    pub const INPUT_OVERRIDE_INVALID: DiagnosticCode = DiagnosticCode::new("L4274");
    /// `L4275` — a binding capture or conflict resolution is impossible.
    pub const INPUT_CAPTURE_IMPOSSIBLE: DiagnosticCode = DiagnosticCode::new("L4275");
    /// `L4276` — there is no binding capture to resolve, or no conflict is pending.
    pub const INPUT_CAPTURE_IDLE: DiagnosticCode = DiagnosticCode::new("L4276");
    /// `L1636` — the input bindings file (`save/input-bindings.yaml`) has a malformed
    /// `format`/`kind` envelope, so the engine backs it up and starts from the default bindings.
    pub const INPUT_BINDINGS_ENVELOPE: DiagnosticCode = DiagnosticCode::new("L1636");
    /// `L4277` — the input bindings file could not be loaded, so the engine backed it up and
    /// started from the default bindings.
    pub const INPUT_BINDINGS_RESET: DiagnosticCode = DiagnosticCode::new("L4277");
    /// `L4278` — the input bindings file could not be written to disk.
    pub const INPUT_BINDINGS_SAVE_FAILED: DiagnosticCode = DiagnosticCode::new("L4278");
    /// `L4279` — a rumble request had a non-finite value; it played as 0.
    pub const RUMBLE_EFFECT_INVALID: DiagnosticCode = DiagnosticCode::new("L4279");

    /// `L1610` — a `UiTween` track has the wrong `from`/`to` arity, a non-finite value or a negative
    /// `duration`.
    pub const UI_TWEEN_INVALID: DiagnosticCode = DiagnosticCode::new("L1610");
    /// `L1611` — a `UiWorldAnchor` has a non-finite number, a negative `margin` or a bad `fade`.
    pub const UI_WORLD_ANCHOR_INVALID: DiagnosticCode = DiagnosticCode::new("L1611");
    /// `L2300` — a `UiTween` or `UiWorldAnchor` names something its element cannot take; skipped.
    pub const UI_MOTION_UNSUPPORTED: DiagnosticCode = DiagnosticCode::new("L2300");
    /// `L1612` — a `UiRect` `uv_rect` or `slice` holds a bad number.
    pub const UI_RECT_SAMPLING_INVALID: DiagnosticCode = DiagnosticCode::new("L1612");
    /// `L1613` — a `UiRect` `uv_rect` or `slice` is ignored.
    pub const UI_RECT_SAMPLING_IGNORED: DiagnosticCode = DiagnosticCode::new("L1613");
    /// `L2301` — a `UiRect` nine-slice border does not fit its region or rect; drawn clamped.
    pub const UI_SLICE_CLAMPED: DiagnosticCode = DiagnosticCode::new("L2301");
    /// `L1614` — a `UiGroup` `scale` or `opacity` is out of range.
    pub const UI_GROUP_INVALID: DiagnosticCode = DiagnosticCode::new("L1614");
    /// `L1615` — `group` references form a cycle.
    pub const UI_GROUP_CYCLE: DiagnosticCode = DiagnosticCode::new("L1615");
    /// `L1616` — a `group` chain is deeper than the limit.
    pub const UI_GROUP_TOO_DEEP: DiagnosticCode = DiagnosticCode::new("L1616");
    /// `L1617` — a `group`, `fill_group` or `fill` combination that has no effect.
    pub const UI_GROUP_IGNORED: DiagnosticCode = DiagnosticCode::new("L1617");
    /// `L1618` — a `UiText` or `UiList` `outline` has a non-finite or negative width, or a non-finite colour.
    pub const UI_TEXT_OUTLINE_INVALID: DiagnosticCode = DiagnosticCode::new("L1618");
    /// `L1619` — a `UiRadial` has a non-finite number, a bad radius, gap, deadzone, hysteresis or size, or more than 16 items.
    pub const UI_RADIAL_INVALID: DiagnosticCode = DiagnosticCode::new("L1619");
    /// `L1620` — a `UiRect`, `UiText` or `UiGroup` `rotation` is not finite.
    pub const UI_ROTATION_INVALID: DiagnosticCode = DiagnosticCode::new("L1620");
    /// `L1621` — a `UiShape` has a non-finite number, a bad radius, thickness, sweep, size or stroke width, a fill or missing stroke on a stroke-only kind, or a bad polygon (non-finite or over 256 points, or a stroke).
    pub const UI_SHAPE_INVALID: DiagnosticCode = DiagnosticCode::new("L1621");
    /// `L2302` — a UI element's `group` could not be resolved; drawn ungrouped.
    pub const UI_GROUP_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L2302");
    /// `L2303` — more than two clipping groups in a UI element's chain; the outer ones are dropped.
    pub const UI_CLIP_NESTING: DiagnosticCode = DiagnosticCode::new("L2303");
    /// `L2304` — a UI text outline wider than the face can draw; clamped.
    pub const UI_TEXT_OUTLINE_CLAMPED: DiagnosticCode = DiagnosticCode::new("L2304");
    /// `L2305` — a `UiShape` polygon is degenerate or self-intersecting; it draws nothing.
    pub const UI_POLYGON_DEGENERATE: DiagnosticCode = DiagnosticCode::new("L2305");

    /// `L1341` — a `Static` entity holds something the static path cannot; it takes the dynamic path.
    pub const STATIC_INELIGIBLE: DiagnosticCode = DiagnosticCode::new("L1341");

    /// `L1342` — a `Static` entity has a `Parent` ancestor without `Static`; it takes the dynamic
    /// path.
    pub const STATIC_PARENT_NOT_STATIC: DiagnosticCode = DiagnosticCode::new("L1342");

    /// `L1343` — a `Static` entity was written on 30 consecutive frames; reported once per entity.
    pub const STATIC_REWRITTEN: DiagnosticCode = DiagnosticCode::new("L1343");

    /// `L1390` — a prefab document's `entities` list is empty, so an instance would spawn nothing.
    pub const PREFAB_EMPTY: DiagnosticCode = DiagnosticCode::new("L1390");

    /// `L1391` — `gen fetch` or `gen status` found a `submitted` run whose jobs the server does not
    /// know at all (every one lost, usually a wrong `--url` or a restarted server), or a run whose
    /// submit was interrupted before any job was accepted. Raised by `ludion-gen`.
    pub const GENERATION_JOBS_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L1391");

    /// `L1392` — a component registration marks an entity reference inside a list, map or tuple not
    /// wrapped in `EntityRefCollection`, which the range check, the save remap and the carry remap
    /// cannot address; wrap it, or hold each reference in its own field. Raised by `ludion-ecs`.
    pub const ENTITY_REF_IN_COLLECTION: DiagnosticCode = DiagnosticCode::new("L1392");

    /// `L1394` — the world holds a `PatternPlayer` naming a pattern, or a `Volley`, but the schedule
    /// never ran `play_patterns` or `advance_volleys`, so no volley fires or none is ever reaped.
    /// A lint raised once, at runtime, after a tick.
    pub const PATTERN_SYSTEM_NOT_RUN: DiagnosticCode = DiagnosticCode::new("L1394");

    /// `L1395` — a mesh or voxel document's `lods:` chain is malformed: more than three levels, a
    /// `screen_size` that is not finite, positive and strictly decreasing, a mesh level path that is
    /// not a `*.mesh.yaml` file inside the project, a mesh level document that carries its own
    /// `lods`, or a voxel level with a key other than `screen_size`. Raised by `ludion-scene` and
    /// the engine's mesh loader.
    pub const MESH_LOD_CHAIN: DiagnosticCode = DiagnosticCode::new("L1395");

    /// `L1396` — a mesh LOD level does not match its base mesh's draw key (alpha mode, cutoff,
    /// `double_sided`, `rim`, the set of texture slots) or skeleton. Raised by the engine's mesh
    /// loader.
    pub const MESH_LOD_LEVEL_MISMATCH: DiagnosticCode = DiagnosticCode::new("L1396");

    /// `L1397` — a `Camera`'s `lod_bias` is not a finite number in `[-4, 4]`, or its
    /// `shadow_bias_levels` is above 3. Raised by `Camera`'s semantic validator.
    pub const CAMERA_LOD_INVALID: DiagnosticCode = DiagnosticCode::new("L1397");

    /// `L1398` — a voxel model's `lods:` chain has a level whose downsampled grid holds no solid
    /// cell, so the level draws nothing. A warning raised by `ludion check-scene`.
    pub const VOXEL_LOD_EMPTY_LEVEL: DiagnosticCode = DiagnosticCode::new("L1398");

    // --- L2xxx: render (raised by `ludion-render-graph`, `ludion-renderer`) ---

    /// `L2001` — a render graph has a dependency cycle: some pass (transitively) reads a resource it
    /// also produces, so no execution order exists. Raised by `ludion-render-graph`.
    pub const RENDER_GRAPH_CYCLE: DiagnosticCode = DiagnosticCode::new("L2001");

    /// `L2002` — a render pass reads a virtual resource that no pass ever writes. Raised by
    /// `ludion-render-graph`.
    pub const RENDER_GRAPH_UNWRITTEN_READ: DiagnosticCode = DiagnosticCode::new("L2002");

    /// `L2003` — no compatible GPU adapter could be found for headless rendering (no hardware
    /// device and no software rasterizer). Raised by `ludion-render-graph`.
    pub const RENDER_NO_ADAPTER: DiagnosticCode = DiagnosticCode::new("L2003");

    /// `L2004` — a GPU adapter was found but a device/queue could not be created from it. Raised by
    /// `ludion-render-graph`.
    pub const RENDER_DEVICE_FAILED: DiagnosticCode = DiagnosticCode::new("L2004");

    /// `L2005` — a rendered frame (or its GPU timings) could not be read back from the GPU, because
    /// the staging buffer failed to map. Raised by `ludion-render-graph`.
    pub const RENDER_READBACK_FAILED: DiagnosticCode = DiagnosticCode::new("L2005");

    /// `L2006` — a rendered image could not be encoded as PNG or written to disk. Raised by
    /// `ludion-render-graph`.
    pub const RENDER_PNG_IO: DiagnosticCode = DiagnosticCode::new("L2006");

    /// `L2007` — windowed presentation could not start or could not continue: the OS event loop
    /// could not be built or run (no display/window server, or not on the main thread), a GPU
    /// surface could not be created for the window, or the surface a present target draws into
    /// failed permanently. Raised by `ludion-window` (the out-of-process viewer) or by
    /// `ludion-render-graph`'s in-process present target. (Distinct from `L2003`/`L2004`, which are
    /// about the GPU adapter/device, not the window system.)
    pub const WINDOW_PRESENTATION_UNAVAILABLE: DiagnosticCode = DiagnosticCode::new("L2007");

    /// `L2008` — an `asset.reload` named a renderer shader (`.wgsl`) but the host is not running with
    /// shader hot-reload enabled (the dev flag). Raised by `ludion-engine`'s asset handler. Shaders
    /// are compiled in for every shipping/test/golden path; only an explicitly dev-flagged host
    /// (`serve --dev-shaders`, `ludion::run --dev-shaders`, `ludion dev`) reloads them from disk.
    pub const SHADER_RELOAD_DISABLED: DiagnosticCode = DiagnosticCode::new("L2008");

    /// `L2009` — a dev-flagged shader reload was rejected: the new WGSL failed naga validation, or the
    /// recreated pipeline is incompatible with the existing bind group layouts (a binding-shape
    /// change), or the shader compiles once and needs an engine rebuild. Raised by `ludion-renderer`.
    /// The last good pipeline stays live — a bad edit never kills the session; the message carries
    /// the naga/wgpu reason.
    pub const SHADER_RELOAD_INVALID: DiagnosticCode = DiagnosticCode::new("L2009");

    /// `L2010` — the GPU rejected work the renderer issued (a wgpu validation, out-of-memory or
    /// internal error reported through the device's uncaptured-error handler). Raised by
    /// `ludion-render-graph` in place of the process panic wgpu would otherwise raise.
    pub const RENDER_GPU_ERROR: DiagnosticCode = DiagnosticCode::new("L2010");

    /// `L2011` — the GPU device was lost (driver reset, sleep, a removed adapter). Raised by
    /// `ludion-render-graph`; sticky — every later render on that device returns it again.
    pub const RENDER_DEVICE_LOST: DiagnosticCode = DiagnosticCode::new("L2011");

    /// `L2012` — a render extent is outside what the renderer can allocate: an output or scaled
    /// render axis of 0 px, or a scaled render axis past the pinned 2D texture limit. Raised by
    /// `ludion-renderer`'s `FrameSize::validate`.
    pub const RENDER_EXTENT_INVALID: DiagnosticCode = DiagnosticCode::new("L2012");

    /// `L2101` — a scene contains more than one `Camera` without a `RenderView`, so the renderer cannot
    /// choose which one frames the shot. Raised by `ludion-engine`'s extract; the JSON Pointer names the
    /// extra camera's entity. (Zero cameras is not an error — a documented default framing is used.)
    pub const RENDER_MULTIPLE_CAMERAS: DiagnosticCode = DiagnosticCode::new("L2101");

    /// `L2102` — a `render.bounds` query ran against a scene that has no `Camera` component, so no
    /// screen-space rects can be computed (world AABBs would still be well-defined, but the query's
    /// whole purpose is the projection). Raised by `ludion-engine`'s bounds computation. (Unlike a
    /// *render*, which falls back to a default framing when no camera exists, `render.bounds` refuses
    /// to invent a viewpoint — a projected rect against a made-up camera would be a lie.)
    pub const RENDER_NO_CAMERA: DiagnosticCode = DiagnosticCode::new("L2102");

    /// `L2103` — a scene's `GiVolume.cell_size` splits the scene's world bounds into a
    /// global-illumination grid the voxelizer will not allocate: too many cells in total, or too
    /// many along one axis for a 3D texture. Raised by `ludion-engine`'s voxelizer during extract,
    /// before any allocation happens.
    pub const GI_VOLUME_CELL_BUDGET_EXCEEDED: DiagnosticCode = DiagnosticCode::new("L2103");

    /// `L2104` — `ludion render --dump-voxels` or `--dump-gi` was asked to dump a scene's
    /// global-illumination grid, but the scene authors no `GiVolume`, so extraction built none.
    /// Raised by `ludion-engine`'s voxel-sheet dump, before any GPU adapter is requested.
    pub const GI_VOLUME_MISSING: DiagnosticCode = DiagnosticCode::new("L2104");

    /// `L2105` — a scene's `GiVolume.probe_spacing` lays more irradiance probes over the
    /// global-illumination grid's world box than the bake will allocate. Raised by `ludion-engine`'s
    /// voxelizer during extract, before any probe volume is allocated.
    pub const GI_VOLUME_PROBE_BUDGET_EXCEEDED: DiagnosticCode = DiagnosticCode::new("L2105");

    /// `L2106` — a `GroundMarker` or `DropShadow` shape dimension (`radius`, `length`, `width`), or
    /// a `Decal` `size` element, is not a strictly positive, finite number, so the marker has no
    /// footprint to draw. Raised by `ludion-engine`'s `GroundMarker`, `DropShadow` and `Decal`
    /// semantic validators, at scene-check / `component.set` time.
    pub const GROUND_MARKER_NON_POSITIVE_DIMENSION: DiagnosticCode = DiagnosticCode::new("L2106");

    /// `L2107` — a `ParticleEmitter` authors an effect that cannot be evaluated: a non-finite or
    /// negative dimension (`extent`, `life`, `speed`, and the scalars a schema bound cannot reach),
    /// a `life`/`speed` range whose minimum exceeds its maximum, or a live population
    /// (`rate × life[1] + burst`) above the per-emitter cap. Raised by `ludion-engine`'s
    /// `ParticleEmitter` semantic validator, at scene-check / `component.set` time.
    pub const PARTICLE_EMITTER_INVALID: DiagnosticCode = DiagnosticCode::new("L2107");

    /// `L2108` — a `PatternPlayer` or `Volley` cannot be evaluated as authored, and the sim
    /// degrades instead of crashing: the player's `pattern` names a document that is not loaded, a
    /// `Volley` names a bullet kind its document lacks, an aimed volley's `target` entity is
    /// missing or dead (the volley falls back to its captured absolute angle), or a `Volley` is
    /// authored with `count` over 256. A **warning**, reported once per offending volley by the
    /// engine's `play_patterns` system, which the game registers in its own schedule
    /// (`Plan/projectile-patterns.md` decision 5).
    pub const VOLLEY_INVALID: DiagnosticCode = DiagnosticCode::new("L2108");

    /// `L2109` — a drawable's world placement is not finite (a NaN or infinite sprite centre or
    /// axis, or mesh model matrix), so extract skips it rather than handing the renderer NaN
    /// geometry. Raised by `ludion-engine`'s extract, once per offending component per session.
    pub const RENDER_PLACEMENT_NON_FINITE: DiagnosticCode = DiagnosticCode::new("L2109");

    /// `L2110` — a `DropShadow` distance is unusable: `max_distance` is not a positive, finite
    /// number, `fade_distance` is negative or non-finite, or `fade_distance` exceeds
    /// `max_distance`. Raised by `ludion-engine`'s `DropShadow` semantic validator, at
    /// scene-check / `component.set` time.
    pub const DROP_SHADOW_INVALID_DISTANCE: DiagnosticCode = DiagnosticCode::new("L2110");

    /// `L2111` — `ludion render --dump-voxels` or `--dump-gi` was asked to dump a scene whose
    /// `GiVolume` resolves to no grid — every placed mesh was skipped, or none is placed. Raised by
    /// `ludion-engine`'s voxel-sheet dump, before any GPU adapter is requested.
    pub const GI_VOLUME_EMPTY: DiagnosticCode = DiagnosticCode::new("L2111");

    /// `L2112` — a `Decal` `angle_fade` is unusable: it must hold `0 <= start <= end <= 90`, all
    /// finite. Raised by `ludion-engine`'s `Decal` semantic validator, at scene-check /
    /// `component.set` time; the pointer is `/angle_fade` or `/angle_fade/<n>`.
    pub const DECAL_INVALID_ANGLE_FADE: DiagnosticCode = DiagnosticCode::new("L2112");

    /// `L2113` — a `GroundMarker` wedge is not drawable as authored: `half_angle_deg` outside
    /// `(0, 180]`, `inner_radius` negative or not below `outer_radius`, or `fill` outside `[0, 1]`;
    /// or a `DropShadow` authors `type: wedge`. Raised by `ludion-engine`'s `GroundMarker` and
    /// `DropShadow` semantic validators, at scene-check / `component.set` time.
    pub const GROUND_MARKER_WEDGE_INVALID: DiagnosticCode = DiagnosticCode::new("L2113");

    /// `L2340` — a `GroundMarker` ring is not drawable as authored: `thickness` above `radius`, or
    /// `fill` outside `[0, 1]`; or a `DropShadow` authors `type: ring`. Raised by `ludion-components`'
    /// `GroundMarker` and `DropShadow` semantic validators.
    pub const GROUND_MARKER_RING_INVALID: DiagnosticCode = DiagnosticCode::new("L2340");

    /// `L2341` — a `GroundMarker.style` field is out of range: `outline_px`/`grid_px` not positive
    /// and finite, or an alpha or `flicker` outside `[0, 1]`. Pointer `/style/<field>`.
    pub const GROUND_MARKER_STYLE_INVALID: DiagnosticCode = DiagnosticCode::new("L2341");

    /// `L2342` — a `GroundMarkerClip` field is out of range (`bins`, `height`, `partial_alpha`), names
    /// an undeclared layer, or sits on a segment marker or an entity with no `GroundMarker`.
    pub const GROUND_MARKER_CLIP_INVALID: DiagnosticCode = DiagnosticCode::new("L2342");

    /// `L2343` — a `GroundMarkerClip` is authored but the world has no physics, so the marker is
    /// drawn unclipped. Raised by extract.
    pub const GROUND_MARKER_CLIP_NO_PHYSICS: DiagnosticCode = DiagnosticCode::new("L2343");

    /// `L2350` — an `OccludedSilhouette` field is out of range: a colour channel outside `[0, 1]`,
    /// `outline.px` outside `(0, 8]`, `outline.alpha` outside `[0, 1]`, or a negative or non-finite `bias`.
    pub const OCCLUDED_SILHOUETTE_INVALID: DiagnosticCode = DiagnosticCode::new("L2350");

    /// `L2351` — an `OccludedSilhouette` resolves onto a `Viewmodel` mesh, which is never silhouetted.
    /// Raised by extract.
    pub const OCCLUDED_SILHOUETTE_ON_VIEWMODEL: DiagnosticCode = DiagnosticCode::new("L2351");

    /// `L2352` — a frame resolves more than 8 distinct silhouette styles; the extras are not drawn.
    /// Raised by extract.
    pub const OCCLUDED_SILHOUETTE_STYLE_LIMIT: DiagnosticCode = DiagnosticCode::new("L2352");

    /// `L2360` — a `Mesh.blend` resolves onto a `Viewmodel` mesh, which is drawn opaque. Raised by extract.
    pub const MESH_BLEND_ON_VIEWMODEL: DiagnosticCode = DiagnosticCode::new("L2360");

    /// `L2370` — a `RenderView` has an empty `name`, a `size` axis outside `16..=2048`, or no `Camera` on
    /// its entity.
    pub const RENDER_VIEW_INVALID: DiagnosticCode = DiagnosticCode::new("L2370");

    /// `L2371` — two `RenderView`s share a `name`.
    pub const RENDER_VIEW_NAME_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L2371");

    /// `L2372` — a `UiRect.view` names no `RenderView`.
    pub const UI_RECT_VIEW_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L2372");

    /// `L2373` — a `UiRect` sets `view` beside `texture`, `atlas`, `frame`, `slice` or `uv_rect`.
    pub const UI_RECT_VIEW_CONFLICT: DiagnosticCode = DiagnosticCode::new("L2373");

    /// `L2374` — more than 4 `RenderView`s are enabled; the extras are not rendered. Raised by extract.
    pub const RENDER_VIEW_BUDGET: DiagnosticCode = DiagnosticCode::new("L2374");

    /// `L2375` — a `RenderLayers` list, `Camera.layers` or `RenderView.layers` is empty, holds an empty or
    /// repeated name, or the scene uses more than 31 layer names besides `default`.
    pub const RENDER_LAYERS_INVALID: DiagnosticCode = DiagnosticCode::new("L2375");

    /// `L2376` — a layer name in `RenderLayers`, `Camera.layers`/`lights` or `RenderView.layers`/`lights` is not
    /// declared under the scene's `render_layers:`.
    pub const RENDER_LAYER_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L2376");

    /// `L2380` — a `Rain` field is non-finite or out of range.
    pub const RAIN_INVALID: DiagnosticCode = DiagnosticCode::new("L2380");

    /// `L2381` — more than one `Rain` is in the scene; the lowest entity id is used.
    pub const RAIN_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L2381");

    /// `L2382` — a `Rain` asks for more streaks than the budget; the per-tile count is clamped.
    pub const RAIN_STREAK_BUDGET: DiagnosticCode = DiagnosticCode::new("L2382");

    /// `L2390` — a `.cube` LUT file is malformed: a bad header, a bad data row or a wrong row count.
    pub const CUBE_LUT_MALFORMED: DiagnosticCode = DiagnosticCode::new("L2390");

    /// `L2391` — a `.cube` LUT is valid but unsupported: a 1D table or a 3D size other than 17, 33 or 65.
    pub const CUBE_LUT_UNSUPPORTED: DiagnosticCode = DiagnosticCode::new("L2391");

    /// `L1760` — a `PostProcess` metering field is out of range: `metering_clip`, `exposure_range`.
    pub const POST_PROCESS_INVALID_METERING: DiagnosticCode = DiagnosticCode::new("L1760");

    /// `L1761` — a `ScreenEffect` region target is unusable: a non-positive or non-finite size, or a
    /// non-finite offset.
    pub const SCREEN_EFFECT_INVALID_TARGET: DiagnosticCode = DiagnosticCode::new("L1761");

    /// `L2392` — more than four `ScreenEffect`s are applied; the extras (highest `order`, then highest
    /// entity id) are dropped.
    pub const SCREEN_EFFECT_CAP: DiagnosticCode = DiagnosticCode::new("L2392");

    /// `L2201` — a golden-test comparison was requested but no golden image exists yet at the
    /// expected path (or it could not be read). Raised by `ludion-testkit`.
    pub const RENDER_GOLDEN_MISSING: DiagnosticCode = DiagnosticCode::new("L2201");

    /// `L2202` — a freshly rendered image's dimensions do not match its committed golden's
    /// dimensions, so no pixel-wise comparison is possible. Raised by `ludion-testkit`.
    pub const RENDER_IMAGE_DIMENSION_MISMATCH: DiagnosticCode = DiagnosticCode::new("L2202");

    /// `L2203` — a golden-test comparison's global SSIM score fell below the configured threshold.
    /// Raised by `ludion-testkit`.
    pub const RENDER_SSIM_BELOW_THRESHOLD: DiagnosticCode = DiagnosticCode::new("L2203");

    /// `L2204` — a committed golden image file exists and was read, but its bytes are not a
    /// decodable PNG. Raised by `ludion-testkit`.
    pub const RENDER_GOLDEN_PNG_DECODE: DiagnosticCode = DiagnosticCode::new("L2204");

    /// `L2205` — a golden test ran on an operating system that has no pinned golden tier (goldens
    /// are blessed per-OS on the two pinned GPU targets, `windows` and `macos`). Raised by
    /// `ludion-testkit`.
    pub const RENDER_GOLDEN_PLATFORM_UNPINNED: DiagnosticCode = DiagnosticCode::new("L2205");

    /// `L2206` — a bless was asked to write a per-OS pinned golden set (`goldens/<os>/`) from a GPU
    /// that is not that platform's pinned reference adapter. Raised by `ludion-testkit`.
    pub const RENDER_GOLDEN_BLESS_ADAPTER_UNPINNED: DiagnosticCode = DiagnosticCode::new("L2206");

    /// `L2207` — a golden-test comparison's per-tile colour error exceeded the ceiling while its
    /// structural SSIM passed. Raised by `ludion-testkit`.
    pub const RENDER_GOLDEN_COLOR_ABOVE_CEILING: DiagnosticCode = DiagnosticCode::new("L2207");

    /// `L2208` — a golden suite's `goldens/manifest.json` is not valid JSON, does not match the
    /// manifest shape (unknown fields are refused), or names a stem no scene in the suite has.
    /// Raised by `ludion-testkit` (`ludion test`).
    pub const GOLDEN_MANIFEST_INVALID: DiagnosticCode = DiagnosticCode::new("L2208");

    /// `L2209` — `ludion test` checked no scene: `--filter` matched none, or `--dir` holds none.
    /// Raised by `ludion-cli` (`ludion test`).
    pub const TEST_CHECKED_NOTHING: DiagnosticCode = DiagnosticCode::new("L2209");

    /// `L2210` — a byte-exact comparison against a pinned per-OS golden ran on a GPU that is not
    /// that platform's reference adapter; the test skips. Raised by `ludion-testkit`.
    pub const RENDER_GOLDEN_COMPARE_ADAPTER_UNPINNED: DiagnosticCode = DiagnosticCode::new("L2210");

    // --- L3xxx: protocol / server (raised by `ludion-protocol`, `ludion-server`) ---

    /// `L3001` — a protocol request envelope is malformed: not a JSON object, missing its `v` or
    /// `method`, or carrying `params` that do not match the method. Raised by `ludion-protocol`.
    pub const PROTOCOL_MALFORMED_ENVELOPE: DiagnosticCode = DiagnosticCode::new("L3001");

    /// `L3002` — a request named a `method` that is a valid string but not one this protocol version
    /// defines (the diagnostic suggests the nearest known method). Raised by `ludion-protocol`.
    pub const PROTOCOL_UNKNOWN_METHOD: DiagnosticCode = DiagnosticCode::new("L3002");

    /// `L3003` — a request envelope declared a protocol version this engine build does not speak.
    /// Raised by `ludion-protocol`.
    pub const PROTOCOL_UNSUPPORTED_VERSION: DiagnosticCode = DiagnosticCode::new("L3003");

    /// `L3004` — a binary frame-streaming message is malformed: too short, wrong magic, an unknown
    /// pixel format, a payload length that disagrees with the bytes, or dimensions past the
    /// largest extent a host renders. Raised by `ludion-protocol`.
    pub const PROTOCOL_MALFORMED_FRAME: DiagnosticCode = DiagnosticCode::new("L3004");

    /// `L3005` — an `input.inject` command carried an `event` value that does not deserialize into a
    /// sim input event, an analog value that is non-finite or out of range, or a `frame` the sim has
    /// already run. Raised by `ludion-engine`'s dispatch.
    pub const PROTOCOL_INVALID_INPUT: DiagnosticCode = DiagnosticCode::new("L3005");

    /// `L3006` — a `component.get` or `component.remove` named an entity that is alive and a
    /// component that is registered, but the entity does not currently carry that component. Raised
    /// by `ludion-engine`'s dispatch.
    pub const PROTOCOL_COMPONENT_ABSENT: DiagnosticCode = DiagnosticCode::new("L3006");

    /// `L3007` — a `stream.unsubscribe` named a subscription id that is not open. Raised by
    /// `ludion-engine`'s dispatch, and by `ludion-server` when the sub is not owned by the asking
    /// connection.
    pub const PROTOCOL_UNKNOWN_SUBSCRIPTION: DiagnosticCode = DiagnosticCode::new("L3007");

    /// `L3008` — an `engine.restore` snapshot document is malformed or was produced by an
    /// incompatible engine build. Raised by `ludion-engine`'s dispatch.
    pub const PROTOCOL_SNAPSHOT_INVALID: DiagnosticCode = DiagnosticCode::new("L3008");

    /// `L3009` — the engine could not answer: the server is unreachable, is shutting down, or its
    /// sim thread has stopped. Raised by `ludion-server` when the command queue or its reply channel
    /// has disconnected, and by `ludion-client` when it cannot reach the server, the handshake
    /// stalls, or the connection drops mid-request. A client call timeout is `L3054`.
    pub const PROTOCOL_ENGINE_UNAVAILABLE: DiagnosticCode = DiagnosticCode::new("L3009");

    /// `L3010` — a replay session file is not well-formed (wrong magic, an unsupported format
    /// version, a truncated record, a record body that is not valid JSON), or was recorded against
    /// an incompatible protocol version. Raised by `ludion-protocol` and `ludion-engine`.
    pub const REPLAY_FORMAT: DiagnosticCode = DiagnosticCode::new("L3010");

    /// `L3011` — a replay session's zstd frame could not be decompressed (the file is corrupt or was
    /// written by an incompatible tool). Raised by `ludion-protocol`.
    pub const REPLAY_COMPRESSION: DiagnosticCode = DiagnosticCode::new("L3011");

    /// `L3014` — an `entity.spawn` supplied both a top-level `name` and a `Name` component in its
    /// `components` map, so the entity's name is specified two ways at once. Raised by
    /// `ludion-engine`'s dispatch, before the entity is spawned.
    pub const PROTOCOL_SPAWN_NAME_CONFLICT: DiagnosticCode = DiagnosticCode::new("L3014");

    /// `L3013` — a replay header names a *path*-based scene reference that could not be loaded while
    /// replaying (the file is absent at, or unreadable under, the replay's project root). Recordings
    /// made by this engine embed the scene inline and never hit this; it flags a hand-authored or
    /// externally produced replay whose external scene did not travel with it. Raised by
    /// `ludion-engine`'s replay.
    pub const REPLAY_SCENE_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L3013");

    /// `L3015` — an `engine.restore` migration **dropped** a component because this engine build no
    /// longer registers it. The entity restored without that component; the drop is reported (not
    /// fatal) as a `RestoreReport` entry rather than failing the restore. Referenced by
    /// `ludion-protocol`'s restore report and `ludion-engine`'s snapshot restore.
    pub const RESTORE_COMPONENT_UNREGISTERED: DiagnosticCode = DiagnosticCode::new("L3015");

    /// `L3016` — an `engine.restore` migration **dropped** a component because its snapshotted value
    /// no longer deserializes into the (still-registered) component type. The entity restored without
    /// it; the serde error and entity id ride the reported `RestoreReport` entry. Referenced by
    /// `ludion-protocol`'s restore report and `ludion-engine`'s snapshot restore.
    pub const RESTORE_COMPONENT_UNDESERIALIZABLE: DiagnosticCode = DiagnosticCode::new("L3016");

    /// `L3017` — a replay zstd frame decompresses to more than its ceiling (256x its compressed
    /// size, within 1 MiB..=256 MiB), so the reader refused to inflate it (a decompression bomb, or
    /// a corrupt frame). Raised by `ludion-protocol`'s replay reader, and by its writer for a
    /// header or record the reader would refuse.
    pub const REPLAY_DECOMPRESSED_TOO_LARGE: DiagnosticCode = DiagnosticCode::new("L3017");

    /// `L3018` — a mid-run scene switch could not load or validate its target district, so the switch
    /// was rejected and the sim continues in the current district (the switch is transactional).
    /// Raised by `ludion-engine`'s transition drain.
    pub const TRANSITION_TARGET_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L3018");

    /// `L3019` — a mid-run scene switch loaded its target district but found no entity named by the
    /// requested spawn anchor, so the switch was rejected and the sim continues in the current
    /// district. Raised by `ludion-engine`'s transition drain.
    pub const TRANSITION_ANCHOR_MISSING: DiagnosticCode = DiagnosticCode::new("L3019");

    /// `L3020` — a mid-run scene switch named a carried entity that was not alive at the switch
    /// frame; it was skipped and the switch proceeded without it (a warning, not a rejection). Raised
    /// by `ludion-engine`'s transition drain.
    pub const TRANSITION_CARRY_DEAD: DiagnosticCode = DiagnosticCode::new("L3020");

    /// `L3021` — an offline replay-to-frames render was asked for a frame window that falls outside the
    /// replay's recorded range (a `--from` past `--to`, or a `--to` past the replay's final frame).
    /// Also raised when replay-to-frames, `--profile-replay` or the divergence bisector is given a
    /// replay whose frame numbers are not monotonic (an `engine.restore`, or a `scene.load` after a
    /// tick), when a profile measures no frame, and when a bisect frame is past the replay's end.
    /// Raised by `ludion-engine`'s replay-to-frames render, replay profiler and bisector.
    pub const REPLAY_FRAME_WINDOW: DiagnosticCode = DiagnosticCode::new("L3021");

    /// `L3022` — the host's listening socket refused to hand over an incoming connection. Raised by
    /// `ludion-server`'s accept loop, once per error class while it retries, and once more when it
    /// gives up and stops the host. Host-side only: no client is connected to answer.
    pub const SERVER_ACCEPT_FAILED: DiagnosticCode = DiagnosticCode::new("L3022");

    /// `L3023` — a request reached the sim thread but no reply came back within the server's wait
    /// bound: the sim thread is running and is not returning to its command loop. Raised by
    /// `ludion-server` in place of the reply, and by the embedded host's `suspend`/`resume` when the
    /// sim thread does not confirm within their 2 s grace.
    pub const PROTOCOL_SIM_STALLED: DiagnosticCode = DiagnosticCode::new("L3023");

    /// `L3024` — a replay re-driven with the current build no longer reproduces a keyframe the
    /// recording embedded: the live world, RNG state, or entity free list differs from the snapshot.
    /// Raised by `ludion-engine`'s divergence-bisect verifier.
    pub const REPLAY_KEYFRAME_DRIFT: DiagnosticCode = DiagnosticCode::new("L3024");

    /// `L3025` — a `render.size` request did not name an extent this host can render: one axis alone,
    /// a zero, or a count past 7680 x 4320. Raised by `ludion-engine`'s `render.size` handler; the
    /// session's extent is left exactly as it was.
    pub const PROTOCOL_RENDER_SIZE_INVALID: DiagnosticCode = DiagnosticCode::new("L3025");

    /// `L3026` — two `Residue`-marked entities in the scene a switch left shared one `Name`, so the
    /// scene's residue can only name one of them: the first was captured and the rest were skipped.
    /// The switch still happened. Raised by `ludion-engine`'s transition capture.
    pub const RESIDUE_DUPLICATE_NAME: DiagnosticCode = DiagnosticCode::new("L3026");

    /// `L3027` — an entity was both **carried** across a scene switch and marked `Residue`, so it was
    /// asked to leave and to stay behind at once. The carry wins and no residue was captured for it.
    /// The switch still happened. Raised by `ludion-engine`'s transition capture.
    pub const RESIDUE_CARRY_CONFLICT: DiagnosticCode = DiagnosticCode::new("L3027");

    /// `L3028` — the session's scene residue hit one of its bounds and dropped state to stay inside
    /// it: past `RESIDUE_MAX_SCENES` the least-recently-visited scene was evicted, and past
    /// `RESIDUE_MAX_RECORDS_PER_SCENE` a scene's capture was truncated. The switch still happened.
    /// Raised by `ludion-engine`'s transition commit.
    pub const RESIDUE_EVICTED: DiagnosticCode = DiagnosticCode::new("L3028");

    /// `L3029` — a scene switch found its spawn anchor in the target district but the anchor carries
    /// no `Transform`, so it names no drop point. The switch is rejected, exactly as a missing anchor
    /// is. Raised by `ludion-engine`'s transition.
    pub const TRANSITION_ANCHOR_NO_TRANSFORM: DiagnosticCode = DiagnosticCode::new("L3029");

    /// `L3030` — a residue record captured before a component changed shape can no longer be read by
    /// this build, so the record was dropped and the scene's authored version stands. The switch
    /// still happened. Raised by `ludion-engine`'s residue application.
    pub const RESIDUE_RECORD_UNMIGRATABLE: DiagnosticCode = DiagnosticCode::new("L3030");

    /// `L3031` — a residue record captured off a **scene-authored** entity found no entity of that
    /// `Name` in the scene it belongs to (it was renamed), so it was dropped rather than spawned as a
    /// duplicate. A code-spawned record respawns instead and never raises this. The switch still
    /// happened. Raised by `ludion-engine`'s residue application.
    pub const RESIDUE_NAME_UNMATCHED: DiagnosticCode = DiagnosticCode::new("L3031");

    /// `L3032` — a `render.view` request carried a pose the host cannot look through: a non-finite
    /// coordinate or angle, or a vertical field of view outside `0..180` degrees. Raised by
    /// `ludion-engine`'s `render.view` handler; the override is left exactly as it was.
    pub const PROTOCOL_RENDER_VIEW_INVALID: DiagnosticCode = DiagnosticCode::new("L3032");

    /// `L3033` — an engine snapshot's `physics` blob could not be decoded into a solver state, or
    /// the live solver state could not be encoded into one. Raised by `ludion-engine`'s
    /// `engine.snapshot`/`engine.restore` handlers; the blob is base64 of a binary codec, so a
    /// truncated document or a build whose solver pin has moved lands here.
    pub const PHYSICS_SNAPSHOT_INVALID: DiagnosticCode = DiagnosticCode::new("L3033");

    /// `L3034` — a restored snapshot carries solver state (`physics`) but no scene physics
    /// settings (`scene_physics`): physics will not step after this restore. Raised as a warning
    /// by `ludion-engine`'s `engine.restore` handler once per distinct restore.
    pub const PHYSICS_SNAPSHOT_NO_SETTINGS: DiagnosticCode = DiagnosticCode::new("L3034");

    /// `L3035` — a `Joint`'s `a` or `b` is dead or has no rigid/kinematic/static body (a rejected
    /// shape, or a `Collider` never claimed): the joint is left inert rather than deleted, and
    /// re-forms once both bodies exist again. Raised as a warning by `ludion-engine`'s
    /// `scene_physics` seam, latched once per carrier for the session (`ludion-engine`'s
    /// `Diagnostics`): a carrier that goes inert, re-forms and then goes inert again reports only
    /// the first time.
    pub const JOINT_BODY_MISSING: DiagnosticCode = DiagnosticCode::new("L3035");

    /// `L3036` — `switch_to_ragdoll` or `switch_skin_to_ragdoll` refused to switch an entity: it is
    /// dead, has no `Transform`, its scale is not uniform, its `Mesh` is not an assembly (or not a
    /// skinned mesh), one of the assembly's part meshes is empty, a part's mass at that scale is zero
    /// or not finite, or the skin's parts prefab breaks the `$skin` rules (not last, a part bound to another skin or another
    /// reference to it, a different mesh, no `SkinBone` part). `switch_skin_to_ragdoll` also refuses a
    /// presentation skin and a skin already driven by `SkinBone` parts. Raised by `ludion-engine`'s
    /// ragdoll helpers; the switch mutates nothing when this fires (all-or-nothing).
    pub const RAGDOLL_SWITCH_INVALID: DiagnosticCode = DiagnosticCode::new("L3036");

    /// `L3037` — a `JointMotor` does not fit its carrier's `Joint`: the joint is `fixed`, the
    /// `target` arity does not match the kind, or a value written past validation is non-finite or
    /// negative. The motor is not applied — the joint stays passive — until the `JointMotor` or
    /// `Joint` changes. A passive motor (stiffness and damping both 0) is not checked, so a mismatch
    /// reports when a system first makes it active. Raised as an error by `ludion-engine`'s
    /// `scene_physics` seam, latched once per carrier for the session.
    pub const JOINT_MOTOR_MISMATCH: DiagnosticCode = DiagnosticCode::new("L3037");

    /// `L3038` — an at-centre or at-point impulse (`PhysicsWorld::apply_impulse`,
    /// `apply_impulse_at_point`) named an entity with no rigid body at apply time: it is dead, its
    /// shape was rejected, or its `Collider` is `static` or `kinematic`. The impulse is dropped.
    /// Raised as a warning by `ludion-engine`'s `scene_physics` seam, latched once per entity for
    /// the session.
    pub const IMPULSE_TARGET_INVALID: DiagnosticCode = DiagnosticCode::new("L3038");

    /// `L3039` — the WebSocket handshake carried a browser `Origin` that is neither loopback nor
    /// the editor's own, and the server refused it with HTTP 403. Raised by `ludion-server`.
    pub const WIRE_ORIGIN_REFUSED: DiagnosticCode = DiagnosticCode::new("L3039");

    /// `L3040` — a `scene.load`, `scene.save` or `scene.check` path is not a plain path relative
    /// to the content root: it is absolute, carries a drive, scheme or stream prefix, or climbs
    /// out with `..`. Raised by `ludion-engine`.
    pub const WIRE_PATH_OUTSIDE_ROOT: DiagnosticCode = DiagnosticCode::new("L3040");

    /// `L3041` — a snapshot's `entities` list disagrees with the live slots its `allocator` field
    /// describes, so `engine.restore` refused it before touching the engine. Raised by
    /// `ludion-engine`.
    pub const RESTORE_ENTITIES_MISMATCH: DiagnosticCode = DiagnosticCode::new("L3041");

    /// `L3042` — a recording died mid-session: a write to the replay sink failed, or a frame was
    /// past its replay ceiling (`L3017`). The recorder is dropped and the failure is reported once
    /// through `Diagnostics` and returned by `Engine::stop_recording`. Raised by `ludion-engine`.
    pub const RECORDING_STOPPED: DiagnosticCode = DiagnosticCode::new("L3042");

    /// `L3043` — a mid-run scene switch carried an entity whose parent, or an entity a component of it
    /// references, was alive but not carried. A parent left behind un-parents the entity, keeping its
    /// world rotation and scale, translation at the anchor; a left-behind reference arrives empty
    /// (optional field) or leaves its component behind (required field). A warning, not a rejection.
    /// Raised by `ludion-engine`'s transition drain.
    pub const TRANSITION_CARRY_ORPHANED: DiagnosticCode = DiagnosticCode::new("L3043");

    /// `L3044` — a server's reply body did not match the command that was sent (a response kind
    /// this client did not expect for the method). Raised by `ludion-client` and the CLI's clients.
    pub const PROTOCOL_UNEXPECTED_RESPONSE: DiagnosticCode = DiagnosticCode::new("L3044");

    /// `L3045` — a wire message was over `ludion_protocol::MAX_MESSAGE_BYTES` and was refused rather
    /// than sent or read. Raised by `ludion-client` and `ludion-server`.
    pub const PROTOCOL_MESSAGE_TOO_LARGE: DiagnosticCode = DiagnosticCode::new("L3045");

    /// `L3046` — a `stream.subscribe` carried a `max_fps` that is not a finite number in
    /// `[0.1, 1000]`. Raised by `ludion-engine`.
    pub const PROTOCOL_INVALID_MAX_FPS: DiagnosticCode = DiagnosticCode::new("L3046");

    /// `L3047` — `Engine::set_recording` was called at a frame other than 0 or with no scene loaded,
    /// so the recording could not start from the scene's initial state. Raised by `ludion-engine`.
    pub const RECORDING_ARM_REFUSED: DiagnosticCode = DiagnosticCode::new("L3047");

    /// `L3048` — an `engine.restore` snapshot was captured by a different engine version than the
    /// running build. Reported in the restore report, not fatal. Raised by `ludion-protocol`'s
    /// `restore` module.
    pub const RESTORE_ENGINE_VERSION_DIFFERS: DiagnosticCode = DiagnosticCode::new("L3048");

    /// `L3049` — an `engine.restore` snapshot carried a top-level field this build does not know,
    /// which the restore ignored. Reported in the restore report, not fatal. Raised by
    /// `ludion-protocol`'s `restore` module.
    pub const RESTORE_SNAPSHOT_FIELD_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L3049");

    /// `L3050` — a replay marks a system panic on a tick that, re-driven with the current build, did
    /// not panic: the recorded session is not reproduced. Raised by `ludion-engine`'s replay drive.
    pub const REPLAY_PANIC_NOT_REPRODUCED: DiagnosticCode = DiagnosticCode::new("L3050");

    /// `L3051` — a server's message could not be parsed: the handshake reply, a response or a pushed
    /// event is not JSON or not the shape this protocol version defines. Raised by `ludion-client`.
    pub const PROTOCOL_MALFORMED_REPLY: DiagnosticCode = DiagnosticCode::new("L3051");

    /// `L3052` — a replay's final record is cut short (a recording stopped mid-write or a killed
    /// host); the reader dropped it and the session replays up to the last whole record. A warning.
    /// Raised by `ludion-protocol`.
    pub const REPLAY_TORN_TAIL: DiagnosticCode = DiagnosticCode::new("L3052");

    /// `L3053` — a scene's residue could not follow an entity reference across a visit: its target
    /// is neither another residue record nor a uniquely identifiable named entity, so an optional
    /// reference arrived empty and a component whose required reference is unresolved was left
    /// behind. The switch still happened. A warning. Raised by `ludion-engine`'s residue capture
    /// and application.
    pub const RESIDUE_REF_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L3053");

    /// `L3054` — a client's own per-call timeout elapsed before the reply arrived. The command may
    /// still run on the host. Raised by `ludion-client`.
    pub const PROTOCOL_CALL_TIMED_OUT: DiagnosticCode = DiagnosticCode::new("L3054");

    /// `L3055` — the server refused a connection because it was already at its connection cap. The
    /// refusal is the handshake's error reply. Raised by `ludion-server`.
    pub const SERVER_CONNECTION_LIMIT: DiagnosticCode = DiagnosticCode::new("L3055");

    /// `L3056` — a connection's text lane was full, so one or more events for a subscription were
    /// dropped; the notice takes the last dropped event's `seq`. A warning. Raised by
    /// `ludion-server`.
    pub const SERVER_EVENTS_DROPPED: DiagnosticCode = DiagnosticCode::new("L3056");

    /// `L3057` — a `sim.step` asked for more ticks than one command may run: the step is
    /// synchronous on the sim thread. Raised by `ludion-engine`.
    pub const SIM_STEP_TOO_LONG: DiagnosticCode = DiagnosticCode::new("L3057");

    /// `L3058` — `scene.save` refused: the destination differs from the text this session loaded or
    /// last saved there (or the session never saw it). Raised by `ludion-engine`.
    pub const SCENE_SAVE_CONFLICT: DiagnosticCode = DiagnosticCode::new("L3058");

    /// `L3059` — `scene.save` refused: the destination is a `kind:` document (a prefab, mesh and so
    /// on), not a scene. Raised by `ludion-engine`.
    pub const SCENE_SAVE_WRONG_KIND: DiagnosticCode = DiagnosticCode::new("L3059");

    // --- L4xxx: sim (raised by `ludion-sim`) ---

    /// `L4001` — a simulation speed factor was not a finite number in `[1/64, 64]`. Raised by
    /// `ludion-sim`.
    pub const INVALID_SIM_SPEED: DiagnosticCode = DiagnosticCode::new("L4001");

    /// `L4002` — an input-script file could not be read from disk. Raised by `ludion-engine`'s `trace`
    /// (`load_input_script`, entered via `ludion sim-hash --script`).
    pub const INPUT_SCRIPT_IO: DiagnosticCode = DiagnosticCode::new("L4002");

    /// `L4003` — an input-script file is not valid input-script JSON. Raised by `ludion-engine`'s `trace`
    /// (`load_input_script`, entered via `ludion sim-hash --script`).
    pub const INPUT_SCRIPT_INVALID: DiagnosticCode = DiagnosticCode::new("L4003");

    /// `L4004` — a profile budget file could not be read, or is not valid budget TOML. Raised by
    /// `ludion-engine`'s `profile` (entered via `ludion profile --check`).
    pub const BUDGET_FILE_INVALID: DiagnosticCode = DiagnosticCode::new("L4004");

    /// `L4005` — a measured timing exceeded its budget. Raised by `ludion-engine`'s
    /// `profile` (entered via `ludion profile --check`), one diagnostic per over-budget line.
    pub const BUDGET_EXCEEDED: DiagnosticCode = DiagnosticCode::new("L4005");

    /// `L4006` — a budget file names a system or GPU pass that no producible name matches: not a
    /// registered system of the profiled engine, and not one of the frame graph's fixed pass
    /// names. Raised by `ludion-engine`'s budget check (`ludion profile --check` / a game's
    /// `--profile-check`) — distinct from a name that is producible but simply was not measured this
    /// run (a conditional GPU pass, or no GPU adapter), which is not a violation.
    pub const BUDGET_UNKNOWN_KEY: DiagnosticCode = DiagnosticCode::new("L4006");

    /// `L4007` — a game system panicked. The schedule's per-system unwind boundary caught it, the
    /// rest of the tick was skipped, and the sim auto-paused instead of taking the host down. Raised
    /// by `ludion-engine` at the frame boundary, from the record `ludion-ecs` left in the world.
    /// Also raised when a system removed the `InputState` resource (the driver restores an empty one).
    pub const SYSTEM_PANICKED: DiagnosticCode = DiagnosticCode::new("L4007");

    /// `L4120` — an agent's machine made more than `MAX_TRANSITIONS_PER_TICK` transitions in one
    /// tick and stopped where it was. Queued by `ludion-ai`'s `ai_tick`, reported by `ludion-engine`.
    pub const AI_TRANSITION_CHAIN_CAPPED: DiagnosticCode = DiagnosticCode::new("L4120");

    /// `L4121` — an agent's event inbox was full and its oldest event was dropped. Queued by
    /// `ludion-ai`, reported by `ludion-engine`.
    pub const AI_INBOX_OVERFLOW: DiagnosticCode = DiagnosticCode::new("L4121");

    /// `L4122` — an agent is a `GroupMember` of an entity that is gone or has no `Group`. Queued by
    /// `ludion-ai`, reported by `ludion-engine`.
    pub const AI_GROUP_MISSING: DiagnosticCode = DiagnosticCode::new("L4122");

    /// `L4123` — an event was sent to an entity with no `Mind` of that machine. Queued by
    /// `ludion-ai`, reported by `ludion-engine`.
    pub const AI_SEND_NO_MIND: DiagnosticCode = DiagnosticCode::new("L4123");

    /// `L4124` — a `Mind`'s authored `state` is not listed in its machine's `ALL`. Queued by
    /// `ludion-ai`, reported by `ludion-engine`.
    pub const AI_STATE_NOT_LISTED: DiagnosticCode = DiagnosticCode::new("L4124");

    /// `L4008` — a render-time scene override (`ludion render --set <json-pointer>=<value>`) could
    /// not be applied: a malformed spec, a pointer that does not resolve in the scene document, or a
    /// scene-level field given a value it cannot hold. Raised by `ludion-scene`'s override layer,
    /// which `ludion render --set` drives, and by `--sweep`, the same primitive with a list of
    /// values (which adds an empty or over-long list to the misshapes).
    /// `ludion render --look-at` and its `--azimuth`/`--pitch`/`--distance`/`--fov` expand into
    /// overrides, so a framing flag without `--look-at`, an unusable scene camera or an out-of-range
    /// value is this code too.
    pub const SCENE_OVERRIDE_INVALID: DiagnosticCode = DiagnosticCode::new("L4008");

    /// `L4009` — an `image-stats --check` threshold file could not be read, is not valid TOML, or
    /// names a key that is not one of the four `ImageStats` metrics. Raised by `ludion-cli`
    /// (`ludion image-stats --check`). Unlike a profile budget's `[systems]`/`[passes]` (dynamic,
    /// producible names, so an unknown one gets its own code, `L4006`), the metric set here is a
    /// fixed four — a name outside it is simply an invalid file, not a second kind of mistake.
    pub const STATS_THRESHOLD_FILE_INVALID: DiagnosticCode = DiagnosticCode::new("L4009");

    /// `L4010` — a measured `ImageStats` metric exceeded its `--check` ceiling. Raised by
    /// `ludion-cli` (`ludion image-stats --check`), one diagnostic per breach, naming the image, the
    /// metric, the measured value, and the ceiling.
    pub const STATS_THRESHOLD_EXCEEDED: DiagnosticCode = DiagnosticCode::new("L4010");

    /// `L4011` — a supersampling render scale was not a finite number in `[1.0, 4.0]`. Raised by
    /// `ludion-renderer`'s `RenderScale::new`, which every `--scale` flag (`render`, `dump-graph`,
    /// `profile`, `serve`) routes through. Rendering *below* native is a separate feature and is
    /// rejected here rather than silently accepted.
    pub const INVALID_RENDER_SCALE: DiagnosticCode = DiagnosticCode::new("L4011");

    /// `L4012` — a `profile.capture` asked for a frame count outside the accepted range (zero, or
    /// above `MAX_CAPTURE_FRAMES`). Raised by `ludion-engine`'s capture handler.
    pub const PROFILE_CAPTURE_INVALID: DiagnosticCode = DiagnosticCode::new("L4012");

    /// `L4013` — a `profile.capture` arrived while another capture was still in flight (or its
    /// report had not yet been delivered). Raised by `ludion-engine`'s capture handler; one capture
    /// at a time is the whole rule.
    pub const PROFILE_CAPTURE_BUSY: DiagnosticCode = DiagnosticCode::new("L4013");

    /// `L4023` — a `profile.capture` cannot close its window: the sim is paused, or too slow to
    /// produce the frames within the wall-clock bound. Raised by `ludion-engine`, at arming or when
    /// a held capture becomes unclosable.
    pub const PROFILE_CAPTURE_CANNOT_CLOSE: DiagnosticCode = DiagnosticCode::new("L4023");

    /// `L4014` — a PNG offered to the sprite-document importer holds more distinct RGBA colours
    /// than a sprite document has glyphs to name them with (`SPRITE_GLYPH_BUDGET`), and no
    /// `--quantize N` was given. Raised by `ludion-sprite`'s import.
    pub const SPRITE_IMPORT_NEEDS_QUANTIZE: DiagnosticCode = DiagnosticCode::new("L4014");

    /// `L4015` — a sprite-document import ran `--quantize N` and merged colours, so the written
    /// document is not a lossless copy of its PNG. A **warning**: the import succeeded and the
    /// message reports colours before and after plus the largest per-channel error, so the loss is
    /// a number rather than a surprise. Raised by `ludion-cli`'s import path.
    pub const SPRITE_IMPORT_QUANTIZED: DiagnosticCode = DiagnosticCode::new("L4015");

    /// `L4016` — a command handler panicked. The live loop's per-command unwind boundary caught it
    /// and answered the request with this diagnostic instead of a reply; the sim is paused, the
    /// world is as the panic left it, and the command was not recorded. Raised by `ludion-engine`.
    /// Distinct from `L4007` (a system panic) and `L3023` (no reply at all).
    pub const COMMAND_PANICKED: DiagnosticCode = DiagnosticCode::new("L4016");

    /// `L4017` — the sim thread ended in a panic outside any command or system boundary. Raised by
    /// `ludion-engine`'s `live::sim_thread_panicked`, used by the server teardown and the embed
    /// host, so a host (`ludion serve`, `ludion::run`, the embed host) exits non-zero instead of
    /// reporting a clean shutdown.
    pub const SIM_THREAD_PANICKED: DiagnosticCode = DiagnosticCode::new("L4017");

    /// `L4018` — two systems share one name, or a game system took a name the engine reserves for
    /// the physics systems. Raised by `ludion-engine`'s builder at build time, and as a panic by
    /// `ludion-ecs`'s `Schedule` when a duplicate is added directly.
    pub const SYSTEM_NAME_DUPLICATE: DiagnosticCode = DiagnosticCode::new("L4018");

    /// `L4019` — a budget file gates GPU timings (`[passes]`, `[post_groups]`, `post_total_ms`,
    /// `gpu_frame_ms`, `gpu_frame_max_ms`) but the profile run measured none: no adapter, one
    /// without timestamp queries, a graph over the pass-timing ring's cap (named, untimed), or (for
    /// the frame keys) a backend that cannot prove one queue timeline. Raised by
    /// `ludion-engine`'s budget check — a gate that measured nothing must not read as a pass.
    pub const BUDGET_GPU_UNMEASURED: DiagnosticCode = DiagnosticCode::new("L4019");

    /// `L4020` — a profile report measured below full quality, with anti-aliasing on, or at a sim
    /// worker count other than the budget's `threads` key was checked against it, which refuses to
    /// compare it. Raised by the `ludion` facade's budget check.
    pub const BUDGET_QUALITY_MISMATCH: DiagnosticCode = DiagnosticCode::new("L4020");

    /// `L4021` — `ludion profile --perfetto` was given a report with no per-frame `samples` to build a
    /// timeline from. Raised by `ludion-cli`.
    pub const PROFILE_SAMPLES_MISSING: DiagnosticCode = DiagnosticCode::new("L4021");

    /// `L4022` — the sim thread did not stop within the shutdown grace period: a command, a tick or a
    /// frame render never returned. Raised by `ludion-engine`'s `live::join_sim`, used by the server
    /// teardown and the embed host, which exit non-zero and leave the thread detached.
    pub const SIM_THREAD_STUCK: DiagnosticCode = DiagnosticCode::new("L4022");

    /// `L4024` — a world hash met a NaN or infinite float in a component. Raised by `ludion-engine`'s
    /// `Engine::world_hash`, once per component type per session.
    pub const HASHED_NON_FINITE_FLOAT: DiagnosticCode = DiagnosticCode::new("L4024");

    /// `L4025` — a budget file gates `load_total_ms` but the profile report has no load section: a
    /// live capture, `--profile-replay`, or a host that observed no load. Raised by `ludion-engine`'s
    /// budget check — a gate that measured nothing must not read as a pass.
    pub const BUDGET_LOAD_UNMEASURED: DiagnosticCode = DiagnosticCode::new("L4025");

    /// `L4026` — a projectile spawn was refused: no pool is configured, the pool is full, or the
    /// description carries a non-finite or out-of-range field. A **warning**: the shot is not
    /// fired and the game continues. Raised by `ludion-engine`'s `spawn_projectile`, once per cause
    /// per session.
    pub const PROJECTILE_SPAWN_REFUSED: DiagnosticCode = DiagnosticCode::new("L4026");

    /// `L4027` — a `ProjectileConfig` is invalid: a capacity outside `1..=32768`, a non-finite
    /// gravity, or an empty or unknown `ignore_layers` name. An **error**. Raised by
    /// `ludion-engine`'s builder at `build`, by snapshot restore, and by `projectiles_step`.
    pub const PROJECTILE_CONFIG_INVALID: DiagnosticCode = DiagnosticCode::new("L4027");

    /// `L4028` — a `ProjectileVisuals` table is invalid: a non-positive or non-finite size, a
    /// negative or non-finite stretch, colour, emissive or convergence distance, or a billboard
    /// softness outside `0..=1`. An **error**. Raised by `ludion-engine`'s builder at `build`.
    pub const PROJECTILE_VISUALS_INVALID: DiagnosticCode = DiagnosticCode::new("L4028");

    /// `L4029` — `ProjectileVisuals.muzzle` names an entity that cannot anchor muzzle convergence:
    /// no entity has that `Name`, it is not a `Viewmodel` (or under one), or the frame has no
    /// first-person camera to compose it onto (or a `render.view` override is in force). A
    /// **warning**: shots draw at their simulated position, with no convergence. Latched once per
    /// name.
    pub const PROJECTILE_MUZZLE_UNRESOLVED: DiagnosticCode = DiagnosticCode::new("L4029");

    /// `L4030` — a projectile pool holds live shots, but `projectiles_step` never ran, so they
    /// never move, hit or expire and the pool fills. A **warning**, latched once per session.
    /// Raised by `ludion-engine` after a tick.
    pub const PROJECTILE_STEP_NOT_RUN: DiagnosticCode = DiagnosticCode::new("L4030");

    /// `L4031` — a snapshot restore found the build's `ProjectileConfig::capacity` below the
    /// snapshot pool's high-water mark, so the pool was not restored and starts empty. A
    /// **warning**; the rest of the restore proceeds.
    pub const PROJECTILE_RESTORE_CAPACITY: DiagnosticCode = DiagnosticCode::new("L4031");

    /// `L4032` — a `NavConfig` is invalid: a non-finite or non-positive size, a grid with no
    /// columns or more than 2^22, an empty height range, a slope outside `(0, 90)`, or an empty or
    /// unknown `ignore_layers` name. An **error**. Raised by `ludion-engine`'s builder at `build`
    /// and by `nav_update` for an unknown layer name.
    pub const NAV_CONFIG_INVALID: DiagnosticCode = DiagnosticCode::new("L4032");

    /// `L4140` — a navigation path query ran with no `Nav` resource or no grid built yet. An **error**
    /// returned to the caller of `nav_path`, `nav_distance`, `nav_reachable` or `nav_project`.
    pub const NAV_NOT_READY: DiagnosticCode = DiagnosticCode::new("L4140");
    /// `L4141` — a navigation path request is invalid: a non-finite point, a negative or NaN
    /// `max_cost`, a negative `snap_radius`, or a zero `max_expanded`. An **error** returned to the caller.
    pub const NAV_PATH_REQUEST_INVALID: DiagnosticCode = DiagnosticCode::new("L4141");
    /// `L4142` — a `PatrolFollower`'s `route` could not be loaded as a `kind: patrol` document. A
    /// **warning**, reported once per entity by `patrol_update`; the guard is skipped.
    pub const PATROL_ROUTE_UNLOADED: DiagnosticCode = DiagnosticCode::new("L4142");
    /// `L4143` — a patrol stop is unreachable on the nav grid and was skipped. A **warning**, reported
    /// once per entity and point index by `patrol_update`.
    pub const PATROL_POINT_UNREACHABLE: DiagnosticCode = DiagnosticCode::new("L4143");
    /// `L4144` — an entity carries a `PatrolFollower` but no `NavAgent`. A **warning**, reported once
    /// per entity by `patrol_update`; the patrol does not run.
    pub const PATROL_WITHOUT_AGENT: DiagnosticCode = DiagnosticCode::new("L4144");
    /// `L4145` — an end of an enabled `NavLink` snaps to no walkable node within its `snap_radius`, so the
    /// link is left out of the table. A **warning**, reported once per link per grid build.
    pub const NAV_LINK_UNSNAPPED: DiagnosticCode = DiagnosticCode::new("L4145");
    /// `L4146` — `nav_link_done` was called for an entity whose `NavRoute` is not `at_link`. An **error**
    /// returned to the caller.
    pub const NAV_NOT_AT_LINK: DiagnosticCode = DiagnosticCode::new("L4146");
    /// `L4147` — a nav agent, a path request or a `NavLink.classes` entry names an agent class that
    /// `NavConfig.classes` does not define. An **error** from the queries; a **warning** from `nav_steer`
    /// (the agent stands still) and `nav_update` (a link entry), reported once per offender.
    pub const NAV_CLASS_UNKNOWN: DiagnosticCode = DiagnosticCode::new("L4147");

    /// `L4033` — a `ProjectileLook::Mesh` asset or `ProjectileLook::Voxel` model cannot be drawn: at `build` the file does not
    /// exist (an **error**, refused with a nearest-name suggestion); at extract it failed to load
    /// or parse (reported once per asset; that tag draws nothing and the frame continues).
    pub const PROJECTILE_LOOK_ASSET: DiagnosticCode = DiagnosticCode::new("L4033");

    /// `L4034` — the scene has more non-static physics bodies (rigid and kinematic, character
    /// motors included) than the gated scale of 512. A **warning**: nothing is dropped. Raised by
    /// `ludion-engine`'s scene check (the authored count) and once per session by the post-tick
    /// latch (the live count).
    pub const PHYSICS_SCALE_UNGATED: DiagnosticCode = DiagnosticCode::new("L4034");

    /// `L4035` — a `PoseBlend` at weight above 0 names a pose that no part of its group carries. A
    /// **warning**: the parts stay limp. Raised at runtime by `ludion-engine`'s `pose_blend` system,
    /// once per (group, pose).
    pub const POSE_BLEND_POSE_MISSING: DiagnosticCode = DiagnosticCode::new("L4035");

    /// `L4250` — a `light_exposure` query is malformed: no points, more than 16, a non-finite point
    /// (`/points/<i>`), or a `Normal` facing that is zero or non-finite (`/facing`). An error
    /// returned to the caller, never a panic.
    pub const LIGHT_EXPOSURE_INVALID: DiagnosticCode = DiagnosticCode::new("L4250");

    /// `L4251` — a `PointLight`, `SpotLight` or `DirectionalLight` on a `presentation: true` entity
    /// in a scene that has a `LightExposure`: it is drawn but never counts toward exposure, because
    /// the exposure system reads sim entities only. A **warning**, raised by `check-scene`.
    pub const LIGHT_EXPOSURE_PRESENTATION_LIGHT: DiagnosticCode = DiagnosticCode::new("L4251");

    /// `L4252` — a `LightExposure`'s `occluder_layers` names no layer that any 3D-claimed collider
    /// in the scene carries (`Collider.layer`, `default` when absent, or `QueryLayers`), so nothing
    /// can occlude. A **warning**, raised by `check-scene`.
    pub const LIGHT_EXPOSURE_OCCLUDERS_UNMATCHED: DiagnosticCode = DiagnosticCode::new("L4252");

    // --- L5xxx: audio (raised by `ludion-audio`) ---

    /// `L5001` — no audio output device is available, so the playback sink cannot open a stream.
    /// This is a **warning**, never a failure: the sink plays silent and keeps retrying the open, so
    /// a headless or device-less environment (CI) runs green and a device that appears later is
    /// picked up. Raised by `ludion-audio`.
    pub const AUDIO_DEVICE_UNAVAILABLE: DiagnosticCode = DiagnosticCode::new("L5001");

    /// `L5002` — an output stream that had opened was lost while playing (an error, a stall, a
    /// gone callback, or no first callback within 5 s). Like [`AUDIO_DEVICE_UNAVAILABLE`] this is a
    /// **warning**: the sink drops the stream, keeps routing every audio event so nothing upstream
    /// blocks, and reopens the default output with backoff, which starts over once a stream has
    /// played for 5 s. One-shots above half the queue are dropped at any time, so a burst never
    /// reads as a lost device. Raised by `ludion-audio`, once per loss.
    pub const AUDIO_STREAM_FAILED: DiagnosticCode = DiagnosticCode::new("L5002");

    /// `L5003` — a music track could not be streamed: the file is missing or unreadable, is not Ogg
    /// Vorbis, failed to decode mid-stream, or has a sample rate below the floor. A **warning**,
    /// like the rest of L5xxx: the sink skips the track and the run continues. Raised by
    /// `ludion-audio`, once per file version.
    pub const MUSIC_TRACK_UNPLAYABLE: DiagnosticCode = DiagnosticCode::new("L5003");

    /// `L5004` — the `[audio]` table of `ludion.toml` is invalid: an unknown key, a value that is
    /// not an integer in `1..=256`, or a `voices_per_sound` above `voices_total`. An **error**: the
    /// manifest is refused, unlike the playback warnings of L5001–L5003. Raised by
    /// `ludion_engine::manifest`.
    pub const AUDIO_VOICE_CAPS_INVALID: DiagnosticCode = DiagnosticCode::new("L5004");

    /// `L5005` — a game asked for an audio gain that is NaN or infinite (a one-shot, a loop sync or
    /// the music). A **warning**: the queue plays that sound at gain 0 instead, so one bad volume
    /// never poisons the mix. Raised by `ludion-engine`, once per sound per session.
    pub const AUDIO_GAIN_NON_FINITE: DiagnosticCode = DiagnosticCode::new("L5005");

    /// `L5006` — a stem of a music set cannot share the set's playhead: its sample rate or length
    /// differs from the set's first playable stem, or the set names more than 8 stems. A
    /// **warning**: the stem is left out and the rest of the set plays. Raised by `ludion-audio`,
    /// once per set (again if the files change).
    pub const MUSIC_STEM_MISMATCHED: DiagnosticCode = DiagnosticCode::new("L5006");

    /// `L5100` — an `audio_duck` rule cannot run: a `depth_db` that is not finite or is above 0, a
    /// `when` bus of `Master` or `Music`, or a `duck` list that is empty or names `Master` or the
    /// `when` bus. An **error**: `build` refuses it. Raised by `EngineBuilder::build`.
    pub const AUDIO_DUCK_RULE_INVALID: DiagnosticCode = DiagnosticCode::new("L5100");
    /// `L5101` — an `audio_occlusion` config cannot run: `rays` not 1, 3 or 5, a negative or
    /// non-finite `spread_m`, a `per_hit_db` or `max_db` that is not finite or is above 0 (or a
    /// `max_db` above `per_hit_db`), an `occluded_low_pass_hz` outside the open range, a zero
    /// `interval_ticks`, or a `max_rays_per_tick` below `rays`. An **error**: `build` refuses it.
    pub const AUDIO_OCCLUSION_INVALID: DiagnosticCode = DiagnosticCode::new("L5101");
    /// `L5102` — the physics query behind audio occlusion refused the configured `QueryFilter`
    /// (an undeclared layer name). A **warning**, once per engine: every source then reads clear.
    pub const AUDIO_OCCLUSION_QUERY_REJECTED: DiagnosticCode = DiagnosticCode::new("L5102");
    /// `L5130` — `dialogue::say` named a line (or a line's speaker) no `kind: dialogue` doc defines.
    pub const DIALOGUE_UNKNOWN_LINE: DiagnosticCode = DiagnosticCode::new("L5130");
    /// `L5131` — a `world`-channel line was said without a live speaker entity.
    pub const DIALOGUE_NO_SPEAKER: DiagnosticCode = DiagnosticCode::new("L5131");
    /// `L5132` — a string had no text in the active locale and fell back to the default locale's.
    /// A warning, once per locale and key.
    pub const STRING_FELL_BACK: DiagnosticCode = DiagnosticCode::new("L5132");
    /// `L5133` — a string key is in no locale, so the key itself is shown. A warning, once per key.
    pub const STRING_MISSING: DiagnosticCode = DiagnosticCode::new("L5133");
    /// `L5134` — a string has a `{placeholder}` the caller passed no argument for; it stays
    /// literal. A warning, once per placeholder.
    pub const STRING_ARG_MISSING: DiagnosticCode = DiagnosticCode::new("L5134");
    /// `L5135` — `dialogue::say_sequence` got an empty sequence or a speakers list of another length.
    pub const DIALOGUE_SEQUENCE_INVALID: DiagnosticCode = DiagnosticCode::new("L5135");
    /// `L5136` — a dialogue call was made on an engine that did not opt in with `.dialogue()`.
    pub const DIALOGUE_NOT_ENABLED: DiagnosticCode = DiagnosticCode::new("L5136");
}

/// One row of the diagnostic-code table: the code, its severity, a short title, and a long-form
/// explanation.
///
/// Obtained by lookup ([`explain`]/[`explain_code`]), never constructed downstream: the struct is
/// `#[non_exhaustive]`, so a future column is an additive, non-breaking change. Its fields stay
/// `pub` for reading.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CodeEntry {
    /// The stable `L####` code.
    pub code: DiagnosticCode,
    /// Whether raising this code is a hard failure or a lint. This row is the **source of truth**:
    /// [`crate::diagnostics::Diagnostic::new`] reads it, so two call sites cannot disagree about
    /// whether one code is fatal. `grep 'Severity::Warning' codes.rs` lists every lint.
    pub severity: Severity,
    /// A short one-line title.
    pub title: &'static str,
    /// The long-form explanation shown by `ludion explain`.
    pub explanation: &'static str,
}

/// The complete diagnostic-code table for `ludion-core`.
///
/// The `L0xxx` range is shared by `ludion-core` and `ludion-ecs` (architecture doc §2). ECS codes
/// are registered here, alongside core's, because the table is deliberately centralized rather than
/// scattered across crates.
pub const CODES: &[CodeEntry] = &[
    CodeEntry {
        code: code::UNKNOWN_DIAGNOSTIC_CODE,
        severity: Severity::Error,
        title: "unknown diagnostic code",
        explanation: "\
The code you asked about is not registered in the engine's diagnostic-code table.

Every engine diagnostic carries a stable `L####` code (an `L` followed by four digits) that names
its class and never changes, so tools, tests, and agents can match on it. Codes are partitioned by
domain on their first digit:

  L0xxx  core / ecs
  L1xxx  scene / assets
  L2xxx  render
  L3xxx  protocol / server
  L4xxx  sim
  L5xxx  audio

The full table lives in one place — `ludion-core`'s `codes.rs` — and `ludion explain L####` is a
lookup into it. If you reached this message, the code was either mistyped or belongs to a newer
engine version than the one you are running. Check the spelling and the engine version; run
`ludion explain` with no argument to list every registered code.",
    },
    CodeEntry {
        code: code::DEAD_ENTITY,
        severity: Severity::Error,
        title: "entity is not alive",
        explanation: "\
An ECS operation named an entity that is not currently alive.

An `EntityId` is a generational handle: a slot index plus the generation that owned it. A handle is
valid only while it names the live entity that currently occupies its slot. It becomes permanently
invalid the moment that entity is despawned — even if the slot is later reused, the reused entity
gets a bumped generation, so the old handle no longer matches. This is deliberate: it turns
use-after-despawn into a clean, reported error instead of a silent read or write against an
unrelated entity that happened to inherit the slot.

You will see this when you `get`, `get_mut`, `insert`, `remove`, or `despawn` with a handle to an
entity that has already been despawned (a stale handle), or one that never existed. Fix: only use
`EntityId`s returned by the current `World::spawn`; drop any handle after you despawn the entity it names.",
    },
    CodeEntry {
        code: code::UNKNOWN_COMPONENT,
        severity: Severity::Error,
        title: "unknown component name",
        explanation: "\
A lookup named a component that is not registered in the component registry.

The registry maps component names to their JSON Schema and to the erased serde functions that power
scene IO, snapshots, the inspector, and protocol validation. A name that is not present cannot be
serialized, deserialized, or validated. This usually means a scene or wire payload referenced a
component the running engine build does not define, or the name was misspelled.

Fix: register the component with `Registry::register::<T>(name)` before referring to it, and check
the spelling against the registry's `iter()` (which lists names in registration order).",
    },
    CodeEntry {
        code: code::DUPLICATE_COMPONENT_REGISTRATION,
        severity: Severity::Error,
        title: "duplicate component registration",
        explanation: "\
A component registration collided with one that already exists.

Each component occupies exactly one row in the registry, keyed by both its name and its Rust type.
Registering a second component under a name already in use, or registering the same Rust type a
second time under any name, is rejected: the registry's order and name set are load-bearing (they
define the canonical key order for scene files and the world hash), so silent overwrites are not
allowed.

Fix: choose a unique name, and register each component type exactly once.",
    },
    CodeEntry {
        code: code::COMPONENT_DESERIALIZE_FAILED,
        severity: Severity::Error,
        title: "component failed to deserialize",
        explanation: "\
A component value could not be deserialized from JSON while being inserted into the world.

The registry's erased deserialize function ran `serde` against the supplied value and it failed —
typically a missing required field, a wrong field type, or an unknown variant. The message names the
component and includes the underlying serde error.

Fix: compare the value against the component's JSON Schema (available from the registry). The schema
lists the expected field names, types, and which fields are required.",
    },
    CodeEntry {
        code: code::COMPONENT_SERIALIZE_FAILED,
        severity: Severity::Error,
        title: "component failed to serialize",
        explanation: "\
A component value could not be serialized to JSON while being read out of the world.

The registry's erased serialize function ran `serde` against a live component and it failed. This is
rare — it means the component's own `Serialize` implementation errored (for example, a map with
non-string keys, or a custom serializer that rejects some state). The message names the component
and includes the underlying serde error.

Fix: make the component's serialized form JSON-compatible; the same shape must round-trip through the
scene format and the wire protocol.",
    },
    CodeEntry {
        code: code::DOCS_DIR_NOT_FOUND,
        severity: Severity::Error,
        title: "agent-docs directory not found",
        explanation: "\
`ludion docs` or `ludion devdocs` needed a documentation directory at the repository root — `docs/`
or `dev-docs/` — and could not use it.

There are two roots, one per verb: `docs/` holds the topics for building a game *with* Ludion
(`ludion docs`), `dev-docs/` the engine-internals topics for working on Ludion itself (`ludion
devdocs`). Both are discovered by walking up from the current working directory, looking for an
engine checkout root — a directory that contains both `crates/ludion-core/Cargo.toml` and `docs/`.
The marker is a path inside the engine's own workspace on purpose: it cannot be produced by
accident, so a downstream project that happens to have a `docs/` folder is never mistaken for the
engine. Three situations raise this code:

  - Any form of `ludion devdocs` ran outside an engine checkout. Engine-internals docs are
    deliberately not baked into the binary — nothing in them can be acted on without the engine's
    source — so outside a checkout there is nothing to list, print, or point at.
  - `ludion docs <topic> --path` ran outside an engine checkout. No ancestor of the working
    directory is an engine checkout, so there is no file whose path could be printed. (Listing and
    plain lookup do not need one: outside a checkout they are served from the docs baked into the
    binary at build time.)
  - The directory was found at the discovered root but could not be read — a permissions problem,
    or something other than a directory in its place.

Fix: for the first two, run the command from inside an engine checkout (any subdirectory works —
discovery walks up), or, for `--path` alone, drop the flag to get the doc text. For the third, the
message names the directory and the underlying IO error; make it a readable directory.",
    },
    CodeEntry {
        code: code::DOCS_UNKNOWN_TOPIC,
        severity: Severity::Error,
        title: "unknown documentation topic",
        explanation: "\
`ludion docs` was asked for a topic that has no doc file.

Topics are not a registry: the `docs/` directory *is* the index. Each topic is one kebab-case
`.md` file in `docs/` at the repo root, and the topic name is exactly the filename stem
(`docs/determinism.md` is the topic `determinism`). A name that matches no file therefore means a
typo, a topic that does not exist yet, or a doc written for a different checkout.

Fix: run `ludion docs` with no argument to list every topic with its one-line description. When
the name is plausibly a typo, this diagnostic attaches a nearest-match suggestion. If the topic
genuinely does not exist yet, create `docs/<topic>.md` (the `ludion-docs` skill has the template).",
    },
    CodeEntry {
        code: code::DOCS_MALFORMED,
        severity: Severity::Error,
        title: "topic doc has malformed frontmatter",
        explanation: "\
A topic doc file in `docs/` has missing or malformed frontmatter, so its one-line description
cannot be read.

Every topic doc opens with strict frontmatter — exactly this grammar, nothing more:

  line 1:  ---
  line 2:  description: <one line, <=100 chars>
  line 3:  ---

`description` is the ONLY allowed key. The grammar is deliberately rigid so that a ~30-line parser
(not a YAML library) can read it, agents can write it without ambiguity, and `ludion docs` can list
every topic with a trustworthy one-liner. Anything else — a missing fence, an extra key, a
multi-line or over-long description, an empty value — is rejected rather than half-parsed.

Fix: edit the named file so its first three lines match the grammar above exactly, then rerun
`ludion docs` to confirm the topic lists cleanly. Note that a malformed doc never blocks
`ludion docs <topic>` path lookup — it surfaces when listing (and in CI).",
    },
    CodeEntry {
        code: code::NEW_TARGET_IN_WORKSPACE,
        severity: Severity::Error,
        title: "cannot scaffold inside the engine workspace",
        explanation: "\
`ludion new` refused to scaffold a project inside the engine checkout's Cargo workspace member
globs.

The engine workspace's root `Cargo.toml` declares `members = [\"crates/*\", \"games/*\"]`. A new
project scaffolded under `crates/` or `games/` would carry its own `[workspace]` table while also
matching a member glob of the outer workspace — Cargo rejects a package that belongs to two
workspaces, so neither the engine nor the new project would build.

Fix: create the project somewhere outside the engine checkout — its parent directory, a sibling of
the engine repo, or any path that is not under the engine's `crates/` or `games/`. From a checkout
the generated project links `.ludion/engine` to this engine and its `Cargo.toml` depends on the
engine through that link, so it builds against the engine from anywhere on disk.",
    },
    CodeEntry {
        code: code::ENGINE_LINK_MISSING,
        severity: Severity::Error,
        title: "the project's engine link is missing",
        explanation: "\
A project scaffolded from an engine checkout reached for `.ludion/engine` and found nothing usable
there: the link is missing, dangling (the checkout moved or was deleted), or points at a directory
that is not an engine checkout.

The project's `Cargo.toml` depends on the engine through `.ludion/engine`, a gitignored per-machine
link to this machine's engine checkout, so the committed project names no absolute path. `ludion
new` creates the link; a fresh clone on another machine does not have one.

Fix: run `node scripts/ludion.mjs link-engine <engine checkout>` from the project root, once per
machine.",
    },
    CodeEntry {
        code: code::ENGINE_LINK_TARGET_INVALID,
        severity: Severity::Error,
        title: "not an engine checkout",
        explanation: "\
`node scripts/ludion.mjs link-engine <path>` was given a path that is not an engine checkout: it has
no `Cargo.toml` at its root, or no `crates/ludion/Cargo.toml`.

The link must point at the root of an engine checkout (the directory holding the workspace
`Cargo.toml` and `crates/`), and the existing link is left unchanged.

Fix: pass the engine checkout's root directory.",
    },
    CodeEntry {
        code: code::ENGINE_LINK_OCCUPIED,
        severity: Severity::Error,
        title: "`.ludion/engine` is not a link",
        explanation: "\
`node scripts/ludion.mjs link-engine` found `.ludion/engine` already present and not a link — a real
directory or file. The wrapper only ever replaces a link and never deletes real content.

Fix: move or delete `.ludion/engine` yourself, then rerun `link-engine`.",
    },
    CodeEntry {
        code: code::NEW_ENGINE_LINK_FAILED,
        severity: Severity::Error,
        title: "could not link the engine",
        explanation: "\
`ludion new` wrote the project's files but could not create its `.ludion/engine` link: Node is not
installed or not on the PATH, or `scripts/ludion.mjs link-engine` failed. The files stay in place.

Fix: install Node, then run `node scripts/ludion.mjs link-engine <engine checkout>` from the project
root.",
    },
    CodeEntry {
        code: code::DEV_NOT_A_GAME,
        severity: Severity::Error,
        title: "not a runnable game project",
        explanation: "\
`ludion dev` was run from a directory that is not a runnable game project.

`ludion dev` is the hot-reload loop: it watches a game's `src/`, rebuilds on every change, and
restarts the running game with its state snapshotted across the restart. It runs *from the game's
own directory* — the same place `cargo run` would host the game — with no `-p` package selection, so
it works identically for a standalone `ludion new` project and a workspace member under `games/`.

For that to work, the current directory must be a Cargo package with a binary to run. This
diagnostic means the check failed: there is no `Cargo.toml` in the current directory, or the
`Cargo.toml` declares no binary target (no `src/main.rs` and no `[[bin]]` table). A Ludion game is a
binary whose `main` is `ludion::run(your_game())`.

Fix: `cd` into the game's project directory (the one containing its `Cargo.toml` and `src/main.rs`)
before running `ludion dev`, or scaffold one with `ludion new <name>`.",
    },
    CodeEntry {
        code: code::DEV_CHILD_FAILED,
        severity: Severity::Error,
        title: "dev game process failed",
        explanation: "\
`ludion dev` started the game as a child process, but it failed before it began serving.

After each successful build, `ludion dev` launches the freshly built game and waits for its
machine-parseable `listening on ws://…` line before connecting to snapshot or restore. This
diagnostic means the child never got there: it exited before printing that line, or exited with a
non-zero status. The child's captured output (stderr, and any stdout) is included so the real cause
is visible — a panic on startup, a scene that failed to load, or a bind failure on the requested
address.

A child that is still running but prints no listening line within 20 s is stopped and reported the
same way, worded as a timeout: on a loaded machine that is a slow boot, not a crash — retry on a
quieter one, and run the game directly if it persists. A child whose output closes with no listening
line, or that cannot be started at all, is worded for that cause.

When the output shows a bind failure, the cause is a same-port bind race on a *reload*: the previous
game had not fully released the port when the new one tried to bind it (a `TIME_WAIT` socket, or the
old process not yet gone). `ludion dev` already retries the bind for a short window; this diagnostic
means the retries were exhausted, so the failure is a real one, not a transient race. Any other
failure (a panic, a scene that no longer loads) is not retried at all.

Fix: read the included child output for the underlying error. If it is an address-in-use bind
failure that persists, another process is holding the port — stop it, or run `ludion dev` with a
different `--addr`. If it is a panic or a scene-load error, fix that in the game and save again; a
failed build never kills the last good binary, and a child that builds yet cannot boot does not end
the loop either: `ludion dev` keeps the snapshot and retries the launch and restore on the next save.",
    },
    CodeEntry {
        code: code::THREADS_ENV_INVALID,
        severity: Severity::Error,
        title: "LUDION_THREADS is not a worker count",
        explanation: "\
The `LUDION_THREADS` environment variable holds a value that cannot be a worker count.

`LUDION_THREADS` is the per-machine sim parallelism knob: thread count is a property of the box, not
of the project, so it lives in the environment rather than in any committed file. Set it once per
machine (a shell profile, the user environment) to a positive integer, or to `auto` to take the
machine's available parallelism. Unset means `auto`.

A host refuses to start rather than ignoring the value, because a silently-ignored setting means a
process running at a worker count nobody asked for — and the whole point of the variable is that the
count is the one you chose. Note that results never depend on it: the sim is bit-identical at any
worker count, so this changes throughput and nothing else.

Fix: set it to a positive integer (`LUDION_THREADS=4`), to `auto`, or unset it. A host's `--threads`
flag beats the variable outright and is not validated against it, so it works even on a machine whose
`LUDION_THREADS` is malformed.",
    },
    CodeEntry {
        code: code::DOCS_UNKNOWN_SECTION,
        severity: Severity::Error,
        title: "unknown documentation section",
        explanation: "\
`ludion docs <topic> --section <heading>` found no ATX heading in that topic matching the requested
one.

`--section` fetches one heading's slice of a topic doc instead of the whole file — useful once a
topic runs long enough that reading all of it wastes an agent's context. A heading match is an ATX
heading (`#` through `######`, followed by a space) whose trimmed text equals the requested one,
case-insensitively; headings inside fenced code blocks (```` ``` ```` or `~~~`) are not counted, so a
YAML comment that happens to start with `#` inside an example never misreads as a section. The first
match wins if a heading's text repeats.

Fix: check the spelling and case against the topic's actual headings — this diagnostic suggests the
nearest one when the name is plausibly a typo, and lists the topic's top-level headings so you can
pick a real one. `ludion docs <topic>` with no `--section` still prints the whole doc.",
    },
    CodeEntry {
        code: code::QUALITY_SETTING_INVALID,
        severity: Severity::Error,
        title: "quality setting is neither a preset nor an override file",
        explanation: "\
A render quality setting names something that cannot be resolved to a quality.

A quality setting is either one of the preset names (`low`, `medium`, `high`, `full`) or the path to
a TOML override file. A value that is not a preset is therefore read as a path, and this diagnostic
means neither reading succeeded: the name is not a preset and the file could not be read, or it was
read and is not a valid override file — malformed TOML, an unknown key or section, or a value of the
wrong type.

An override file is a base preset plus per-effect overrides, and every key is optional:

    preset = \"medium\"

    [effects]
    ssao = false
    shafts = true
    dof = false

Unknown keys and unknown sections are rejected rather than ignored, so a file written for a build
that knows more effects than this one fails here instead of silently rendering at a quality it does
not describe.

Like `LUDION_THREADS`, quality is a per-machine knob — how much GPU a box is willing to spend, not a
property of the project — so it lives in the environment (`LUDION_QUALITY`) or on a host's
`--quality` flag, never in the scene. A host refuses to start rather than falling back to a default,
because a silently-ignored setting means rendering at a quality nobody chose. Nothing about the
simulation depends on it: the `WorldHash` is identical at every quality, so this changes pixels and
frame cost and nothing else.

Fix: set it to one of the preset names, or to the path of an override file that exists and parses.
A host's `--quality` flag beats `LUDION_QUALITY` outright and is not validated against it, so it
works even on a machine whose variable is malformed.",
    },
    CodeEntry {
        code: code::RENDER_SIZE_INVALID,
        severity: Severity::Error,
        title: "render extent is not a WIDTHxHEIGHT pixel pair",
        explanation: "\
A render extent names something that cannot be a resolution.

An extent is `WIDTHxHEIGHT` in whole pixels — `960x540` — separated by a lowercase ASCII `x`, and
there is no second form. Both axes run from 1 pixel up to 7680 x 4320: past that the readback ring's
three staging buffers and the frame graph's physical cache stop being a stream and become an
allocation failure several layers below the place that could explain it. This diagnostic means the
value had no separator, used the wrong one (the unicode multiplication sign reads identically to a
human and is refused deliberately, as is a capital `X`), named something that is not a whole number,
or named an axis outside that range.

Like `LUDION_THREADS` and `LUDION_QUALITY`, the extent is a per-machine knob — every stage of the
frame path (GPU readback, the unpad copy, the per-format encode, the socket write, the viewer's
decode and blit) costs in proportion to the pixel count, so how many pixels a box is willing to move
is a property of the box, not of the project. It lives in the environment (`LUDION_RENDER_SIZE`) or
on a host's `--render-size` flag, never in the scene. A host refuses to start rather than falling
back to the default, because a silently-ignored setting means streaming at a resolution nobody chose.
Nothing about the simulation depends on it: the `WorldHash` is identical at every extent.

One rule travels with the knob. A game's mouse input arrives as *render-target pixel* positions, so
a session played at 960x540 and the same session played at the default extent are different input
traces. Each still replays bit-identically, but they are not comparable — so committed golden-session
recordings (`games/*/run.lrp`) are made at the default extent: never record one from a resized host.

Fix: set it to `WIDTHxHEIGHT` in whole pixels (`960x540`), or unset it to keep the engine's default
extent. A host's `--render-size` flag beats `LUDION_RENDER_SIZE` outright and is not validated
against it, so it works even on a machine whose variable is malformed.",
    },
    CodeEntry {
        code: code::WINDOW_FEATURE_OFF,
        severity: Severity::Error,
        title: "--window asked of a build with no native viewer",
        explanation: "\
A host was asked to open its native window, but this build has no window to open.

The `ludion` facade's `window` cargo feature carries `ludion-window` — the winit + gilrs viewer that
`run --window` opens beside the listener. It is on by default, so a game built from a checkout has
it. A build for a device with no desktop turns it off (`default-features = false`, which also drops
`wire`, the listener) and hosts through `ludion::launch` with neither; `--window` in such a build
cannot be honoured. The host refuses before it binds or plays anything rather than serving headless
without the window: a flag that asked for a viewer and got none would be surprising, and the
listener-only session left behind is one nobody asked for.

Fix: build the game with the facade's default features (a plain `cargo build` from a checkout does),
or drop `--window` and attach a viewer — the editor, or any protocol client — to the `listening on`
address the host prints.",
    },
    CodeEntry {
        code: code::NO_DEFAULT_SCENE,
        severity: Severity::Error,
        title: "no scene to host: the engine builder has no default scene",
        explanation: "\
A host was given no scene: no `--scene` on the command line (`ludion::run`) and no `default_scene`
on the engine builder, or a host that takes no scene argument handed a builder with none.

`ludion::run` reads the scene from `--scene`, falling back to the builder's default. A packaged host
— `ludion::launch`, or the embedding core a native app starts — has no command line, so the builder's
`default_scene` (resolved against the content root) is the only place the scene can come from. With
none set there is nothing to load, and the host refuses before building anything.

Fix: call `EngineBuilder::default_scene(\"scenes/<name>.yaml\")` on the game's builder, with the path
relative to the content root the host is given.",
    },
    CodeEntry {
        code: code::EMBED_ARGUMENT_INVALID,
        severity: Severity::Error,
        title: "the embedding C ABI was handed an unusable argument",
        explanation: "\
A native host called into the embedding core's C ABI (`ludion::ffi`) with an argument it cannot use.

The C layer validates everything it is handed before touching the engine: the content and save roots
must be non-null, NUL-terminated UTF-8 paths; `surface_kind` must be one of the named constants
(`LUDION_SURFACE_NONE`, `LUDION_SURFACE_UIKIT`); a UIKit surface needs a non-null `view`; and a pad
button or axis index must be one the table names (`GamepadButton` / `GamepadAxis` declaration
order), and a touch phase one of `LUDION_TOUCH_BEGAN`..`LUDION_TOUCH_CANCELLED`. A refused call
does nothing — a start returns null, a forwarder returns false — and the diagnostic goes to
stderr.

Fix: pass valid UTF-8 C strings for both roots, a known surface kind with its view, and pad indices
within the tables in the `ludion::ffi` module docs, and a known touch phase.",
    },
    CodeEntry {
        code: code::TOUCH_LAYOUT_INVALID,
        severity: Severity::Error,
        title: "a touch layout names a control the mapper cannot use",
        explanation: "\
The `touch.yaml` beside the hosted scene (`kind: touch-layout`) describes the on-screen controls a
native host draws and the touch mapper reads. A control in it cannot be used as written: a deadzone
outside `[0, 1)`, a flick threshold not above its stick's deadzone, a size that is not positive, a
negative offset, an empty label, or a stick side or a pad button named by two controls.

The layout is checked before the engine is built, so a malformed file refuses the start rather than
leaving a device with dead controls. The diagnostic's path is a JSON Pointer into the document
(`/sticks/0/deadzone`); a file the mapper cannot read at all is reported against its name.

Fix: edit the named field. Sizes and offsets are fractions of the safe area's height, `deadzone` is
in `[0, 1)`, and `flick`, when present, lies above `deadzone` and at most `1`.",
    },
    CodeEntry {
        code: code::EXPORT_TOOLCHAIN_UNAVAILABLE,
        severity: Severity::Error,
        title: "the iOS export toolchain is not available",
        explanation: "\
`ludion export --target ios` (or `ios-sim`) builds the game as a static library for Apple's iOS
target and lays it into an Xcode project. Two things must exist on the machine first: Xcode, whose
SDK for the target (`iphoneos` for a device, `iphonesimulator` for the simulator) the C dependencies
compile against — `xcrun --sdk <sdk> --show-sdk-path` must answer — and the rustup target
(`aarch64-apple-ios` or `aarch64-apple-ios-sim`) the Rust standard library is built for.

The verb probes both before it builds anything, so a machine without them refuses at once and
leaves the output folder untouched.

Fix: install Xcode (and run it once so its command-line tools are selected with `xcode-select`),
and `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`.",
    },
    CodeEntry {
        code: code::MANIFEST_INVALID,
        severity: Severity::Error,
        title: "the project manifest cannot be used",
        explanation: "\
`ludion.toml` beside the game's `Cargo.toml` names the project (`name`), the scene the game hosts
by default (`default_scene`, a path with a directory — its directory is the game's content), and,
for an iOS export, its bundle identifier (`[ios] bundle_id`). Each `[[import]]` table (`from`, a
glob of glTF files; `to`, the derived tree) is what `ludion sync` compiles.

The verb refused the manifest: the file is missing or unreadable, it is not valid TOML, it holds a
key the manifest does not define, a required key is absent, `name` holds a character
outside `A-Z a-z 0-9 _ -`, `default_scene` is absolute, holds a `.` or `..` component or has no
parent directory, an `[[import]]` table's `from` or `to` is absolute, empty, holds a `.` or `..`
component, or two tables share a `to`, or one table's `to` lies inside another's, or a `to` lies
inside (or holds) a `from` directory, the part of the glob before its first `*` (each sync would
rewrite files the next one hashes, so the tree is never fresh), or a `[cook] roots` entry is spelled
like those paths, repeats or nests another root, or overlaps a table's `to` (a path has one owner), or the bundle
identifier holds a character outside `A-Z a-z 0-9 . -`. The diagnostic's help names the key.

Fix: edit `ludion.toml`. A bundle identifier is reverse-DNS (`com.example.mygame`); a project name
may hold `_`, a bundle identifier may not (use `-`). Pick your own identifier before the first
device export and keep it: a free Apple team registers only a few app IDs a week.",
    },
    CodeEntry {
        code: code::EXPORT_FLAGS_INVALID,
        severity: Severity::Error,
        title: "export flags that cannot go together",
        explanation: "\
`ludion export` refused its command line before reading the manifest or building anything.

Three things are refused: `--run` with `--target ios` (only the simulator is launched from the
command line; a device build is installed from Xcode), `--replay <file>` or arguments after `--`
without `--debug` (only a build with `wire` reads app arguments; the arguments also need `--run`),
and a `--replay` path that is not a file.

Fix: drop the flag, or add the one it needs — `--target ios-sim --run` to launch, `--debug` for
replays and app arguments. For a device, open the exported project in Xcode.",
    },
    CodeEntry {
        code: code::ENTITY_ALLOCATOR_STATE_INVALID,
        severity: Severity::Error,
        title: "entity allocator state is inconsistent",
        explanation: "\
A world was being restored from an entity allocator state or live set that cannot describe a real
allocator: a free-list index appears twice, a free-list index is past the generation table, or two
live entities share one slot index.

The restore is refused because either shape would alias two entities onto one slot, or panic on
the next spawn when the allocator hands out an index that has no slot. Nothing was changed.

This comes from a hand-edited or foreign snapshot; an engine-written snapshot never carries it.

Fix: re-take the snapshot with `engine.snapshot` rather than patching the text.",
    },
    CodeEntry {
        code: code::LIFTED_COMPONENT_READ,
        severity: Severity::Error,
        title: "component read while its column is lent to a parallel region",
        explanation: "\
A system read a component through `ctx.world` that the enclosing `par_for_each` took `&mut`.

Inside a `par_for_each` region the exclusive component columns are lifted out of the world, so each
chunk can hold a `&mut` slice of its column while every closure also holds a `&World`. A lifted
column is absent from that `&World` for the whole region: `World::get` on it returns this error
(it would otherwise answer `Ok(None)`, indistinguishable from \"the entity has no such component\"),
and `World::has` panics with this text, because a `bool` cannot carry a diagnostic.

Fix: read the component through the row item the closure already receives, read a different
component, or make the system sequential (`world.query` / `get_mut`).",
    },
    CodeEntry {
        code: code::COMPONENT_NOT_REGISTERED,
        severity: Severity::Warning,
        title: "component type has values but is not registered",
        explanation: "\
A component type holds at least one value in the world, but it is not in the component registry.

The world hash, snapshots, `scene.save` and the wire all walk the registry, so values of an
unregistered type are invisible to every one of them: they do not move the `WorldHash`, are lost by a
snapshot/restore round-trip and never reach a saved scene or a protocol client. The message names the
Rust type. It is reported once per type per session, when the engine drains diagnostics at a frame
boundary.

Fix: register the type with `EngineBuilder::components` so it is hashed, snapshotted and saved, or
keep it as a resource if it is scratch state rather than per-entity state.",
    },
    CodeEntry {
        code: code::OPERATION_FAILED_SILENTLY,
        severity: Severity::Error,
        title: "operation failed without a diagnostic",
        explanation: "\
An engine operation failed but produced no diagnostic to report. This is an engine bug.

The request did fail; nothing in your input is known to be wrong.

Fix: report it with the command that triggered it.",
    },
    CodeEntry {
        code: code::CREATE_TARGET_EXISTS,
        severity: Severity::Error,
        title: "a create verb's target already exists",
        explanation: "\
`ludion new <name>` scaffolds `./<name>`; `ludion voxel new`, `ludion voxel upscale` and
`ludion sculpt new` write a new model file; `ludion import <png>` writes the `kind: sprite`
document beside the image. The path they were given already exists. A glTF `ludion import` raises it
too when its `--out` holds `meshes/`, `clips/` or `textures/` files the new import would not write.

These verbs never overwrite: merging a template into a project someone already has, or replacing a
hand-edited model, would destroy work silently, and a half-scaffolded tree is harder to recover from
than a refusal.

Fix: choose a path that does not exist yet, or remove the existing file or directory first.",
    },
    CodeEntry {
        code: code::KIND_CHECKED_ELSEWHERE,
        severity: Severity::Error,
        title: "`check-scene` does not validate this document kind",
        explanation: "\
`ludion check-scene` was handed a document whose `kind:` is validated by a different verb.

Scene validation covers scenes and the kinds `check-scene` knows. Player profiles, maps, map kits,
touch layouts and projectile patterns are read by another verb or host, with their own schema, and
running them through the scene validator would report nonsense or, worse, a clean pass over a
document it never read.

Fix: the diagnostic's help names the verb that checks this kind; run that instead.",
    },
    CodeEntry {
        code: code::DEV_CARGO_FAILED,
        severity: Severity::Error,
        title: "`ludion dev` could not run or wait on `cargo build`",
        explanation: "\
`ludion dev` builds the game by spawning `cargo build`, and the process could not be started or
could not be waited on. A build that ran and failed is different: cargo prints its own errors.

The usual cause is `cargo` not being on the PATH of the shell that ran `ludion dev`, or the process
being killed from outside.

Fix: check that `cargo --version` works in the same shell, then rerun `ludion dev`.",
    },
    CodeEntry {
        code: code::DEV_DEGRADED,
        severity: Severity::Warning,
        title: "`ludion dev` continues with reduced function",
        explanation: "\
`ludion dev` hit a problem that does not stop it but costs it something: the status file or the kept
state snapshot could not be written under the project's `.ludion/` directory, or the build
fingerprints that tell it whether a rebuild is needed were not found, so it falls back to cargo's
own freshness check, which can serve a binary built before your edit.

Fix: the message names which. For the first two, make the project's `.ludion/` directory writable.
For the fingerprints, export `CARGO_TARGET_DIR` (or `CARGO_BUILD_TARGET`) in the shell that runs
`ludion dev` so it looks where cargo builds.",
    },
    CodeEntry {
        code: code::EXPORT_FAILED,
        severity: Severity::Error,
        title: "an export step failed",
        explanation: "\
`ludion export` failed in a step that is not about the manifest or the toolchain: resolving the
output location, building the game's static library, or running the simulator.

When the staticlib build failed, cargo printed its own errors above this one. Nothing is left half
written that a rerun does not overwrite.

Fix: read the message for the step that failed and fix its cause; a failed staticlib build is fixed
in the game's code, then rerun `ludion export`.",
    },
    CodeEntry {
        code: code::NEW_NAME_INVALID,
        severity: Severity::Error,
        title: "`ludion new` name would scaffold a broken project",
        explanation: "\
`ludion new <name>` uses the name as the directory, the Cargo package and binary name, the library
identifier `src/main.rs` calls (`<name with - as _>::game()`), the iOS bundle id and the `name` in
`ludion.toml`. A name that breaks any of those scaffolds a project that does not build.

The rule: only ASCII letters, digits, `-` and `_` (the charset `ludion.toml` accepts); the first
character an ASCII letter; and not a name that is reserved — a Rust keyword once `-` becomes `_`
(`fn`, `gen`, `self`, ...), a name Cargo refuses or that collides with `target/` (`test`, `std`,
`core`, `deps`, `build`, ...), or a Windows device name (`con`, `nul`, `com1`, ...).

Fix: use the suggested name from the help, or any name matching the rule.",
    },
    CodeEntry {
        code: code::GUIDE_STALE,
        severity: Severity::Warning,
        title: "project's `LUDION-GUIDE.md` is not this CLI's engine guide",
        explanation: "\
`LUDION-GUIDE.md` in a project is a copy of the engine guide, written when `ludion new` scaffolded
the project. `ludion docs` compares it with the guide baked into the CLI that is running, and warns
when they differ: the engine has moved on since the copy was made, or the copy was edited. Nothing
else reads the file. `CLAUDE.md` is the game's own and is never compared.

Fix: run `ludion new --refresh-guide` from the project (it rewrites only `LUDION-GUIDE.md`), or read
the current guide with `ludion docs engine-guide`.",
    },
    CodeEntry {
        code: code::FULLSCREEN_SETTING_INVALID,
        severity: Severity::Error,
        title: "`LUDION_FULLSCREEN` is not `1` or `0`",
        explanation: "\
`LUDION_FULLSCREEN` chooses whether a game window opens borderless fullscreen: `1` opens fullscreen,
`0` opens windowed, and an empty or unset variable defers to the game's own default (the builder's
`fullscreen`). Anything else is refused, and the window host refuses to start rather than guess,
because a silently ignored setting means a window nobody asked for.

Like `LUDION_THREADS` and `LUDION_RENDER_SIZE`, it is a per-machine knob and never reaches the
simulation, the `WorldHash` or a snapshot. A host's `--fullscreen` flag beats the variable outright
and is not validated against it, so it works even on a machine whose variable is malformed.

Fix: set the variable to `1` or `0`, or unset it.",
    },
    CodeEntry {
        code: code::DEFAULT_SCENE_MISMATCH,
        severity: Severity::Error,
        title: "the builder's default scene and the manifest's `default_scene` differ",
        explanation: "\
A game names its default scene in two places that different readers use. The engine builder's
`.default_scene(...)` in `game()` is read by the game itself: a desktop run and the iOS boot. The
`default_scene` in the content root's `ludion.toml` is read by `ludion export` and the editor. When
the two name different files, the game opens one scene while the export and the editor assume
another, and the mismatch would otherwise surface only on a device.

The host compares them at boot, and `ludion export` runs the same check before the iOS build. The
manifest is looked up at the content root exactly, never in an ancestor directory, and a builder with
no default scene or a content root with no `ludion.toml` has nothing to compare. The check applies
even when `--scene` is passed.

Fix: make `.default_scene(...)` in `game()` and `default_scene` in `ludion.toml` name the same file.",
    },
    CodeEntry {
        code: code::PRESENTATION_BOUNDARY,
        severity: Severity::Error,
        title: "presentation boundary crossed",
        explanation: "\
A system crossed the boundary between sim state and presentation state.

Presentation entities (`World::spawn_presentation`, `presentation: true` in a scene) live in their own
id range and are never hashed. Every system runs in a phase, and the world refuses the crossing in
both directions, so presentation code cannot move the `WorldHash` and sim code cannot depend on
anything unhashed:

- A sim system cannot see a presentation entity: `get`, `get_mut`, `insert`, `remove` and `despawn`
  on one return this error, `has` answers `false`, queries skip them, and `spawn_presentation`
  panics with this text.
- A presentation system cannot write sim state: `get_mut`, `insert`, `remove` and `despawn` on a sim
  entity return this error, and `spawn` panics with this text. Sim entities stay readable.
  `resource_mut`, `insert_resource` and `remove_resource` panic with this text too, unless the type
  is an allowlisted outbox (`World::allow_presentation_writes`).

The engine also logs every refused `get`, `get_mut`, `insert`, `remove` and `despawn` once per
system, op and component, even when the caller discards the `Result` or reads through
`World::component`; `ludion_testkit::assert_no_boundary_crossings` fails a test on any.

A `bool`, `Option` or bare `EntityId` cannot carry a diagnostic, so those calls panic; the schedule
reports the panic as `L4007`, naming the system. Every message names the system, the entity (its id,
plus its `Name` when it has one), the call and component it went through, and the direction crossed.

Fixes, by direction:
- A sim system reads or writes a presentation entity: if the sim must read it, make it a sim entity
  (spawn it with `World::spawn`, or drop `presentation: true`); a camera the sim unprojects through
  must be a sim entity. Otherwise move the logic into a presentation system.
- A presentation system writes a sim entity: do the write in a sim system, or write the derived
  value onto a presentation entity.
- A presentation system calls `spawn`: use `World::spawn_presentation`.
- A sim system calls `spawn_presentation`: move the spawn into a presentation system that reads the
  sim state.
- A presentation system writes a sim resource: write only through an allowlisted outbox
  (`World::allow_presentation_writes`); for randomness, use `DeterministicRng::derived` from
  `WorldSeed` and the frame.",
    },
    // --- L1xxx: scene / assets ---
    CodeEntry {
        code: code::SCENE_YAML_SYNTAX,
        severity: Severity::Error,
        title: "scene file is not well-formed YAML",
        explanation: "\
The scene file could not be parsed as YAML at all.

Scene files are a strict subset of YAML 1.2, but before any subset rule applies the text must be
well-formed YAML: consistent indentation, balanced quotes, `key: value` mappings, and `- item`
sequences. The message includes the parser's reason and the line and column where parsing stopped;
the error is usually at or just before that position.

Fix: go to the reported line and column and repair the structure there. The usual culprits are a
tab used for indentation (YAML requires spaces), a missing space after `:`, an unterminated quoted
string, or inconsistent indentation between sibling keys.",
    },
    CodeEntry {
        code: code::SCENE_YAML_ANCHOR,
        severity: Severity::Error,
        title: "YAML anchors and aliases are not allowed",
        explanation: "\
The scene file defines an anchor (`&name`) or references an alias (`*name`).

Ludion scene files are a strict subset of YAML 1.2 that excludes anchors and aliases
(engine-architecture.md §7). Aliases create invisible sharing: one edit changes several places at
once, diffs stop meaning what they say, and a document no longer reads the same to an agent and to
the parser. The canonical writer never emits them, so the parser rejects them.

Fix: replace the alias with the actual value, repeated where needed. Repetition in a scene file is
deliberate — every entity block stands alone and diffs stay local.",
    },
    CodeEntry {
        code: code::SCENE_YAML_TAG,
        severity: Severity::Error,
        title: "YAML tags are not allowed",
        explanation: "\
The scene file used a YAML tag such as `!!str`, `!!int`, or a custom `!tag`.

Ludion scene files are a strict subset of YAML 1.2 that excludes tags. Types in a scene are decided
by the component's JSON Schema from the registry, never by per-node annotations in the document —
one source of truth for what a value means.

Fix: remove the tag. If you were using a tag to force a scalar's type (e.g. `!!str 42`), quote the
scalar instead (`\"42\"` is a string; `42` is an integer).",
    },
    CodeEntry {
        code: code::SCENE_YAML_MULTIPLE_DOCUMENTS,
        severity: Severity::Error,
        title: "multiple YAML documents in one scene file",
        explanation: "\
The scene file contains more than one YAML document (a `---` document-start or `...` document-end
marker followed by more content).

A Ludion scene file is exactly one document describing one scene. Multi-document streams would make
'one file = one scene = one diff' ambiguous, so the strict subset excludes them
(engine-architecture.md §7).

Fix: remove the extra `---` / `...` markers, or split the content into one scene per file. (A single
leading `---` before the document is tolerated by YAML but never emitted by the canonical writer.)",
    },
    CodeEntry {
        code: code::SCENE_YAML_FLOW_COLLECTION,
        severity: Severity::Error,
        title: "non-empty flow collection",
        explanation: "\
The scene file used a non-empty flow-style collection, e.g. `[1, 2, 3]` or `{x: 1, y: 2}`.

The canonical scene writer is line-oriented: every element of a non-empty collection gets its own
line, so a one-value change is a one-line git diff (engine-architecture.md §7). Flow style packs
many values onto one line and destroys that property, so the strict subset allows flow syntax only
for the two empty collections `[]` and `{}` (which have no per-element diff to preserve).

Fix: rewrite the collection in block style:

  position: [1, 2]      ->      position:
                                  - 1
                                  - 2",
    },
    CodeEntry {
        code: code::SCENE_YAML_BLOCK_SCALAR,
        severity: Severity::Error,
        title: "block scalars are not allowed",
        explanation: "\
The scene file used a literal (`|`) or folded (`>`) block scalar.

Block scalars come with a family of chomping/indentation indicators (`|-`, `>+`, `|2`…) whose
whitespace semantics are easy for humans and agents to misread, and the canonical writer never
emits them. The strict subset therefore excludes them (engine-architecture.md §7): every string in
a scene is a plain or quoted scalar on one line.

Fix: rewrite the value as a quoted scalar, using `\\n` escapes inside double quotes if the string
must contain line breaks.",
    },
    CodeEntry {
        code: code::SCENE_YAML_DUPLICATE_KEY,
        severity: Severity::Error,
        title: "duplicate mapping key",
        explanation: "\
A mapping in the scene file repeats a key.

YAML itself declares duplicate keys invalid, and parsers that tolerate them silently keep only one
of the values — a data-loss trap. Ludion rejects the duplicate outright and points at the second
occurrence.

Fix: keep exactly one of the entries. If both were meant to exist, one of them is misnamed — check
for a typo — or the values belong in a sequence under a single key.",
    },
    CodeEntry {
        code: code::SCENE_YAML_COMPLEX_KEY,
        severity: Severity::Error,
        title: "non-scalar mapping key",
        explanation: "\
A mapping in the scene file used a sequence or mapping as a key (YAML's `? complex key` syntax), or
a non-string scalar where the scene format requires a name.

Every key in a scene document is a plain string: top-level keys, entity fields, component names,
and component field names. Complex keys have no JSON equivalent — they cannot be validated against
a JSON Schema, cannot travel over the JSON wire protocol, and have no JSON Pointer form.

Fix: use a string key.",
    },
    CodeEntry {
        code: code::SCENE_TYPE_MISMATCH,
        severity: Severity::Error,
        title: "wrong type or shape in scene document",
        explanation: "\
A node in the scene document has the wrong type or shape for its position.

The scene format is a fixed skeleton: the document root is a mapping; `format` is an integer;
`name` is a string; `seed` is a non-negative integer; `entities` is a sequence of mappings; each
entity's `components` is a mapping from component name to component value. This code also covers
numeric literals the engine cannot represent: integers outside the 64-bit range and non-finite
floats (`.inf`, `.nan`), which have no JSON equivalent and cannot cross the wire protocol.

The diagnostic's JSON Pointer names the offending node and the message states what was expected and
what was found. Fix: reshape the value; if a scalar was meant to be a string, quote it.",
    },
    CodeEntry {
        code: code::SCENE_UNKNOWN_KEY,
        severity: Severity::Error,
        title: "unknown key in scene document",
        explanation: "\
A mapping in the scene document contains a key the scene format does not define.

The scene skeleton accepts a fixed set of keys: `format`, `name`, `seed`, and `entities` at the top
level; `name` and `components` inside an entity. Anything else is rejected rather than ignored —
silently ignored keys are how typos become invisible bugs (`entites:` would otherwise produce an
empty scene that loads fine).

The diagnostic suggests the nearest valid key when one is plausibly a typo. Fix: rename or remove
the key. Component *fields* are validated separately, against the component's JSON Schema (L1014).",
    },
    CodeEntry {
        code: code::SCENE_MISSING_KEY,
        severity: Severity::Error,
        title: "missing required key in scene document",
        explanation: "\
The scene document is missing a key the format requires.

Currently the only required key is the top-level `format`, the scene-format version marker. It is
required precisely so that this engine — and any future one — can tell versions apart instead of
guessing from shape: a scene that omits it is not self-describing.

Fix: add the key. A minimal valid scene is:

  format: 1
  entities: []",
    },
    CodeEntry {
        code: code::SCENE_UNSUPPORTED_FORMAT,
        severity: Severity::Error,
        title: "unsupported scene format version",
        explanation: "\
The scene file declares a `format` version this engine build does not support.

This engine reads scene format version 1 only. A higher number means the file was written by a
newer engine; re-emit the scene with a matching engine version or upgrade this one. Version bumps
are deliberate, breaking events — there is no silent best-effort reading of a newer format.",
    },
    CodeEntry {
        code: code::SCENE_UNKNOWN_COMPONENT,
        severity: Severity::Error,
        title: "unknown component in scene",
        explanation: "\
An entity in the scene names a component that is not in the component registry.

Components are validated against the registry — the single table mapping component names to their
JSON Schemas and serde functions. A name the registry does not know cannot be validated,
instantiated, or inspected. This usually means a typo in the component name (the diagnostic
suggests the nearest registered name), or a scene written for a game/engine build that registers
components this one does not.

Fix: correct the spelling, or run the check against the build that defines the component.
`ludion check-scene` and `ludion render` link no game code, so they know only the engine's
components; for a game's own, run `cargo run -- --scene <path> --check-scene` from the game's
project.",
    },
    CodeEntry {
        code: code::SCENE_COMPONENT_MISMATCH,
        severity: Severity::Error,
        title: "component value does not match its schema",
        explanation: "\
A component value does not match that component's JSON Schema from the registry — in a scene file,
or sent over the wire by `component.set` / `entity.spawn` (which reject it before the world is touched,
with a pointer relative to the component).

The diagnostic's JSON Pointer names the exact field, and the message states what the schema expects
versus what the document contains: a wrong type (`\"fast\"` where a number belongs), a missing
required field, a field the component does not declare (typos are flagged, not ignored, with the
nearest valid field suggested), a value outside the allowed range, or an enum variant that does not
exist.

Fix: adjust the value to the shape the schema declares. `ludion explain` cannot print per-component
schemas; the registry is the source of truth, and `engine.describe` exports every registered
component's schema over the wire.",
    },
    CodeEntry {
        code: code::SCENE_RESERVED_COMPONENT,
        severity: Severity::Error,
        title: "reserved component declared as a scene block",
        explanation: "\
A scene entity declared a reserved component inside its `components:` mapping.

A few built-in components are *field-backed*: a dedicated scene field is their one authoring
channel. `Name` is projected from an entity's `name:` field, so it may not also be written as a
`components:` block — that would be a second way to say the same thing, and the two could disagree.

Fix: move the value to the entity field. Instead of

  components:
    Name: \"paddle\"

write

  name: \"paddle\"
  components: {}",
    },
    CodeEntry {
        code: code::SCENE_INVALID_ENTITY_REF,
        severity: Severity::Error,
        title: "invalid entity reference in scene",
        explanation: "\
A scene entity's component carries an entity reference that does not name a valid entity in the
scene: `Parent`'s implicit link, or `Joint`'s `a`/`b`.

Such a reference names another entity by that entity's **document position** — its 0-based index in
the scene's `entities` sequence — written as `{index: <position>, generation: 0}`. The engine
instantiates a scene by spawning one entity per document entry, in order, into a fresh world, so a
document position always maps to a live entity and `generation` is always 0. A reference is rejected
here when it cannot refer to a real entity that way (`component.set` and `entity.spawn` raise the
self-reference case on the wire too, by entity id):

  - `index` is out of range: there is no entity at that document position (it must be less than the
    number of entities in the scene);
  - `generation` is not 0: a scene never refers to a recycled slot, so the only valid generation is
    0 (a nonzero value is almost always a hand-edited or mis-remapped id);
  - the reference names the entity *itself*, where self-reference is not allowed: a `Parent` always
    (an entity parents itself when its `Parent.index` equals its own document position), a `Joint`'s
    `a`/`b` only when `a == b` (either alone may equal the joint's own carrier).

The diagnostic's JSON Pointer names the exact offending value. Fix: set `index` to the position of
the intended entity in the `entities` sequence (with `generation: 0`), or remove the referencing
component. Longer `Parent` loops are a separate error, `L1035`; this one checks only that each
reference names a real entity.

Inside a map fragment the references are local to the fragment and rebased on composition; one that
names no entity of its fragment is reported here with the pointer `/fragments/<i>`.",
    },
    CodeEntry {
        code: code::SCENE_SAVE_DANGLING_REF,
        severity: Severity::Error,
        title: "cannot save: an entity reference points at a missing entity",
        explanation: "\
A `scene.save` could not write the world, because an entity's component carries an entity
reference — `Parent`'s implicit link, `Joint`'s `a`/`b`, or any field marked as one — that points at
an entity that is not part of the saved scene, or at the carrying entity itself where the field
forbids that (`Parent` always; `PatternPlayer`'s `target`).

`entity.destroy` is deliberately non-cascading: destroying an entity leaves any reference to it
dangling rather than silently rewriting or deleting other entities. A dangling reference cannot be
expressed as a valid scene — the scene format identifies an entity by its document position, and a
destroyed target has no position — so rather than write a scene that would reload with a corrupt
reference, the save fails here and names the referencing entity, the field, and its missing target.

Fix: repair the reference before saving — `component.set` the referencing component to point at a
live entity, or `component.remove` it entirely. The same code refuses a world whose `Parent` links
loop (a scene with a cycle fails validation with `L1035`): `component.set` one link in the loop to
another entity, or remove it. (Cascading destruction, if you want it, is the
editor's job, layered over these commands; the engine keeps `entity.destroy` a single, reversible
operation.)",
    },
    CodeEntry {
        code: code::TILEMAP_RAGGED_ROWS,
        severity: Severity::Error,
        title: "tilemap rows are ragged",
        explanation: "\
A `Tilemap` or `MeshGrid` component's `rows` are not all the same length, so the grid is not
rectangular.

A `Tilemap` (and likewise a `MeshGrid`) is a grid of single-glyph cells: `rows` is a list of strings, one glyph per cell, laid
out row-major. Every row must therefore contain the same number of glyphs — the grid's width. A row
whose glyph count differs from the first row's means either a stray or missing character, and the
engine cannot know which cells are real, so it refuses rather than guess a shape.

Glyph count is measured in Unicode scalar values (Rust `char`s), not bytes: a multi-byte glyph
counts as one cell, exactly as it renders as one cell. The diagnostic's JSON Pointer names the first
offending row (`/rows/<n>`), and the message states the expected width (row 0's) versus what that
row has.

Fix: make every row string the same glyph count. A level edit is a glyph-grid diff — line the rows
up so each column holds one glyph, padding with a legend glyph (e.g. a floor or void tile) rather
than leaving a row short.",
    },
    CodeEntry {
        code: code::TILEMAP_INVALID_GLYPH,
        severity: Severity::Error,
        title: "invalid tilemap glyph",
        explanation: "\
A `Tilemap` or `MeshGrid` glyph is invalid — either a row uses a glyph the `legend` does not define,
or a `legend` key is not a single glyph.

A `MeshGrid` differs in one way: a space is its one empty cell, so it needs no legend entry and can
never be a legend key.

A `Tilemap`'s `legend` maps each glyph to a tile appearance (color and/or texture), and every glyph
in every row must have a legend entry — there is no implicit 'empty' glyph, so an unmapped glyph is
always a typo or a missing legend entry (to leave a cell blank, give its glyph a legend entry with a
background color). A glyph is exactly one Unicode scalar value (one Rust `char`): a legend key that
is empty or more than one character is not a glyph and is rejected here too, folded into this same
code because both failures are about what a valid glyph is.

The diagnostic's JSON Pointer names the offending row (`/rows/<n>`, with the column and glyph in the
message) or the offending legend key (`/legend/<key>`).

Fix: add the missing glyph to the `legend`, correct the glyph in the row, or make the legend key a
single character. The set of legend glyphs and the set of glyphs used in rows must match up.

A map-kit `tiles` key may not be the space either: the space is the reserved void glyph.",
    },
    CodeEntry {
        code: code::MAP_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "malformed map or map-kit document",
        explanation: "\
A map source (`kind: map`) or map-kit (`kind: map-kit`) document is structurally malformed.

A map compiles a glyph grid plus a marker list into a scene of real entities; a map-kit maps each
glyph and each marker kind to a list of engine primitives (a mesh source, an optional collider, a Y
offset). Both are `format: 1` + `kind:` documents, hand-parsed like the mesh/voxel formats, with the
same strictness: a required top-level key must be present and correctly typed (`cell_size` a positive
number, `origin` two numbers, `rows` a sequence of strings, `kit` a string; a map-kit's `tiles`/
`kinds` mappings of glyph/kind to a primitive list), an unrecognized `kind` is rejected, and any key
the format does not define is an error rather than silently ignored. `bounds` is exclusive with
`rows`/`base`, and a fragment's `physics`/`seed` is refused (declare it on the map: the merge would
drop it).

The diagnostic's JSON Pointer names the offending node. Fix: reshape the value to match the format —
`ludion docs map-authoring` documents the map and map-kit shapes field by field.",
    },
    CodeEntry {
        code: code::MAP_KIT_INVALID,
        severity: Severity::Error,
        title: "invalid map-kit entry",
        explanation: "\
A map-kit entry, or a marker's reference to one, is semantically invalid.

A kit entry maps a glyph or a marker kind to a non-empty list of primitives; each primitive names
exactly one mesh source (a `shape` primitive, a `voxel` path, or a `mesh` asset path) and may carry a
color, a collider, a Y offset, a scale, and either `merge` (rectangle-merged floor) or a `footprint`
(modular tiling) — but not both. This code covers the semantic violations the shape checker cannot:
an empty primitive list, a primitive with zero or several mesh sources, a `footprint` that is not two
positive integers, a primitive that sets both `merge` and `footprint`, a non-finite `y`/`scale`/
`color`/collider size, a `probe` radius or speed that is not a positive number, or a marker whose
`variant` index names no candidate in the referenced entry. A merged primitive needs a cell-sized
`aabb`, never a `circle`.

The diagnostic's JSON Pointer names the offending primitive or marker field. Fix: give the entry
exactly one mesh source per primitive and a valid modular/merge choice, or correct the marker's
`variant` to an in-range index.",
    },
    CodeEntry {
        code: code::MAP_MARKER_OUT_OF_BOUNDS,
        severity: Severity::Error,
        title: "map marker out of bounds",
        explanation: "\
A map marker's world position falls outside the grid.

Markers are placed in world units (`x`, `z`), and the grid covers the rectangle from `origin` to
`origin + cell_size × [columns, rows]` on the XZ plane. A marker outside that rectangle names no cell;
rather than silently clamp it (which would move a gate or a landmark somewhere the author did not
intend), the generator rejects it.

A map with markers but no frame — empty `rows` and no `bounds:` — has no extent at all, so every
marker is outside it; the diagnostic's pointer is `/rows`. A marker-only map declares its extent with
`bounds: [width, height]` (in cells) instead of glyph rows.

The diagnostic's JSON Pointer names the marker. Fix: move the marker inside the grid's extent, or
enlarge the grid (`rows` or `bounds`) / shift its `origin` so the intended position is covered; for a
frameless map, add `rows` or `bounds`.",
    },
    CodeEntry {
        code: code::MAP_UNKNOWN_KIND,
        severity: Severity::Error,
        title: "unknown map marker kind",
        explanation: "\
A map marker names a `kind` with no entry in the referenced map-kit.

A marker's `kind` selects a list of primitives from the kit's `kinds` mapping (a well, a gate marker,
a torch). A kind the kit does not define cannot be stamped — usually a typo, or a marker written for a
different kit. (A marker may instead carry only a `components:` passthrough and no `kind`, in which
case no kit lookup happens.)

The diagnostic suggests the nearest kit kind and its JSON Pointer names the marker's `kind`. Fix:
correct the spelling, add the kind to the kit, or drop the `kind` if the marker is pure passthrough.",
    },
    CodeEntry {
        code: code::MAP_NAME_COLLISION,
        severity: Severity::Error,
        title: "generated entity name collision",
        explanation: "\
The map generator would emit two entities with the same `name`.

Generated entity names are coordinate-derived (`floor-r012-c007`, a merged rect encoding its extent)
and must be globally unique across the three sources the generator concatenates: fragment entities,
grid rects/props, and markers. Uniqueness is load-bearing, not cosmetic — gate and transition anchors
resolve a target by `name`, so two entities sharing one would make an anchor ambiguous. The generator
validates it and refuses rather than emit a scene with a silent duplicate.

The message names the colliding name and both origins, each with its JSON Pointer. Fix: give the offending kit primitive or marker an explicit,
distinct `name` prefix, or (if a fragment and the grid collide) rename the fragment entity — the grid
names are derived from the kit, the fragment names are yours.",
    },
    CodeEntry {
        code: code::MAP_FOOTPRINT_MISFIT,
        severity: Severity::Error,
        title: "footprint tiling does not divide evenly",
        explanation: "\
A run of a footprint-tiled glyph does not divide evenly into whole footprint spans.

A `footprint: [w, h]` kit primitive tiles the glyph's cells with a modular piece `w` cells wide and
`h` cells deep (a wall segment, a fence panel), placed row-major and greedily. Every covered region
must partition exactly into `w × h` blocks: an 8-cell horizontal run with `footprint: [4, 1]` emits
two pieces, but a 6-cell run leaves a 2-cell remainder that no whole piece fits, so the generator
reports it rather than emit an overhanging or a gapped wall.

The diagnostic's JSON Pointer names the offending grid row. Fix: make the glyph's runs a whole
multiple of the footprint (add or remove cells), or choose a kit piece whose footprint divides the
run.",
    },
    CodeEntry {
        code: code::MAP_MIRROR_FRAGMENTS,
        severity: Severity::Error,
        title: "a mirrored map cannot carry fragments",
        explanation: "\
`generate-map --mirror` was asked of a map that carries `fragments:`.

The mirror reflects the generated grid and markers across the grid's centre line. Fragments are
hand-authored scenes whose transforms (parents, pitch/roll) the compiler does not reflect, so a
mirrored map would leave them standing in the unreflected room.

The diagnostic's JSON Pointer is `/fragments`. Fix: move the fragment's entities onto markers (a
marker's `components:` carry lights and game components), or drop the mirror.",
    },
    CodeEntry {
        code: code::MAP_SPAWN_INVALID,
        severity: Severity::Error,
        title: "spawn_map could not use the map",
        explanation: "\
`spawn_map` was asked to stamp a map it could not read or accept, so nothing was spawned.

The map's path (and a `MapSpawn.kit` override) is relative to the content root, like `spawn_prefab`'s.
This code covers a path that is absolute or climbs out of the content root, a file that cannot be read
(or, under replay, that the recording does not hold), a file that is not a `kind: map` (or a kit that
is not a `kind: map-kit`), a map carrying a `physics:` block (the scene that spawns the room owns
physics), a room root carrying `Animation` (a room has no clips: put it on a part below the root or
on an assembly or character mesh), and a world with no prefab library (it was not built by an
engine).

Bad map or kit content keeps its own codes (`L1019`-`L1026`), a mirror on a map with fragments is
`L1399`, and the spawn-time passes keep the prefab codes. Fix: name the map relative to the content
root, e.g. `rooms/crypt.map.yaml`, pass a map (not a prefab or scene) and a kit, and keep `physics:` in
the scene that spawns the room.",
    },
    CodeEntry {
        code: code::MAP_DANGLING_GATE_ANCHOR,
        severity: Severity::Error,
        title: "dangling cross-district gate anchor",
        explanation: "\
A map marker's cross-district gate names a spawn anchor that resolves against no entity in the sibling
district.

A `Gate` marker carries a `to` (the sibling district's map) and an `anchor` (a `Name`d inbound entity
in that district). For the gate to work, generating the sibling map must produce an entity whose name
equals the anchor; otherwise the switch fails only at runtime, as `L3019`. This code lets the failure
surface at authoring time instead.

**Reserved:** the map format registers this code, but the check that raises it — resolving the sibling
map off disk, generating it, and looking the anchor up — is `ludion map-check`'s job, because the core
generator is a pure, IO-free function and never reads a sibling file. Fix: name the gate's `anchor` to
match an inbound anchor entity the target district's map actually generates (a fragment entity, or a
marker with that `name`).",
    },
    CodeEntry {
        code: code::MAP_GATE_SEALED,
        severity: Severity::Warning,
        title: "gate unreachable from the anchor",
        explanation: "\
A map marker carrying a `Gate` component cannot be reached from the reachability anchor.

A gate is reachable when its own cell, or one of its four orthogonal neighbours, is walkable and on a
walkable path from the anchor. The anchor is `--anchor <name>` when given, else the first marker, else
the grid centre; a gate that is itself the anchor is trivially reachable. This diagnostic means no such
cell exists: the gate sits behind solid wall, or in a pocket the walk from the anchor never enters —
typically a wall gap authored on the wrong side of a boundary, or a gate placed in a different region
than the playable area.

A gate the anchor cannot reach is usually also disconnected from the gates that can be (`L1031`); the
two answer different questions (one gate against the anchor, one pair against each other) and both can
fire on a bisected map.

This is a **warning**, not an error: it is a heuristic on the occupancy probe (`--radius`) and the grid
geometry, so it surfaces the bug in the map-check report without failing the command. (The provable
`L1027` — an anchor name that does or does not resolve — stays an error.) Fix: open the wall between
the gate and the playable area, move the gate onto a reachable cell, or pass `--anchor` a marker
inside the region the gate belongs to.",
    },
    CodeEntry {
        code: code::MAP_MARKER_BLOCKED,
        severity: Severity::Warning,
        title: "marker on a blocked or unreachable cell",
        explanation: "\
A map marker sits where it should not: another prop's collider lies within the probe radius of its
point, or its cell is walkable but sealed off from the rest of the map.

A marker whose point has a collider of a *different* entity within the probe radius (`--radius`)
would spawn blocked by that prop.
A marker on a walkable cell that the reachability flood-fill never reaches sits in a sealed interior
pocket nothing can walk to. Either is almost always a placement mistake, whatever the marker's role.

The marker's **own** collider is excluded from the blocked test: a building authored as a passthrough
marker (a voxel mesh plus a hand-authored AABB) legitimately blocks its own cell, and that is a
footprint, not a bug — only being blocked by someone else's collider fires this code.

This is a **warning**, not an error: it is a heuristic on the occupancy probe (`--radius`) and the
reachability anchor, so it surfaces the problem in the map-check report without failing the command.
Fix: move the marker onto a clear, reachable cell, or open the pocket it sits in.",
    },
    CodeEntry {
        code: code::SCENE_YAML_KEY_TOO_LONG,
        severity: Severity::Error,
        title: "mapping key too long in canonical form",
        explanation: "\
A mapping key in the scene file is too long once written the way the canonical writer would write
it.

Every document Ludion accepts must survive a round trip: the canonical writer re-emits it and the
parser reads the result back. The writer spells each key as a single-line scalar — plain when it is
unambiguous, otherwise double-quoted with escapes — and the YAML scanner caps a simple key (a key
written `key: value` on one line) at 1024 characters. A key longer than that in canonical form would
be emitted and then rejected on the next parse, so it is refused here, at the only point where the
error can still name the key's position.

The usual cause is not a long name but an expanding one: quoting adds two characters, and every
control character costs six (`\\u00XX`), so a key of a few hundred control characters crosses the
limit even though the source line looks short.

Fix: shorten the key. Keys in a scene document are identifiers — top-level keys, entity fields,
component names, component field names — not payload; long or binary-ish text belongs in a value.",
    },
    CodeEntry {
        code: code::SCENE_YAML_TOO_DEEP,
        severity: Severity::Error,
        title: "collections nested too deeply",
        explanation: "\
A block or flow collection in the document is nested more than 128 levels deep.

Every reader of a parsed document — the tree builder, the checker, the canonical writer — walks it
recursively, so an unbounded depth would turn a hostile or corrupt file into a stack overflow that
aborts the process. The strict YAML subset therefore caps nesting at a fixed limit and reports it
here, at the line where the limit is crossed. Real scenes nest about ten levels deep, so the cap is
far above anything authored by hand.

Fix: flatten the structure. A document this deep is usually corrupt or was generated by mistake —
check for a runaway generator or a repeated `- ` / `key:` prefix.",
    },
    CodeEntry {
        code: code::SCENE_YAML_DIRECTIVE,
        severity: Severity::Error,
        title: "YAML directive not allowed",
        explanation: "\
The file starts with a YAML directive line (`%...`) other than `%YAML 1.2`.

Scene and doc files are always read as YAML 1.2 (core schema). A `%YAML 1.1` file would read
`yes`/`no`/`on`/`off` differently than it declares, so it is refused rather than half-honoured.
`%TAG` and unknown directives are refused too. `%YAML 1.2` is tolerated but never written back.

Fix: delete the directive line.",
    },
    CodeEntry {
        code: code::SCENE_PARENT_CYCLE,
        severity: Severity::Error,
        title: "entity hierarchy has a cycle",
        explanation: "A scene's `Parent` links form a loop: following an entity's parent, then that parent's parent, and
so on leads back to the entity you started from (`a` is the parent of `b`, and `b` of `a`, or a
longer ring).

A hierarchy is a tree: every entity has at most one parent and the chain ends at a root. A loop has
no root, so any system that walks the chain upward (a transform that composes with its parent's, a
despawn that follows children) would never terminate. The engine does not walk `Parent` itself, but
a scene that carries a loop is broken for every system that does, so validation refuses it. An entity
that parents *itself* is reported as `L1017` instead. A loop made at runtime, past validation, is
the warning `L1218`: the entity degrades to a root.

The diagnostic names the chain by document position (and the entity's `name`, when it has one) and
points at the lowest-positioned member's `Parent`; each cycle is reported once. `scene.save` refuses
to write a world whose `Parent` links loop, with `L1018`.

Fix: re-point one link in the loop at a real parent (or another entity outside it), or remove that
entity's `Parent` component to make it a root.",
    },
    CodeEntry {
        code: code::SCENE_PRESENTATION_REF,
        severity: Severity::Error,
        title: "sim entity references a presentation entity",
        explanation: "A sim entity's entity-reference field (a `Parent`, a `Target`, any field declared as an entity
ref) names an entity marked `presentation: true`.

Presentation entities live in their own id range and are never hashed. A sim system that read a
reference to one would feed presentation state into the simulation, so the `WorldHash` would depend
on something the replay does not verify. The other directions are fine: a presentation entity may
reference a sim entity or another presentation entity (a popup parented to the unit it floats over).

The diagnostic names the carrier and the target by document position (and `name`, when set) and
points at the reference's `index`. `component.set` and `entity.spawn` raise it too, naming the carrier
and target by id. `scene.save` refuses to write a world with the same reference, with `L1018`.

A prefab instance carries one implicit reference: every part's `PrefabInstance.root` names the
instance's first part. A prefab (or an instance, after `removed:`) whose first part is presentation
but that holds a sim part would hash a presentation id, so it is refused here too, pointed at the
instance entry or the prefab's `/entities/0/presentation`. Put the sim part with the `Collider` first.

Fix: point the reference at a sim entity, or mark the carrier `presentation: true` too. See
`ludion docs presentation`.",
    },
    CodeEntry {
        code: code::MAP_GATES_DISCONNECTED,
        severity: Severity::Warning,
        title: "two gates with no walkable path between them",
        explanation: "\
Two `Gate` markers on the same map sit in different walkable regions: no 4-connected path of walkable
lattice samples joins them.

A district is entered by one gate and left by another, so every gate must reach every other gate on
foot. This diagnostic means the map is bisected — a wall run closed a corridor, a merged floor rect
left a seam, or a marker prop's collider plugged the only doorway — and a player arriving at one gate
is stranded. `L1028` catches the neighbouring bug (one gate with no gap through the perimeter wall);
this one catches the split *inside* the map, which no per-gate check can see.

This is a **warning**, not an error: the path is searched over the same heuristic occupancy probe the
rest of the report uses — a lattice of `--radius` circles, several samples to the radius, joined
4-connected — so a doorway narrower than the walker reads as blocked, and a gap only just wider than
the walker can still fall between the samples.

Fix: open the wall between the two regions, or move the gate into the region the rest of the map can
reach. `ludion map-check --traverse` prints the gate-to-gate distances and names the pair that has
none; the report's `?` cells show which region is cut off.",
    },
    CodeEntry {
        code: code::MAP_RESIDUE_MARKER_UNNAMED,
        severity: Severity::Warning,
        title: "a Residue-marked marker has no explicit name",
        explanation: "\
A map marker's `components:` bag authors a `Residue` component, but the marker itself has no explicit
`name:`.

Scene residue (`Plan/sector-state-persistence.md`) remembers state for an entity by keying it under
its `Name`. A marker's `Name` is `MapMarker::derived_name`: the explicit `name:` if one is authored,
else `<kind>-<index>`, else `marker-<index>` — an index-derived fallback. A `Residue`-marked marker
without an explicit `name:` therefore has an identity tied to its position in the marker list, so
inserting or reordering a marker above it silently re-binds the remembered state to whichever marker
now holds that index on the next map edit.

This is a **warning**, not an error: the marker is well-formed and the map generates correctly: the
hazard is latent, surfacing only on a later edit.

Fix: give the marker an explicit `name:` — residue matches entities by `Name`, and a stable name keeps
a sector's memory attached to the same entity across map edits.",
    },
    CodeEntry {
        code: code::SCENE_PHYSICS_INVALID,
        severity: Severity::Error,
        title: "invalid value in a scene's physics: block",
        explanation: "\
A scene's `physics:` block carries a value the block's shape allows but its meaning rejects.

This covers eight cases: an unrecognized `space` (only `free` and `planar` are defined — the
diagnostic suggests the nearest match), a `solver_iterations` or `stabilization_iterations` outside
`1..=64` (the cap exists because a runaway iteration count stalls every host's tick), a
`sleep_after_ticks` above `u32::MAX`, a `gravity` that is not exactly three numbers each
representable as `f32`, and four in `layers`: an empty or duplicate layer `name` (or `default`, which
is reserved as the implicit layer of every collider that names none), more than 31 layers (the
solver keeps one 32-bit group mask, and `default` takes a bit), an `ignores` entry that names no
declared layer (the diagnostic suggests the nearest match; `default` may be ignored), and an
`ignores` entry listed twice.

A wrong *type* — a number where `space` wants a string, a float where an iteration count wants an
integer — is `L1009` instead; this code is for a well-typed value that is still not a legal one.

Fix: correct the value. This is always legal, with every other field at the pinned solver's own
default:

  physics:
    space: free",
    },
    CodeEntry {
        code: code::SCENE_IO,
        severity: Severity::Error,
        title: "input file could not be read",
        explanation: "\
A file (or directory listing) a command reads as input could not be read from disk: a scene, a
model, sculpt, mesh, atlas, font or map document, a replay session (`.lrp`), a profile or sweep
source, a golden suite directory or its manifest file.

The message includes the operating system's reason — typically the file does not exist, the path is
a directory, or permissions deny access.

Fix: check the path and the file's permissions. Where a path resolves depends on who reads it: the
CLI resolves paths relative to the current directory; wire commands (`scene.load`, `scene.check`)
relative to the content root — the directory of the scene the engine was started with; a file
referenced from inside another document (a map's kit) relative to that document. The help line names
the base. A failure *writing* an output file is L1038; an asset file referenced by a scene is L1101.",
    },
    CodeEntry {
        code: code::OUTPUT_IO,
        severity: Severity::Error,
        title: "output file could not be written",
        explanation: "\
An output file could not be produced: a directory could not be created, a file could not be written,
renamed or removed, or its bytes could not be encoded. A stream counts: writing a trace to stdout
fails the same way when the pipe is closed or the redirect target is full.

The message includes the operating system's reason — typically the destination directory does not
exist or the path is a directory, permissions deny access, another process holds the file open, or
the disk is full. CLI paths are relative to the current directory; wire paths (`scene.save`) are
relative to the content root. Where the writer is atomic (`write_atomic`: write a temporary file,
then rename) nothing is half-written and the previous file is intact. A generated file's
`generated_from` is a relative path; an output on a different drive from its source cannot have
one, and is refused before anything is written.

Fix: check the destination path, its directory and its permissions, and close whatever holds the
file. A failure reading an input file is L1015.",
    },
    CodeEntry {
        code: code::MAP_SCENE_DRIFT,
        severity: Severity::Error,
        title: "generated scene differs from its map source",
        explanation: "`generate-map --check` found a committed scene whose bytes differ from what its map source
generates now.

A scene bearing `generated_from:` is write-only: it is the output of `ludion generate-map` over a
map, a kit and fragments. A mismatch means the map, kit or a fragment changed without regenerating,
or the generated scene was hand-edited. Either way the committed scene is stale or wrong.

Fix: run `ludion generate-map <map>` and commit the result; never hand-edit a generated scene — edit
the map source, the kit or a fragment instead. A scene that cannot be read at all is L1015.",
    },
    CodeEntry {
        code: code::MAP_EDGE_LEAK,
        severity: Severity::Warning,
        title: "the walker can leave the map",
        explanation: "\
`map-check`'s seal check found a walk of the player's radius from the map's start to the edge of the
grid. The map leaks: a player can walk out of the playable area.

The check floods a lattice finer than the reachability grid (four samples to the walker radius) and is
sound: it never misses a way out that the walker fits. A sample is free when no static collider is
within the radius shrunk by half a lattice diagonal. It may flag a gap up to twice that shrink
narrower than the walker, so the warning can be a near miss. The flood is rerun at the full radius to classify the
leak: still leaking, the way out is walker-wide; otherwise it is a gap from twice the shrunk radius
up to about the walker's width plus a lattice diagonal, which the walker may fit. The report's grid may
not show it, because its coarser lattice can read the gap as closed.

A gate's trigger never stops the walk, since a game may let a player pass one without crossing (a hold
gate, a key held through an arrival), so every opening in the frame needs a collider behind its gate.

Fix: plug the gap with a collider (`map-check --at x,z` probes the reported point), or close the
opening behind its gate.",
    },
    CodeEntry {
        code: code::MAP_DRIFT_FAULT,
        severity: Severity::Error,
        title: "a map-generated scene fails the drift check",
        explanation: "\
`map-drift` checks every `*.map.yaml` under its scan directories and every scene or prefab whose
`generated_from:` names one. This code is any failure that is not byte drift (L1039): a map whose
default output is missing or does not compile, a scene naming a map from anywhere but that map's
default output (a moved or orphaned copy), a recorded `--kit`, `--seed` or `--mirror` flag that does
not parse, a named map that does not exist, a map whose seal is not `sealed` (a leak is L1040, which
the gate treats as an error), a warning denied by `--deny-warnings`, or a scan that found no map.

A map with at least one scene or prefab claiming it needs no default output: a room map compiled only
into prefabs is checked through those.

Fix: the diagnostic's help names the file and the command. Usually `ludion generate-map <map>`
(default output) or `ludion generate-map <map> --out <file> [--kit <k>] [--seed <n>] [--mirror x|z]
[--prefab]`, then commit the result. For a denied warning that is intended, add
`--allow CODE[:PREFIX]`.",
    },
    CodeEntry {
        code: code::ASSET_FILE_IO,
        severity: Severity::Error,
        title: "asset file could not be read",
        explanation: "\
An asset file referenced by a scene could not be read from disk.

Asset paths (for example a `Sprite`'s `texture`) are resolved relative to the content root — the
directory of the scene the engine was started with (not the process's current directory). This
diagnostic means the file at that resolved path
does not exist, is a directory, or could not be opened for reading. The message includes the
operating system's reason, and — when the diagnostic comes from extracting a scene — a JSON Pointer
naming the exact component field, plus a suggestion of the nearest filename that does exist in the
assets directory (a likely fix for a typo). `check-scene` reports it for a scene's mesh, voxel, sculpt, assembly,
character, collider-mesh, mesh-texture and decal-map files before any render.

Fix: correct the path so it names an existing file relative to the scene, or add the missing asset.
Paths are case-sensitive on some platforms, so check the spelling and extension.",
    },
    CodeEntry {
        code: code::ASSET_PNG_DECODE,
        severity: Severity::Error,
        title: "asset is not a valid PNG",
        explanation: "\
An asset file was read from disk but could not be decoded as a PNG image.

The renderer's image loader accepts PNG files (8-bit grayscale, grayscale+alpha, RGB, RGBA, or
palette; 16-bit channels are narrowed to 8-bit). This diagnostic means the bytes are not a
well-formed PNG at all, are truncated, use a feature the loader does not support, or declare more
than 8192×8192 pixels or a side over 8192 (the decode budget, checked from the header before any
pixel memory is allocated). The message includes the decoder's reason.

Fix: re-export the image as a standard 8-bit PNG. If the image is over the pixel budget, downscale
it or split it into tiles. If the file is not actually an image (a wrong path that happens to point
at a text file, say), correct the reference instead.",
    },
    CodeEntry {
        code: code::WATCHER_START_FAILED,
        severity: Severity::Error,
        title: "filesystem watcher could not start",
        explanation: "\
A filesystem watcher could not be started.

`ludion dev` (watching a game's `src/`) and the server's asset hot-reload (`serve`, watching the
project root and forwarding each changed path as an `asset.reload`) both watch a directory through
the OS's native file-notification API. This diagnostic means the watcher could not be created at all, or
could not begin watching the requested directory. Typical causes: the directory does not exist, the
process lacks permission to watch it, or the OS ran out of watch descriptors (on Linux, an exhausted
`inotify` instance/watch limit).

Fix: ensure the directory exists and is readable (for `ludion dev`, the game's `src/` directory must
be present — run from the game's project root). On Linux, if the failure mentions a limit, raise
`fs.inotify.max_user_watches` / `max_user_instances`. `ludion dev` treats a watcher that will not
start as fatal, because hot reload cannot function without it.",
    },
    CodeEntry {
        code: code::ASSET_WAV_DECODE,
        severity: Severity::Error,
        title: "asset is not a valid WAV",
        explanation: "\
An asset file was read from disk but could not be decoded as a WAV audio clip.

The audio loader accepts uncompressed PCM WAV files with 16-bit samples and one (mono) or two
(stereo) channels — the minimal format a small committed sound effect needs. This diagnostic means
the bytes are not a RIFF/WAVE container at all, declare a non-PCM codec or a bit depth other than
16, use an unsupported channel count, declare a sample rate below 8000 Hz (the floor keeps the
device-rate resampler from expanding a clip by an implausible factor), or are truncated (the
`fmt `/`data` chunks are missing or shorter than their declared size). The message names which of
these it is.

Fix: re-export the sound as a 16-bit PCM WAV, mono or stereo (in most editors: 'WAV', 'PCM',
'16-bit signed') at 8000 Hz or above (44100 Hz is the usual choice). Compressed containers (MP3, OGG,
ADPCM-in-WAV) are not decoded — convert them to PCM first. If the file is not actually audio (a
wrong path pointing at another file), correct the reference instead.",
    },
    CodeEntry {
        code: code::MESH_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "mesh document structure error",
        explanation: "\
A mesh asset document (`meshes/*.mesh.yaml`) is malformed: a required top-level key is missing or
has the wrong type, an unrecognized `kind` value was found, or the document has a key the mesh
format does not define.

A mesh document is a strict-subset YAML mapping with a fixed top-level shape:

  format: 1
  kind: mesh
  name: \"rock\"          # optional
  origin:                  # optional, the provenance of a generated asset
    tool: comfyui
    ...
  positions:              # required, 3 floats per vertex
    - 0.0
  normals:                 # optional, 3 floats per vertex, must match `positions`' vertex count
    - 0.0
  uvs: []                  # optional, 2 floats per vertex
  indices:                 # required, a flat triangle list (3 indices per triangle)
    - 0

The optional skin keys `joints`, `weights` and `bone_indices` come together and their inner shape
is `L1190`. Three optional material keys sit before them: `alpha_mode` (`opaque`, the default, or
`mask`), `alpha_cutoff` (a number, `mask` only) and `double_sided` (`true` or `false`, default
`false`).

`format` must be `1`; `kind` must be exactly `mesh` (a document with no `kind` key at all is a
scene, not a mesh, so `ludion check-scene` never reaches this validator for one); `positions` and
`indices` are required, `name`/`origin`/`normals`/`uvs` are optional, and no other key is
recognized. `origin:` is the provenance block a generated asset carries — its own key set, the same
one a `kind: origin` sidecar spells beside a generated image. This
one code covers all of that shape checking — the mesh analog of the scene format's structural
codes (`L1009`–`L1012`). The JSON Pointer names the offending key or list element; unknown-key
messages suggest the nearest valid key when one is plausibly a typo.

Fix: match the shape above — add the missing key, fix its type, correct `kind` to `mesh`, or
remove/rename the unrecognized key. Data-level problems (counts, index range, non-finite floats)
are the separate `L1111`.",
    },
    CodeEntry {
        code: code::MESH_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "mesh data is inconsistent",
        explanation: "\
A mesh asset document parsed structurally cleanly, but its vertex/index data does not hang
together.

Specifically, one of:

  - `positions` has a length not divisible by 3 (3 floats per vertex);
  - `normals`, if present, has a different length than `positions` (it must supply exactly one
    normal per vertex) — omit `normals` entirely to have faceted normals computed at load instead;
  - `uvs`, if present, has a length not divisible by 2, or half that count does not equal the
    vertex count (`uvs` must supply exactly one UV pair per vertex);
  - `indices` has a length not divisible by 3 (a triangle list needs 3 indices per triangle), or
    some index names a vertex position that does not exist (must be less than the vertex count
    derived from `positions`);
  - some float in `positions`, `normals`, or `uvs` is not finite (`NaN` or `±Infinity`);
  - `alpha_cutoff` is not a finite value in `[0, 1]`, or is set without `alpha_mode: mask`.

The JSON Pointer names the offending list or element.

`ludion import` raises this code too, for a mesh it compiled from a glTF primitive; the message is
prefixed with the primitive, and the fix is in the authoring tool, then re-export.

Fix: correct the counts so every list describes the same vertices, fix the out-of-range index, or
replace the non-finite value with a real number. Shape/type problems (a missing key, a wrong-typed
value, a bad `kind`) are the separate `L1110`.",
    },
    CodeEntry {
        code: code::MESH_VERTEX_BUDGET_EXCEEDED,
        severity: Severity::Warning,
        title: "mesh exceeds the style vertex budget",
        explanation: "\
A mesh asset document has more than 10,000 vertices — well above what the HD-2D low-poly style
expects (the built-in sphere primitive is about 600 vertices).

This is a **warning**, not a validation failure: `ludion check-scene` still exits success, and the
mesh loads and renders normally. It exists to catch content that drifted from the style by
accident — a forgotten high-poly import, a generator run with the wrong subdivision count — while
a deliberately dense mesh is free to keep its vertex count.

Fix: if the density is unintentional, reduce the mesh's vertex count (retopologize, lower a
generator's subdivision level, or split the asset into several meshes). If it is intentional, no
action is needed — the warning does not block anything.",
    },
    CodeEntry {
        code: code::VOXEL_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "voxel document structure error",
        explanation: "\
A voxel model document (`voxels/*.voxel.yaml`) is malformed: a required top-level key is missing or
has the wrong type, an unrecognized `kind` value was found, or the document has a key the voxel
format does not define.

A voxel document is a strict-subset YAML mapping with a fixed top-level shape:

  format: 1
  kind: voxel
  name: \"lantern\"        # optional
  size:                    # required, three positive integers: width x, height y, depth z
    - 3
    - 4
    - 3
  palette:                 # required, single-glyph keys mapping to a color
    A:
      color:               # four numbers: red, green, blue, alpha
        - 1.0
        - 0.8
        - 0.2
        - 1.0
  slices:                  # required, one row-string layer per y level, bottom-up (y = 0 first)
    -
      - \"A.A\"            # each layer is `size.z` rows of exactly `size.x` characters

`format` must be `1`; `kind` must be exactly `voxel` (a document with no `kind` key at all is a
scene, so `ludion check-scene` never reaches this validator for one); `size`, `palette`, and
`slices` are required, `name` is optional, and no other key is recognized. This one code covers all
of that shape checking — the voxel analog of the mesh format's `L1110`. The JSON Pointer names the
offending key or list element; unknown-key messages suggest the nearest valid key when one is
plausibly a typo.

Fix: match the shape above — add the missing key, fix its type, correct `kind` to `voxel`, or
remove/rename the unrecognized key. Data-level problems (slice dimensions, glyphs, palette keys,
non-finite colors) are the separate `L1114`.",
    },
    CodeEntry {
        code: code::VOXEL_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "voxel data is inconsistent",
        explanation: "\
A voxel model document parsed structurally cleanly, but its grid data does not hang together.

Specifically, one of:

  - `slices` has a number of layers other than `size.y`, a layer has a number of rows other than
    `size.z`, or a row has a character count other than `size.x` (each layer is `size.z` rows of
    exactly `size.x` characters; row r is z = r, column c is x = c);
  - a character in a slice is neither `.` (an empty cell) nor a glyph defined in the palette;
  - a palette key is not a single printable-ASCII character — it is multi-character, non-ASCII, or
    the reserved `.` (which always means an empty cell);
  - two palette keys name the same glyph;
  - a palette color component is not finite (`NaN` or `±Infinity`).

The JSON Pointer names the offending slice row, palette entry, or color component.

Fix: make the slice dimensions match `size`, pad empty cells with `.`, add any missing glyph to the
palette (or correct it), give each glyph a distinct single printable-ASCII character, and replace
any non-finite color component with a real number. Shape/type problems (a missing key, a wrong-typed
value, a bad `kind`) are the separate `L1113`.",
    },
    CodeEntry {
        code: code::VOXEL_SIZE_BUDGET_EXCEEDED,
        severity: Severity::Warning,
        title: "voxel model exceeds the style cell budget",
        explanation: "\
A voxel model document declares more than 262,144 cells (`size.x * size.y * size.z`, i.e. 64^3) —
well above what author-scale voxel content expects (the sweet spot is roughly 16^3 to 48^3).

This is a **warning**, not a validation failure: `ludion check-scene` still exits success, and the
model loads and meshes normally. It exists to catch content that drifted toward world scale — a
single enormous file where chunked models belong.

Fix: if the size is unintentional, shrink the model. If a large model is genuinely needed, split it
into several smaller models (the engine has no chunked-voxel streaming); one huge file is what this
budget warns against. If the size is intentional and a single file is acceptable, no action is
needed — the warning does not block anything.",
    },
    CodeEntry {
        code: code::VOXEL_DISCONNECTED_COMPONENTS,
        severity: Severity::Warning,
        title: "voxel model has disconnected components",
        explanation: "\
A voxel model document's filled cells do not all touch: they split into more than one 6-connected
component (two cells are connected only when they share a face — not an edge or a corner), so part of
the model floats free of the main body.

This is the classic \"disconnected limb\" — a floating arm, a detached head, a stray cell left after
an edit — that used to be invisible until a GPU render caught it. It is a **warning**, not a
validation failure: `ludion check-scene` still exits success and the model meshes normally, because a
deliberately detached accent (a hovering rune, a dotted trail) is legal content.

The first line names the total component count and the main body's cell count. One indented line per
stray piece follows, smallest first — the smallest stray component is the likeliest mistake:

    this voxel model's filled cells split into 2 disconnected components (main body 1058 cells); each stray piece, smallest first:
      141 cells, sample (2, 0, 1); nearest cells to the main body: stray (5, 18, 6) vs main (7, 18, 6), gap 2 (1 bridge cell to connect)

The sample cell locates the piece; the **nearest pair** locates the fix, and the two are often far
apart — a stray shoulder's first cell in scan order can sit a dozen layers below the single gap that
severed it. `gap N` is the 6-connected step distance between the two named cells (their Manhattan
distance), and `N - 1` is how many cells lie between them.

Fix: if a piece was meant to attach, fill the cells between the nearest pair (6-connectivity, so a
purely diagonal touch does not count as connected). Beware the `line` op with a diagonal step: it
draws exactly such a chain of edge-touching cells, which looks continuous in a slice print and is not
connected — bridge with face-adjacent `set` steps instead. If the detachment is intentional, no
action is needed — the warning does not block anything.",
    },
    CodeEntry {
        code: code::VOXEL_OPS_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "voxel ops document structure error",
        explanation: "\
A voxel ops document — the `--ops` input of `ludion voxel apply` — is malformed.

An ops document is a strict-subset YAML mapping with a fixed shape: an envelope plus a sequence of
typed operations, each addressed in **voxel coordinates**, applied in order:

  format: 1
  kind: voxel-ops
  ops:
    - op: set              # one cell
      at: \"3,10,4\"         # x,y,z as one comma-joined token
      glyph: H
    - op: box              # every cell of an inclusive box
      from: \"0,0,0\"
      to: \"5,2,5\"
      glyph: F
      mode: paint          # optional: `place` (default) writes anywhere, `paint` only over filled

Coordinates are comma-joined strings because the strict YAML subset has no flow sequences — the same
convention `check-scene --column x,z` and `map-check --at x,z` use. `ellipsoid`/`cylinder` take
decimal `center`/`radii`/`base`/`radius`/`height`; every other coordinate is a whole number.

This code covers the document's shape only: a missing or wrong-typed `format`/`kind`/`ops`, an entry
that is not a mapping, an unknown `op` name or an unknown key inside one, a key the named op
requires but does not have, or a coordinate token that is not the expected shape. The JSON Pointer
names the offending op and key (`/ops/3/at`), and an unknown name suggests the nearest valid one.

Fix: match the shape above. An op that is well-formed but cannot be applied to the model (a cell
outside the grid, a glyph with no palette entry) is the separate `L1132`.",
    },
    CodeEntry {
        code: code::VOXEL_OP_INAPPLICABLE,
        severity: Severity::Error,
        title: "voxel op cannot be applied to this model",
        explanation: "\
A voxel op parsed cleanly but does not fit the model it was applied to.

Specifically, one of:

  - a point op (`set`) addresses a cell outside the grid — point ops are exact, so an out-of-range
    coordinate is a mistake, not something to clip away silently (volume ops — `box`, `ellipsoid`,
    `cylinder`, `line` — DO clip to the grid and never raise this, except a `line` spanning more
    than 16 777 216 cells, which is not a model edit);
  - a glyph names no palette entry (only `.`, which erases, needs no entry);
  - a reshape op (`crop`) names an empty or inverted range, or one outside the grid;
  - a `grow` or an `upscale` would make a grid of more than 16 777 216 cells (256x256x256).
  - `voxel apply` targets a model carrying `generated_from` — generated models are write-only; edit
    the script.

The message names the op's index in the `ops` sequence and the offending coordinate or glyph; the
JSON Pointer points at the op (`/ops/2`). Nothing is written when this fires — `ludion voxel apply`
validates the whole result in memory and only then replaces the file, so a failed op leaves the
model byte-identical.

Fix: correct the coordinate (`ludion check-scene <model> --slice <y>` prints a layer with a column
ruler, `--column x,z` a vertical profile), or add the glyph to the model's palette before the op
that uses it.",
    },
    CodeEntry {
        code: code::VOXEL_GEN_INTERMEDIATE,
        severity: Severity::Error,
        title: "voxel generator intermediate is malformed",
        explanation: "\
The JSON intermediate that `ludion voxel from-json` reads is malformed.

Generation of a voxel model from scratch is a two-step pipeline: a committed Node script
(`<name>.voxel.gen.mjs`, over the shared `scripts/voxel/grid.mjs` library) builds the grid and
prints ONE JSON object; the engine validates it and writes the canonical `.voxel.yaml`. Scripts
never write YAML — canonical serialization lives in exactly one place.

The intermediate's shape:

  {
    \"format\": 1,
    \"kind\": \"voxel-gen\",
    \"name\": \"brazier\",
    \"size\": [20, 20, 20],
    \"scale\": 0.1,
    \"generated_from\": \"brazier.voxel.gen.mjs\",
    \"budget_exempt\": true,
    \"palette\": {
      \"H\": { \"color\": [0.2, 0.2, 0.2, 1.0], \"emissive\": 2.0, \"metallic\": 0.1,
              \"roughness\": 0.7 }
    },
    \"header\": [\"one line on the silhouette\", \"Front = row z=0.\"],
    \"layer_comments\": { \"0\": \"splayed feet\", \"12\": \"hips\" },
    \"layers\": [ [\"....\", \".HH.\"], [\"....\", \".HH.\"] ]
  }

`format`, `kind`, `size`, `palette`, and `layers` are required; `name`, `scale`,
`generated_from`, `budget_exempt`, `header`, and `layer_comments` are optional. `layers[y][z]` is
one row string per z within layer y — the same orientation as the YAML `slices`, so `layers` can
be handed straight through. `layer_comments` keys are layer indices written as decimal strings
(JSON has no integer keys).

This code covers the intermediate's own shape only. Once it parses, the described model goes
through exactly the checks `check-scene` runs, so a row of the wrong width or a glyph with no
palette entry surfaces as the usual `L1114`, and the cell budget as `L1115`.

Fix: match the shape above. `scripts/voxel/README.md` documents the library and the convention.

`ludion voxel from-json --check <model>` also raises this code for a second, unrelated reason: the
intermediate parsed fine, but regenerating it in memory produces bytes that no longer match the
committed `<model>` — a `DRIFT` message names the first line that differs. That is a stale commit,
not a malformed intermediate; regenerate and commit the result. `--validate` runs the same
generation and validation with nothing to compare against, so it can only raise the shape form.",
    },
    CodeEntry {
        code: code::VOXEL_OPS_SHADOWED_BY_MIRROR,
        severity: Severity::Warning,
        title: "a mirror op discarded cells an earlier op wrote",
        explanation: "\
A `mirror` op in a `voxel apply` ops document copies its kept half onto the other half, discarding
whatever was there. If an earlier op in the same run wrote cells inside the half the mirror then
discards, that content is silently gone — usually because the ops were meant to run in the other
order, or the earlier op meant to target the kept half.

This is a **warning**, not a validation failure: the model the ops produce is well-formed either
way, and a mirror deliberately overwriting hand-placed detail (an asymmetric prosthetic sculpted
first, then squared off by a mirror) is legal. The warning exists because that is rarely what an
author meant.

One warning per writer op, tallying every cell of its that the mirror erased:

    op 20 (mirror x keep low) overwrote 6 cells that op 14 (ellipsoid) wrote in the discarded half (x >= 10)

The discarded-half range (`x >= 10`) names exactly the cells the mirror overwrote, in the same
coordinate space `check-scene --slice`/`--column` read. Fix: move the writer op after the mirror,
or aim it at the kept half instead; if the overwrite is intended, no action is needed.

A reshape (`shift`/`grow`/`crop`) between the writer and the mirror does not defeat this — the
write-history that feeds the lint moves with the reshape.",
    },
    CodeEntry {
        code: code::VOXEL_GEN_DOUBLE_ENCODED,
        severity: Severity::Warning,
        title: "the generator intermediate was encoded as UTF-8 twice",
        explanation: "\
The JSON `ludion voxel from-json` just read contains text that went through UTF-8 twice: something
decoded the generator's UTF-8 bytes as Latin-1/CP1252 and re-encoded the result. An em-dash written
as `—` arrives as `\u{e2}\u{20ac}\u{201d}`; `é` arrives as `\u{c3}\u{a9}`. (Those two are written by
code point on purpose — a doc that spells its own corruption literally is one bad re-save from being
wrong.) The JSON is still valid, so the model is still
written — with the mojibake baked into its header docstring and layer comments, where it is
committed and then regenerated forever by the drift gate. Hence a warning at the door.

WHERE IT COMES FROM. On Windows, PowerShell re-encodes a process's stdout as it moves it, so BOTH

    node model.voxel.gen.mjs | ludion voxel from-json --out model.voxel.yaml
    node model.voxel.gen.mjs > model.voxel.json

can corrupt every non-ASCII character in the intermediate. This has reached a committed model in
this repo once. The pipe is safe under a POSIX shell and safe when one program spawns another
directly (the drift gate does exactly that), so it is a Windows-shell hazard, not a pipeline defect.

Fix: keep the bytes away from the shell entirely — have the generator write the file itself and
hand `from-json` the path. Take the path as an argument, so the same script still prints to stdout
when `voxel-drift` spawns it bare (that path involves no shell and must keep working).

    emit({ grid, name, palette, out: process.argv[2] });         // in the generator
    node model.voxel.gen.mjs model.voxel.json                    // then
    ludion voxel from-json --in model.voxel.json --validate

THE DETECTION IS DELIBERATELY NARROW, so ordinary accented prose never trips it. It fires only on a
two-character sequence: one of `Â` `Ã` `â` — what UTF-8's 0xC2, 0xC3 and 0xE2 lead bytes become when
decoded as Latin-1/CP1252 — immediately followed by a character in U+0080..U+00BF or in CP1252's
0x80..0x9F punctuation block (`€ ‚ ƒ „ … † ‡ ˆ ‰ Š ‹ Œ Ž ' ' \" \" • – — ˜ ™ š › œ ž Ÿ`), which is
what a UTF-8 continuation byte becomes. Real text puts an ASCII letter after those three, so `â` in
`château` and `Ã` in `São` are silent. The message names how many sequences were found and the
first one's line.

If the warning is wrong, the text is unusual rather than broken and nothing needs doing — this code
never fails the command.",
    },
    CodeEntry {
        code: code::ASSEMBLY_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "assembly document structure error",
        explanation: "\
An assembly document (`*.assembly.yaml`) is malformed: a required key is missing or has the wrong
type at some level, the document or one of its part entries has a key the format does not define, a
three-component field does not have three components, `mirror` names an axis other than `x`, or a
`clips` entry's name or path is not a string.

An assembly is the model of record for a multi-part voxel model (`Plan/voxel-animation.md`
decisions 3, 4 and 9): it owns the one palette every part's glyphs resolve against, places lean
`kind: part` files into one assembly-global grid, declares the part hierarchy, and binds animation
clips by short name. It is a strict-subset YAML mapping with a fixed top-level shape:

  format: 1
  kind: assembly
  name: player                    # optional
  size:                           # required: the assembled bounding grid, rest pose
    - 12
    - 24
    - 8
  palette:                        # required: exactly a voxel document's palette
    S:
      color:
        - 0.3
        - 0.22
        - 0.18
        - 1.0
  parts:                          # required, non-empty, in authored order
    - name: torso                 # required, unique across the assembly
      file: parts/torso.part.yaml # required, a path relative to this document
      place:                      # required: where the part's cell (0,0,0) sits in the grid
        - 4
        - 10
        - 2
    - name: upper-arm-r
      file: parts/arm.part.yaml
      parent: torso               # optional; absent marks the one root
      place:
        - 9
        - 14
        - 3
      pivot:                      # optional, part-local voxel coords (halves are legal)
        - 1.0
        - 5.0
        - 1.0
      mirror: x                   # optional; `x` is the only axis
  clips:                          # optional: short name -> clip file
    walk: anims/humanoid-walk.anim.yaml

Note the block-style collections: the crate's strict-YAML subset rejects non-empty flow collections
(`L1005`), so `place: [4, 10, 2]` and `S: { color: [...] }` are not accepted here even though
earlier design sketches wrote them that way — write one entry per line instead.

`format` must be `1`; `kind` must be exactly `assembly`; `size`, `palette`, and `parts` are
required, `name`, `clips`, `joints` and `masks` are optional, and no other top-level key is
recognized. A `masks` entry is a mapping with a non-empty `joints` list of part names and optional
`descendants` and `weights` (one number in [0, 1] per listed part). A part entry
takes `name`, `file`, `place`, and the optional `parent`, `pivot`, `mirror` — nothing else; in
particular a part entry has no palette (the assembly owns the only one) and no size (the part file
declares its own). The JSON Pointer names the offending key or list element; unknown-key messages
suggest the nearest valid key when one is plausibly a typo.

Fix: match the shape above — add the missing key, fix its type, correct `kind` to `assembly`,
supply exactly three components for `size`/`place`/`pivot`, or write `mirror: x` (the only mirror
this format has). Value-coherence problems (a duplicate part name, an unknown or cyclic `parent`,
zero or several roots, a part that does not fit the grid) are the separate `L1138`.",
    },
    CodeEntry {
        code: code::ASSEMBLY_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "assembly data is inconsistent",
        explanation: "\
An assembly document parsed structurally cleanly, but its data does not hang together.

Within the document itself, one of:

  - a palette key is not exactly one printable-ASCII character, is the reserved `.` (always an
    empty cell), or is defined more than once; a palette color component is not finite; or an
    `emissive`/`metallic`/`roughness` value is non-finite or out of range (the voxel format's
    palette rules, which an assembly's palette shares exactly);
  - `parts` is empty — an assembly with no parts models nothing;
  - two part entries share a `name`, or a part's `name`/`file` is empty;
  - a `parent` names no part in this assembly (the message suggests the nearest name);
  - a part names itself as its own `parent`;
  - the hierarchy has no root (every part declares a `parent`), more than one root, or a cycle —
    exactly one part must omit `parent`, and following `parent` from any part must reach it;
  - a `pivot` component is not finite;
  - a `place` component falls outside the assembly's own `size` on that axis;
  - a mask has an empty name, or lists a part this assembly does not have (the message suggests the
    nearest part).

Across files — reported when the assembly is checked together with its resolved part documents:

  - a glyph used by a part's slices names no entry in the assembly's palette (parts have no palette
    of their own: that is the whole point of the assembly owning the one palette);
  - a placed part overflows the assembled grid: `place + part size` exceeds `size` on some axis.

The JSON Pointer names the offending palette entry or part entry, always inside the *assembly*
document — a cross-file diagnostic points at `/parts/<i>/...` and names the part file in its
message, because the assembly is where the placement is authored.

Fix: give each part a unique non-empty name, leave `parent` off exactly one part, point every other
`parent` at a real part without forming a cycle, keep `place` and `place + part size` inside `size`,
and add every glyph a part draws with to the assembly's palette. Shape/type problems (a missing key,
a wrong-typed value, a bad `kind`, a `mirror` other than `x`) are the separate `L1137`.",
    },
    CodeEntry {
        code: code::PART_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "part document structure error",
        explanation: "\
A part document (`parts/*.part.yaml`) is malformed: a required top-level key is missing or has the
wrong type, an unrecognized `kind` value was found, or the document has a key the part format does
not define.

A part is one body part of a multi-part `kind: assembly` (`Plan/voxel-animation.md`) — a lean
voxel-slices-only document with no palette of its own. It is a strict-subset YAML mapping with a
fixed top-level shape:

  format: 1
  kind: part
  name: \"upper-arm\"      # optional
  size:                    # required, three positive integers: width x, height y, depth z
    - 2
    - 6
    - 2
  slices:                  # required, one row-string layer per y level, bottom-up (y = 0 first)
    -
      - \"AA\"             # each layer is `size.z` rows of exactly `size.x` characters
      - \"AA\"

`format` must be `1`; `kind` must be exactly `part` (a document with no `kind` key at all is a
scene, and one declaring another `kind` is a different document family entirely); `size` and
`slices` are required, `name` is optional, and no other key is recognized — in particular, `palette`
is not: a part's glyphs resolve against the *owning assembly's* palette, never its own. This one
code covers all of that shape checking — the part analog of the voxel format's `L1113`. The JSON
Pointer names the offending key or list element; unknown-key messages suggest the nearest valid key
when one is plausibly a typo.

Fix: match the shape above — add the missing key, fix its type, correct `kind` to `part`, or
remove/rename the unrecognized key (a `palette` key belongs in the assembly document, not here).
Data-level problems (slice dimensions, illegal glyphs) are the separate `L1140`.",
    },
    CodeEntry {
        code: code::PART_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "part data is inconsistent",
        explanation: "\
A part document parsed structurally cleanly, but its grid data does not hang together.

Specifically, one of:

  - `slices` has a number of layers other than `size.y`, a layer has a number of rows other than
    `size.z`, or a row has a character count other than `size.x` (each layer is `size.z` rows of
    exactly `size.x` characters; row r is z = r, column c is x = c);
  - a character in a slice is neither `.` (an empty cell) nor a printable-ASCII character
    (`0x21..=0x7E`).

A part has no palette of its own, so this is where the resemblance to the voxel format's `L1114`
ends: there is no \"names no palette entry\" case here, because a printable-ASCII glyph is always
structurally legal in a part — it resolves against the owning assembly's palette at composition
time, and a glyph the assembly's palette does not claim is that assembly's lint to raise, not this
one's.

The JSON Pointer names the offending slice row.

Fix: make the slice dimensions match `size`, pad empty cells with `.`, and replace any non-ASCII or
non-printable character with a printable-ASCII glyph (`!` through `~`). Shape/type problems (a
missing key, a wrong-typed value, a bad `kind`) are the separate `L1139`.",
    },
    CodeEntry {
        code: code::ANIMATION_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "animation clip document structure error",
        explanation: "\
An animation clip document (`anims/*.anim.yaml`) is malformed: a required top-level key is missing
or has the wrong type, an unrecognized `kind` value was found, the document (or a track, a
keyframe, or an event) has a key the format does not define, `generated_from` is not a string,
`events` is not a list of `{t, name}` mappings, a keyframe's `v` does not have exactly three
components, or a keyframe's `ease` names no recognized easing.

A clip is one keyframed animation of named targets — the parts of a multi-part `kind: assembly`
(`Plan/voxel-animation.md` decision 8) or the joints of a skin; the binding to a real part or joint
happens later, in whatever plays the clip. It is a strict-subset YAML mapping with a fixed
top-level shape:

  format: 1
  kind: animation
  name: \"humanoid-walk\"    # optional
  generated_from: \"walk.glb\"  # optional; set on an imported clip
  duration: 0.6              # required, the clip length in seconds
  loop: true                 # required, no default: one-shots hold their final pose
  events:                    # optional; named instants, strictly increasing `t`
    - t: 0.3
      name: footstep
  tracks:                    # required, target name (part or joint) -> animated channels
    upper-leg-l:
      rotation:               # optional; `v` is Euler XYZ degrees (`translation`, `scale` likewise)
        - t: 0.0
          v:
            - 25.0
            - 0.0
            - 0.0
        - t: 0.3
          v:
            - -25.0
            - 0.0
            - 0.0
          ease: smooth        # optional; one of `linear` (the default), `smooth`, `step`

Note the block-style keyframes: the crate's strict-YAML subset rejects non-empty flow collections
(`L1005`), so `{ t: 0.0, v: [25, 0, 0] }` is not accepted here even though earlier design sketches
wrote it that way — write each of `t`/`v`/`ease` as its own line instead.

`format` must be `1`; `kind` must be exactly `animation`; `duration`, `loop`, and `tracks` are
required, `name`, `generated_from` and `events` are optional, and no other top-level key is
recognized. A track is a mapping with `rotation`, `translation` and/or `scale`, each a sequence of
keyframes; a keyframe is a mapping with required `t` (seconds) and `v` (three numbers), and an
optional `ease`; an event is a mapping with required `t` and `name`. The JSON Pointer names the offending
key or list element; unknown-key and bad-`ease` messages suggest the nearest valid one when
plausibly a typo.

Fix: match the shape above — add the missing key, fix its type, correct `kind` to `animation`,
supply exactly three numbers for `v`, spell `ease` as one of `linear`/`smooth`/`step`, or remove the
unrecognized key. Value-coherence problems (a non-finite duration, out-of-range or non-monotonic
keyframe times, an empty channel or trackless clip, a non-positive scale) are the separate
`L1142`; event data is `L1198`.",
    },
    CodeEntry {
        code: code::ANIMATION_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "animation clip data is inconsistent",
        explanation: "\
An animation clip document parsed structurally cleanly, but its data does not hang together.

Specifically, one of:

  - `duration` is not finite, not strictly greater than zero, or over 3600 seconds;
  - `tracks` is empty (a clip must animate at least one target);
  - a track declares none of the `rotation`, `translation` or `scale` channels — including a
    channel key present with zero keyframes, which reads the same as never declaring it;
  - a keyframe's `t` or any component of `v` is not finite;
  - a keyframe's `t` falls outside the clip's `[0, duration]` range;
  - a channel's keyframe times are not strictly increasing (keyframes are read in the order
    authored — this format never re-sorts them);
  - a `scale` component is not strictly greater than zero;
  - a `rotation` segment (not a `step` one) turns 180 degrees or more on an Euler axis, or whose
    axes combine past half a circle (the segment would play the other way round) — rotation
    plays the shortest way round, so the segment would not play as written.

The JSON Pointer names the offending clip field, track, or keyframe. Event data (`events`) is the
separate `L1198`.

Fix: give `duration` a finite, positive value of at most 3600 seconds; author at least one track
with at least one non-empty channel; keep every keyframe's `t` inside `[0, duration]`; and order
each channel's keyframes with strictly increasing `t`; keep `scale` positive; split a rotation that
turns half a circle or more, on an axis or combined, into shorter segments. Shape/type problems (a
missing key, a wrong-typed value, a bad `kind`, a malformed `v`) are the separate `L1141`.",
    },
    CodeEntry {
        code: code::ASSEMBLY_UNEXPECTED_OVERLAP,
        severity: Severity::Warning,
        title: "assembly parts overlap outside a joint",
        explanation: "\
Two parts of an assembly fill some of the same cells in the composed rest-pose grid, and neither the
hierarchy nor the declared pivots explain it.

Overlap is legal and expected at a joint: the **joint-plug technique** sinks a limb's end into its
parent's socket so the limb can rotate without ever opening a gap. So an overlap between a part and
its own `parent`, clustered around that child's declared `pivot`, is silent — nothing to report. This
warning is what is left over:

  - **parts that are not parent and child** share cells (a limb intersecting a limb, a prop through a
    torso) — nothing rotates there, so the interpenetration is invisible geometry at best;
  - a part overlaps its own parent **far from its declared `pivot`** — usually a wrong pivot, and the
    more valuable catch: the plug is at the shoulder while the pivot was authored at the wrist, so
    the limb will swing about the wrong end the moment a clip plays.

Overlapping cells are grouped into 6-connected clusters per pair of parts (the same flood fill
`L1129` and `--mirror-x` report regions with), and each cluster is judged on its own. The message
names the pair, the cluster's cell count and bounding box in assembly-global coordinates, and — for a
parent/child pair — the distance from the cluster's center to the pivot, against the plug radius it
was compared to. That radius is a deliberately generous heuristic: half the child part's largest grid
extent, never below 1 cell. The JSON Pointer names the child part (or the later-authored part of an
unrelated pair) in the assembly's `parts` list.

It is a **warning**, not a validation failure: `ludion check-scene` still exits success, the assembly
still meshes, and a deliberate interpenetration this heuristic cannot explain is legal content.

Fix: for a wrong pivot, move the named part's `pivot` onto the socket the overlap sits in (part-local
voxel coordinates, floats allowed — and remember a `mirror: x` part's pivot is authored unmirrored).
For unrelated parts, move one of them with `place` or trim its geometry so they no longer share
cells; if the two really are one joint, declare it with `parent:` so the pivot check applies. If the
overlap is intentional, no action is needed — the warning does not block anything.",
    },
    CodeEntry {
        code: code::DOC_REFERENCE_UNRESOLVED,
        severity: Severity::Error,
        title: "a data-doc names something that does not exist",
        explanation: "\
A game data-doc cross-references an id, a district, a kind, or another doc, and nothing by that name
exists.

Schema validation cannot catch this: the field is a well-typed string, and only the game knows what
the string is supposed to name. So the rule lives in the game's own doc-validation pass, registered
with `EngineBuilder::validate_docs(|world| ...)`. The engine runs every registered pass after the
data-doc stores are installed — at host start, again after a live `asset.reload` of a doc, and under
`--check-docs` — and reports whatever the pass returns.

The diagnostic's path is a document path of the form `<kind>/<doc name>/<field>`, naming the field
that failed to resolve rather than a position in a file.

At host start and under `--check-docs` this is fatal: the game refuses to run on content whose
references do not resolve, exactly as it refuses a malformed doc (`L1119`/`L1120`). After a live
reload it is printed once and the run continues — a half-finished edit mid-tune must not kill the
session.

Fix: correct the naming field, or add the doc/id it names. If the reference is legitimately optional,
the rule belongs in the game's pass, not the content — teach the validator to accept the absence.",
    },
    CodeEntry {
        code: code::DOC_NAME_DUPLICATE,
        severity: Severity::Error,
        title: "two data-docs of one kind share a name",
        explanation: "\
Two game data-docs registered under the same `kind:` carry the same `name`, and a doc's `name` is
the identity everything else resolves it by: another doc's cross-reference, a save file's record, a
system's lookup.

Both files load — a `DocStore` is keyed by the doc's path, not its name — so nothing fails at
schema time. What fails is every read afterwards: a lookup by name answers with whichever doc the
store iterates first, so the second doc is content that exists and can never be reached, while a
system that walks the store still sees both (spawning two of a thing meant to be one).

The rule lives in the game's own doc-validation pass (`EngineBuilder::validate_docs`), which the
engine runs after the stores are installed — at host start, after a live doc reload, and under
`--check-docs`. The diagnostic's path is `<kind>/<doc name>/name`.

Fix: rename one of the two docs. If both are meant to exist, they need distinct names; if one is a
leftover copy, delete it.",
    },
    CodeEntry {
        code: code::DOC_SET_OVER_CAPACITY,
        severity: Severity::Warning,
        title: "more data-docs than the surface that shows them",
        explanation: "\
A game data-doc set is larger than the fixed surface that displays it, so the overflow is loaded,
live, and invisible: more quests than the HUD tracker has rows, more wares than a shop menu lists.

This is a warning, not an error. The content is valid, every doc loads, and the game runs — the
only thing wrong is that a player can never see part of it, which is exactly the kind of failure
that otherwise shows up as \"I authored it and nothing happened\". The rule and the capacity both
belong to the game, so it is raised by its own doc-validation pass
(`EngineBuilder::validate_docs`); the message names the count and the capacity.

Fix: cut the set back to the capacity, or grow the surface (a `UiList` with `max_visible` scrolls a
long set through a short window, where a fixed row per doc cannot).",
    },
    CodeEntry {
        code: code::PATTERN_BULLET_UNKNOWN,
        severity: Severity::Error,
        title: "pattern timeline entry names an unknown bullet kind",
        explanation: "\
A `kind: pattern` document's timeline entry names a bullet under `bullet:` that the document's own
`bullets:` map does not define.

Every timeline entry that isn't a `play:` fires a volley of one bullet kind, looked up by name in
the same document — the vocabulary is local, so a name from a different pattern's `bullets:` map is
exactly as unknown as a typo. The message carries a nearest-match suggestion over the document's
own bullet names.

The JSON Pointer is `/timeline/<i>/bullet`.

Fix: use the suggested name, or add the missing kind under the document's `bullets:` map.",
    },
    CodeEntry {
        code: code::PATTERN_ENTRY_INVALID,
        severity: Severity::Error,
        title: "pattern document value falls outside a bound the schema cannot express",
        explanation: "\
A `kind: pattern` document authors a value outside a bound the JSON Schema cannot express: a
bullet kind's `motion` list with more than four segments, a segment authoring both `ramp` and
`turn` (no combined closed form), a volley `count` above 256 (the kill bitset's width), an
`every: 0` paired with `repeat > 1` (an infinite-rate loop), or a non-finite or negative speed,
frame count, or `life`. Every bound schemars can express is `L1014` instead; this code covers only
what it cannot.

The pointer names the offending field.

Fix: split the motion list, keep only one of `ramp`/`turn` per segment, cap `count` at 256, give
`every` a positive value, and make every speed, frame, and life value finite and non-negative.

Two cases concern a pattern's `loop`: an entry or repeat that fires at or past `loop` (the timeline
wraps first, so it never fires), and a `play` whose leaf fires at or past `loop` once offset by the
entry's `at`. A per-repeat `speed_step` that drives a repeat's speed negative, or an
`angle_step_deg` that drives an angle non-finite, is also `L1149`.",
    },
    CodeEntry {
        code: code::PATTERN_PLAY_UNRESOLVED,
        severity: Severity::Error,
        title: "pattern timeline's play entry does not resolve",
        explanation: "\
A timeline entry's `play: <name>` does not resolve to a playable pattern: the named document does
not exist, the named document itself contains its own `play:` entry (nesting reaches only one
level), or the chain of `play:` references cycles back to the document already playing.

The pointer is `/timeline/<i>/play`.

Fix: name a leaf pattern document — one whose own timeline holds no `play:` entry — or inline its
volleys directly instead of playing it.",
    },
    CodeEntry {
        code: code::SPRITE_ATLAS_UNRESOLVED,
        severity: Severity::Error,
        title: "sprite atlas reference is invalid",
        explanation: "\
A `Sprite` component, or one `Tilemap` legend entry, names an atlas that could not be resolved.

An atlas reference is a pair: `atlas` names a `kind: atlas` document and `frame` names one of its
regions. Both halves are required together, and neither may be combined with `texture` — a quad
takes its pixels from exactly one place, so a half-written reference (or two competing ones) is a
mistake the renderer cannot resolve by picking a side.

This code covers the four ways the pair itself fails: the atlas document could not be read from
disk, its text did not parse as a `kind: atlas` document (`L1153` reports why), `frame` was set with
no `atlas`, or `atlas` was set with no `frame`, or `texture` and `atlas` were both set. A reference
that resolves to a real atlas but names a region the atlas does not have is the separate `L1152`.

Fix: set `atlas` and `frame` together, drop `texture` from the entry, or correct the atlas path.
Atlas paths resolve like every other asset path — relative to the scene that names them. If the
atlas file is missing entirely, regenerate it with `ludion generate-atlas` from its
`kind: atlas-source`; a committed atlas is derived, never hand-edited.",
    },
    CodeEntry {
        code: code::SPRITE_FRAME_UNKNOWN,
        severity: Severity::Error,
        title: "atlas has no region with that name",
        explanation: "\
The atlas resolved and parsed, but has no region named by this `frame`.

A region name is the atlas's own identity for one rectangle of the packed image, minted by the bake:
a sprite document contributes `<doc name>/<frame name>` per frame, a loose PNG input contributes its
file stem. So the usual cause is a stale name — a frame renamed in the `kind: sprite` document, or
a sprite doc renamed, without regenerating the atlas — and the second most common is a plain typo,
which is why the message carries the nearest matching region name.

Fix: use the name the message suggests, or regenerate the atlas from its `kind: atlas-source` with
`ludion generate-atlas` so it carries the region you expect. The atlas document's `regions:` mapping
lists every legal value; it is plain text, sorted by name.",
    },
    CodeEntry {
        code: code::SPRITE_PALETTE_STRUCTURE,
        severity: Severity::Error,
        title: "palette document is malformed or a sprite's palette include does not resolve",
        explanation: "\
A `kind: palette` document does not hold together, or a `kind: sprite` document's shared-palette
include cannot be resolved.

A palette document is a standalone glyph palette — `format: 1`, `kind: palette`, an optional `name`,
and a `palette:` map of single-glyph keys to `{ color: \"#rrggbb\", emissive? }` (the legacy four-float `[r, g, b, a]` spelling still loads), exactly the shape
a sprite document's own inline `palette:` uses. A sprite can reference one instead of inlining a
palette by writing `palette: { from: <path to a kind: palette document>, overrides?: { … } }`; the
`from` path resolves relative to the sprite document's own directory, and the include is resolved at
bake/generate time — the referenced palette with the local `overrides` applied on top produces
exactly the palette an equivalent inline document would, so the two bake to byte-identical images.

This code covers the palette document's own text (a missing or wrong-typed key, an unrecognized key,
a bad palette entry, a colour that is not `\"#rrggbb\"`/`\"#rrggbbaa\"`) and the sprite include's reference (`from` missing or wrong-typed, an
unrecognized key in the include block, a `from` naming a file that cannot be read or that is not a
`kind: palette` document). A palette document's `ramps:` list is checked here too: a ramp of fewer
than two glyphs, a repeated glyph, or a ramp glyph the palette lacks (`ludion docs sprite-format`).
A glyph a frame or effect uses that is still undefined *after* the include
resolves is the ordinary `L1153` — by then the sprite has a concrete palette like any inline one, and
the defect is a sprite that does not agree with itself.

Fix: follow the pointer and the help line. Correct the palette document, or point `palette.from` at a
readable `kind: palette` document and add the glyph the sprite needs to it or to `overrides`.",
    },
    CodeEntry {
        code: code::SPRITE_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "sprite, atlas-source or atlas document is malformed",
        explanation: "\
One of the three sprite-pipeline document families does not hold together: `kind: sprite` (the
authored pixel grid), `kind: atlas-source` (the bake's input list), or `kind: atlas` (the derived,
generated output).

The three share one code because they share one shape and one failure mode — a text document whose
keys, types, or cross-references do not agree with each other. It covers both passes: *structure*
(a missing or wrong-typed key, an unrecognized key with a nearest-match suggestion, a malformed
list, a colour that is not `\"#rrggbb\"`/`\"#rrggbbaa\"`) and *semantics* (a frame's `rows` not
matching `size` exactly, a glyph naming no palette entry, a duplicate frame or region name, a pivot
outside its frame, a region rect outside the packed image, a mirrored frame's rows not matching its
kept half, a patch outside its frame, a `base` naming no `rows:` frame, `mirror` on a patched frame, a sprite's `ramps:` list with a one-glyph ramp, a repeated glyph or a glyph the palette lacks (`ludion docs sprite-format --section \"Ramps\"`),
an input path that names no file, a `hold` list a different length from its flipbook's
`frames`). `generate-atlas --check` drift — a committed atlas that no longer matches what its source
regenerates — reports under this code too, since the defect is the same one: a document that does
not agree with itself.

Defects inside a sprite document's `effects:` list are the separate `L1154`.

The JSON Pointer names the exact position: `/frames/idle/rows/3`, `/palette/~1` (a `/` glyph, RFC
6901-escaped), `/regions/ship~1idle/rect/2`, `/flipbooks/0/hold`.

Fix: follow the pointer and the help line. For a `kind: atlas` document, do not hand-fix it —
correct its `kind: atlas-source` and re-run `ludion generate-atlas`; a derived document is
write-only, and an edit to it is undone by the next bake and caught by the `atlas-drift` gate.",
    },
    CodeEntry {
        code: code::FONT_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "font document is malformed",
        explanation: "\
A `kind: font` document does not hold together: the glyph table, the metrics, or the atlas it
describes disagree with themselves.

It covers both passes: *structure* (a missing or wrong-typed key, an unrecognized key with a
nearest-match suggestion, a `spread_px` on a bitmap face or missing on an SDF one, a `bounds`
without its `rect`) and *semantics* (a `units_per_em` outside `1..=16384`, a font-unit value
beyond ±1048576, metrics that leave no line height, an `image` path that is absolute or climbs
out with `..`, glyphs not in strictly ascending code-point order, a glyph `rect` outside the atlas or with no
area, an alias whose target has no glyph or whose source is itself a glyph, a kerning pair naming a
char with no glyph, a `missing` char that has a glyph).

`ludion generate-atlas --check` raises it too when a committed font pair drifts from its source.

The JSON Pointer names the exact position: `/glyphs/12/rect/2`, `/atlas/spread_px`,
`/kerning/A/V`.

Fix: follow the pointer and the help line. A generated font document is never hand-edited —
correct its source and regenerate it.",
    },
    CodeEntry {
        code: code::FONT_SOURCE_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "font source document is malformed",
        explanation: "\
A `kind: font-source` document — the authored input of a font bake — does not hold together.

It covers: a missing or wrong-typed key (`file`, `mode`, `em_px` and `charset` are required), an
unrecognized key with a nearest-match suggestion, an `em_px` outside `1..=256`, a `spread_px` on a
bitmap face or missing on an SDF one (or larger than `em_px`), a `charset` range keyword that is not
`ascii`, `latin-1` or `general-punctuation`, a `charset` that resolves to no character or to more
than 4096 (kerning shapes every ordered pair), a `line_height` outside `1..=65535` font units or one
that leaves the ascender at or below zero (raised by the bake), an atlas over 4096 on a side, and a
`name` that is not a plain file stem (it names the baked `<name>.font.yaml` and `<name>.png`, so it holds no separator).

The JSON Pointer names the exact position: `/em_px`, `/charset/ranges/1`.

Fix: follow the pointer and the help line.",
    },
    CodeEntry {
        code: code::FONT_FILE,
        severity: Severity::Error,
        title: "font file cannot be baked",
        explanation: "\
The font file named by a `kind: font-source` document could not be read as a face.

It is raised when the file is missing or unreadable, is not a TrueType/OpenType face, is a font
collection (`.ttc`), is truncated or corrupt, has no usable `cmap` table to map characters to
glyphs, or has a glyph that flattens past the segment limit (16384).

Fix: point `file` at a single `.ttf` or `.otf` face. Split a collection into its faces first. For a
glyph past the segment limit, drop the character from `charset` or use a face with simpler
outlines.",
    },
    CodeEntry {
        code: code::FONTS_TABLE_STRUCTURE,
        severity: Severity::Error,
        title: "font table is malformed",
        explanation: "\
A `kind: fonts` document — the content root's `fonts.yaml`, mapping logical face names to
`*.font.yaml` documents — does not hold together.

It covers: a missing or wrong-typed key (only `format`, `kind` and `fonts` are allowed, and `fonts`
is a map), a face name that is empty or contains whitespace or `/`, and a path that is absolute,
carries a drive or stream prefix, contains a `..` segment, is not in canonical form (a `\\`, an
empty or `.` segment: write `fonts/x.font.yaml`), or does not end in `.font.yaml`.

The JSON Pointer names the exact position: `/fonts/body`.

Fix: follow the pointer and the help line. The reserved name `default` sets the face used by text
that names none.",
    },
    CodeEntry {
        code: code::FONT_NAME_UNKNOWN,
        severity: Severity::Error,
        title: "font name is not in the font table",
        explanation: "\
A `Text`, `UiText` or `UiList` component's `font` names a face that the content root's
`fonts.yaml` does not define, or the root has no `fonts.yaml` at all.

`ludion check-scene` reports it as an error with a nearest-name suggestion; a running engine
reports it once per name and draws and measures the text with the default face — the table's
`default` entry, else the built-in 8x8 face.

Fix: add the name to `fonts.yaml` (`kind: fonts`), or correct the component's `font`.",
    },
    CodeEntry {
        code: code::FONT_LOAD,
        severity: Severity::Error,
        title: "font face cannot load",
        explanation: "\
A face listed in the content root's `fonts.yaml` could not be loaded: the file is missing or
unreadable, or its `kind: font` document is invalid (the pointer names the position in that
file), or the table itself cannot be read. A face whose atlas PNG is a different size from the
size its document declares is also this code, raised by extract and `render --font`, and the text
is not drawn. `asset.reload` of a font document is refused with it while a recording is armed,
because measurement is sim-visible. An unreadable atlas image is `L1101`/`L1102`.

Text naming a face that failed to load falls back to the table's `default` face, else the built-in
8x8 face.

Fix: check the path in `fonts.yaml`, and regenerate the face with `ludion generate-atlas` from its
`kind: font-source` document.",
    },
    CodeEntry {
        code: code::FONT_GLYPH_MISSING,
        severity: Severity::Warning,
        title: "text uses characters the font lacks",
        explanation: "\
Authored text contains characters that the face it resolves to has no glyph for, so they draw as
the face's `.notdef` glyph (tofu). The built-in 8x8 face draws them blank. Newlines are not
reported.

`ludion check-scene` raises it as a warning per text field, listing the distinct missing
characters. It covers faces from the font table only, never the built-in 8x8 face.

Fix: add the characters to `charset.chars` in the face's `kind: font-source` document and re-run
`ludion generate-atlas`, or change the text.",
    },
    CodeEntry {
        code: code::DOC_TYPE_RESERVED_FIELD,
        severity: Severity::Error,
        title: "data-doc type declares an envelope field",
        explanation: "\
A type registered as a data-doc kind (`doc_kind`) or as the player profile declares a top-level
field named `format` or `kind`. Every such document starts with an envelope of `format:` and
`kind:` lines, so the field collides with it: writing the document emits two `kind:` lines (a
duplicate-key error on the next read), and checking it strips the field before validation, so the
value never arrives.

Only the top level is reserved; a nested field called `kind` or `format` is fine. The engine refuses
the registration when it is built.

Fix: rename the field, for example `#[serde(rename = \"weapon_kind\")]`, and update the committed
documents to match.",
    },
    CodeEntry {
        code: code::VOXEL_COMMENT_DROPPED,
        severity: Severity::Warning,
        title: "voxel edit dropped a comment",
        explanation: "\
`ludion voxel apply` or `ludion voxel upscale` re-emitted a model and a comment in the source had
no place to go in the result, so it was dropped. The model itself is correct.

Comments live next to the layer or cell rows they annotate. A reshape or y-shift can move the layer
a comment sat on off the grid; an upscale multiplies every row, so a comment is not re-anchorable;
and a comment where the writer cannot put it back (between keys it does not emit) has no slot. The
message names the comment text and the line it sat on.

Fix: re-add the comment by hand in the written model, or keep the information elsewhere (the
model's `name:` or a neighbouring document).",
    },
    CodeEntry {
        code: code::COMFY_REPLY_MALFORMED,
        severity: Severity::Error,
        title: "ComfyUI server reply is malformed",
        explanation: "\
The ComfyUI server answered, but its reply was not the shape `ludion gen` reads: `/object_info` was
not an object of node classes.

The address resolved and the server responded, so this is not `L1167`; it is usually a different
service on that port, a reverse proxy answering with its own page, or a ComfyUI version whose API
changed.

Fix: check that `LUDION_COMFY_URL` (or `--url`) points at a ComfyUI server, and open it in a browser
to see what is actually answering.",
    },
    CodeEntry {
        code: code::SPRITE_EFFECT_INVALID,
        severity: Severity::Error,
        title: "sprite effects entry is invalid",
        explanation: "\
One entry of a sprite document's `effects:` list is not a valid effect.

`effects:` is a fixed, enumerated op set — `gradient`, `outline`, `shade`, `shadow`, `dither`,
`noise`, `blur`, `glow`, `light` — applied in list order at bake time. It is deliberately not an expression language
and not a node graph (`engine-pillars.md`: Rust is the only Turing-complete layer), so every op has
a closed parameter set that is checked exactly, and an op the set does not name is an error with a
nearest-match suggestion rather than a silently ignored entry.

This code covers: an unrecognized `op`; an unknown key inside an entry; a required parameter absent
(`gradient`'s `from`/`to`, `outline`'s `color`, `noise`'s `amount`, `blur`'s `radius`, `dither`'s
`glyphs`); a colour that is not `\"#rrggbb\"`/`\"#rrggbbaa\"`; a parameter outside its range (`strength`, `density` and `amount` in `0..=1`, `radius` in
`1..=8`, a `light` or `shadow` `offset` of `[0, 0]`, which would make the op a no-op); a `glyphs:`
entry that is not a single glyph the document's `palette` defines; a `frames:` entry naming no frame
of the document; and a `dither` whose `glyphs:` is not exactly two.

Ramp mode (`mode: ramp` on `shade`/`gradient`, with a signed whole-number `steps`) adds its own cases:
no `ramps` to step along, a target glyph that is in no ramp, a ramp-mode op placed after a colour-mode
op (every ramp-mode op comes first; `dither` may sit anywhere), and a colour key (`from`, `to`,
`blend`, `strength`) on a ramp op (`ludion docs sprite-effects --section \"Ramp mode\"`).

`light` adds its own cases: no source (an empty or unknown `sources` glyph, or no `emissive` glyph in
the palette), a `radius_px` outside `0 < radius_px <= 32`, `bands: 0`, a negative `steps`, and a
`strength` or `color` on a ramp-mode light (`ludion docs sprite-effects --section \"Self-light\"`).

The JSON Pointer names the entry and the field: `/effects/2/glyphs/0`, `/effects/0/from`.

Fix: correct the named field. If the effect you want has no op, that is a gap in the op set to
record, not something to paint into the glyph grid by hand — a gradient, glow, shadow or shading
drawn as glyphs is a defect `ludion docs sprite-format` calls out, because it cannot be retuned.",
    },
    CodeEntry {
        code: code::COLLIDER_NON_POSITIVE_EXTENT,
        severity: Severity::Error,
        title: "collider size is not positive and finite",
        explanation: "\
A `Collider` component's shape has a size, or a `rigid` body has a `mass`, that is not a strictly
positive, finite number.

A collider is a shape on the ground plane (x, z) or in 3D: a `circle`/`sphere` with a `radius`, an
`aabb`/`box` with two or three `half_extents`, or a `capsule`'s `half_height`/`radius`. A `rigid`
body's `mass` is likewise a size in this sense — the solver divides by it. Every one of those numbers
must be strictly greater than zero and finite. A zero, negative, or non-finite value has no
meaningful collision volume or mass — it would collapse the shape, or feed a NaN/infinity into the
spatial queries or the solver. JSON Schema's range keyword is inclusive (`minimum: 0` still admits
zero) and does not reach array elements, so this invariant lives in the component's semantic
validator rather than its schema — the same split the `Tilemap` validator uses.

The JSON Pointer names the offending value: `/shape/circle/radius`, `/shape/aabb/half_extents/<index>`,
or `/body/rigid/mass`, among others. Fix: give every collider radius, half-extent, and rigid mass a
value greater than zero. (A negative `radius` is already rejected by the schema as `L1014` with a
field pointer; this code covers the zero and non-finite cases the schema cannot express.)

A `rigid` body's `gravity_scale` is the one exception to \"positive\": zero floats and a negative
value rises, so it must only be finite (a number beyond `f32` range is rejected). The pointer is
`/body/rigid/gravity_scale`.

A `cylinder`'s `half_height`/`radius` follow the same rule as a `capsule`'s, and every size inside a
`compound` part is checked at `/shape/compound/parts/<i>/shape/...`.",
    },
    CodeEntry {
        code: code::AUDIO_EMITTER_NON_POSITIVE_RANGE,
        severity: Severity::Error,
        title: "AudioEmitter range is not positive and finite",
        explanation: "\
An `AudioEmitter` component's `range` is not a strictly positive, finite number.

`range` is the distance at which the emitter falls silent: the engine syncs the loop at
`gain × clamp(1 − distance/range, 0, 1)`, so `range` divides the listener distance. Zero, negative,
or non-finite has no meaningful falloff — the emitter would be inaudible everywhere, or feed a
NaN/infinity into the gain. JSON Schema's range keyword is inclusive (`minimum: 0` still admits
zero), so this invariant lives in the component's semantic validator rather than its schema — the
same split the `Collider` validator uses (`L1116`).

The JSON Pointer names the offending value: `/range`. Fix: give the emitter a `range` greater than
zero — the distance, in world units, past which the sound should no longer be heard. To silence an
emitter without removing it, set `gain: 0.0` (or remove the component) instead.",
    },
    CodeEntry {
        code: code::AUDIO_LISTENER_DUPLICATE,
        severity: Severity::Warning,
        title: "more than one audio listener",
        explanation: "More than one entity carries an `AudioListener`. A world has one ear, so one listener. `ludion
check-scene` names both entities at `/entities/<i>/components/AudioListener`; at runtime only the
listener on the lowest entity id is heard from and the rest are ignored.

Fix: keep one `AudioListener`, on the entity the sound should be measured from (a child of the player at
head height, say).",
    },
    CodeEntry {
        code: code::AUDIO_ZONE_INVALID,
        severity: Severity::Error,
        title: "an audio zone is invalid",
        explanation: "\
An `AudioZone` field is out of range (pointer `/entities/{i}/components/AudioZone/{field}`): a box
`half_extents` entry or a sphere `radius` that is not finite and above 0, a `blend_m` that is negative
or not finite, more than eight `beds` (`/beds/8`), a bed `gain` or a `bus_gains` entry that is negative
or not finite, or an explicit `reverb` field outside its range (`decay_s` 0.1..=20, `pre_delay_ms`
0..=200, `damping`, `size` and `wet` 0..=1). A scene with one is refused.

Fix: correct the named field. See `ludion docs audio-zones`.",
    },
    CodeEntry {
        code: code::MODEL_SHEET_INPUT,
        severity: Severity::Error,
        title: "model view harness input is not a renderable model",
        explanation: "\
`render --model` (the model view harness) synthesizes a turntable contact sheet around a single
model document — a `voxels/*.voxel.yaml`, `meshes/*.mesh.yaml`, or `*.assembly.yaml` file. This error
means the input it was pointed at is not one of those:

  - the file could not be read (a wrong path, or a permissions problem);
  - the document has no top-level `kind:` key — a scene document has no `kind:`, so it is a scene,
    not a model, and is rendered with `render --scene` instead;
  - the document's `kind:` is something other than `mesh`, `voxel`, or `assembly`.

The message names the path and what was found. Fix: point `render --model` at a `.voxel.yaml`,
`.mesh.yaml`, or `.assembly.yaml` document (add the `kind:` key if it is missing), or render a scene
with `render --scene`. A model document that IS recognized but malformed fails instead with its own
`L1110`/`L1111` (mesh), `L1113`/`L1114` (voxel), or `L1137`/`L1138` (assembly) diagnostics, which
name the offending field.

The same code covers the pose flags — `--clip <name>`, `--time <seconds>`, `--strip <n>` — which
render an assembly at an instant of one of its clips instead of at its rest pose:

  - `--time`/`--strip` without `--clip`: both are modifiers of a clip, and name no clip themselves;
  - `--clip` against a `.voxel.yaml`/`.mesh.yaml` model, which is one rigid piece with no parts to
    pose and no `clips:` map;
  - `--clip` naming a clip the assembly does not bind (the message lists the ones it does);
  - `--clip` alongside several `--model` paths: a clip poses one model, and a lineup compares the
    rest poses of several;
  - `--strip <n>` outside 2..=8, or more than one `--views` entry with `--strip` (a strip lays every
    phase out in one cell, so it takes one view, exactly like a lineup);
  - a pose that scatters the model past the gap analysis's grid limit — a clip whose `translation`
    keys, which are in part-local voxel units, are being read as world metres.

Fix: `check-scene <model>.assembly.yaml` lists the clips an assembly binds, and the flags' `--help`
gives their spelling.

The same code covers the optional `--rig <scene>` that borrows an existing scene's lighting for the
sheet, in two cases:

  - the rig scene could not be read (a wrong path, or a permissions problem);
  - the rig scene is valid but holds nothing to lift — none of its entities carries an
    `Environment`, `PostProcess`, `DirectionalLight`, `PointLight`, or `SpotLight` component.

Fix: point `--rig` at a scene that actually holds the lighting you want borrowed (e.g.
`examples/gallery/night-street.yaml`). A rig scene that is not a VALID scene reports the scene
format's own diagnostics instead, unchanged.",
    },
    CodeEntry {
        code: code::UI_ELEMENT_NON_POSITIVE_SIZE,
        severity: Severity::Error,
        title: "text or UI element size is not positive and finite",
        explanation: "\
A `UiRect`, `UiText`, `UiList` or `Text` component's size is not a strictly positive, finite number.

`UiRect.size` is a `[width, height]` pair in canvas units; `UiText.size` and `UiList.size` are the
glyph cell height in canvas units; `Text.size` is the world height of one line. Every one of those
numbers must be strictly greater than zero and finite. A zero, negative, or non-finite size has no meaningful extent — it would collapse the element
to nothing, or feed a NaN/infinity into the anchor-resolution math. This invariant lives in the
component's semantic validator rather than its schema, the same split the `Collider` validator
(`L1116`) uses.

The JSON Pointer names the offending value: `/size/0` or `/size/1` for a `UiRect` axis, `/size` for a
`UiText`, a `UiList` or a `Text`. Fix: give the element a strictly positive, finite size.
(`offset`, `order`, an empty `UiText.text` and an empty `UiList.items` are all legal and
unvalidated — only the size axes are checked.)",
    },
    CodeEntry {
        code: code::DOC_ENVELOPE_INVALID,
        severity: Severity::Error,
        title: "game data-doc envelope is invalid",
        explanation: "\
A game data-doc (a `kind:`-tagged text document a game registers with `doc_kind::<T>`, e.g. a
`kind: weapon` file) must open with a two-key envelope before its own fields:

  format: 1
  kind: weapon
  <the registered type's own fields follow, flattened at the top level>

This error means that envelope is malformed, for one of:

  - `format` is missing, is not an integer, or is not version 1 (the only version this engine
    reads);
  - `kind` is missing, is not a string, or does not match the kind the doc was loaded as — a
    `kind: enemy` file cannot be read as a `weapon`;
  - a top-level key is a near-miss of an envelope key (a typo'd `format`/`kind`);
  - the engine's boot/reload content-root walk found a `*.yaml` whose `kind:` names neither an
    engine kind (`mesh`/`voxel`) nor a kind the game registered with `doc_kind::<T>` — a typo'd or
    unregistered kind, reported with a nearest-match over the registered kinds.

The message names the kind. The JSON Pointer names the offending key — `/format`, `/kind`, or the
typo'd key — and is empty when a required key is entirely missing (there is no node to point at).

Fix: put `format: 1` and a matching `kind:` at the top of the document. Field-level problems (a
wrong-typed value, a missing required field, a typo'd field name) are the separate `L1120`, which
reports each with a pointer into the field.",
    },
    CodeEntry {
        code: code::DOC_SCHEMA_MISMATCH,
        severity: Severity::Error,
        title: "game data-doc does not match its kind's schema",
        explanation: "\
A game data-doc's envelope (`format`/`kind`) is well-formed, but its own fields do not match the
JSON Schema of the type the kind is registered with (`doc_kind::<T>` captures `schema_for::<T>()`).
This is the exact same schema check scene components pass through (`L1014`), applied to a doc's
fields instead of a component's:

  - a field has the wrong type (a string where a number is expected, and so on);
  - a numeric field is out of the schema's declared range;
  - a required field is missing;
  - a field name is not one the type declares — reported as a typo, with a nearest-match suggestion.

The message names the kind. The JSON Pointer names the offending field (`/damage`, `/stats/0`, …),
and a typo carries `nearest match is \\`<field>\\``.

Fix: correct the field the pointer names to match the type — the same discipline a scene component's
`L1014` asks for. Envelope problems (a missing/wrong `format` or `kind`) are the separate `L1119`.",
    },
    CodeEntry {
        code: code::DOC_KIND_NOT_REGISTERED,
        severity: Severity::Warning,
        title: "data doc skipped: kind not registered on this engine",
        explanation: "\
The engine walked its content root, found a `*.yaml` declaring a `kind:` it does not know, and
skipped the file instead of loading it.

This is the built-in engine's answer, not a game's. `ludion serve` and `ludion sim-hash` host an
arbitrary scene through an engine that registers only the engine's *own* data-doc kinds (`pattern`);
it never sees a game's `doc_kind::<T>` registrations, because those live in the game binary. A
content root holding a game's own docs — `enemies/*.yaml`, `weapons/*.yaml` — therefore presents
kinds this engine cannot know, and refusing to boot on them would make every game scene unhostable
by the CLI.

Once a game registers any kind of its own, the strict reading is back: an unknown `kind:` is then a
typo in content the engine *does* own, and fails the boot as `L1119` with a nearest-match
suggestion.

The message names the file and the kind it declares. Nothing is loaded from the skipped file, so a
system reading that kind's store sees nothing.

`<game> --check-docs` treats this code as a failure, not a warning: a document nothing validated is
not validated, so a game whose content root holds data-docs of a kind it never registered cannot pass.

Fix: nothing is required to host the scene — the skip is expected. To load the documents, host the
scene through the game's own binary, whose builder registers the kind.",
    },
    CodeEntry {
        code: code::SCULPT_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "sculpt document structure error",
        explanation: "\
A `kind: sculpt` document (`sculpts/*.sculpt.yaml`, an implicit-surface model of named primitives)
does not have the shape the format requires.

The document is a strict-subset YAML mapping, keys in this canonical order:

  format: 1
  kind: sculpt
  name: idol            # optional
  resolution: 32        # optional integer, cells per world unit
  triangles: 4000       # optional integer, decimation target
  budget_exempt: true   # optional
  palette:              # required, the voxel palette shape
    S:
      color:            # block style: one number per line, no [a, b] lists
        - 0.5
        - 0.5
        - 0.5
        - 1.0
  bones:                # required; the first entry is the root
    - name: root
  parts:                # required, evaluated in file order
    - name: body
      shape: capsule
      ...

This code covers: an empty or non-mapping document; a missing `format`, `kind`, `palette`, `bones`
or `parts`; a wrong-typed value (a string where a number is expected, a float where `resolution`,
`triangles` or `repeat.count` needs an integer, a vector without exactly three numbers); a bone or
part that is not a mapping, or is missing `name` (and, for a part, `shape` or that shape's required
dimension); a `shape` other than `sphere`, `capsule`, `ellipsoid`, `box`, `torus`, `cone`, `voxel`,
`mesh`; an `op`
other than `union`, `subtract`, `intersect`; a `mirror` other than `x`, `y`, `z`; and a key that no
document, bone, part or `repeat` has at all.

The JSON Pointer names the offending key (`/parts/2/radius`, `/bones/1`); an unknown key or value
carries a nearest-match suggestion.

Fix: correct the named key. A key that exists but belongs to a *different* shape (a `radii` on a
`box`) is the separate `L1157`, which lists the keys the part's shape takes.",
    },
    CodeEntry {
        code: code::SCULPT_DOCUMENT_DATA,
        severity: Severity::Error,
        title: "sculpt document data is inconsistent",
        explanation: "\
A `kind: sculpt` document parsed structurally, but its values do not describe a model the mesher
can build.

This code covers:

  - a non-finite number anywhere (`NaN`, `±Infinity`);
  - `resolution` outside `4..=128`, or `triangles` below 12;
  - `budget_exempt: false` (the key only means something as `true`);
  - an empty part name, or a part name used twice (an empty `parts: []` is legal — a scaffold);
  - a non-positive dimension: `radius`, `tube`, `height`, or a component of `radii` or `size`;
  - a box `round` that is negative or exceeds half the box's smallest `size`;
  - a negative `blend`, or a `repeat` whose `count` is below 2;
  - a union part with no `material`, a `material` naming no palette glyph, or a `material` on a
    subtract or intersect part (they contribute no material, so it would never be seen);
  - a capsule part carrying a non-zero `at` (a capsule is placed by `from` and `to`);
  - a `deform` with none of `bend`, `twist`, `taper`, or a `taper` scale that is not positive;
  - a `noise` whose `amplitude` is negative or whose `scale` is not positive;
  - a key that belongs to another shape than the part's own — a `radii` on a `box` — reported
    with a nearest match and the key list of the part's shape;
  - a palette defect: a key that is not one printable-ASCII glyph (or is the reserved `.`), a
    duplicate glyph, a non-finite color, a `metallic`/`roughness` outside `[0, 1]`, a negative
    `emissive`.

The JSON Pointer names the offending value (`/parts/3/size/1`, `/resolution`,
`/palette/S/roughness`).

Fix: correct the named value. Structure problems (a missing key, a wrong type, an unknown shape) are
`L1156`; bone-tree problems are `L1160`.",
    },
    CodeEntry {
        code: code::SCULPT_PART_UNRESOLVED,
        severity: Severity::Warning,
        title: "sculpt part is not resolved by the mesh",
        explanation: "\
A part of a `kind: sculpt` document leaves no trace in the meshed surface: it is thinner than two
grid cells at the document's `resolution` (spacing `1 / resolution` world units), or it contributes
no surface vertex at all — swallowed by a larger part, carved away by a later `subtract`, or placed
inside another body.

This is a **warning**, not a failure: the model meshes and renders. It exists because a part that
is authored and never seen is almost always a mistake, the mesh analogue of a wrong pivot — and one
a render alone does not reveal.

The message names the part and its JSON Pointer (`/parts/<i>`), its smallest dimension against the
cell size, or its surface vertex count. When the part is swallowed it also names the culprit, as
\"hidden by part <name>\": the part that owns the field where the victim sits inside, which is often
written *after* the victim rather than beside it in the list.

Fix: thicken the part to at least two cells, raise `resolution`, move it out of the body that hides
it, or delete it if it is not meant to show.",
    },
    CodeEntry {
        code: code::SCULPT_DISCONNECTED,
        severity: Severity::Warning,
        title: "sculpt mesh has disconnected components",
        explanation: "\
The meshed surface of a `kind: sculpt` document splits into more than one connected component, so
part of the model floats free of the main body.

This is a **warning**, not a failure: a deliberately detached accent (a hovering rune, a halo) is
legal content. It exists to catch the limb a changed `at`, `radius` or `blend` pulled away from the
body — a gap that is easy to miss on a sheet.

The message has the shape of the voxel `L1129` report: the component count and the main body's
vertex count, then one line per stray piece, smallest first, naming its vertex count and the
**nearest pair** of vertices between it and the main body with the distance between them in world
units. The nearest pair locates the fix.

Fix: if the piece was meant to attach, move the part toward the named point, grow it, or raise the
`blend` of the part joining it. If the detachment is intentional, no action is needed.",
    },
    CodeEntry {
        code: code::SCULPT_BONE_TREE,
        severity: Severity::Error,
        title: "sculpt bone tree is invalid",
        explanation: "\
A `kind: sculpt` document's `bones:` list does not form one tree, or a part is parented to a bone
that is not in it.

Every part carries a `bone` (default: the root), and `bones:` lists `{name, parent}` entries in
authored order; the first entry is the root and has no `parent`, every later entry names one. A bone
may also carry `head` and `tail`, the two ends of the segment it spans — both or neither.

This code covers: an empty `bones:` list (no root); an empty bone name, or one used twice; a first
bone with a `parent`; a later bone without one (a second root); a `parent` naming the bone itself or
no bone of the document; a chain of `parent` links that loops; a `head` without a `tail` or the
reverse, or a non-finite component in either; and a part `bone` naming no bone.

The JSON Pointer names the offending entry (`/bones/2/parent`, `/parts/4/bone`, `/bones`); an unknown
bone name carries a nearest-match suggestion over the document's bone names.

Fix: give the first bone no `parent` and every other bone the name of an earlier one, and make
every part `bone` name a bone in the list. An unrigged model needs exactly one bone:

  bones:
    - name: root",
    },
    CodeEntry {
        code: code::SETTINGS_ENVELOPE,
        severity: Severity::Warning,
        title: "settings file envelope is malformed",
        explanation: "\
The player settings file (`save/settings.yaml`) has no valid `format`/`kind` envelope: `format` is
missing or not an integer, `kind` is missing or not `settings`, or the document is not a mapping. The
engine backs the file up (`settings.yaml.bak.<hash>`), starts from defaults and reports `L4340`.

Fix: nothing is required; the next `save()` writes a fresh file. To recover the old values, open the
backup and copy them across; a minimal file is:

  format: 1
  kind: settings",
    },
    CodeEntry {
        code: code::SETTINGS_RESET,
        severity: Severity::Warning,
        title: "settings file could not be loaded — defaulted",
        explanation: "\
The player settings file (`save/settings.yaml`) could not be honored at all: it is not parseable,
its envelope is malformed (`L1700`), a value has the wrong shape, or its `format` is newer than this
engine reads. The engine renamed it aside as `settings.yaml.bak.<hash>` and started from defaults. The
game still boots and the player's bytes are kept.

Fix: nothing is required. To recover the old values, reconcile the backup with the new file by hand.",
    },
    CodeEntry {
        code: code::SETTINGS_KEY_DROPPED,
        severity: Severity::Warning,
        title: "settings file has an unknown key",
        explanation: "\
The settings file has a key the engine does not know (pointer `/audio/volume`, say): a typo or a key a
newer build wrote. It is dropped on load. An unknown *bus name* under `/audio/buses` is kept and
written back, so a game's own buses survive, but it is reported once.

Fix: correct the key (the message lists the known keys) or delete the line.",
    },
    CodeEntry {
        code: code::SETTINGS_VALUE_CLAMPED,
        severity: Severity::Warning,
        title: "a settings value was replaced",
        explanation: "\
A settings value was out of range, not a number or not a known name. A volume outside `0..=1` is
clamped, a NaN volume is replaced by `1.0`, and an unknown `quality` preset or `anti_aliasing` name is
ignored (the field stays unset). The pointer names the value.

Fix: write a volume between 0 and 1 and a name from `ludion docs settings`.",
    },
    CodeEntry {
        code: code::SETTINGS_SAVE_FAILED,
        severity: Severity::Warning,
        title: "settings file could not be written to disk",
        explanation: "\
A game system called `Settings::save()` and the engine could not write `save/settings.yaml` at the
frame boundary: the save directory is missing or not writable, or the disk is full. The write is staged
to a hidden `.tmp` sibling and renamed, so the previous whole file stays on disk. The requested values
still apply to this session.

Fix: make `save/` writable and call `save()` again.",
    },
    CodeEntry {
        code: code::PROFILE_LOAD_FAILED,
        severity: Severity::Warning,
        title: "saved profile could not be loaded — defaulted",
        explanation: "\
A saved player profile (`save/profile.yaml`, the persistent save state a game registers with
`profile::<P>`) could not be loaded as it was written on disk, so the engine started a fresh profile from
`P::default()`. A persisting host first renames the file aside as a backup
(`profile.yaml.bak.<content-hash>`); any other engine, or a failed rename, leaves the file in place.
The game still boots — this is a warning, never a hard failure — and the player's original bytes are
never destroyed.

A profile is loaded *leniently*, unlike an authored data-doc (`L1119`/`L1120`): a field the game
added since the save is filled from its default, and a field the game removed is dropped — neither
triggers this code. The default-profile path is only taken when the file cannot be honored at all:

  - its envelope is malformed — a missing or non-integer `format`, or a `kind` that is missing, not a
    string, or not `profile` (a non-mapping or unparseable document counts too);
  - a value no longer deserializes into the game's current profile type — e.g. a field was retyped
    since the save, so the stored value has the wrong shape;
  - the file's `format` version is newer than this engine reads — an older build must not silently
    downgrade a save a newer build wrote.

Fix: nothing is required — the game runs on a fresh default profile and the next save overwrites it.
To recover the old values, open the `profile.yaml.bak.<hash>` sibling (or, when no backup was made,
the untouched `profile.yaml`) and reconcile it by hand.",
    },
    CodeEntry {
        code: code::RUN_SAVE_WRITE_FAILED,
        severity: Severity::Warning,
        title: "suspended run could not be written or removed",
        explanation: "\
A game system called `RunSave::suspend()` or `RunSave::discard()`, and the engine could not carry it
out at the frame boundary. Causes:

  - the run could not be captured as a snapshot (a component that does not serialize, such as a NaN
    float, reports `L0007`);
  - the save location is not writable, or the disk is full (`suspend`);
  - the suspended file could not be deleted (`discard`).

This is a **warning**: the run continues, a previous whole `save/suspend.json` stays on disk (the
write is staged to a hidden `.tmp` sibling and renamed over it), and the next `suspend()` retries.
The message names the path and the underlying error.

Fix: make sure the save location is writable and has free space.",
    },
    CodeEntry {
        code: code::RUN_SAVE_RESUME_REFUSED,
        severity: Severity::Warning,
        title: "suspended run was refused",
        explanation: "\
A game system called `RunSave::resume()` and the engine refused the suspended run
(`save/suspend.json`). Causes:

  - it was written by another engine build or a changed game schema (the build stamp differs);
  - its envelope `format` is newer than this build reads;
  - the file or its snapshot is unreadable or unparsable;
  - restoring it would only work with drift (dropped or migrated components, an unknown field);
  - there is no suspended run;
  - a run recording is open.

This is a **warning**: nothing is restored, the file is renamed to `suspend.json.bak.<hash>` (left in
place if the rename fails), `RunSave::last_resume()` is `Refused { reason }`, and the run continues.
The open-recording cause is the exception: the file is kept, no backup is made, and `pending()` is
unchanged.

Fix: start a new run. The backup can be inspected by hand. For an open recording, call
`RunRecording::stop()`, then `resume()` again.",
    },
    CodeEntry {
        code: code::RUN_RECORDING_FAILED,
        severity: Severity::Warning,
        title: "run recording could not start, be written or be finalized",
        explanation: "\
A game system called `RunRecording::start(name)` or `stop()`, and the engine could not carry it out
at the frame boundary. Causes:

  - the name is empty, longer than 128 characters, starts with `.` or holds a character outside
    `[A-Za-z0-9._-]`;
  - a session recording (`--record`, `set_recording`) is already running and wins;
  - the current scene has no inline text to restart from;
  - the replays directory or the recording file could not be created, written or renamed;
  - the recording died mid-run (a write error stopped it).

This is a **warning**: the run continues. A start refused before the reset changes nothing; a start
that fails after the reset leaves the reset world and no recording (the message says so). A
previous `<name>.lrp` is replaced only by a recording that finished.

Fix: use a plain file-safe name, stop the session recording, and make sure the replays directory is
writable with free space.",
    },
    CodeEntry {
        code: code::CHECKPOINT_CONFIG_INVALID,
        severity: Severity::Error,
        title: "checkpoint config is invalid",
        explanation: "\
`EngineBuilder::checkpoints` was given a `CheckpointConfig` whose `max_slots` is 0 or above 64.

Fix: choose a `max_slots` from 1 to 64 (the default is 4).",
    },
    CodeEntry {
        code: code::CHECKPOINT_NO_SLOT,
        severity: Severity::Warning,
        title: "checkpoint slot does not exist",
        explanation: "\
A system called `Checkpoints::restore(name)` for a name that holds no slot: it was never captured, was
discarded, or the store was cleared by a scene load, an `engine.restore` or a run-save resume.

This is a **warning**: the world is untouched and `Checkpoints::last_restore()` is `Refused { reason }`.

Fix: capture the slot first, and check `Checkpoints::slot(name)` before restoring.",
    },
    CodeEntry {
        code: code::CHECKPOINT_SLOTS_FULL,
        severity: Severity::Warning,
        title: "checkpoint slots are full",
        explanation: "\
A system called `Checkpoints::capture(name)` for a new name while `max_slots` slots are held. Nothing is
captured and no slot is evicted; recapturing an existing name is always allowed.

This is a **warning**.

Fix: `discard` a slot you no longer need, or raise `CheckpointConfig::max_slots` (at most 64).",
    },
    CodeEntry {
        code: code::CHECKPOINT_NAME_INVALID,
        severity: Severity::Warning,
        title: "checkpoint name is invalid",
        explanation: "\
A checkpoint request named a slot that is empty, longer than 64 characters or holds a character outside
`[A-Za-z0-9_.-]`. The request is ignored.

This is a **warning**.

Fix: use a short name such as `b_to_c`.",
    },
    CodeEntry {
        code: code::CHECKPOINT_CAPTURE_FAILED,
        severity: Severity::Warning,
        title: "checkpoint capture failed",
        explanation: "\
Capturing the world into a slot failed, for example because a component holds a non-finite number
(`L0007`). The previous slot of that name, if any, is kept.

This is a **warning**; the message carries the underlying diagnostic.

Fix: correct the value the underlying diagnostic names.",
    },
    CodeEntry {
        code: code::CHECKPOINT_RESTORE_FAILED,
        severity: Severity::Warning,
        title: "checkpoint restore failed",
        explanation: "\
Restoring a slot failed while decoding or validating its document. The world is untouched and
`Checkpoints::last_restore()` is `Refused { reason }`.

This is a **warning**; the message carries the underlying diagnostic.",
    },
    CodeEntry {
        code: code::CHECKPOINT_SWITCH_DROPPED,
        severity: Severity::Warning,
        title: "scene switch dropped by a checkpoint restore",
        explanation: "\
A scene switch and a checkpoint restore were requested in the same frame. The restore wins and the
switch is dropped. A slot captured in another district performs that switch itself.

This is a **warning**.

Fix: request one or the other, or request the switch on a later frame.",
    },
    CodeEntry {
        code: code::CHECKPOINT_KEYFRAME_SKIP,
        severity: Severity::Warning,
        title: "replay keyframe skipped while checkpoint slots are held",
        explanation: "\
A recording was due a keyframe while checkpoint slots were held. A snapshot cannot carry slots yet, so
no keyframe is written; a seek lands on the last keyframe before the first capture. The replay itself
is unaffected: replaying regenerates the slots.

This is a **warning**, reported once per session.

Fix: `discard` every slot before a recording needs keyframes, or accept the coarser seek.",
    },
    CodeEntry {
        code: code::CHECKPOINT_RECORDING_ARM_REFUSED,
        severity: Severity::Warning,
        title: "recording not armed while checkpoint slots are held",
        explanation: "\
A run recording was started while checkpoint slots were held. A recording must start from a world its
keyframes can reproduce, and a snapshot cannot carry slots yet, so it was not armed.

This is a **warning**.

Fix: start the recording before the first capture, or `discard` every slot first.",
    },
    CodeEntry {
        code: code::CHECKPOINT_DISK_WRITE_FAILED,
        severity: Severity::Error,
        title: "checkpoint disk slot could not be written or removed",
        explanation: "\
`Checkpoints::persist` or `Checkpoints::discard_disk` could not write or delete
`save/checkpoint.json`. The previous whole file stays in place (writes are atomic), the in-memory
slots are unaffected, and the next request retries.

This is an **error**; the sim keeps running.

Fix: check the save root is writable and has free space.",
    },
    CodeEntry {
        code: code::CHECKPOINT_DISK_LOAD_REFUSED,
        severity: Severity::Warning,
        title: "disk checkpoint not loaded",
        explanation: "\
`Checkpoints::load_disk` refused `save/checkpoint.json`: the file is garbled, was written by a newer
format or a different build (engine version, component or data schemas, projectile pool), holds an
invalid name, or no longer restores cleanly. The file is renamed to `checkpoint.json.bak.<hash>` and
`disk()` reads `None`, so a menu stops offering it; the world is untouched.

While a `RunRecording` is open the load is refused too, with the file kept and no backup: stop the
recording first (a replay never reads disk).

This is a **warning**.

Fix: start a new game, or load the backup with the build that wrote it.",
    },
    CodeEntry {
        code: code::GAME_SCALE_INVALID,
        severity: Severity::Warning,
        title: "game clock scale value ignored",
        explanation: "\
A system called `GameClock::scale` with a value that was not finite, was `<= 0`, or was `> 1`. The
call was ignored, so gameplay time ran at the other requested scale, or at full speed. The message
names the first bad value of the frame and is reported once per session.

This is a **warning**.

Fix: clamp to `(0, 1]`; use `hold()` for a freeze.",
    },
    CodeEntry {
        code: code::RUN_RECORDING_CONFIG_INVALID,
        severity: Severity::Error,
        title: "run recording config is invalid",
        explanation: "\
`EngineBuilder::run_recordings` was given a config the engine cannot use:

  - `keyframe_every` is 0 (a recording needs a keyframe interval of at least 1 tick);
  - `dir` is not a relative path inside the save root (empty, absolute, or holding `..`).

The build fails with this error.

Fix: use a `keyframe_every` of 1 or more (the default is 3600) and a relative `dir` such as
`save/replays`.",
    },
    CodeEntry {
        code: code::REPLAY_GAME_MISMATCH,
        severity: Severity::Error,
        title: "replay was recorded by a different build of the game",
        explanation: "\
A run recording's header carries the identity of the game build that recorded it: the hash of the
game's registered component and data-doc schemas (engine built-ins included, so an engine component
change also moves it) and the optional `EngineBuilder::game_version` string. Session recordings do not
write it. This build's identity differs, so the replay was refused rather than played into a divergence.

Fix: this file cannot be replayed by this build. A game lists it as recorded by an older version (or
another build); a developer re-records it under the current build.",
    },
    CodeEntry {
        code: code::REPLAY_PLAYBACK_FAILED,
        severity: Severity::Warning,
        title: "replay playback could not start, finish or be managed",
        explanation: "\
A game system called `ReplayPlayback::play(name, return_to)`, `refresh()` or `delete(name)`, or a
replay in progress failed. Causes:

  - the name is empty, longer than 128 characters, starts with `.` or holds a character outside
    `[A-Za-z0-9._-]`;
  - `<name>.lrp` is missing or unreadable in the replays directory;
  - a recording is running (a session or run recording wins) or a replay is already playing;
  - the `return_to` scene does not resolve or cannot be read;
  - the replay hit a driver error mid-play (the outcome is `Failed`);
  - a command was refused while a replay plays: anything that would change the sim or the recording
    (`sim.pause`, `sim.step`, `component.set`, …), `asset.reload`, and the unrecorded commands that would
    write the replayed world to disk (`scene.save`, `prefab.create`, `prefab.apply`); input and reads
    pass, and `render.size` waits for the return;
  - the replayed sim asked to quit: the request is dropped, never shutting the host;
  - a replay file could not be deleted;
  - the `replay_playback` builder `dir` is not a relative path inside the save root (a build error).

This is a **warning**: the run continues. A start refused before any change leaves the live world
untouched. A host that does not persist its profile only logs the request.

Fix: use a plain file-safe name of a recording that exists, stop the recording first, and name a
scene file that exists as `return_to`.",
    },
    CodeEntry {
        code: code::PROFILE_SAVE_FAILED,
        severity: Severity::Warning,
        title: "saved profile could not be written to disk",
        explanation: "\
A saved player profile (`save/profile.yaml`, the persistent save state a game registers with
`profile::<P>`) could not be written to disk when a game system requested a save at a frame boundary.

The write is crash-durable: the engine renders the profile to canonical text, stages it to a hidden
`.tmp` sibling in the same directory and syncs it to disk, renames the temp over the destination
(`fs::rename` replaces atomically on both Windows and POSIX), then syncs the directory. Any step can
fail:

  - the save directory does not exist and could not be created (an ancestor is a file, not a
    directory);
  - the directory or file is not writable (a permissions error, a read-only volume);
  - the disk is full, or the underlying filesystem rejected the write or rename.

This is a **warning**, never a hard failure — the simulation keeps running, a whole save stays on
disk (the failure lands on the temp, never on the real file; a directory sync can fail after the
rename, in which case the new save is the whole one on disk), and the next `profile.save()` retries
the write. The message names the target path and the underlying IO error.

Fix: make sure the save location (`save/` beside the scene by default) is writable, and
that the volume has free space. The in-memory profile is unchanged and will be written on the next
successful save, but a `scene.load` first starts a fresh session from the file on disk and discards
it. A scene switch carries it.",
    },
    CodeEntry {
        code: code::DIRECTIONAL_LIGHT_INVALID_FRUSTUM,
        severity: Severity::Error,
        title: "directional light cannot form a shadow frustum",
        explanation: "\
A `DirectionalLight` component has a `direction` or a shadow depth range that cannot form a shadow
frustum.

A directional (\"sun\") light travels along an explicit `[x, y, z]` direction (chosen over a
Transform rotation so hand-authored YAML reads legibly). That vector need not be unit length — it is
normalized at extract — but it must be non-zero and finite: a zero vector (all three components
zero) has no direction to normalize, and a non-finite component (`NaN` or `±Infinity`) would feed a
NaN into the shadow frustum. The shadow depth range must also be a real interval: `shadow_near`
must be less than `shadow_far`, or the orthographic frustum has no depth. JSON Schema cannot express
either (each component is individually a valid number, and it cannot compare two fields), so these
invariants live in the component's semantic validator rather than its schema — the same split the
`Collider` validator (`L1116`) uses.

The JSON Pointer names `/direction`, or `/shadow_far` (`/shadow_near` if far is not authored). Fix:
give the direction a non-zero, finite `[x, y, z]` — for example `[-0.3, -1.0, -0.2]`, a slightly
angled overhead sun (the component default) — or make `shadow_far` greater than `shadow_near` (the
defaults are 1.0 and 200.0).

`cascade_splits` is checked here too: it needs `cascades`, must hold exactly `cascades - 1` values,
and those must be finite, strictly ascending and inside (0, 1). The pointer is `/cascade_splits`
(missing `cascades` or wrong length) or `/cascade_splits/{i}` (the first bad value). Fix: write
`cascades: 4, cascade_splits: [0.06, 0.15, 0.4]`, or drop `cascade_splits` to use the engine's split
scheme.",
    },
    CodeEntry {
        code: code::SCHEDULE_MODULE_CONCENTRATION,
        severity: Severity::Warning,
        title: "one module concentrates too many schedule systems",
        explanation: "\
A single definition *module* registers more schedule systems than the engine's per-module threshold,
so the engine emitted this warning at boot. It is a warning, never a failure — the game runs
normally; nothing about the simulation, its `WorldHash`, or a replay is affected.

The wiring-root convention. A game's systems are defined in per-concern modules (`movement.rs`,
`combat.rs`, `spawner.rs`, …) that glob re-export into the crate root, and the crate root's `game()`
holds the *ordered* `.system(\"name\", func)` list — the one place the schedule's execution order is
read top to bottom. `games/shmup-stress/src/` is the worked example: definitions live in the modules, the
wiring lives in the root. This split keeps each file small enough to fit in an agent's working
context and keeps the schedule order legible in one place.

The threshold, and why. The engine groups every registered system by its definition module — the
module part of `std::any::type_name` of the registered function, with generic arguments stripped and
the trailing `::function` segment removed — and warns when one module owns more than the threshold
(12). The number is calibrated: a real game's pre-split single-file version defined 27 systems at the
crate root and would fire here, so the teeth are real; after the split, its largest module holds five or
six systems and every other game five or fewer, so all current games clear it with roughly 2x headroom.
Twelve systems plus the helpers they need is already a 1,500-plus-line file — past the point where an
agent's edit loop and a human's reading both start to suffer.

The fix. Split the offending module into per-concern modules and glob re-export them from the crate
root (`mod movement; pub use movement::*;`), moving each system's *definition* into the module that
owns its concern while leaving the ordered `.system(...)` calls in the root's `game()`. A fresh
project scaffolded by `ludion new` starts from this multi-module shape, so a game grows into modules
rather than growing a catch-all module it must later break apart.

Honesty caveats. The grouping is a best-effort, human-facing heuristic, deliberately kept out of
anything load-bearing. \"Module\" equals \"file\" only for idiomatic layouts (one module per file); a
module split across `mod` blocks in one file, or several modules in one file, is grouped by module
path, not by file. The module path itself comes from `std::any::type_name`, whose exact format Rust
does not guarantee — a system whose path cannot be parsed (a closure, or a name with no `::`) is
skipped rather than risk a false positive, so the count can undercount in unusual layouts. None of
this touches sim state: the warning is computed once at build from registration metadata and never
enters the `WorldHash`, a snapshot, or a replay.",
    },
    CodeEntry {
        code: code::QUERY_DECL_UNKNOWN_COMPONENT,
        severity: Severity::Error,
        title: "declared query names an unregistered component",
        explanation: "\
A system's declared query names a component type the engine does not register.

Systems declare the component queries they run at registration, chained onto the builder:

  .system(\"fire\", fire).query::<(&Loadout, &Transform)>().query::<(&mut Health,)>()

Each `.query::<...>()` names the same tuple of `&T`/`&mut T` the system body iterates. At engine
build the declaration's element types are resolved to registry component names — the same registry a
scene, a snapshot, and the wire protocol use — so `engine.describe` can publish each system's queries
and a drift verifier can compare them to what actually ran. A type that is not registered has no
component name to resolve to, so the build fails here rather than shipping an unresolvable
declaration.

The message names the system, which declared query (by index) is at fault, and the offending Rust
type. Fix: register the component before building — `EngineBuilder::components(|r|
r.register::<T>(\"Name\"))` — or correct the `.query::<...>()` to name a component the game actually
registers. (A type used only in a system *body* but never declared is fine; only *declared* types are
resolved.)",
    },
    CodeEntry {
        code: code::QUERY_DECL_NO_SYSTEM,
        severity: Severity::Error,
        title: "declared query has no system",
        explanation: "\
`EngineBuilder::query::<...>()` was called before any `.system(name, fn)`, so there is no system to
attribute the declaration to. The call is recorded and the build fails here, naming the call site's
file and line.

A query declaration describes the system registered just before it. Fix: chain it onto that system:

  .system(\"fire\", fire).query::<(&Loadout, &Transform)>()",
    },
    CodeEntry {
        code: code::QUERY_DRIFT_UNDECLARED,
        severity: Severity::Error,
        title: "system ran an undeclared query shape",
        explanation: "\
A system iterated a query whose component shape it never declared.

The query-drift verifier (`ludion_engine::check_query_drift_replay` /
`check_query_drift_with`) runs a game through a committed replay — or a driver closure — with
query recording enabled, then compares the shapes each system *actually ran* against the shapes it
*declared* with `.query::<...>()`. A shape is compared as a set of `(component, access)` pairs, so
tuple order and declaration order do not matter; declaring a read (`&T`) where the body took a write
(`&mut T`) counts as undeclared, because the access differs.

This diagnostic means an observed shape had no matching declaration — the system touches components
it never told anyone it would. That breaks the promise `engine.describe` publishes (the editor's Game
Designer view, and later the deterministic job system, rely on it). Fix: add the missing
`.query::<...>()` declaration for the observed shape to that system, matching the exact `&T`/`&mut T`
access the body uses. The message lists the observed component set and the shapes that *were*
declared, so the gap is explicit.",
    },
    CodeEntry {
        code: code::QUERY_DRIFT_UNOBSERVED,
        severity: Severity::Warning,
        title: "declared query shape was never observed",
        explanation: "\
A system declared a query shape that the verified run never exercised.

The query-drift verifier compares each system's declared query shapes against the ones it ran during
a recorded replay (or driver closure). This is the reverse of an undeclared query: here a
`.query::<...>()` declaration produced no matching observation. It is a **warning**, never a failure
— a query guarded by a conditional (an ability that did not trigger, a branch the replay did not
reach) legitimately does not run in every session, and the declaration is still correct.

The message names the system and the unobserved shape. If the branch is genuinely reachable, extend
the replay (or driver) so the verification exercises it; if the declaration is stale — the system no
longer runs that query at all — remove it. No action is required for a legitimately-conditional
query.",
    },
    CodeEntry {
        code: code::ENVIRONMENT_INVALID_FIELD,
        severity: Severity::Error,
        title: "environment color is negative or non-finite",
        explanation: "\
An `Environment` component has a sky or ambient color component that is negative or non-finite.

The `Environment` component authors the scene's sky and lighting environment. Its four color fields —
the sky gradient (`sky_zenith`/`sky_horizon`/`sky_ground`) and the flat `ambient` — are linear RGB
and HDR: a component may exceed 1.0, but it must be finite and non-negative. A negative value has no
meaning as a radiance and would poison the prefiltered image-based lighting.

JSON Schema's `range` keyword does not reach array elements, so this invariant lives in the
component's semantic validator rather than its schema — the same split the `Collider` (`L1116`)
validator uses for its `half_extents`. The scalar fields (`sun_disc_size`, `sun_disc_intensity`,
`ibl_intensity`) instead carry a schemars `range(min = 0)`, so a negative scalar is reported as the
schema's `L1014`, not this code.

The JSON Pointer names the offending element: `/sky_zenith/0`, `/ambient/2`, and so on. Fix: give
every color component a finite, non-negative value (HDR values above 1.0 are fine).",
    },
    CodeEntry {
        code: code::POST_PROCESS_INVALID_TRIPLE,
        severity: Severity::Error,
        title: "post-process triple element is out of range",
        explanation: "\
A `PostProcess` component has a three-element field whose element is outside its valid range.

Three of the component's triples are bounded. Two are the ASC CDL shape the composite pass applies
after tonemapping, `out = pow(max(colour * gain + lift, 0), 1 / gamma)`:

  * `gain` is the slope, so a component must be `>= 0` — a negative slope inverts the channel into
    values the CDL's own floor immediately clamps to black, which is never what an author meant.
  * `gamma` is the power, applied as its **reciprocal** (a higher `gamma` brightens the midtones), so
    a component must be strictly greater than 0 — `1 / 0` is infinity and would collapse the frame
    to hard black and white.

The third is a colour:

  * `fog_color` is what the fog blends toward in linear HDR, so a component must be `>= 0`. Values
    above 1.0 are fine (it is HDR, like every other colour in the engine); negative light is not —
    the same rule `Environment`'s sky and ambient colours carry as `L1128`.

`lift` is the offset and is deliberately unbounded: a negative lift legitimately crushes the blacks,
which is an ordinary grading move.

JSON Schema's `range` keyword does not reach array elements, so this invariant lives in the
component's semantic validator rather than its schema — the same split the `Environment` (`L1128`)
and `Collider` (`L1116`) validators use. Every scalar field (`contrast`, `saturation`, `temperature`,
`tint`, `fog_density`, `vignette`, …) instead carries a schemars `range`, so an out-of-range scalar
is reported as the schema's `L1014`, not this code. A non-finite element needs no code at all: YAML's
`.nan`/`.inf` are unrepresentable in the scene format (`L1009`) and JSON has no spelling for them on
the wire.

The JSON Pointer names the offending element: `/gain/0`, `/gamma/1`, `/fog_color/2`, and so on. Fix:
give every `gain` and `fog_color` component a value of 0 or more, and every `gamma` component a value
greater than 0 (the identity grade is `gain: [1, 1, 1]`, `gamma: [1, 1, 1]`, `lift: [0, 0, 0]`).",
    },
    CodeEntry {
        code: code::GENERATION_STRUCTURE,
        severity: Severity::Error,
        title: "generation document structure is invalid",
        explanation: "\
A `kind: generation` document — a generation recipe — does not have the shape this engine reads.

A recipe names a committed ComfyUI workflow and overrides its inputs by node title:

  format: 1
  kind: generation
  name: idol-portrait     # optional, defaults to the file stem
  workflow: ../workflows/sdxl-txt2img.json   # required, relative to THIS document
  seed: 7                 # optional, default 0
  count: 4                # optional, default 1
  images:                 # optional, name -> relative path to a committed image
    subject: ../renders/idol-front.png
  inputs:                 # optional, node title -> input name -> value
    Empty Latent:
      width: 1024
      height: 1024
    Load Subject:
      image: { image: subject }

This code covers: an empty or non-mapping document; a missing `format`, `kind` or `workflow`; a
`kind` other than `generation`; a wrong-typed value (a non-string `workflow` or `name`, a
non-integer `seed` or `count`, a non-mapping `images` or `inputs`, a non-mapping entry under
`inputs`); a `null` input value; and a top-level key a recipe does not have at all.

The JSON Pointer names the offending key (`/workflow`, `/inputs/Empty Latent/width`,
`/images/subject`); an unknown key carries a nearest-match suggestion.

Fix: correct the named key. Values that parse but do not hang together — a `count` of 0, an
`{ image: x }` naming no declared entry — are the separate `L1162`.",
    },
    CodeEntry {
        code: code::GENERATION_DATA,
        severity: Severity::Error,
        title: "generation document data is inconsistent",
        explanation: "\
A `kind: generation` document parsed structurally, but its values do not describe a run the client
can submit.

This code covers:

  - `count` below 1 — a run produces at least one candidate;
  - a `seed` that is negative or does not fit in 64 bits (`seed + i` labels candidate `i`);
  - a non-finite number (`NaN`, `±Infinity`) anywhere under `inputs`;
  - an empty `workflow` path, or an empty `images` entry name or path;
  - a `name` (at `/name`) that is not a plain file stem — a path separator, `.`/`..`, or a reserved
    device name such as `con` — because it names the run folder under `generation/runs/`;
  - an `{ image: x }` input naming no entry of `images:`;
  - a `workflow` that resolves outside the project root — runs and accepted assets record it
    project-relative, so it belongs under the project (`generation/workflows/`);
  - any other structured input value — a map that is not the one image-reference form, or a list.
    A recipe sets leaf inputs; rewiring the graph is the workflow's business, so an edge cannot be
    spelled here.

The JSON Pointer names the offending value (`/count`, `/seed`, `/inputs/Load Subject/image`,
`/images/subject`).

Fix: correct the named value. Whether the `workflow` and `images` paths exist on disk is checked by
the caller that resolves them against the recipe's own directory, not by the document layer.
Structure problems (a missing key, a wrong type, an unknown key) are `L1161`; problems inside the
workflow file are `L1163`-`L1166`.",
    },
    CodeEntry {
        code: code::WORKFLOW_NOT_API,
        severity: Severity::Error,
        title: "workflow is not in API format",
        explanation: "\
The workflow JSON a recipe names is a ComfyUI **UI** (litegraph) export, not an **API** export, so
the server's `/prompt` endpoint cannot execute it.

The two files look similar and are not interchangeable. A UI export is the editor's own save format:
a top-level object with `nodes` and `links` arrays, widget values positional, node positions and
sizes included. An API export is the flat submit format:

  {
    \"3\": { \"class_type\": \"KSampler\",
            \"inputs\": { \"seed\": 7, \"model\": [\"4\", 0] },
            \"_meta\": { \"title\": \"Sampler\" } },
    \"4\": { \"class_type\": \"CheckpointLoaderSimple\", \"inputs\": { ... } }
  }

— an object keyed by node id, each value carrying `class_type` and `inputs`, where a link is a
`[node_id, slot]` pair inside `inputs`.

Fix: in ComfyUI, export the graph again with **Workflow -> Export (API)** (older builds spell it
**File -> Save (API Format)**; the developer-mode setting must be on for it to appear), and commit
that file. A recipe addresses nodes by their `_meta.title`, which only the API export carries, so
titling the nodes you intend to override before exporting is part of authoring the workflow.",
    },
    CodeEntry {
        code: code::WORKFLOW_UNKNOWN_NODE,
        severity: Severity::Error,
        title: "workflow names a node class the server does not have",
        explanation: "\
A node in the workflow declares a `class_type` that is absent from the ComfyUI server's
`/object_info` — the live list of every node class that server can execute.

This is almost always one of two things: the workflow came from a machine with a custom-node pack
this server does not have installed, or the class name is misspelled. The message names the node id,
its title, the class as written, and the nearest installed class name.

This check needs the live server, so it is part of the *online* half of `gen check`; the offline half
(`L1163`, `L1166` and the recipe's own `L1161`/`L1162`) answers from the files alone.

Fix: install the missing custom node on the server and restart it, or re-author that part of the
graph with a class the server has. `gen nodes` lists the installed classes and their inputs.",
    },
    CodeEntry {
        code: code::WORKFLOW_INPUT_INVALID,
        severity: Severity::Error,
        title: "workflow input is missing or has the wrong type",
        explanation: "\
An input of a workflow node does not match what the server's `/object_info` says that node class
takes: a required input nothing supplies, or a value whose type or enum membership is wrong.

The message names the node title, the input, the value as written and the expected type — for an
enum input (a checkpoint name, a sampler name, a scheduler) it lists the accepted values, since those
come from the server's own installed files and differ from machine to machine.

An override a recipe wrote and an input the committed workflow already carried are reported the same
way; the pointer distinguishes them, naming `/inputs/<title>/<input>` when the recipe is the source.

Fix: correct the value in the recipe (or in the workflow, when no override covers it). A checkpoint
or LoRA name must be spelled exactly as the server lists it — `gen nodes <class>` prints the input
table for one class, including every enum's accepted values.",
    },
    CodeEntry {
        code: code::WORKFLOW_TITLE_UNRESOLVED,
        severity: Severity::Error,
        title: "recipe override names no single workflow node",
        explanation: "\
A recipe's `inputs:` addresses nodes by their `_meta.title`, and one of its titles either matches no
node of the workflow or matches two, so the override has no single node to apply to.

Titles are the recipe's whole addressing scheme: node ids are an export detail that changes when the
graph is re-exported, while a title is something the author chose and can keep stable. That makes a
duplicate title as fatal as a missing one — the engine will not guess which node was meant.

The message names the title as the recipe wrote it and lists every title in the graph, with the node
id beside each so a duplicate pair is visible.

Fix: rename the node in ComfyUI (right-click -> Title) so the title is unique and matches the recipe,
re-export the workflow in API format, and commit it. Or change the recipe to a title the graph
already has.",
    },
    CodeEntry {
        code: code::COMFY_UNREACHABLE,
        severity: Severity::Error,
        title: "ComfyUI server is unreachable",
        explanation: "\
Nothing answered at the resolved ComfyUI address: the connection was refused, timed out, or the host
did not resolve. An address this client cannot use is refused before anything is tried: `https://`
(it is built without TLS; point at a plain `http://` address or a tunnel that exposes one), or any
value that does not start with `http://`.

The address is an environment property, never a committed file — a server address is machine state,
and a checked-in one would be wrong for every other machine and would leak a private hostname into a
public repository. It is resolved in this order:

  1. the `--url` flag, when given;
  2. the `LUDION_COMFY_URL` environment variable;
  3. `http://127.0.0.1:8188`, the ComfyUI default.

The message names which of the three supplied the address it tried, without printing the address
itself when it came from the environment.

Fix: start ComfyUI (`python main.py` in its checkout), or point `LUDION_COMFY_URL` at the machine
that runs it. A remote server needs `--listen` on its side. If it is running and still unreachable,
check that nothing between the two machines blocks the port.",
    },
    CodeEntry {
        code: code::COMFY_REJECTED,
        severity: Severity::Error,
        title: "ComfyUI server rejected the request",
        explanation: "\
The ComfyUI server answered, and refused. The request reached it intact, so this is the server's
verdict on the content rather than a connection problem (`L1167`).

The common case is a `/prompt` 400, whose body carries a structured `node_errors` map: per node, the
input it objected to and why. That map is reprinted node by node rather than dumped as raw JSON. Any
other endpoint's HTTP error — an upload the server refused, a history or view request for an id it
does not have — is reported here too, with the endpoint, the status and the body. So is a reply that
could not be read to the end: the server answered, then the connection dropped, the download timed
out, or the body passed the 512 MiB cap.

Fix: read the reprinted per-node errors; most are a value the graph or an override supplies that the
node will not take (`L1165` catches these before submit when `/object_info` is reachable). Running
`gen check --url ...` against the same server reproduces the diagnosis offline of a run.",
    },
    CodeEntry {
        code: code::GENERATION_RUN_FAILED,
        severity: Severity::Error,
        title: "generation run failed on the server",
        explanation: "\
A submitted job failed on the server — its history entry carries an `execution_error` or an error
status — or is lost: the server has neither a history entry nor a queue place for it, which is what a
ComfyUI restart does to every job it held. One `L1169` is printed per failed or lost job. The other
jobs of the run are not affected: the run is `partial` (some candidates downloaded) or `failed`
(none), `run.json`'s `failed` lists each job, and `gen sheet`/`gen accept` still take a partial run.
A job the server ran and reported finished, but that wrote no file, is the same failure with reason
`empty`: the graph's output node may be muted or bypassed in the exported API graph.

The message names the node id and title, its `class_type`, and the server's own message and traceback
summary. A failure here is the model's or the graph's, not the recipe's shape — the recipe already
passed `L1161`/`L1162` and the graph `L1163`-`L1166` — so the usual causes are a model file the
server could not load, an out-of-memory during sampling, or a resolution the graph's nodes cannot
handle together.

Fix: read the named node's message. For out-of-memory, lower the resolution or `batch_size` through
the recipe's `inputs:` rather than editing the workflow. A lost job needs `ludion gen run <recipe>`
again. The run folder keeps whatever was written before the failure, so a partial run is inspectable
rather than lost.",
    },
    CodeEntry {
        code: code::GENERATION_RUN_TIMED_OUT,
        severity: Severity::Error,
        title: "generation run timed out",
        explanation: "\
`--timeout` elapsed before the run finished, or `gen fetch` found a job still queued. This says
nothing about the run itself: the server is very likely still working on it, and the job keeps its
place in the server's queue.

The message names the job ids (the server's `prompt_id`), and the help the run folder, so the run
can be picked up again:

  ludion gen status <run-dir>    # where it is in the queue, or whether it finished
  ludion gen fetch <run-dir>     # download a finished run's outputs into its run folder

Fix: nothing, usually — wait and `gen fetch`. A first run on a cold server pays for loading the
checkpoint into VRAM, which can be minutes on its own, so a timeout tuned to a warm run will trip on
the first one. Raise `--timeout` if the model genuinely takes that long.",
    },
    CodeEntry {
        code: code::GENERATION_OUTPUT_KIND,
        severity: Severity::Error,
        title: "generation output kind is unusable",
        explanation: "\
What the graph produces and what is being asked of it do not line up.

This code covers:

  - a workflow with no output node at all, so a run would compute and save nothing;
  - a graph whose only output is a preview (a temp output, not a saved file);
  - a graph that produces images *and* a mesh — one run yields one kind of artifact, because a
    candidate's index, its seed and the file accepted from it are meant to be the same fact;
  - `gen sheet` of a mesh run, or of a run with no candidate.

The message names the output nodes the graph has, with their classes, and what was asked for. A job
that ran and wrote no file is `L1169`, not this.

Fix: split a mixed graph into two workflows, or add the missing output node (`SaveImage` for
images); look at a mesh run with `render --model`. `gen check` reports the graph causes offline,
before a run is submitted.",
    },
    CodeEntry {
        code: code::GENERATION_ACCEPT_REFUSED,
        severity: Severity::Error,
        title: "accept refused",
        explanation: "\
`gen accept` — the step that freezes one candidate of a run into the committed asset folders — would
not proceed.

This code covers:

  - a candidate index outside the run's range (a run of 4 has indices 0-3);
  - a destination that already exists — the asset file for an image, or, for a mesh candidate (whose
    `--out` is the directory `ludion import` writes its tree into), a directory holding anything at
    all: acceptance never overwrites content, because an accepted asset is content, hand-editable
    from that moment on, and silently replacing it would destroy edits made since;
  - a run folder that is missing or incomplete — run folders are ignored scratch, so a pruned or
    half-fetched one is a normal state, not corruption;
  - a run that has no candidate: it failed (every job failed or was lost) or is not finalized yet
    (`gen fetch` it first);
  - a report from before the prompt snapshot or the image record whose prompt must be re-read from
    the recipe, when the recipe or workflow hash has drifted, the recipe no longer resolves, its
    overrides no longer apply, or the run folder's `inputs/` copy of an image is missing or is not
    the file the recipe declares; and a report whose inputs snapshot cannot be read.

The message names the index and the run, or the exact destination path.

Fix: pick a valid index (`gen sheet` labels each candidate with its index and seed), choose a
destination name that is free, or re-`gen fetch` the run before accepting from it.",
    },
    CodeEntry {
        code: code::GENERATION_IMAGE_UNBOUND,
        severity: Severity::Error,
        title: "generation image is declared but never bound",
        explanation: "A `kind: generation` recipe lists an entry under `images:`, but no `inputs:` override feeds it to a
node as `{ image: <name> }`. A run would upload nothing for it and the workflow would silently use
the image baked into its own `LoadImage` node — burning GPU time on a result that ignores the
declared reference.

The JSON Pointer names the entry (`/images/subject`).

Fix: add an override that binds it, e.g.

  inputs:
    Load Reference:
      image: { image: subject }

where `Load Reference` is the `_meta.title` of the graph's `LoadImage` node — or delete the unused
`images:` entry.",
    },
    CodeEntry {
        code: code::MESH_EDIT_STRUCTURE,
        severity: Severity::Error,
        title: "mesh-edit document structure is invalid",
        explanation: "\
A `kind: mesh-edit` document — the ordered op list that derives an edited copy of a mesh's texture
maps — does not have the shape this engine reads.

An edit document sits beside the mesh it edits and never rewrites it:

  format: 1
  kind: mesh-edit
  name: idol-lit          # optional, defaults to the file stem
  source: mesh-0.mesh.yaml   # required, relative to THIS document's directory
  out: idol-lit           # optional, the derived directory; defaults to this document's stem
  ops:
    - op: paint
      map: emissive
      color:
        - 0.05
        - 1.0
        - 0.35
      region:
        sphere:
          at:
            - 0.118
            - 1.342
            - 0.203
          radius: 0.035

This code covers: an empty or non-mapping document; a missing `format`, `kind`, `source` or `ops`;
a `kind` other than `mesh-edit`; a wrong-typed value (a non-string `source`, `out` or `name`, a
non-sequence `ops` or `color`, a non-mapping op or region); a top-level or per-op key the document
does not have at all; an unrecognized `op`; a `map` that is not one of the five texture slots
(`base_color`, `metallic_roughness`, `normal`, `occlusion`, `emissive`); a `region` that is not
exactly one shape of `sphere`, `capsule`, `box` or `fill`; a region `key` that is not a mapping of
exactly `map`, `color` (three numbers) and `tolerance`; the retired capsule spelling `from`/`to`,
which is now a polyline `points:` (the message shows the rewrite); and a `map` this engine
recognizes but does not paint — `emissive` is the one map an op may target.

The JSON Pointer names the offending key (`/source`, `/ops/0/map`, `/ops/0/region/sphere/radius`);
an unknown key carries a nearest-match suggestion.

Fix: correct the named key. Values that parse but do not hang together — a radius of 0, a colour
component above 1 — are the separate `L1174`. This code also carries the drift report when a
committed derived tree no longer matches what the edit document derives, the same way the atlas
pipeline reports its drift under the atlas document's own structure code.",
    },
    CodeEntry {
        code: code::MESH_EDIT_DATA,
        severity: Severity::Error,
        title: "mesh-edit document data is inconsistent",
        explanation: "\
A `kind: mesh-edit` document parsed structurally, but its values do not describe an edit that can be
applied.

This code covers:

  - a non-finite number (`NaN`, `±Infinity`) anywhere — a position, radius, extent, colour
    component, strength or feather;
  - a non-positive radius, `max_radius` or box extent — a region with no volume covers no texel;
  - a capsule with fewer than two `points` — one point is not a segment;
  - a `size` of 0 for a map the source does not have — a created map needs a square extent;
  - a colour component outside `[0, 1]`; the authored value is an sRGB colour, not an intensity,
    because intensity lives on the scene's `Mesh.emissive`;
  - a `strength` outside `[0, 1]` — it scales the region's weight, and a weight is already `1`
    inside the region's surface;
  - a negative `feather` — the falloff is a distance outside the surface, so `0` is the sharp edge;
  - a negative colour `tolerance`, on a `key` or a `fill` — it is a distance, so `0` is an exact match;
  - a negative `color_feather`, on a `key` or a `fill` — the colour fade past `tolerance`, so `0`
    is the hard colour edge;
  - an empty `source`, or an empty `out`;
  - an `out` that escapes the document's own directory: a `..`-only segment, a leading `/` or `\\`,
    any `:` (drive, scheme or stream), or a Windows device name such as `nul`. The derive owns
    every byte under `out`, so it may only write below the document it derives from;
  - a `source` that is absolute or carries a `:` (a `..` is fine, the derive only reads it);
  - a `source` inside the edit's own `out`: the derive rewrites that directory, so the mesh it reads
    would be replaced by its own output. Point `source` at the mesh the tree was derived from, or
    give a chained edit an `out` of its own;
  - a base-colour map outside the 1..=4096 texel range when a created map takes its size from it:
    add `size:` to create a smaller map.

The JSON Pointer names the offending value (`/ops/0/color/1`, `/ops/0/region/sphere/radius`,
`/out`).

Fix: correct the named value. Whether `source` exists on disk, whether it carries `uvs`, and whether
a `size` names a map the source already has are checked by the derive, which resolves those paths
against the document's own directory — not by the document layer. Structure problems (a missing key,
a wrong type, an unknown key) are `L1173`.",
    },
    CodeEntry {
        code: code::MESH_EDIT_PICK_UNRESOLVED,
        severity: Severity::Error,
        title: "pick resolved to no surface",
        explanation: "\
A pick — one ray cast through a model sheet's own camera, turning a pixel an author is looking at
into a point on the mesh — hit nothing.

This code covers:

  - no triangle under that pixel: the ray passed beside the model, or through a hole in it;
  - a view name the model sheet does not draw;
  - a pixel outside the extent the pick assumed.

The message names the view and the axis it looks down, plus the pixel extent assumed, because a pick
is only meaningful against the exact image the author read the pixel off: the sheet's `front` view
sits on `-Z`, the documented voxel convention, and calling the authored front `+Z` gives an answer
that is 180 degrees wrong rather than an error.

Fix: re-read the pixel off the sheet the pick names, pass the same `--width`/`--height` the sheet was
rendered at, or pick a pixel that lands on the model.",
    },
    CodeEntry {
        code: code::MESH_EDIT_INAPPLICABLE,
        severity: Severity::Error,
        title: "edit cannot apply to this source",
        explanation: "\
The edit document is valid on its own terms, but it does not fit the mesh it names.

This code covers:

  - a `source` mesh with no `uvs`: a texture map is sampled at vertex UVs, so an edit has nowhere to
    paint;
  - a target map the source does not have and the op does not size: the first op targeting a new map
    must say how large to make it (`size:`), or the source must carry a base-colour map to take the
    dimensions from;
  - a derived directory holding files the derive did not write. The derive owns every byte under
    `out`, so a stray file there is either a hand edit of generated content or the wrong directory;
  - a derived directory holding the tree of another edit (its `generated_from:` names a different
    document). One edit owns one `out`; two sharing it would overwrite each other's maps;
  - a source whose triangles overlap so heavily in UV space that rasterising or bridging its seams
    would exceed the derive's work ceiling (`MAX_RASTER_WORK`). Re-unwrap the source so its islands
    do not stack, or lower the map `size`;
  - a UV outside the `[0, 1]` square;
  - a map extent of 0, or above `MAX_EXTENT` (4096) texels a side;
  - position, UV and index lists that disagree in length, or an index past the vertices.

The message names the source, the map, and the exact path when a file is the problem.

Fix: give the source mesh `uvs` (re-import or re-generate it with a UV set), add `size:` to the op
that creates the map, give the edit an `out` of its own (omitting `out:` takes the document's stem,
which is unique per edit), delete a tree whose edit is gone, or empty the derived directory and let
the derive rewrite it. The derived tree is write-only: it is regenerated from the source, the edit
document and the source's maps, and never hand-edited.",
    },
    CodeEntry {
        code: code::MESH_EDIT_OP_PAINTED_NOTHING,
        severity: Severity::Error,
        title: "op painted nothing",
        explanation: "\
An op ran and changed not one texel, because its region covers no part of the mesh's surface.

This is an error rather than a warning because it is the failure a blind author is most likely to
make and least likely to see: a region primitive written in the wrong coordinates, at the wrong
scale, or around a point that is near the model but not on it produces a derived map identical to
the one before it, and every downstream check passes.

The message names the op's index, the region's world-space box and the mesh's own bounds, so the two
can be compared without rendering anything. When the region does reach texels but every weight is
zero (a `strength` of 0), the message says so instead and the fix is the `strength`.

Fix: move or grow the region until the two boxes overlap. A pick names a point that is certainly on
the surface, in the mesh's own coordinates, which is the space a region is written in; the per-op
report then says how many texels the op painted and where they landed.",
    },
    CodeEntry {
        code: code::MESH_EDIT_MATCH_FAILED,
        severity: Severity::Error,
        title: "colour match failed",
        explanation: "\
A region's `key` filter kept no texel, or a `fill` region could not start from its seed.

This code covers:

  - a `key` that rejects every texel its region reaches: the region does reach the surface, but no
    texel of the key's map lies within `tolerance` of the key's `color` (nearer than
    `tolerance + color_feather` when the key has a `color_feather`);
  - a `fill` whose seed `at` is not on the surface, or lies outside its `max_radius` of every
    texel the surface covers (with a `feather`, a seed exactly on the radius counts as outside,
    since the fill would weigh it `0`).

A `key` message names the op, how many texels the region reached, the nearest distance found and
the colour of that texel. The distance is Euclidean over sRGB components in [0, 1], the same metric
the `tolerance` is written in, so the two numbers compare directly. A `fill` message names the seed,
its distance from the surface or from the nearest texel, and the bound it broke.

This is an error rather than a warning because a match that finds nothing is indistinguishable from
a region written in the wrong place: the derived map is identical to the one before it, and every
downstream check passes.

Fix: raise `tolerance` to at least the nearest distance the message names, or re-sample the `color`
from the map at a picked point, since a pick names a point that is certainly on the surface. For a
`fill`, re-pick the seed on the surface with `ludion mesh pick`, or raise `max_radius` above the
distance the message names.",
    },
    CodeEntry {
        code: code::COLLIDER_DEGENERATE_HULL,
        severity: Severity::Error,
        title: "convex hull encloses no volume",
        explanation: "\
A `Collider` component's `convex` shape's `points` do not enclose any volume.

`convex` points are hand-authored scene data (no hull is generated from a mesh or model), so this is a hull an
author wrote by hand. Five cases raise it. Four are caught on load, by the semantic validator, each
naming which applies and how many points were found: fewer than four points, or every point
coincident, collinear (all on one line), or coplanar (all on one plane). The fifth is a runtime
case the validator cannot see: the points pass that check but the solver's own hull builder still
rejects them within its own tolerance (nearly coplanar or nearly coincident) — the entity then gets
no rigid body, this diagnostic is reported once, naming the entity rather than a JSON Pointer, and
it stays bodiless until its `Collider` changes.

The JSON Pointer names `/shape/convex/points` for the on-load cases.

Fix: spread the points so they clearly enclose a volume — e.g. the corners of the prop's bounding
shape — or use `box`/`sphere`/`capsule` instead for a shape that is genuinely flat or thin.",
    },
    CodeEntry {
        code: code::COLLIDER_HEIGHTFIELD_GRID,
        severity: Severity::Error,
        title: "heightfield grid is malformed",
        explanation: "\
A `Collider` component's `heightfield` shape's grid does not hang together.

`rows` and `cols` must each be at least 2 — the solver needs at least one cell in each direction —
and `heights` must hold exactly `rows * cols` values, row-major.

The JSON Pointer names `/shape/heightfield/rows`, `/shape/heightfield/cols`, or
`/shape/heightfield/heights`, whichever is wrong; the message states the grid the given `rows`/`cols`
need and how many heights were actually found.

Fix: set `rows`/`cols` to at least 2 each, and supply exactly `rows * cols` heights.",
    },
    CodeEntry {
        code: code::JOINT_INVALID,
        severity: Severity::Error,
        title: "joint is structurally invalid",
        explanation: "\
A `Joint` component's fields do not hang together as a constraint JSON Schema alone cannot verify.

Five cases raise it, each naming the offending field with a JSON Pointer:

  - `a` and `b` name the same entity (at `/a`) — a joint needs two distinct bodies, even though
    either one alone may equal the joint's own carrier;
  - `anchor`, or a kind's `axis` (`hinge`/`slider`), is not finite — a `NaN` or infinite coordinate
    would poison the solver frame the joint is built from;
  - a kind's `axis` is the zero vector — there is no rotation or slide direction to build a joint
    frame from;
  - a `[min, max]` limit (`hinge`'s, `slider`'s, or one of a `ball`'s three) is non-finite or has
    `min > max`;
  - `break_impulse` is set but is not a strictly positive, finite number.

Fix: name two distinct bodies, give `anchor` and every axis finite, non-zero coordinates, order each
limit as `[min, max]` with both finite, and give `break_impulse` a value greater than 0 if set at
all — or leave it unset for a joint that never breaks.",
    },
    CodeEntry {
        code: code::ASSEMBLY_JOINT_INVALID,
        severity: Severity::Error,
        title: "assembly joint entry is structurally invalid",
        explanation: "\
An assembly document's `joints:` entry does not hang together as a constraint `L1137`'s shape check
alone cannot verify — the `Joint` component's `L1183` checks, one for one, applied to an assembly's
authored joint list instead of a live `Joint` component.

Six cases raise it, each naming the offending field with a JSON Pointer under `/joints/<part>`:

  - the entry's key names a part this assembly does not place;
  - the entry's key names the assembly's one root part — a joint is carried by a non-root part and
    joins it to its `parent`, which the root does not have;
  - a kind's `axis` (`hinge`/`slider`) is not finite, at `/axis`;
  - a kind's `axis` is the zero vector, at `/axis`;
  - a `[min, max]` limit (`hinge`'s, `slider`'s, or one of a `ball`'s three) is non-finite or has
    `min > max`, at `/limits` (or `/limits/<index>` for a ball);
  - `break_impulse` is set but is not a strictly positive, finite number, at `/break_impulse`.

Fix: key the entry by a non-root part's name, give every axis finite, non-zero coordinates, order
each limit as `[min, max]` with both finite, and give `break_impulse` a value greater than 0 if set
at all — or leave it unset for a joint that never breaks.",
    },
    CodeEntry {
        code: code::MESH_PART_UNKNOWN,
        severity: Severity::Error,
        title: "assembly part mesh names an unknown part",
        explanation: "\
A `Mesh` component's `shape: {assembly_part: {assembly, part}}` names a `part` the referenced
assembly does not place. `assembly_part` draws one part of an assembly as its own entity (the
ragdoll switch's per-part meshes, or a hand-authored part-only entity) — the assembly must load, and
`part` must equal one of its `parts:` entries' `name`.

The JSON Pointer names the mesh's `/entities/<index>/components/Mesh/shape/assembly_part/part`; a
failure to load the assembly itself is reported at the sibling `.../assembly` pointer instead, under
whatever code the underlying load failure carries.

Fix: check the assembly document's `parts:` for the part's exact name — a typo is the likeliest
cause — and correct `part` (or `assembly`) to match.",
    },
    CodeEntry {
        code: code::COLLIDER_LAYER_UNKNOWN,
        severity: Severity::Error,
        title: "collider names an undeclared collision layer",
        explanation: "\
A `Collider`'s `layer`, or an entry of a `QueryLayers`, names a layer the scene's `physics.layers`
does not declare. Layers are declared once, in the scene's `physics:` block, with the layers each
one ignores; a collider names at most one of them as its body layer, and a collider that names none
is on the implicit `default` layer.

The JSON Pointer is `/entities/<index>/components/Collider/layer`, or
`/entities/<index>/components/QueryLayers/layers/<j>`. At runtime a collider with an unknown body
layer gets no body until its `Collider` or the scene's layers change; an unknown `QueryLayers` entry
keeps the body and gives the entity no query-only layers until the entry or the table changes.

Fix: check the spelling against `physics.layers` (the diagnostic suggests the nearest declared
name), or declare the layer:

  physics:
    space: free
    layers:
      - name: dummy
        ignores:
          - dummy",
    },
    CodeEntry {
        code: code::COLLIDER_LAYER_UNCLAIMED,
        severity: Severity::Error,
        title: "collision layer on a collider the 3D solver does not claim",
        explanation: "\
A `Collider` carries a `layer`, but the 3D solver never sees it: it is a 2D `circle` or `aabb`
that is not `rigid`, and the 2D tier has no filtering, so the layer would silently do nothing.

The JSON Pointer is `/entities/<index>/components/Collider/layer`.

Fix: remove `layer`, or give the collider a 3D shape (`sphere`, `box`, `capsule`, `convex`,
`heightfield`) or a `rigid` body, which the 3D solver claims.",
    },
    CodeEntry {
        code: code::QUERY_LAYERS_INVALID,
        severity: Severity::Error,
        title: "query-only layers entry is invalid",
        explanation: "\
A `QueryLayers` entry cannot be used. Two cases raise it, each with a JSON Pointer
`/entities/<index>/components/QueryLayers/layers/<j>`:

  - an entry is empty or repeats an earlier entry (the component validator);
  - an entry names the entity's own body layer (`check-scene`), which a query already sees.

Fix: delete the entry, or name another declared layer:

  QueryLayers:
    layers: [light_occluder]",
    },
    CodeEntry {
        code: code::QUERY_LAYERS_UNCLAIMED,
        severity: Severity::Error,
        title: "query-only layers on an entity no query can see",
        explanation: "\
An entity carries `QueryLayers` but has neither a `Collider` the 3D solver claims nor a
`CharacterMotor`, so no query ever reaches it and the layers would silently do nothing. Raised by
`check-scene` at `/entities/<index>/components/QueryLayers`.

Fix: add a 3D `Collider` (or a `CharacterMotor`) to the entity, or remove `QueryLayers`.",
    },
    CodeEntry {
        code: code::SURFACE_UNKNOWN,
        severity: Severity::Error,
        title: "a surface tag names no declared surface",
        explanation: "\
A `SurfaceTag.surface` (at `/entities/<index>/components/SurfaceTag/surface`) or the scene's
`physics.default_surface` (at `/physics/default_surface`) names a surface that `physics.surfaces`
does not declare. At runtime the entity keeps its body and its hits report the scene's default
surface, with one rejection per distinct cause.

Fix: add the name to `physics.surfaces`, or correct the spelling (the message names the nearest
declared surface):

  physics:
    surfaces: [concrete, metal_grate]
    default_surface: concrete",
    },
    CodeEntry {
        code: code::SURFACES_INVALID,
        severity: Severity::Error,
        title: "the scene's surface list is malformed",
        explanation: "\
A `physics.surfaces` entry is empty or repeats an earlier entry, the list holds more than 256
names, or a `SurfaceTag.surface` is empty. The JSON Pointer names the offending entry.

Fix: give each surface one unique, non-empty name; keep the list to 256 or fewer.",
    },
    CodeEntry {
        code: code::SURFACE_TAG_UNCLAIMED,
        severity: Severity::Error,
        title: "a surface tag on an entity no hit can report",
        explanation: "\
An entity carries `SurfaceTag` but has neither a `Collider` the 3D solver claims nor a
`CharacterMotor`, so no query or ground probe ever reaches it and the tag would silently do
nothing. Raised by `check-scene` at `/entities/<index>/components/SurfaceTag`.

Fix: add a 3D `Collider` (or a `CharacterMotor`) to the entity, or remove `SurfaceTag`.",
    },
    CodeEntry {
        code: code::SURFACE_OVERLAY_NO_SENSOR,
        severity: Severity::Error,
        title: "a surface overlay without a sensor volume",
        explanation: "\
An entity carries `SurfaceOverlay` but has no `Collider` the 3D solver claims with
`sensor: true`, so the overlay has no volume and would never apply. Raised by `check-scene` at
`/entities/<index>/components/SurfaceOverlay`.

Fix: give the entity a 3D `Collider` with `sensor: true` shaped like the patch (a box, sphere,
cylinder, capsule, convex or compound), or remove `SurfaceOverlay`.",
    },
    CodeEntry {
        code: code::SURFACE_PART_INVALID,
        severity: Severity::Error,
        title: "a surface tag part entry that names no part",
        explanation: "\
A `SurfaceTag.parts` entry has an empty or repeated key, an integer key that is not a part of the
entity's `compound` `Collider` (no compound, or an index past its last part), or a string key on an
entity whose `Mesh` is not an `assembly`. Raised by `check-scene` at
`/entities/<index>/components/SurfaceTag/parts/<j>/part`; an empty or repeated key is also a
semantic-validation error at `/parts/<j>/part`.

Fix: use an integer below the compound's part count, or an assembly part name on an `assembly`
entity; give each key once.",
    },
    CodeEntry {
        code: code::SURFACE_PART_UNMATCHED,
        severity: Severity::Warning,
        title: "a surface tag part name that matches no assembly part",
        explanation: "\
A `SurfaceTag.parts` string key names no part of the assembly `switch_to_ragdoll` switched, so no
ragdoll part took that entry's surface; every part took the root's `surface` instead. Reported once
per entity and key; the switch itself succeeds.

Fix: correct the key to one of the assembly's part names (the help names the closest).",
    },
    CodeEntry {
        code: code::MESH_SURFACES_INVALID,
        severity: Severity::Error,
        title: "a malformed per-triangle or per-cell surface list",
        explanation: "\
A mesh document's `surfaces`/`triangle_surfaces` keys (or a `SurfaceTag.cells` table) are
malformed: only one of the two mesh keys is present, a name is empty or repeated, there are more
than 256 names, a run has a zero count or a local index past the name list, or the run counts do
not sum to the mesh's triangle count (a heightfield's `(rows-1)*(cols-1)` cells). Raised at
`/surfaces` or `/triangle_surfaces/<k>` in the mesh document, or at `/cells/...` on `SurfaceTag`.

Fix: give both keys together, name each surface once, and make the run counts cover every
triangle (or cell) exactly once.",
    },
    CodeEntry {
        code: code::MESH_SURFACE_UNDECLARED,
        severity: Severity::Warning,
        title: "a mesh surface name the scene does not declare",
        explanation: "\
A mesh document's `surfaces` list names a surface the scene's `physics.surfaces` does not
declare. The triangles tagged with that name fall through to the part, entity and default surface.
Reported by `check-scene` at the mesh's path and once per name at runtime.

Fix: declare the name in `physics.surfaces`, or correct it to one of the declared names (the help
names the closest).",
    },
    CodeEntry {
        code: code::AI_MACHINE_HIERARCHY,
        severity: Severity::Error,
        title: "a machine's parent function is not a tree under ROOT",
        explanation: "\
A `Machine`'s `parent` fn does not describe a tree rooted at `ROOT`: either `ROOT` itself has a
parent, or some state's chain of parents never reaches `ROOT` (a missing parent, or a cycle).
`register_machine` refuses it, naming the state, because the dispatch walk (root to leaf, leaf up
to the common ancestor on a transition) needs every active path to start at `ROOT`.

Fix: return `None` from `parent(ROOT)` and `Some(..)` for every other state, leading up to `ROOT`.",
    },
    CodeEntry {
        code: code::AI_MACHINE_INITIAL,
        severity: Severity::Error,
        title: "a machine's initial state is not a child of its state",
        explanation: "\
A `Machine`'s `initial(s)` returned a state whose `parent` is not `s`. Entering `s` follows
`initial` down one level at a time, so the initial child has to sit directly below it; otherwise the
active path would skip or leave the hierarchy. `register_machine` refuses it.

Fix: make `parent(child)` return `Some(s)`, or return a different child from `initial(s)`.",
    },
    CodeEntry {
        code: code::AI_MACHINE_VARIANTS,
        severity: Severity::Error,
        title: "a machine's ALL list is incomplete or repeats a state",
        explanation: "\
A `Machine`'s `ALL` is empty, omits `ROOT`, or lists a variant twice. `ALL` is the machine's table
of states, used to validate the hierarchy and an authored start state; a wrong one would hide a state
from every check. `register_machine` refuses it.

Fix: list every variant of the enum exactly once, `ROOT` included.",
    },
    CodeEntry {
        code: code::AI_EVENT_REF_NOT_NULLABLE,
        severity: Severity::Error,
        title: "a machine's event holds an entity reference that cannot be null",
        explanation: "\
A `Machine`'s `Event` type holds an entity reference that is not an `Option`. Events wait in an
agent's inbox, which a scene save, a scene carry and a snapshot all remap. A target that despawns
before the event is handled would make the save refuse (`L1018`) or drop the whole `Mind` on a
carry (`L3043`); a nullable reference is written as `null` instead. `register_machine` refuses it.

Fix: write the field as `Option<EntityId>` marked
`#[schemars(with = \"Option<EntityRefSchema>\")]`, and treat `None` as a target that is gone.",
    },
    CodeEntry {
        code: code::AI_TRANSITION_CHAIN_CAPPED,
        severity: Severity::Warning,
        title: "an agent changed state too many times in one tick",
        explanation: "\
An agent's machine made more than `MAX_TRANSITIONS_PER_TICK` (8) transitions in a single gameplay
tick and stopped where it was; the rest of the chain was refused. This is what a loop looks like:
two states whose `enter` or `exit` callbacks (through `Cx::go`) or `update` answers send the agent
back and forth. The message names the agent and the state it stopped in.

Fix: break the cycle. Give one of the states a condition that is not true on entry (a timer, an
event), or move the work to `update` where one answer applies per tick.",
    },
    CodeEntry {
        code: code::AI_INBOX_OVERFLOW,
        severity: Severity::Warning,
        title: "an agent's event inbox overflowed",
        explanation: "\
An agent's inbox already held `INBOX_CAPACITY` events (16 by default) when another arrived, so the
oldest was dropped and the agent's `dropped` counter rose. The inbox is drained once per gameplay
tick, so an overflow means more events arrive in a tick than the machine can read, or the agent's
`ai_tick` is not scheduled (or the game is held).

Fix: send fewer events (broadcast only on a change), raise `Machine::INBOX_CAPACITY`, or check that
`ai_tick::<M>` is registered.",
    },
    CodeEntry {
        code: code::AI_GROUP_MISSING,
        severity: Severity::Warning,
        title: "an agent belongs to a group that does not exist",
        explanation: "\
An agent's `GroupMember.group` names an entity that is despawned or carries no `Group`. The agent
keeps running, with `Cx::group` set to `None`, so a callback that broadcasts to its squad does nothing.

Fix: point `group` at a live entity with a `Group` component, or clear the field.",
    },
    CodeEntry {
        code: code::AI_SEND_NO_MIND,
        severity: Severity::Warning,
        title: "an event was sent to an entity with no mind",
        explanation: "\
`send` (or `Cx::send` / `Cx::broadcast`) addressed an entity that has no `Mind` of that machine: it
is despawned, never had one, or runs a different machine. The event was dropped. A broadcast reaches
only `GroupMember`s, so a member without a `Mind` of the sender's machine raises this too.

Fix: send to an agent of the same machine, or give the entity the mind.",
    },
    CodeEntry {
        code: code::AI_STATE_NOT_LISTED,
        severity: Severity::Warning,
        title: "a mind starts in a state its machine does not list",
        explanation: "\
A `Mind`'s authored `state` is not one of its machine's `ALL`. Only a hand-edited document or a
restored snapshot from an older build can hold such a value. The agent starts from `ROOT` instead.

Fix: author a state the enum still has, or add it to `ALL`.",
    },
    CodeEntry {
        code: code::JOINT_MOTOR_INVALID,
        severity: Severity::Error,
        title: "joint motor is structurally invalid",
        explanation: "\
A `JointMotor` component's fields are not a motor the solver can drive a joint with.

Four cases raise it, each naming the offending field with a JSON Pointer:

  - `target` is not 1 number (a hinge's degrees or a slider's units) or 3 numbers (a ball's x/y/z
    degrees), at `/target`;
  - a `target` element is not finite, at `/target/<i>`;
  - `stiffness` or `damping` is negative or not finite, at `/stiffness` or `/damping`;
  - `max_force` is negative or not finite, at `/max_force`.

Whether the arity fits the carrier's joint kind is a separate runtime check (`L3037`), since a motor
is validated alone, before it meets its `Joint`.

Fix: give `target` one number for a hinge or slider and three for a ball, and give `stiffness`,
`damping` and `max_force` finite values of zero or more:

  JointMotor:
    target: [30.0]
    stiffness: 40.0
    damping: 8.0",
    },
    CodeEntry {
        code: code::JOINT_ZERO_INVALID,
        severity: Severity::Error,
        title: "joint zero is structurally invalid",
        explanation: "\
A `Joint`'s `zero` is not a quaternion the solver can measure the joint from.

Two cases raise it, both at `/zero`:

  - an element is not finite;
  - the four elements have a length under 1e-6, so the quaternion cannot be normalized.

`Joint.zero` stores the joint's zero rotation (`q_a⁻¹·q_b`, `[x, y, z, w]`) in place of the one the
physics layer would capture from the bodies' pose at build.

Fix: give `zero` four finite numbers of a non-zero quaternion:

  Joint:
    a: { index: 0, generation: 0 }
    b: { index: 1, generation: 0 }
    kind: { fixed: {} }
    zero: [0.0, 0.0, 0.0, 1.0]",
    },
    CodeEntry {
        code: code::POSE_SNAPSHOT_INVALID,
        severity: Severity::Error,
        title: "pose snapshot is structurally invalid",
        explanation: "\
A `PoseSnapshot` component holds a part pose a crossfade cannot start from.

Two cases raise it, each at `/parts/<index>/<field>`:

  - a `rotation` or `translation` element is not finite;
  - a `rotation`'s four elements have a squared length under 1e-12, so it is not a quaternion.

`switch_ragdoll_to_assembly` writes a valid snapshot of a folded ragdoll's parts; this code means
the component was authored or edited by hand.

Fix: give each part four finite rotation numbers (`[x, y, z, w]`, not all zero) and three finite
translation numbers:

  PoseSnapshot:
    parts:
      - part: torso
        rotation: [0.0, 0.0, 0.0, 1.0]
        translation: [0.0, 0.0, 0.0]",
    },
    CodeEntry {
        code: code::PAIRED_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "a paired document is malformed",
        explanation: "\
A `kind: paired` document is structurally invalid: an empty or non-mapping root, a missing `format`, `kind`,
`name`, `roles` or `align`, an unknown key (the message names the nearest valid one), or a role, offset,
event, contact or number of the wrong shape. The pointer names the spot, such as `/roles/target/clip`.
Data rules (an unknown reference role, an event out of order) are the separate `L1753`.

Fix: a paired document is `format: 1`, `kind: paired`, a `name`, two or more `roles` and an `align` block;
see `ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::PAIRED_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "a paired document is inconsistent",
        explanation: "\
A well-shaped `kind: paired` document breaks a data rule inside the file: fewer than 2 roles, an
`align.reference` or `align.moves` that names no role (the message names the nearest), an offset for an
unknown role, for the reference, or missing for a non-reference role, `ticks: 0`, a number that is not
finite or is beyond the f32 range, a `scale` of 0 or less, a role with both or neither of `clip` and
`align_only`, an empty name, an event with a negative `t`, out of order or repeated, or a contact that is
not `role/part`, names an unknown role, has both sides on one role, names an unknown event in `at`, or has
a negative `max_gap`. The pointer names the field.

Fix: correct the named field; see `ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::PAIRED_ROLE_MISMATCH,
        severity: Severity::Error,
        title: "a paired document does not match its role assemblies",
        explanation: "\
A `kind: paired` document is consistent on its own but disagrees with the assemblies its roles name: a
role's `assembly` does not load, a `clip` is not one of that assembly's clips (the message names the
nearest), an event `t` is past a role clip's duration, or a contact names a part the role's assembly lacks.
The pointer names the field, such as `/roles/target/clip` or `/contacts/0/b`.

Fix: use a clip and part name the assembly has, or shorten the events to fit the clip; see
`ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::PAIRED_CONTACT_GAP,
        severity: Severity::Warning,
        title: "a paired contact is farther apart than allowed",
        explanation: "\
`ludion check-scene` posed both roles at a contact's event time and measured a gap between the two named
parts that exceeds the contact's `max_gap`. The pointer is `/contacts/<index>`. The gap is in the first
role's cells; a role's `scale` converts the metre offsets to cells, so a wrong `scale` shows up here.

Fix: move the target's `align.offsets` entry closer, change the event time, or raise `max_gap`; see
`ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::COLLIDER_COMPOUND_INVALID,
        severity: Severity::Error,
        title: "compound collider is structurally invalid",
        explanation: "\
A `Collider` whose shape is a `compound` cannot be built into one solver collider.

Three cases raise it, each naming the offending value with a JSON Pointer:

  - `parts` is missing or empty, at `/shape/compound/parts`;
  - a part's `shape` is a `compound`, a `heightfield`, a `circle` or an `aabb`, at
    `/shape/compound/parts/<i>/shape` — a part must be a `sphere`, `box`, `capsule`, `cylinder` or
    `convex` (`circle` and `aabb` are the 2D forms);
  - a part's `translation` or `rotation` element is beyond `f32` range, at
    `/shape/compound/parts/<i>/translation/<j>` or `/shape/compound/parts/<i>/rotation/<j>`.

A part's own sizes are still `L1116` and a degenerate `convex` part is still `L1181`, each with the
pointer under `/shape/compound/parts/<i>/shape`.

Fix: give the compound at least one part; move a `heightfield` or a nested `compound` to its own
entity, or flatten nested parts into the one list; keep every `translation` and `rotation` (Euler XYZ
degrees) finite:

  Collider:
    shape:
      compound:
        parts:
          - shape:
              box:
                half_extents: [0.5, 0.5, 0.5]
          - shape:
              sphere:
                radius: 0.25
            translation: [0.0, -0.75, 0.0]
    body:
      rigid: {}",
    },
    CodeEntry {
        code: code::MESH_SKIN_STRUCTURE,
        severity: Severity::Error,
        title: "mesh skin keys are malformed",
        explanation: "\
A `kind: mesh` document carries the three skin keys `joints`, `weights` and `bone_indices`, and
their shape is wrong.

The three keys are optional but come together: a skinned mesh has all of them, an unskinned one has
none. Specifically, one of:

  - only one or two of the three keys are present, at the first present key's pointer;
  - `joints` is not a sequence of mappings, or `weights` / `bone_indices` is not a flat sequence of
    numbers / non-negative integers;
  - a joint is missing `name`, `rest` or `inverse_bind`, or its `rest` is missing `t`, `r` or `s`,
    at `/joints/<i>` or `/joints/<i>/rest`;
  - a joint or its `rest` holds a key the format does not define (the message suggests the nearest
    valid key), or `parent` is not a non-negative integer;
  - a joint list has the wrong length: `t` and `s` are 3 numbers, `r` is a quaternion of 4 (x, y, z,
    w), `inverse_bind` is 16 (a column-major matrix).

The JSON Pointer names the offending key or element, such as `/joints/2/inverse_bind`.

Fix: write all three keys, each list one entry per line, and each joint as

  joints:
    - name: torso
      parent: 0
      rest:
        t: [...3]
        r: [...4]
        s: [...3]
      inverse_bind: [...16]

(written as block lists in a document). The values themselves — counts per vertex, index range,
weight sums — are the separate `L1191`.",
    },
    CodeEntry {
        code: code::MESH_SKIN_DATA,
        severity: Severity::Error,
        title: "mesh skin data is inconsistent",
        explanation: "\
A `kind: mesh` document's skin keys parsed cleanly, but the skin does not hang together with the
mesh.

Specifically, one of:

  - `joints` is empty, or has more than 256 entries;
  - a joint's `name` is empty or repeats an earlier joint's name;
  - a joint's `parent` is not an earlier index in `joints` (a parent always precedes its children,
    so a self, forward or out-of-range parent is refused);
  - a `rest` or `inverse_bind` element is not finite, or a `rest.r` quaternion is not unit length
    (within 1e-3);
  - `weights` or `bone_indices` does not have exactly 4 entries per vertex;
  - a weight is not finite or is negative, or a vertex's four weights do not sum to 1 (within
    1e-4);
  - a bone index is not less than the number of joints.

The JSON Pointer names the offending element, such as `/weights/12` or `/joints/3/parent`.

Fix: renormalize the weights, correct the parent order or bone index, or re-export the skin. The
canonical importer output (`ludion import`) always satisfies these. Shape and type problems in the
same keys are the separate `L1190`.",
    },
    CodeEntry {
        code: code::QUEST_SAFE_ZONE_TOO_SMALL,
        severity: Severity::Error,
        title: "a rescue quest's safe zone is too small",
        explanation: "\
A `rescue`-archetype `kind: quest` document authors a `safe_radius` that cannot hold both the player
and a trailing escortee, so the quest's own completion condition — the player standing in the safe
zone with the escortee in tow — can never be satisfied.

Schema validation cannot catch this: `safe_radius` is a well-typed float, and only the game knows how
wide its own escortee's trail distance is. So the rule lives in the game's own doc-validation pass
(`EngineBuilder::validate_docs`), which the engine runs after the stores are installed — at host
start, after a live doc reload, and under `--check-docs`. The diagnostic's path is
`quest/<doc name>/safe_radius`.

Fix: raise `safe_radius` to at least the game's floor (twice the escortee's trail distance), and keep
the zone clear of any gate trigger it might otherwise overlap — a player who stops inside both a gate's
zone and the safe zone crosses instead of completing the rescue.",
    },
    CodeEntry {
        code: code::SKIN_BONE_NOT_SKINNED,
        severity: Severity::Error,
        title: "SkinBone's skin is not a skinned mesh",
        explanation: "\
A `SkinBone` component's `skin` names an entity, but that entity is not a skinned mesh: it carries no
`Mesh` component, its `Mesh.shape` is not `asset`, or the mesh document `shape.asset` names has no
`joints:` skin data.

`SkinBone` drives a part's pose from one rest joint of a skin, so the entity it points at must be the
skinned mesh the joint belongs to.

Fix: point `skin` at the entity carrying the skinned `Mesh` (`shape: { asset: \"meshes/rig.mesh.yaml\" }`
where the mesh document was exported with its armature, e.g. through `ludion import`).",
    },
    CodeEntry {
        code: code::SKIN_BONE_UNKNOWN_BONE,
        severity: Severity::Error,
        title: "SkinBone's bone is not a joint of its skin",
        explanation: "\
A `SkinBone` component's `bone` does not name any joint of its `skin`'s mesh document.

The help names the nearest joint name when one is close enough to suggest a typo, or lists every
joint name the mesh document declares.

Fix: spell `bone` exactly as one of the skin mesh's `joints[].name` entries — `ludion check-scene
<mesh> --bounds` prints the joint tree.",
    },
    CodeEntry {
        code: code::SKIN_BONE_DUPLICATE,
        severity: Severity::Error,
        title: "two SkinBone parts name the same skin and bone",
        explanation: "\
Two `SkinBone` components name the same `(skin, bone)` pair. Each bone of a skin drives at most one
part, so a second part naming the same pair is refused rather than silently letting the earlier one
win.

The JSON Pointer names the later of the two parts; the message names the earlier one by its `name:`
when it has one, else its document position.

Fix: give the second part a different `bone`, or remove one of the two parts.",
    },
    CodeEntry {
        code: code::SKIN_BONE_ANIMATED_SKIN,
        severity: Severity::Error,
        title: "SkinBone's skin also plays an Animation",
        explanation: "\
A `SkinBone`'s `skin` entity carries an `Animation` component. A skin has exactly one pose source: a
skin driven by its parts' `SkinBone` rest poses cannot also play a clip, and a skin playing a clip
needs no `SkinBone` parts at all.

Raised once per skin, no matter how many parts name it, pointed at that skin entity's own `Animation`
component.

Fix: remove the skin entity's `Animation` component, or remove every `SkinBone` part that names this
skin.",
    },
    CodeEntry {
        code: code::ANIMATION_EVENT_INVALID,
        severity: Severity::Error,
        title: "animation clip events are invalid",
        explanation: "\
An animation clip's `events` list does not hold together. One of:

  - `events:` is declared with zero entries (remove the key instead);
  - an event's `t` is not finite or falls outside the clip's `[0, duration]` range;
  - event times are not strictly increasing (events are read in the order authored);
  - an event's `name` is empty or only whitespace;
  - the clip carries `generated_from` — it is derived from an import and never hand-edited, so its
    events live in the character's clip overlay.

The JSON Pointer names the offending list (`/events`) or event field (`/events/<i>/t`,
`/events/<i>/name`).

Fix: keep each event's `t` inside `[0, duration]` with strictly increasing times, give every event a
name, drop an empty `events:` key, and move a derived clip's events into the character overlay.
Shape/type problems (a missing `t`, a non-mapping event) are the separate `L1141`.",
    },
    CodeEntry {
        code: code::CHARACTER_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "character document is structurally invalid",
        explanation: "\
A `kind: character` document does not have the shape the format requires. One of:

  - the file is empty, or its root is not a mapping;
  - `format` is missing or is not the supported version, or `kind` is missing or not `character`;
  - a required key is missing (`mesh`) or a key has the wrong type (`mesh` must be a string, `clips` a
    mapping, a clip entry a mapping with a string `clip`, `speed` a number, `events` a sequence of
    `{t, name}` mappings);
  - a clip entry is missing its `clip` path;
  - a `blends` entry is not a mapping with `points` (two to sixteen `{clip, at}` entries, every `at`
    a finite number or all of them an `[x, z]` block list) and an optional `sync` string;
  - a key is not part of the format (the diagnostic suggests the nearest valid key).

The JSON Pointer names the offending key or entry (`/mesh`, `/clips/walk/clip`, `/clips/walk/speed`,
`/blends/locomotion/points/1/at`).

Fix: write `format: 1`, `kind: character`, `mesh: <path to the skinned mesh document>`, then one
`clips:` entry per clip, each with a `clip:` path and optionally `speed:` and `events:`. Data problems
in a well-shaped document (a blank path, a non-positive speed) are the separate `L1200`.",
    },
    CodeEntry {
        code: code::CHARACTER_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "character document data is inconsistent",
        explanation: "\
A well-shaped `kind: character` document holds values that do not hang together. One of:

  - `mesh` or a clip's `clip` path is blank;
  - a clip's `speed` is not finite or not greater than zero;
  - a clip carries `events` in the overlay while the clip document has its own (a clip's events live
    in one place);
  - a clip track animates a joint the character's mesh does not have (the diagnostic names the joint
    and the nearest one the mesh has);
  - the character's mesh document has no skin, so there is nothing for the clips to drive;
  - a mask has a blank name, or lists a joint the mesh does not have;
  - a blend has the name of a clip binding, two points at one position, or 2D points that are all
    collinear; one of its points names a clip that is not bound, a clip that does not loop, or (with
    `sync`) a clip with no event of that name.

The JSON Pointer names the offending value (`/mesh`, `/clips/<name>/clip`, `/clips/<name>/speed`,
`/clips/<name>/events`, `/masks/<name>/joints/<i>`, `/blends/<name>/points/<i>/clip`,
`/blends/<name>/sync`).

Fix: point `mesh` at a skinned mesh document (an imported glTF with a skin), give every path, keep
`speed` positive, put a clip's events in either the clip or the overlay, and rename the clip's tracks
(or re-export it from the same rig) so every target is a joint of the mesh.",
    },
    CodeEntry {
        code: code::ANIMATION_LAYERS_TOO_MANY,
        severity: Severity::Error,
        title: "AnimationLayers holds too many layers",
        explanation: "\
An `AnimationLayers` component lists more than four layers. Every layer is another clip sampled per
skinned entity per frame, so the list is capped at four.

Pointed at `/layers` of the component.

Fix: drop a layer, or merge two layers that never play together into one and switch its `clip` with
`play_layer`.",
    },
    CodeEntry {
        code: code::ANIMATION_LAYER_INVALID,
        severity: Severity::Error,
        title: "an AnimationLayers layer is invalid",
        explanation: "\
One layer of an `AnimationLayers` component holds a value the compositor cannot use. One of:

  - `name` is empty or repeats an earlier layer's name;
  - `weight` or `from_weight` is outside `[0, 1]` or not finite;
  - `fade` or `speed` is negative or not finite.

Pointed at `/layers/<i>/<field>`.

Fix: give each layer a unique non-empty name, keep weights in `[0, 1]` and `fade`/`speed` finite and
non-negative.",
    },
    CodeEntry {
        code: code::ANIMATION_LAYERS_ON_SKIN_BONE_SKIN,
        severity: Severity::Error,
        title: "AnimationLayers on a SkinBone-driven skin",
        explanation: "\
A skin named by `SkinBone` parts also carries an `AnimationLayers`. Such a skin is posed by its parts'
rest poses, so there is no base pose for layers to compose over.

Raised once per skin, pointed at that skin entity's `AnimationLayers` component.

Fix: remove `AnimationLayers` from the skin, or remove every `SkinBone` part that names it.",
    },
    CodeEntry {
        code: code::IK_TARGET_INVALID,
        severity: Severity::Error,
        title: "an IkTargets target is invalid",
        explanation: "\
One target of an `IkTargets` component holds a value the IK pass cannot use. One of:

  - it sets none, or both, of `point` and `direction`;
  - `weight` or `from_weight` is outside `[0, 1]` or not finite;
  - `fade` is negative or not finite;
  - a `point` or `direction` number is not finite.

Pointed at `/targets/<name>/<field>`.

Fix: set exactly one of `point` or `direction`, keep weights in `[0, 1]`, and keep `fade`
finite and non-negative.",
    },
    CodeEntry {
        code: code::IMPORT_INVALID_GLTF,
        severity: Severity::Error,
        title: "glTF input is unreadable or invalid",
        explanation: "\
`ludion import` could not read or parse the glTF input.

`ludion import` is a compiler: it reads a `.gltf` (JSON) or `.glb` (binary) document and writes a
canonical Ludion scene plus its mesh and texture assets. This diagnostic means it never got past
reading the input — the file could not be opened, its bytes are not a well-formed glTF document
(malformed JSON, a failed structural validation), or a buffer or image the document references
could not be resolved (a missing external `.bin`/image file in the `.gltf`'s directory or below it, or
a `data:` URI whose base64 payload is corrupt). It is also raised when the document parses but its
data is malformed: a non-finite number, a zero rotation quaternion, an accessor that overruns its
buffer view or has a stride shorter than one element, or attributes of different lengths. A node
graph that is not a forest is refused the same way (a node listed as a child twice, or its own
ancestor); the JSON Pointer names the listing to remove. The message includes the underlying reason
and names the element.

Fix: check the path and that the file is a valid glTF export. If the `.gltf` references external
files (its buffers or images use relative URIs), make sure those files sit in its directory or below
it at the paths the document names — `ludion import` resolves them relative to the input file's
directory, exactly as a glTF viewer would. A self-contained `.glb`, or a `.gltf` with `data:` URIs,
has no external dependencies to misplace. For malformed data, re-export from the authoring tool.",
    },
    CodeEntry {
        code: code::IMPORT_UNSUPPORTED_FEATURE,
        severity: Severity::Error,
        title: "glTF feature has no Ludion mapping",
        explanation: "\
A glTF document uses a feature `ludion import` deliberately does not translate, and it was named so
you can see exactly what was dropped.

The importer maps a fixed, exhaustive set of glTF onto Ludion components: the node tree onto
prefab parts with a `Transform` (TRS, quaternion rotations converted to Euler XYZ degrees) and a
`Parent`; mesh primitives onto `meshes/*.mesh.yaml` documents; a material's `baseColorFactor`,
`roughnessFactor`, and `metallicFactor` onto `Mesh.color`, `Mesh.roughness`, and `Mesh.metallic`; the
five metallic-roughness maps onto the mesh document's texture references, each referenced PNG
re-encoded through the deterministic encoder and named by its content hash; `TANGENT`, or a generated
frame where a normal map needs one, onto the document's `tangents`; a `KHR_lights_punctual` point light onto `PointLight`; and a skin — `JOINTS_0`/`WEIGHTS_0`
and inverse bind matrices — onto the mesh document's `joints`/`weights`/`bone_indices`, its joint
nodes folded in rather than emitted as entities; and a material's `MASK` alpha mode with its
`alphaCutoff`, and `doubleSided`, onto the mesh document's `alpha_mode`/`alpha_cutoff` and
`double_sided` keys.

`emissiveFactor` maps onto `Mesh.emissive` as a scalar when it equals some `k` times
`baseColorFactor`'s RGB exactly on the f32 values (`[0, 0, 0]` maps to `0.0`). Any other factor is
baked into an emissive map: `Mesh.emissive` becomes the largest factor/base ratio and the map, the
`emissiveTexture` or one texel, carries the hue. It is reported here, naming the material, only when
a channel's base is below 1/255 while its emissive is not, or when a tint lands on a primitive
without `TEXCOORD_0`; `--allow-lossy` skips just the emissive and warns.

Everything else is reported here rather than silently discarded — a texture on a second UV set or
with per-axis wrap modes that disagree (a reference holds one mode for both axes), a textured
material on a primitive without `TEXCOORD_0`, a `BLEND` alpha mode (sorted alpha-blend has no
mapping; `MASK` maps onto `alpha_mode: mask`), a node whose `matrix` shears (a skew, or a non-uniform scale
above a rotation, which a `Transform` cannot hold), a camera (of any kind; a prefab carries no `Camera`, which a
scene has one of: the node stays as a `Transform`-only part when `--allow-lossy`), a directional light (point and
spot lights map onto `PointLight`/`SpotLight`), a morph target, an animation channel of morph weights, on a node that has no part (it hangs under
a skin joint), or mixing skin joints and prefab parts in one animation, a sparse or
non-triangle-list primitive, a JPEG image that a material references, or any other required extension. A skin is refused where it does not fit the mesh document: more than 256 joints, a
second `JOINTS_n`/`WEIGHTS_n` set, a mesh shared by several skinned nodes (or skinned and unskinned),
a node parented to a skin joint, or a mesh, camera or light on a joint node. A non-joint node between
two joints of a skin is dropped as an entity, and its transform is folded into the rest of the
joints below it; where that fold (or a root's rest relative to the mesh) shears — a non-uniform scale
above a rotation, which a translation/rotation/scale rest cannot hold — the joints below deform off
the source and this reports it. A fold or root rest that is singular (zero scale on an axis, or near-parallel axes)
imports collapsed to a point (scale 0, no rotation) and is reported too. A clip channel on a folded joint is dropped. Each diagnostic names
the offending node, mesh, material, or accessor.

By default any such feature is a hard error and nothing is written — an import either maps cleanly
or tells you precisely what it cannot carry. Pass `--allow-lossy` to downgrade these to printed
warnings and compile the part of the document that does map; the output is then a faithful
translation of the supported subset, with the listed features left out.

Fix: to import losslessly, remove or bake the unsupported feature in your DCC tool before exporting
(put every map on UV set 0 with one wrap mode per texture, delete the camera and place it in the
scene that instances the prefab, delete morph targets, set `emissiveFactor` to a scalar multiple of
`baseColorFactor`, apply scale on the object — Blender: Ctrl+A, Scale — and parent joints to each other
directly, not through an Empty). To import anyway and accept the loss, re-run with `--allow-lossy`.",
    },
    CodeEntry {
        code: code::IMPORT_OUTPUT_IO,
        severity: Severity::Error,
        title: "imported output could not be written",
        explanation: "\
`ludion import` finished compiling the glTF but could not write the output tree to disk.

The importer validates the whole compile plan before it writes anything — so this diagnostic never
leaves a half-written tree. It means the output directory (or its `meshes/`/`textures/`
subdirectories) could not be created, or one of the files (`model.prefab.yaml`, a `*.mesh.yaml`, a
`*.png`) could not be written. The message includes the path and the operating system's reason,
typically a missing parent directory, a permission denial, or a full disk. An `--out` on a
different drive from the glTF is refused here too, before any write: the prefab and clips record
the source as a relative `generated_from`, which has no form across drives.

The same code covers an in-memory failure to re-encode a PNG the importer had just decoded or baked.
That is not a path or disk problem but an importer bug: report it with the input file.

Fix: check that the `--out` directory's parent exists and is writable and that there is free disk
space, then re-run. The import is a pure function of its input, so re-running after fixing the
destination reproduces the identical tree.",
    },
    CodeEntry {
        code: code::IMPORT_URI_REFUSED,
        severity: Severity::Error,
        title: "glTF external source refused",
        explanation: "\
A glTF buffer or image names an external file the import will not read.

External sources resolve only at or below the directory of the `.gltf` being imported. A URI is
refused when it is absolute (`/x`, `\\\\host\\share`), carries a drive, scheme or stream prefix
(`C:`, `file:`), climbs out with `..` (also when percent-encoded, `%2e%2e/`), or contains a NUL
byte. A URI that passes must name a regular file — a directory, device or pipe is refused — of at
most 512 MiB. The diagnostic names the buffer or image, the URI and the reason.

`ludion gen accept` reads a generated `.glb` and takes embedded sources only, so any external URI
in a candidate is refused there.

Fix: keep external buffers and images in the `.gltf`'s directory or below it, referenced by
relative URIs such as `textures/albedo.png`, or export a `.glb` with everything embedded.",
    },
    CodeEntry {
        code: code::IMPORT_SKIN_UNWEIGHTED,
        severity: Severity::Warning,
        title: "skinned vertex with no weight bound to the root joint",
        explanation: "A skinned vertex in the glTF has four weights that sum to zero, so it follows no joint.

`ludion import` maps a glTF skin's `JOINTS_0`/`WEIGHTS_0` onto the mesh document's `bone_indices`
and `weights`, renormalizing each vertex's four weights to sum to one. A vertex whose weights sum to
zero cannot be normalized; the importer binds it fully to the skin's root joint (weight 1 on joint
0), so it follows the root rigidly instead of collapsing at the origin. The diagnostic names the
primitive, how many vertices were bound this way and the root joint. The import still succeeds.

Fix: weight-paint the listed vertices in the DCC tool (every vertex of a skinned mesh needs at least
one influence) and re-export.",
    },
    CodeEntry {
        code: code::IMPORT_EXTENSION_IGNORED,
        severity: Severity::Warning,
        title: "glTF extension ignored by the import",
        explanation: "The glTF lists an optional extension in `extensionsUsed` that `ludion import` has no mapping for.

The asset is still valid without the extension, so the import succeeds and leaves out whatever the
extension adds (its name says what: a specular layer, a clearcoat, a texture transform). The
diagnostic names the extension. An extension the asset *requires* and the importer does not map is
refused outright instead.

Mapped extensions: `KHR_lights_punctual` and `KHR_materials_emissive_strength`.

Fix: re-author the asset without the extension, or bake its effect into data the importer maps (a
base-colour map, an emissive factor). If the extension's effect is not needed, the warning is safe
to leave.",
    },
    CodeEntry {
        code: code::PLACEMENT_FLOATING,
        severity: Severity::Warning,
        title: "placed prop floats above the ground",
        explanation: "`ludion check-scene --placement` found a placement group whose lowest point is above the ground and that touches no other group.

A group is a prefab instance (all its parts) or a hierarchy root with its descendants. Its box is
the union of its meshes' world boxes. It floats when its bottom is more than `--tolerance` (default
0.05 m) above `--ground-y` (default 0) and no other group's box, grown by the tolerance, meets it, so
a torch on a wall or a barrel on a terrace counts as supported. The check is a heuristic over mesh
bounds, so it is a warning and never changes the exit code.

Fix: lower the prop onto the ground or the prop that carries it, or raise `--tolerance` / set
`--ground-y` when the scene's ground is not at y 0.",
    },
    CodeEntry {
        code: code::PLACEMENT_SUNK,
        severity: Severity::Warning,
        title: "placed prop sunk below the ground",
        explanation: "`ludion check-scene --placement` found a placement group whose lowest point is more than `--sink` (default 0.1 m) below `--ground-y` (default 0).

Ground tiles embed a little by design (a cobble's bottom sits a few centimetres under y 0), which is
why the allowance exists. A group further down is usually a prop placed at the wrong height. The
check is a heuristic over mesh bounds, so it is a warning and never changes the exit code.

Fix: raise the prop, or raise `--sink` / set `--ground-y` when the scene's ground is not at y 0.",
    },
    CodeEntry {
        code: code::PLACEMENT_OVERLAP,
        severity: Severity::Warning,
        title: "placed props' colliders overlap",
        explanation: "`ludion check-scene --placement` found two placement groups whose static in-plane colliders overlap.

Colliders are the solid footprints the game collides with, so two overlapping ones mean two props
occupying the same ground. The test shrinks each collider by `--tolerance` (default 0.05 m) so props
that merely touch pass, and ignores parts of one group (a house's walls overlap by design). Props
without a collider may intersect freely and are never reported. Each pair is reported once. It is a
heuristic, so it is a warning and never changes the exit code.

Fix: move one of the props apart, or shrink an over-sized collider.",
    },
    CodeEntry {
        code: code::MESH_GRID_ORIENT,
        severity: Severity::Error,
        title: "mesh grid orientation does not match its rows",
        explanation: "\
A `MeshGrid` component's `orient` grid does not line up with its `rows`, or holds a glyph that is not
an orientation.

`orient` is optional. When present it has the same shape as `rows`: one string per row, one glyph per
cell, so cell `(col, row)` takes its orientation from `orient[row][col]`. Its glyphs are `0`-`3` (that
many quarter-turns about local +Y) and `4`-`7` (the same turn after mirroring local X); a space is
orientation 0. A different row count, a row of a different width, or any other glyph means the
engine cannot know which cell an orientation belongs to, so it refuses rather than guess.

The diagnostic's JSON Pointer names the offending `orient` row (`/orient/<n>`); the message gives the
column and glyph, or the expected and actual shape.

Fix: make `orient` the same size as `rows`, using only `0`-`7` and spaces, or delete `orient` to
leave every cell unrotated.",
    },
    CodeEntry {
        code: code::MESH_GRID_ANIMATED_SHAPE,
        severity: Severity::Error,
        title: "mesh grid legend names an animated shape",
        explanation: "\
A `MeshGrid` component's legend maps a glyph to a `Mesh` whose shape is a `character`.

All of a grid's cells share one entity, so they share one `Animation` clip and one set of `SkinBone`
followers; a skinned character cannot pose or play per cell. The engine refuses the legend entry
rather than draw every cell in the same pose.

The diagnostic's JSON Pointer names the legend entry (`/legend/<glyph>`).

Fix: place animated characters as their own entities (a `Mesh` with a `character` shape, plus an
`Animation`), and keep the grid for static shapes: assets, voxel, sculpt, assembly and primitives.",
    },
    CodeEntry {
        code: code::IMPORT_SYNC_REFUSED,
        severity: Severity::Error,
        title: "derived-import sync refused an import table",
        explanation: "\
`ludion sync` compiles each `[[import]]` table in `ludion.toml` (`from`, a glob of `.gltf` files;
`to`, the derived tree) and refused one before writing anything: `from` matches no file (usually a
typo), two matched files share a stem (each lands in `<to>/<stem>/`; stems that differ only in case
count as shared, being one directory on Windows and macOS), or `to` already holds files
the sync did not write, or its `.ludion-sync.json` record does not parse, or a source directory holds
a symbolic link or junction, which the sync neither follows nor hashes. A tree is the sync's own
only when it carries that record; the sync never deletes a directory it does not own.

The diagnostic names the table, the sources, the directory or the link. A second sync of the same
root waits for the first, and a sync killed midway recovers on the next run: neither is a refusal.

Fix: correct `from`, rename one of the colliding sources, or delete `to` by hand if it is derived
(or point `to` somewhere else); a corrupt record is fixed the same way, then `ludion sync` again.
For a link, replace it with the files, or move it out of the glTF's directory.",
    },
    CodeEntry {
        code: code::IMPORT_CLIP_ERROR_BOUND,
        severity: Severity::Error,
        title: "imported animation clip exceeds its model-space error bound",
        explanation: "After `ludion import` reduced a glTF animation's keys, some joint's model-space pose strays
from the source motion by more than the bound.

`ludion import` writes one `kind: animation` clip per glTF animation, reducing each joint channel's
keys greedily against a per-channel tolerance, then measures the result in **model space** (joint
transforms composed down the skeleton, at every source frame): a small local error on a parent joint
is amplified by the bones below it. The bound is 0.5 degrees of rotation, 0.002 mesh units (2 mm) of
translation and 0.005 of scale by default; keys are reduced at 0.1 degrees, 0.0005 and 0.001. The
diagnostic names the animation, the joint, the frame (time and index), the channel, the error and the
bound; only the worst offence per clip is reported. A refused clip fails the import in both modes:
`--allow-lossy` never downgrades this.

Fix: key the motion more densely or less extremely in the DCC tool (a very fast spin sampled at a low
rate cannot be reproduced between its keys), or raise the bound for this animation with
`bound_rotation_deg`, `bound_translation` or `bound_scale` in its glTF `extras` (Blender: a custom
property on the Action, exported with `export_extras=True`). Tightening `reduce_rotation_deg`,
`reduce_translation` or `reduce_scale` keeps more keys and lowers the error.

A CUBICSPLINE interval is subdivided into 4 to 64 equal steps until each step's straight chord is
within the reduce tolerance of the spline, so the clip stays within that tolerance between frames. An
interval that 64 steps cannot follow is refused with this code too: key the curve more densely,
export it LINEAR, or raise the matching `reduce_*` value (raising a `bound_*` does not help).",
    },
    CodeEntry {
        code: code::IMPORT_CLIP_EXTRAS_INVALID,
        severity: Severity::Error,
        title: "animation extras are invalid",
        explanation: "A glTF animation's `extras` object holds a key `ludion import` does not know, or a tolerance
that is not a finite positive number.

The importer reads these animation `extras` keys: `loop` (boolean), and the tolerances
`reduce_rotation_deg`, `reduce_translation`, `reduce_scale`, `bound_rotation_deg`, `bound_translation` and
`bound_scale` (finite positive numbers; translation in mesh units, 1 unit = 1 metre). Anything else,
or a wrong type, is refused so a typo cannot silently leave the default in force. The diagnostic
names the animation and the key.

Fix: correct or remove the key in the DCC tool (Blender: the Action's custom properties) and
re-export with extras enabled.",
    },
    CodeEntry {
        code: code::IMPORT_CLIP_AT_REST,
        severity: Severity::Error,
        title: "imported animation never leaves the rest pose",
        explanation: "Every channel of a glTF animation stays within the key-reduction tolerance of the rest pose,
so `ludion import` would write a clip with no tracks.

`ludion import` drops a joint channel that is constant at rest and reduces the others against
`reduce_rotation_deg` (0.1 degrees), `reduce_translation` (0.0005) and `reduce_scale` (0.001) by
default. When nothing survives, the action keys no joint away from rest and a trackless clip would be
useless. The diagnostic names the animation. A clip refused this way fails the import in both modes:
`--allow-lossy` never downgrades it.

Fix: delete the empty action in the DCC tool, key the motion you meant to export, or lower the
`reduce_*` tolerances in the action's glTF `extras` (Blender: a custom property on the Action,
exported with `export_extras=True`) if the motion is real but very small.",
    },
    CodeEntry {
        code: code::ANIMATION_SPRITE_CROSSFADE,
        severity: Severity::Warning,
        title: "a sprite flipbook cannot crossfade",
        explanation: "A `Sprite` drawn from an atlas carries an `Animation` with `fade` above zero.

A sprite flipbook shows exactly one atlas region at a time, so it hard-cuts to `clip` when the
clip changes: `from`, `from_started`, `from_speed`, `from_anchor`, `fade_started` and `fade` are
ignored.
Crossfading blends the poses of a skinned character or a voxel assembly, which a flat quad has
none of.

Raised at document time by `check-scene`, and at runtime (once per entity) by the `animation_events`
system for a sprite whose `Animation` gains a `fade` while the game runs.

Fix: drop `fade` and `from` from the sprite's `Animation`, or use a skinned character or an
assembly where the blend matters.",
    },
    CodeEntry {
        code: code::ANIMATION_MODEL_UNLOADED,
        severity: Severity::Error,
        title: "an animated model failed to load at runtime",
        explanation: "An entity carries an `Animation` on a `Mesh` whose assembly or character document
failed to load while the game was running.

Without the model there is no clip timing, so the entity fires no animation events at all: not
its authored ones, and not `end`. A game that waits on `end` (a game whose attack releases on it) stalls.

Raised once per entity by the `animation_events` system, naming the entity, the model path and the
first error the loader gave.

Fix: run `ludion check-scene <model>` on the assembly or character document, and check that the
`Mesh` path resolves relative to the content root the game runs from.",
    },
    CodeEntry {
        code: code::COLLIDER_MESH_ON_RIGID,
        severity: Severity::Error,
        title: "a mesh or heightfield collider on a rigid body",
        explanation: "A `Collider` with `shape: { mesh: <path> }` sits on a `body: rigid`.

A `Collider` with `shape: { heightfield: ... }` on a `body: rigid` is refused the same way.

A mesh collider is a triangle mesh and a heightfield is a surface: neither has an interior, so a
dynamic body has no well-defined contact with it. They are for level geometry, on a `static` body
(or a `kinematic` one that a `PathFollower` moves). No convex hull is generated for you.

Raised at document time by `check-scene`, and at runtime by `ludion-physics` for a `Collider` that
game code inserts directly. The body is not created.

Fix: set `body: static` (or `kinematic`), or give the rigid body a `convex` shape or a `compound`
of convex parts.",
    },
    CodeEntry {
        code: code::COLLIDER_MESH_UNAVAILABLE,
        severity: Severity::Error,
        title: "a mesh collider's document cannot be built",
        explanation: "A `Collider` with `shape: { mesh: <path> }` has no usable triangle mesh.

The cause is one of: a blank `path`; the `kind: mesh` document is missing, unreadable or invalid;
the physics engine refuses the triangles (degenerate or non-finite geometry); or no collider-mesh
library is installed on the world. The body is not created; the diagnostic carries the reason.

A scene with a `physics:` block whose mesh file cannot be opened is refused when it loads or
switches, with the `L1101` as related; `ludion sim-hash` exits 1 with no hash. A restore, a runtime
spawn or a `component.set` gets no body instead.

The mesh is read when a scene loads, a snapshot restores or the scene switches, not by
`asset.reload`. A body whose mesh failed is retried once the document's content changes, at the
next such point.

Fix: run `ludion check-scene <path>` on the mesh document, check that the path is relative to the
content root the game runs from, and give `path` a value.",
    },
    CodeEntry {
        code: code::PATH_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "path document is structurally invalid",
        explanation: "\
A `kind: path` document does not have the shape the format requires. One of:

  - the file is empty, or its root is not a mapping;
  - `format` is missing or is not the supported version, or `kind` is missing or not `path`;
  - `points` is missing, or a key has the wrong type (`name` and `generated_from` strings,
    `interpolation` one of `catmull_rom` or `linear`, `closed` a boolean, `points` a sequence);
  - a point is not a sequence of exactly three numbers;
  - a key is not part of the format (the diagnostic suggests the nearest valid key).

The JSON Pointer names the offending key or entry (`/interpolation`, `/points`, `/points/2`).

Fix: write `format: 1`, `kind: path`, then `points:` as a list of three-number points, one number per
line (`- - x`, `  - y`, `  - z`). Data problems in a well-shaped document (too few points, coincident
neighbours, a non-finite number) are the separate `L1214`.",
    },
    CodeEntry {
        code: code::PATH_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "path document data is inconsistent",
        explanation: "\
A well-shaped `kind: path` document holds values that cannot make a curve. One of:

  - fewer than 2 points, or fewer than 3 when `closed: true`;
  - a point component that is not finite, or is outside the f32 range;
  - two neighbouring points at exactly the same position, including the last and the first when the
    path is closed (a zero-length segment has no direction);
  - a `generated_from` that is blank, absolute, or climbs out of the content root with `..`.

The JSON Pointer names the count (`/points`), the offending point (`/points/<i>`) or
`/generated_from`.

Fix: add points until the minimum is met, move the coincident point apart or delete it, and keep
every component a finite number. The document's shape problems are the separate `L1213`.",
    },
    CodeEntry {
        code: code::PATH_FOLLOWER_UNLOADED,
        severity: Severity::Error,
        title: "a path follower's path failed to load",
        explanation: "\
An entity's `PathFollower.path` names a `kind: path` document the engine could not load: the file is
missing or unreadable, or it fails validation (`L1213`, `L1214`). The entity stays where it is and the
diagnostic carries the loader's reason. Raised once per entity by the engine's `path_follow` system.

The path is read the first time a follower names it, and again after a `scene.load`, `engine.restore`
or `scene.switch`; `asset.reload` does not re-read it.

Fix: run `ludion check-scene <path>` on the document, and check that `PathFollower.path` is relative
to the content root the game runs from.",
    },
    CodeEntry {
        code: code::HIERARCHY_CYCLE,
        severity: Severity::Warning,
        title: "a Parent chain loops at runtime",
        explanation: "An entity's `Parent` chain leads back to itself, so it has no root to compose its `Transform` with.
Scenes refuse such a loop (`L1035`); this one arrived through a raw component insert at runtime. The
entity is treated as a root: its `Transform` is read as a world pose. Raised once per loop.

Fix: re-point one link in the loop at a real parent, or remove that entity's `Parent` component.",
    },
    CodeEntry {
        code: code::HIERARCHY_PARENT_INVALID,
        severity: Severity::Warning,
        title: "a Parent names a dead or presentation entity",
        explanation: "An entity's `Parent` names an entity that is no longer alive, or a sim entity names a presentation
entity as its parent (presentation state is never an ancestor of the simulation). The parent chain
stops there and the entity acts as a root, reading its `Transform` as a world pose. Raised once per
entity.

Fix: re-point `Parent` at a live sim entity, or remove the `Parent` component.",
    },
    CodeEntry {
        code: code::HIERARCHY_TOO_DEEP,
        severity: Severity::Warning,
        title: "a Parent chain is deeper than 64",
        explanation: "An entity has more than 64 ancestors. World transforms are composed along the whole chain on every
read, so the depth is capped; the entity is treated as a root and reads its `Transform` as a world
pose. Raised once per entity.

Fix: flatten the hierarchy so no chain exceeds 64 levels.",
    },
    CodeEntry {
        code: code::HIERARCHY_RIGID_BODY,
        severity: Severity::Error,
        title: "a rigid body under a Parent",
        explanation: "A `Collider` with `body: rigid` sits on an entity that has a `Parent`. A rigid body's pose belongs to
the solver, which writes it back to `Transform` as a world pose; a parent would compose that world
pose a second time. The entity gets no physics body until the `Parent` is removed. `entity.reparent`,
`component.set Parent` and `entity.spawn` refuse to create it.

Fix: un-parent the entity, or join it to the other entity with a `Joint` instead.",
    },
    CodeEntry {
        code: code::HIERARCHY_STATIC_COLLIDER,
        severity: Severity::Error,
        title: "a static solver collider under a Parent",
        explanation: "A `Collider` with `body: static` that the physics solver claims (a `box`, `sphere`, `capsule` or
other 3D shape) sits on an entity that has a `Parent`. A static body has no velocity, so a parent that
moves would teleport it through whatever it touches. The entity gets no physics body until the
`Parent` is removed or the body is made kinematic. `entity.reparent`, `component.set Parent` and
`entity.spawn` refuse to create it.

2D-tier colliders — an in-plane `circle` or `aabb` that is static or kinematic, which the solver does
not claim — are allowed under a parent and resolve their world positions.

Fix: make it `body: kinematic`, or un-parent it.",
    },
    CodeEntry {
        code: code::HIERARCHY_KINEMATIC_SHEAR,
        severity: Severity::Warning,
        title: "a kinematic body under a sheared parent chain",
        explanation: "A kinematic body sits under an ancestor chain with a non-uniform scale and a rotation, so the composed
world transform has a shear. Physics bodies carry a translation and a rotation only: the shear is
dropped and the solver sees the closest rigid pose. Raised once per entity.

Fix: use a uniform scale on its ancestors, or remove the rotation between the scaled ancestor and the
body.",
    },
    CodeEntry {
        code: code::HIERARCHY_REPARENT_CYCLE,
        severity: Severity::Error,
        title: "a reparent would make an entity its own ancestor",
        explanation: "`entity.reparent` or `component.set Parent` named a new parent that is the entity itself or one
of its descendants, which would close a loop in the `Parent` chain. `entity.spawn` raises it when the
named parent's ancestry already loops. The call is refused and nothing changes.

Fix: pick a parent outside the entity's own subtree, or un-parent the descendant first.",
    },
    CodeEntry {
        code: code::HIERARCHY_REPARENT_PRESENTATION,
        severity: Severity::Error,
        title: "a sim entity cannot be reparented under a presentation entity",
        explanation: "`entity.reparent`, `component.set Parent` or `entity.spawn` named a presentation entity as the
parent of a sim entity. Presentation state is never an ancestor of the simulation, so the call is refused
and nothing changes.

Fix: parent the entity under another sim entity, or reparent it under nothing.",
    },
    CodeEntry {
        code: code::HIERARCHY_REPARENT_TOO_DEEP,
        severity: Severity::Error,
        title: "a reparent would make a Parent chain deeper than 64",
        explanation: "`entity.reparent`, `component.set Parent` or `entity.spawn` named a new parent that would give
the entity more than 64 ancestors. World transforms are composed along the whole chain on every read, so
the depth is capped. The call is refused and nothing changes.

Fix: pick a shallower parent, or flatten the chain so no entity has more than 64 ancestors.",
    },
    CodeEntry {
        code: code::PREFAB_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "prefab document structure is invalid",
        explanation: "A `kind: prefab` document is malformed at the top level: it is empty, its root is not a
mapping, `format` or `kind` is missing or wrong, it carries an unrecognized top-level key, or it
carries a scene-only key (`seed`, `physics`, `generated_from`). A prefab is `format`, `kind`, an
optional `name`, an optional `clips:` map of short name to clip file, and an `entities:` list; the
scene that spawns it owns the seed and the physics block.

Fix: write `format: 1`, `kind: prefab` and `entities:`, and remove any `seed`, `physics` or
`generated_from` key.",
    },
    CodeEntry {
        code: code::PREFAB_PART_NAME,
        severity: Severity::Error,
        title: "a prefab part has no usable name",
        explanation: "Every entity of a prefab is a named part: the name is how an instance's overrides and
references address it, and an instance's entities are named `<instance>/<part>`. This part has no
`name`, an empty one, or one containing `/`, which would make the composed name ambiguous.

Fix: give the part a non-empty `name:` without a `/`.",
    },
    CodeEntry {
        code: code::PREFAB_PART_DUPLICATE,
        severity: Severity::Error,
        title: "two prefab parts share a name",
        explanation: "Part names must be unique within a prefab, since an instance addresses a part by name. The
message names both positions.

Fix: rename one of the parts.",
    },
    CodeEntry {
        code: code::PREFAB_SINGLETON,
        severity: Severity::Error,
        title: "a prefab part carries a scene-wide singleton component",
        explanation: "A part carries `Camera`, `DirectionalLight`, `Environment`, `PostProcess` or `GiVolume`. A
prefab is spawned many times and a scene has one of each, so the prefab is refused.

Fix: put the component on an entity in the scene instead.",
    },
    CodeEntry {
        code: code::PREFAB_UNRESOLVED,
        severity: Severity::Error,
        title: "the prefab text could not be resolved",
        explanation: "A spawn named a prefab whose text could not be found: the file cannot be read from the
content root, or the world was not built by an engine and so has no prefab library.

Fix: check the path is relative to the content root and the file exists, and spawn prefabs from a
system running in an engine.",
    },
    CodeEntry {
        code: code::PREFAB_INSTANCE_STRUCTURE,
        severity: Severity::Error,
        title: "a prefab-instance entry is malformed",
        explanation: "An entity entry with a `prefab:` key is a prefab instance. It takes `name` (required,
non-empty, no `/`), `prefab` (a path relative to the content root), and optionally `transform`
(`translation`, `rotation` in Euler XYZ degrees, `scale`; each three numbers), `presentation` (a
boolean; `true` spawns every part presentation), `overrides` (part path to component changes) and
`removed` (part paths). It takes no `components:`, and two instances in one document cannot share a
name.

Fix: name the instance and use only those keys; put component changes under
`overrides: <part>: <Component>:`.",
    },
    CodeEntry {
        code: code::PREFAB_INSTANCE_CYCLE,
        severity: Severity::Error,
        title: "a prefab instances itself",
        explanation: "A prefab contains an instance of itself, directly or through nested prefabs, so expanding it
would never end.

Fix: break the loop: remove the instance entry that closes the cycle.",
    },
    CodeEntry {
        code: code::PREFAB_INSTANCE_PART,
        severity: Severity::Error,
        title: "a prefab instance's overrides or removals do not fit its prefab",
        explanation: "An instance's `overrides` or `removed` names a part path the prefab does not have, names a
part both removed and overridden, leaves a surviving part referencing a removed one, or removes every
part.

Fix: use part paths exactly as the prefab names them (nested parts as `<instance>/<part>`), and keep at
least one part.",
    },
    CodeEntry {
        code: code::PREFAB_INSTANCE_SAVE,
        severity: Severity::Error,
        title: "a live prefab instance cannot be saved as an instance entry",
        explanation: "`scene.save` writes a live prefab instance as an instance entry (`prefab`, `transform`,
`overrides`, `removed`) that reloads to the same world, and refused one it cannot express: an entity
refers to a part of an instance other than its first surviving part; a part lost a component its
prefab gives it, or carries live components the entry cannot reproduce; a part is not in the prefab
as it was expanded (or on disk, after an `engine.restore`), an override names a part the current
prefab file no longer has, or two parts share a path; the
parts of one instance come from different prefabs, are only partly named `<instance>/<part>`, or share
an instance name with another instance; or the saved text would not load (a removed part is still
referenced).

Fix: author the reference inside the prefab, or unpack the instance so its parts become plain entities.",
    },
    CodeEntry {
        code: code::PREFAB_NOT_INSTANCE,
        severity: Severity::Error,
        title: "the entity is not part of a prefab instance",
        explanation: "Applying, reverting or unpacking acts on a live prefab instance, and the entity given is dead
or carries no `PrefabInstance` (it was spawned as a plain entity, or the instance was unpacked).

Fix: pass a part of a spawned or scene-loaded instance.",
    },
    CodeEntry {
        code: code::PREFAB_CREATE,
        severity: Severity::Error,
        title: "a prefab cannot be created from these entities",
        explanation: "Creating a prefab from live entities was refused: the path is not a relative `.prefab.yaml`
path, `entities` is empty or lists an id twice, an id is dead, a part name is missing, duplicated or
contains `/`, a part refers to an entity outside the set, the path names a generated prefab (one
with `generated_from`, which is write-only), an asset path cannot be written relative to the prefab
(it is absolute), or the file could not be written.

Fix: pass a relative `*.prefab.yaml` path, give every entity a unique `Name` without `/`, add the
referenced entities to `entities` or remove the reference, and give asset paths relative to the
content root.",
    },
    CodeEntry {
        code: code::PREFAB_APPLY,
        severity: Severity::Error,
        title: "a prefab instance's changes cannot be applied to its prefab",
        explanation: "Applying an instance's overrides to its prefab file was refused: a listed part path is not a
changed part of the instance, an override carries an entity reference (the prefab's own positions
cannot be derived from live ids), a part refers to an entity outside the instance, a removed part is
still referred to by a surviving one, the prefab is generated (it has `generated_from` and is
write-only: edit the source and re-import), an overridden asset path cannot be written relative to the
prefab (it is absolute), or the prefab file could not be written.

Fix: pass a path from the instance's changed parts, re-author the reference inside the prefab, give
asset paths relative to the content root, or unpack the instance.",
    },
    CodeEntry {
        code: code::PREFAB_REVERT,
        severity: Severity::Error,
        title: "a prefab instance cannot be reverted as asked",
        explanation: "Reverting to the prefab's values was refused: the pointer is malformed, names no part,
component or field of the prefab, or names a part removed from the instance (removed parts are not
re-spawned).

Fix: use `/<part path>[/<Component>[/<field>...]]` with a part the instance still has.",
    },
    CodeEntry {
        code: code::COLLIDER_NO_PHYSICS_SCENE,
        severity: Severity::Error,
        title: "a solver collider in a world with no physics block",
        explanation: "A `Collider` with a 3D shape (`box`, `sphere`, `capsule`, ...) or a `rigid` body exists, but the scene
has no `physics:` block, so the physics solver is not installed and nothing simulates the collider: a
rigid body never falls, and a 3D static never blocks. `ludion check-scene` reports it at the collider;
a collider spawned by game code is reported once at runtime.

Fix: add `physics: { space: free }` (or `space: planar`) to the scene, or use a 2D `circle`/`aabb`
collider that is static or kinematic.",
    },
    CodeEntry {
        code: code::COLLIDER_NO_SPATIAL_INDEX,
        severity: Severity::Error,
        title: "2D colliders but no spatial index",
        explanation: "The world holds a 2D `Collider` (an in-plane `circle` or `aabb` that is static or kinematic), but the
schedule never runs `spatial::rebuild`, so no `SpatialIndex` exists: `overlaps`, `raycast` and
`move_and_slide` see nothing and every mover passes through every wall. Raised once.

Fix: register the rebuild before the systems that query, e.g.
`.system(\"spatial_rebuild\", spatial::rebuild::<Transform, TransformHierarchy>)` on the `EngineBuilder`.",
    },
    CodeEntry {
        code: code::COLOR_COMPONENT_INVALID,
        severity: Severity::Error,
        title: "colour has a negative component or an alpha outside 0..1",
        explanation: "\
A colour field of a light or a drawn component has a negative or non-finite RGB element, or an alpha
outside `[0, 1]`.

RGB is HDR: a component may exceed 1.0 (that is how emissive things bloom), but it must be finite
and non-negative, because the scene target is linear radiance and a negative colour subtracts light
from what is already there. Alpha is a coverage weight in `[0, 1]`: premultiplied `over` turns an
alpha above 1 into a negative weight on the destination, so brighten with the RGB components
instead. A particle's alpha is the one exception: the particle shader clamps it to 1, so it must
be finite and `>= 0`, and a value above 1 holds full opacity longer through the fade.

Covered: `PointLight.color`, `SpotLight.color`, `DirectionalLight.color`, `Sprite.color` and
`flash_color`, `Mesh.color` and `flash_color`, `Text.color`, `GroundMarker.color`, a `Tilemap` or
`MeshGrid` legend entry's `color`, `ParticleEmitter.start_color` and `end_color`, and a pattern
document's bullet-kind `color`. `Environment` and `PostProcess` colours keep `L1128` and `L1130`; a
negative scalar intensity is the schema's `L1014`.

The JSON Pointer names the offending element: `/color/0`, `/end_color/3`, `/legend/#/color/1`. Fix:
give every RGB component 0 or more (HDR values above 1.0 are fine) and keep alpha within 0..=1
(a particle's: 0 or more).",
    },
    CodeEntry {
        code: code::CAMERA_INVALID_DEPTH_RANGE,
        severity: Severity::Error,
        title: "camera near plane is not less than its far plane",
        explanation: "\
A `Camera`'s `near` clip distance is not less than its `far`, so the projection has no depth: a
perspective projection comes out non-finite and nothing draws. An absent bound compares as its
default (`near` 0.1, `far` 1000), so `near: 2000` alone is also refused.

`near` must also be above zero; that is the schema's `L1014` at `/near`. The JSON Pointer is `/far`
when `far` is authored, else `/near`.

Fix: make `far` greater than `near`, e.g. `near: 0.1`, `far: 1000.0`.",
    },
    CodeEntry {
        code: code::PATTERN_AUTHORING_LINT,
        severity: Severity::Warning,
        title: "pattern document authors something that does not do what it reads as",
        explanation: "\
A `kind: pattern` document authors a value that validates but misleads. Two cases:

- A full-turn `arc` (`spread_deg` of 360 or more in magnitude, `count` above 1): the arc overlaps
  itself, and at exactly 360 its first and last bullet share a heading and fly stacked. Use
  `shape: ring` for an even circle.
- A `loop` on a pattern that another pattern `play`s: a played pattern runs its timeline once, so
  its own `loop` is ignored.

The JSON Pointer names the field: `/timeline/<i>/spread_deg` or `/timeline/<i>/play`.

Fix: switch to `shape: ring`, or lower the spread below 360; drop the played pattern's `loop`, or
repeat the `play` entry in the playing pattern.",
    },
    CodeEntry {
        code: code::GENERATION_IDENTICAL_CANDIDATES,
        severity: Severity::Warning,
        title: "a run would submit identical graphs",
        explanation: "\
A generation run with `count` above 1 gives each job its own seed by writing it into the
workflow's seed input (`seed` or `noise_seed`). Here no seed input can take it, so every job
submits the same graph. Two causes:

- The recipe pins every literal seed input (`inputs: <node>: seed: <n>`), so each job runs at that
  one value.
- The seed is fed by a link or a primitive node, not a literal input on the sampler.

The extra candidates are byte-identical, served from ComfyUI's cache, and the run records the
pinned seed (or `null` when it is unknowable) for each.

Fix: drop the pin from the recipe, expose a literal `seed` or `noise_seed` input on the sampler
node, or run with `--count 1`.",
    },
    CodeEntry {
        code: code::SCENE_SAVE_GENERATED,
        severity: Severity::Error,
        title: "generated scene cannot be saved",
        explanation: "\
A scene bearing `generated_from:` is write-only output of `ludion generate-map`. `scene.save`
refuses it, so no client overwrites it or writes a copy the drift gate would flag. The editor
disables Save for the same reason. The same refusal covers a destination file that bears
`generated_from:`, whatever the world holds.

Fix: edit the map source, kit or fragment named by `generated_from` and regenerate with
`ludion generate-map <generator>`.",
    },
    CodeEntry {
        code: code::VOXEL_VIEW_OUT_OF_RANGE,
        severity: Severity::Error,
        title: "voxel view is out of range",
        explanation: "\
A voxel view request (a slice `y`, or a column `x`/`z`) names a layer or column the model does not
have. Valid coordinates run from 0 up to one less than the model's size on that axis; the message
names the model's size and the valid range. The request is a command-line argument, so the
diagnostic carries no document path.

Fix: pick a coordinate inside the range the message names, or read the model's `size:` first.",
    },
    CodeEntry {
        code: code::PATH_FOLLOWER_INVALID,
        severity: Severity::Error,
        title: "a path follower's speed or phase is not a finite number",
        explanation: "A `PathFollower`'s `speed` or `phase` is not a finite `f32`: it is NaN or infinite, or a number
outside `f32` range (such as `1e39`), which the component would store as infinity and spread through
the entity's `Transform`.

From a scene or a wire write it is a validation error at `/speed` or `/phase`. A follower built in
Rust never passes validation: the engine's `path_follow` holds such an entity where it is, leaving
its `phase` and `Transform` untouched, and reports once per entity.

Fix: give `speed` a finite number of metres per second and `phase` a finite number of metres along
the path.",
    },
    CodeEntry {
        code: code::COOK_ENCODE_FAILED,
        severity: Severity::Error,
        title: "a texture map could not be cooked",
        explanation: "\
`ludion sync`'s cook stage compresses every PNG map an imported `*.mesh.yaml` (or one under a
`[cook] roots` directory) references into a block-compressed KTX2 file (BC7 for desktop, ASTC 4x4 for iOS) under `.ludion/cooked/<target>/`, and
the encoder refused one: the image is empty or malformed, or the encoder rejected a mip level.

The import tree is already written and the map keeps loading from its source PNG, so the project still
runs; only the faster cooked copy is missing. A PNG that does not decode keeps the decoder's own
diagnostic, and a store that cannot be written is `L1203`.

Fix: re-export the PNG from its source (a truncated or zero-sized file is the usual cause), then run
`ludion sync` again. If the PNG is sound, report the encoder error the diagnostic quotes.",
    },
    CodeEntry {
        code: code::COOK_ROOT_INVALID,
        severity: Severity::Error,
        title: "a [cook] root cannot be cooked from",
        explanation: "\
A `[cook] roots` entry in `ludion.toml` names a directory whose `*.mesh.yaml` files `ludion sync`
cooks. The root is absent, is not a directory, passes through a symbolic link or junction below the
project, or holds a link somewhere in its tree.

An absent root is never read as an empty one: the sync would sweep the cooked maps it should keep.
A bad root aborts the sync before anything is written; a link found while walking leaves the store
unswept.

Fix: run the import that writes the root, or drop it from `[cook] roots`; replace a link with the real
directory.",
    },
    CodeEntry {
        code: code::COOK_MAP_LEAVES_ROOT,
        severity: Severity::Warning,
        title: "a mesh under a [cook] root references a map outside it",
        explanation: "\
A `*.mesh.yaml` under a `[cook] roots` directory names a texture map by an absolute path, by `../`
out of the root, or through a dot segment below the root. `ludion sync` does not cook that map, and
the runtime keeps loading it from its PNG. The run still succeeds; one warning lists the first meshes
and references. A reference from one root into another counts as leaving.

Fix: move the map inside the root (or the mesh next to its maps), or add the map's directory as a
root of its own.",
    },
    CodeEntry {
        code: code::TEXTURE_NOT_COOKED,
        severity: Severity::Warning,
        title: "mesh maps loaded from their PNG sources under `textures: cooked`",
        explanation: "\
Under `textures: cooked` the engine loads each imported mesh map from its block-compressed KTX2 file
in `.ludion/cooked/<target>/`, and some maps could not be: the file is missing (the map was never
synced, or its PNG changed since), the file is corrupt or does not match its slot, or this GPU offers
neither BC7 nor ASTC. Those maps load from their PNG sources instead, so the scene renders the same
but loads slower and uses more video memory. One warning names every such map, up to eight.

Fix: run `ludion sync` to cook the missing maps. For a corrupt file, delete it from
`.ludion/cooked/<target>/` first, because sync reuses any file present under a key. On a GPU without
BC7 or ASTC there is nothing to fix; the PNG fallback is the intended path.",
    },
    CodeEntry {
        code: code::SPOT_LIGHT_INVALID_CONE,
        severity: Severity::Error,
        title: "spot light cone is invalid",
        explanation: "\
A `SpotLight` cannot form a cone of light. Three causes:

- `direction` is zero-length or non-finite, so there is no axis to normalize (pointer `/direction`).
- An angle is non-finite or outside `[0, 90)` (pointer `/inner_angle` or `/outer_angle`). Both are
  half-angles in degrees from the axis; 90 would be a hemisphere, which a point light already is.
- `inner_angle` is greater than `outer_angle` (pointer `/inner_angle`): the full-brightness core
  cannot be wider than the edge of the cone. Equal angles are legal and give a hard edge.

Fix: give `direction` a non-zero, finite `[x, y, z]` (e.g. `[0, -1, 0]`), keep both angles in
`[0, 90)`, and make `inner_angle` at most `outer_angle` (defaults 20 and 30).",
    },
    CodeEntry {
        code: code::POST_PROCESS_INVALID_SSR,
        severity: Severity::Error,
        title: "post-process screen-space reflection field is out of range",
        explanation: "\
A `PostProcess` screen-space reflection field breaks a rule the schema's `range` cannot spell. Three
causes:

- An `ssr_fade` end is outside `[0, 1]` (pointer `/ssr_fade/0` or `/ssr_fade/1`). The band is a
  roughness range, and roughness is between 0 and 1.
- `ssr_fade[0]` is not below `ssr_fade[1]` (pointer `/ssr_fade`): the confidence divides by the
  band's width, so an empty or reversed band has no meaning.
- `ssr_max_distance` or `ssr_thickness` is exactly 0 (pointer `/ssr_max_distance` or
  `/ssr_thickness`): a ray that travels no distance hits nothing, and a surface with no thickness
  cannot be hit. A negative value is the schema's `L1014`.

Fix: give `ssr_fade` two roughness values with start below end (the default is `[0.45, 0.65]`), and
give `ssr_max_distance` and `ssr_thickness` values above 0 (defaults 30 and 0.5).",
    },
    CodeEntry {
        code: code::REFLECTION_PROBE_INVALID,
        severity: Severity::Error,
        title: "reflection probe is invalid",
        explanation: "\
A `ReflectionProbe` has a value that cannot describe a volume. Two causes:

- A box `extent` component or a sphere `radius` is non-finite or not above 0 (pointer
  `/shape/box/extent/<i>` or `/shape/sphere/radius`). `extent` holds half-extents, so a zero or
  negative one is an empty volume.
- `blend` is non-finite or negative (pointer `/blend`). It is a distance outside the volume.

Fix: give every `extent` component and the `radius` a finite value above 0 (defaults 2), and
`blend` a finite value of 0 or more (default 0.5).",
    },
    CodeEntry {
        code: code::REFLECTION_PROBE_EQUAL_RANK,
        severity: Severity::Warning,
        title: "overlapping reflection probes have equal rank",
        explanation: "\
Two reflection probes overlap, share a `priority` and have the same volume, so nothing but their
entity ids decides which one wins where they meet. Each probe's `blend` band, the fade outside its
volume, counts as part of it, so volumes that only touch overlap when either has a `blend`. Reordering the scene file would not change that,
but adding or removing an entity can. The path is the lower-ranked probe; the related diagnostic is
the other.

Fix: give one of the two a higher `priority`, or resize or move one so they no longer overlap.",
    },
    CodeEntry {
        code: code::REFLECTION_PROBE_SHADOWED,
        severity: Severity::Warning,
        title: "reflection probe is shadowed",
        explanation: "\
A reflection probe lies entirely inside a higher-ranked probe's volume (inside it that probe's
weight is 1), and its own `blend` band lies inside it too, so it never contributes a pixel. Probes rank by
`priority`, then boxes before spheres, then the smaller volume. The path is the shadowed probe; the
related diagnostic is the one covering it. Only a single covering probe is detected; a union of
several is not.

Fix: raise the shadowed probe's `priority`, shrink or move the other probe, or delete the shadowed
one.",
    },
    CodeEntry {
        code: code::REFLECTION_PROBE_DUPLICATE,
        severity: Severity::Error,
        title: "reflection probes are duplicates",
        explanation: "\
Two reflection probes have the same shape, size and pose, which is almost always a copy-pasted
probe that was never moved. One of them can never contribute. The path is the later probe; the
related diagnostic is the other. `priority` and `blend` are ignored by this check.

Fix: delete one of the two, or move it to where it was meant to go.",
    },
    CodeEntry {
        code: code::REFLECTION_PROBE_OVER_CAP,
        severity: Severity::Error,
        title: "too many reflection probes",
        explanation: "\
The scene has more than 32 reflection probes, the most the renderer binds at once. The path is the
first probe past the cap; each further probe is a related diagnostic.

Fix: merge neighbouring probes into larger ones. Streaming probes in and out of the budget is not
supported.",
    },
    CodeEntry {
        code: code::REFLECTION_PROBE_NOT_STORED,
        severity: Severity::Warning,
        title: "reflection probes captured at load, not read from the store",
        explanation: "\
This host reads reflection probes from the content-addressed probe store
(`.ludion/cooked/probes/`), and the store held no entry for some of them: they were never baked, or
the scene, its lighting, its textures or the quality setting changed since. The engine captured them
instead, which renders the same but costs a capture at load. The related diagnostics name the probes.

Bakes use cooked textures, so a host run with `--textures source` misses the entries a bake wrote.

Fix: bake the store (`ludion bake-probes <scene>`, or the game's `--bake-probes`). To iterate on a
scene's look without the store, run the host with `--probes rebake`.",
    },
    CodeEntry {
        code: code::REFLECTION_PROBE_UNKNOWN,
        severity: Severity::Error,
        title: "no reflection probe has that name",
        explanation: "\
A probe bake was asked to force one probe by `Name`, and no `ReflectionProbe` entity in the scene
carries it. The help lists the names the scene has; an unnamed probe cannot be forced by name.

Fix: use one of the listed names, or give the probe a `Name`.",
    },
    CodeEntry {
        code: code::REFLECTION_PROBE_NOT_BUNDLED,
        severity: Severity::Error,
        title: "a baked reflection probe is missing from the export",
        explanation: "\
`ludion export` baked the reflection probes of a shipped scene into the bundle's probe store
(`content/.ludion/cooked/probes/`), but an entry the bake reported is not there. The bake drops a
failed write silently, so this check is what catches it. The related diagnostics name each missing
probe and its scene.

Fix: rerun `ludion export`; the bake could not write the entry (disk full or permissions?).",
    },
    CodeEntry {
        code: code::DIRECTIONAL_LIGHT_INVALID_ANGULAR_RADIUS,
        severity: Severity::Error,
        title: "directional light angular radius is out of range",
        explanation: "\
A `DirectionalLight`'s `angular_radius` (pointer `/angular_radius`) is non-finite or outside
`[0, 45]`. It is the sun disc's angular radius in degrees; above 0 the sun's specular highlight
becomes a soft disc instead of a point. 0 (the default) is a point light, and 45 is far wider than
any real light: the moon and the sun are both about 0.27.

Fix: use a finite angular radius in `[0, 45]` degrees, e.g. `0.27`.",
    },
    CodeEntry {
        code: code::TEXTURE_STRUCTURE,
        severity: Severity::Error,
        title: "texture recipe structure is invalid",
        explanation: "\
A `kind: texture` recipe — named steps forming a graph that evaluates to a tileable PBR map set —
does not have the shape this engine reads.

  format: 1
  kind: texture
  seed: 7
  params:
    scale: 6
  steps:
    cells:
      op: noise
      type: cellular
      scale: $scale
  outputs:
    base_color: cells
    roughness: cells
    height: cells

This code covers: a missing `steps` or `outputs`; a top-level key a recipe does not have; a step
that is not a mapping or has no `op`; an unknown `op`; a field the op does not have; a missing
required field; a wrong-typed field; an unknown `outputs` key; an `outputs` entry missing
(`base_color`, `roughness` and `height` are required); a sub-recipe `params` name the op does not
bind.

The JSON Pointer names the offending key (`/steps/cells/scale`); an unknown name carries a
nearest-match suggestion.

Fix: correct the named key. Values that parse but are out of range are `L1284`; step references and
cycles are `L1285`.",
    },
    CodeEntry {
        code: code::TEXTURE_DATA,
        severity: Severity::Error,
        title: "texture recipe data is inconsistent",
        explanation: "\
A `kind: texture` recipe parsed structurally, but a value cannot be evaluated.

This code covers: a number out of its range (a noise `scale` below 1, an `opacity` outside
`[0, 1]`, a `gamma` of 0, a `scale` whose finest octave exceeds 65536 cells); a non-finite number; a
colour that is not `\"#rrggbb\"`; `stops` or `points` unsorted or fewer than two; a gray image where
a color one is needed or the reverse; an `outputs` entry whose image has the wrong channel count
(`roughness`, `metallic`, `occlusion` and `height` are gray, `normal` is color); a `$param` that
is undefined or whose type does not match the slot it fills; a `remap` with `from_low == from_high`;
a build size outside 8..=8192.

The JSON Pointer names the offending value (`/steps/tint/stops/1/at`, `/outputs/roughness`).

Fix: correct the named value, e.g. `scale: 6` instead of `scale: 0`.",
    },
    CodeEntry {
        code: code::TEXTURE_GRAPH,
        severity: Severity::Error,
        title: "texture recipe step graph cannot be resolved",
        explanation: "\
A `kind: texture` recipe's steps do not form a graph that can be evaluated.

This code covers: an `input`, `a`, `b`, `mask` or `by` slot (or an `outputs` entry) naming a step
that does not exist, with a nearest match; a cycle among steps — the message names it, `a → b → a`;
a `recipe` op whose `path` cannot be read, or whose file chain refers back to itself; a `recipe`
op whose `output` is not a step of the sub-recipe; a `std:` reference when the embedded stdlib is
not available.

A diagnostic raised inside a sub-recipe starts its message with the sub-recipe's id.

Fix: correct the named reference, or break the cycle by removing one of its edges.",
    },
    CodeEntry {
        code: code::TEXTURE_DRIFT,
        severity: Severity::Error,
        title: "committed texture maps differ from their recipe",
        explanation: "\
A `kind: texture` recipe's map set is generated, write-only output. `ludion texture check` rebuilds
the recipe at the size recorded in the set's `generated.json` and byte-compares every map. This code
reports a map whose bytes differ, a map that is missing, a stray `.png` the recipe does not produce,
or a set with no readable `generated.json` (never built).

Each message names the file. An op-library or recipe change shows up here as a reviewed diff, never
as silent re-texturing.

Fix: run `ludion texture build <recipe>` (with the same `--size`) and commit the result. Never edit
the maps by hand.",
    },
    CodeEntry {
        code: code::TEXTURE_CHECK_WARNING,
        severity: Severity::Warning,
        title: "texture validator outside its bounds",
        explanation: "\
A built map set measures outside the bounds of a validator whose level is `warn`. The message names
the validator, the measured value and the bounds. The engine's defaults cover albedo range, metallic
ambiguity, mirror-smooth roughness, normal validity, height range and seam continuity; a rules file
(`--rules`) overrides any bound or level.

Fix: adjust the recipe so the value lands inside the bounds, or relax the validator in the rules
file when the departure is intended.",
    },
    CodeEntry {
        code: code::TEXTURE_CHECK_FAILED,
        severity: Severity::Error,
        title: "texture validator failed",
        explanation: "\
A built map set measures outside the bounds of a validator whose level is `error`. The maps are
still written so they can be inspected, but the command exits non-zero. The message names the
validator, the measured value and the bounds.

Fix: adjust the recipe so the value lands inside the bounds, or lower the validator to `warn` or
widen its bounds in the rules file when the departure is intended.",
    },
    CodeEntry {
        code: code::TEXTURE_RULES_INVALID,
        severity: Severity::Error,
        title: "texture rules file cannot be used",
        explanation: "\
The rules file given to `ludion texture build` or `check --rules` could not be read, is not valid
TOML, holds a wrong-typed value, or names a validator that does not exist. The shape is
`<validator> = { min = .., max = .., level = \"warn\" | \"error\" }` for the project base and
`[textures.\"<recipe file name>\"]` tables for per-recipe overrides.

Fix: correct the named key; an unknown validator name comes with the nearest valid one.",
    },
    CodeEntry {
        code: code::COLLIDER_SCALE_IGNORED,
        severity: Severity::Warning,
        title: "a collider's entity is scaled",
        explanation: "\
A collider the physics solver owns sits on an entity whose world scale (its own `Transform.scale`
times its ancestors') is not `[1, 1, 1]`. Collision ignores `Transform.scale`, so the shape the solver
sees differs from the shape that renders. Raised once per entity.

Fix: bake the scale into the mesh or the collider's shape, and set the scale to `[1, 1, 1]`.",
    },
    CodeEntry {
        code: code::COLLIDER_SENSOR_ON_RIGID,
        severity: Severity::Error,
        title: "a sensor collider on a rigid body",
        explanation: "A `Collider` with `sensor: true` sits on a `body: rigid`.

A sensor detects overlaps and never collides or pushes, so it has no rigid-body behaviour to
simulate. It belongs on a `static` body, or on a `kinematic` one that something moves.

Raised at document time by `check-scene`, and at runtime by `ludion-physics` for a `Collider` that
game code inserts directly. The body is not created.

Fix: set `body: static` (or `kinematic`, and move it), or drop `sensor`.",
    },
    CodeEntry {
        code: code::COLLIDER_SENSOR_NO_INTERIOR,
        severity: Severity::Error,
        title: "a sensor collider with no interior",
        explanation: "A `Collider` with `sensor: true` has a `mesh` or `heightfield` shape.

A triangle mesh and a heightfield are surfaces without an interior, so a sensor on one would fire
only at the moment something crosses the surface, never while something is inside it.

Raised at document time by `check-scene`, and at runtime by `ludion-physics` for a `Collider` that
game code inserts directly. The body is not created.

Fix: use a volume shape (`box`, `sphere`, `capsule`, `cylinder`, `convex` or `compound`), or drop
`sensor`.",
    },
    CodeEntry {
        code: code::COLLIDER_SENSOR_UNCLAIMED,
        severity: Severity::Error,
        title: "a sensor on a collider the solver does not claim",
        explanation: "A `Collider` with `sensor: true` has a 2D `circle` or `aabb` shape on a body the
physics solver does not own. Sensors exist only in the physics tier, so the flag would do nothing.

Raised at document time by `check-scene`, at `/entities/<i>/components/Collider/sensor`.

Fix: give the collider a 3D shape (`sphere`, `box`, ...), or drop `sensor`.",
    },
    CodeEntry {
        code: code::CHARACTER_MOTOR_INVALID,
        severity: Severity::Error,
        title: "a character motor field is out of range",
        explanation: "A `CharacterMotor` field is outside what the capsule and the sweeps can use: `radius`, `height` and
`step_height` must be finite and above 0; `snap_distance` finite and at least 0; `skin_width` finite and above 0;
`slope_limit_deg` strictly between 0 and 90; `step_height` below `height`; `height` at least
`2 * radius` plus a millimetre; `max_slides` from 1 to 16.

From a scene or a wire write it is a validation error at the field's JSON Pointer
(`/entities/<i>/components/CharacterMotor/<field>`). A motor built in Rust gets no body and is
reported once per entity.

Fix: use the pointer the message names; the defaults (radius 0.35, height 1.6) are a safe start.",
    },
    CodeEntry {
        code: code::CHARACTER_MOTOR_WITH_COLLIDER,
        severity: Severity::Error,
        title: "a character motor and a collider on one entity",
        explanation: "An entity carries a `CharacterMotor` and a `Collider`. The motor owns one kinematic capsule body sized by
its `radius` and `height`; a second collider on the same entity would be a second source of truth for
the body, so the motor wins and the `Collider` is ignored.

Fix: remove the `Collider`; the motor owns its capsule.",
    },
    CodeEntry {
        code: code::CHARACTER_MOTOR_NO_PHYSICS_SCENE,
        severity: Severity::Error,
        title: "a character motor in a world with no physics block",
        explanation: "A `CharacterMotor` exists, but the scene has no `physics:` block, so the physics solver is not installed
and the motor's capsule body is never built. `ludion check-scene` reports it at the motor; a motor
spawned by game code is reported once at runtime.

Fix: add `physics: { space: free }` to the scene.",
    },
    CodeEntry {
        code: code::CHARACTER_MOTOR_PARENTED,
        severity: Severity::Error,
        title: "a character motor under a parent",
        explanation: "A `CharacterMotor` entity has a `Parent`. The motor writes world-space feet positions straight into its
`Transform`, which a parent would reinterpret as local, so the character would jump.

Fix: remove the `Parent`, or move the motor to a root entity and follow the mover from game code.",
    },
    CodeEntry {
        code: code::CHARACTER_MOTOR_POSTURE_UNKNOWN,
        severity: Severity::Error,
        title: "a character motor state names an undeclared posture",
        explanation: "A `CharacterMotorState` has a `posture` that the entity's `CharacterMotor` does not declare under
`postures`, so the motor would run as its base shape.

Fix: declare the posture under `CharacterMotor.postures`, or correct the name (the help names the nearest match).",
    },
    CodeEntry {
        code: code::CHARACTER_MOTOR_POSTURE_FALLBACK,
        severity: Severity::Warning,
        title: "a character motor fell back to its base shape",
        explanation: "At run time a motor's `CharacterMotorState.posture` names a posture its `CharacterMotor` does not declare
(the component was edited live, or the posture was removed). The motor runs as its base shape, reported once per entity.

Fix: declare the posture under `CharacterMotor.postures`, or call `set_character_posture` with a declared name or `None`.",
    },
    CodeEntry {
        code: code::SURFACE_VELOCITY_INVALID,
        severity: Severity::Error,
        title: "a surface velocity is not a finite vector",
        explanation: "A `SurfaceVelocity` has a `velocity` component that is NaN, infinite, or a number outside `f32`
range (such as `1e39`). A character motor standing on the collider would add it to its carry every
tick; at runtime the motor skips a non-finite surface velocity.

From a scene or a wire write it is a validation error at `/velocity/0`, `/velocity/1` or `/velocity/2`.

Fix: give each component a finite number of metres per second, in the collider's local frame.",
    },
    CodeEntry {
        code: code::SURFACE_VELOCITY_NO_SURFACE,
        severity: Severity::Error,
        title: "a surface velocity on an entity with no carrying collider",
        explanation: "A `SurfaceVelocity` entity has no `Collider`, or its `Collider` is `body: rigid`. A character
motor only reads the surface velocity of ground it can be carried by: a `static` or `kinematic`
collider. A rigid support is never carried, so the velocity would do nothing.

Fix: put the `SurfaceVelocity` on the belt's `static` or `kinematic` collider.",
    },
    CodeEntry {
        code: code::LANE_CONSTRAINT_INVALID,
        severity: Severity::Error,
        title: "a lane constraint field is out of range",
        explanation: "A `LaneConstraint` field is out of range: `engage_time`, `disengage_time`, `pull_speed` or
`search_window` is negative or not finite, `offset` or `s` is not finite, or `blend` is not finite or
lies outside [0, 1]. The pointer names the field. A lane built in Rust with such a value is reported
once per entity at runtime and left inert, so the character moves freely.

Fix: give the times, `pull_speed` and `search_window` finite non-negative numbers, `offset` a finite
number of metres, and `blend` a number from 0 to 1 (leave `s` null to let the lane find its own place).",
    },
    CodeEntry {
        code: code::LANE_PATH_UNLOADED,
        severity: Severity::Error,
        title: "a lane constraint path failed to load",
        explanation: "The `kind: path` document named by `LaneConstraint.path` could not be loaded, so the lane has no
centreline. It is reported once per entity and the lane stays inert: the character moves freely, as
if `engaged` were false. The cause (missing file, wrong kind, malformed document) follows in the
diagnostic.

Fix: point `path` at an existing `kind: path` document, relative to the content root, and check it
with `ludion check-scene`.",
    },
    CodeEntry {
        code: code::LANE_WITHOUT_MOTOR,
        severity: Severity::Error,
        title: "a lane constraint on an entity with no character motor",
        explanation: "An entity carries a `LaneConstraint` but no `CharacterMotor`. A lane shapes the displacement that
`move_character` applies, so without a motor nothing ever reads it. `ludion check-scene` reports it
at `/entities/<i>/components/LaneConstraint`; a lane on an entity spawned by game code is reported
once at runtime.

Fix: add a `CharacterMotor` to the entity, or remove the `LaneConstraint`.",
    },
    CodeEntry {
        code: code::LANE_PATH_VERTICAL,
        severity: Severity::Warning,
        title: "a lane path has a near-vertical span",
        explanation: "A lane's path has a span whose horizontal tangent is almost zero (the span is steeper than about
87 degrees from the horizontal plane), so \"forward\" and \"sideways\" are barely defined there from the
tangent alone: the frame swings sharply across such a span. The message names the arc-length range. The
lane keeps working; only where the tangent is exactly vertical is the sideways direction carried over
from the path's previous stretch.

Fix: tilt the span so it keeps some horizontal travel, or shorten it; a lane is meant for paths that
run along the ground, not for ladders or shafts.",
    },
    CodeEntry {
        code: code::CAMERA_RIG_INVALID,
        severity: Severity::Error,
        title: "a camera rig field is out of range",
        explanation: "A `CameraRig` field is out of range. A half-life, `look_ahead.distance`, `up_slack`, `down_slack`,
`blend_in.seconds`, `decay_per_second`, `max_offset` or `max_angle_deg` is negative or not finite;
`search_window`, `full_speed`, `frequency` or `exponent` is not a positive finite number; `offset` or
`focus_offset` or the first-person `eye_offset` is not finite; `pitch_limit_deg` is outside (0, 90);
`look_deg`, `look_deg_per_count`, `kick_deg`, `fov_kick_deg`, `roll_deg` or `fov_offset_deg` is not finite; a `kick`
half-life is negative or not finite; `fov_y_deg` is outside (0, 180); `trauma` is outside [0, 1];
`collision.radius` or `framing.distance` is not positive; `collision.min_distance`, `skin`,
`recover_delay` or `recover_half_life`, a chase `recenter` field or `framing.half_life` is negative or
not finite; `framing.weight` or a framing offset is not finite. The pointer
names the field. A rig built in Rust with such a value is reported once per entity at runtime and
holds, so the camera stays where it was.

Fix: give the field a value in the range above (units are in the field's name: seconds, metres,
degrees).",
    },
    CodeEntry {
        code: code::CAMERA_RIG_TARGET_MISSING,
        severity: Severity::Warning,
        title: "a camera rig target no longer exists",
        explanation: "The entity named by `CameraRig.target` has been despawned, or never existed. The rig reports
this once and holds its last pose until the target is set again. A `target` of null is not an
error: a game may assign it later.

Fix: point `target` at a live entity, or set it to null while there is nothing to follow.",
    },
    CodeEntry {
        code: code::CAMERA_RIG_PATH_UNLOADED,
        severity: Severity::Error,
        title: "a camera rig track path failed to load",
        explanation: "The `kind: path` document named by the rig's track `path` could not be loaded, so the rig has no
track. It is reported once per rig and the camera holds its last pose. The cause (missing file, wrong
kind, malformed document) follows in the diagnostic. An empty `path` is not an error: the rig is inert.

Fix: point `path` at an existing `kind: path` document, relative to the content root, and check it
with `ludion check-scene`.",
    },
    CodeEntry {
        code: code::CAMERA_RIG_WITHOUT_CAMERA,
        severity: Severity::Error,
        title: "a camera rig on an entity with no camera",
        explanation: "An entity carries a `CameraRig` but no `Camera`, so there is nothing to pose. `ludion check-scene`
reports it at `/entities/<i>/components/CameraRig`; a rig on an entity spawned by game code is reported
once at runtime and holds.

Fix: put the rig on the camera entity.",
    },
    CodeEntry {
        code: code::CAMERA_RIG_DUPLICATE,
        severity: Severity::Error,
        title: "more than one camera rig",
        explanation: "More than one entity carries a `CameraRig`. A scene has one camera, so one rig. `ludion
check-scene` names both entities; at runtime only the rig on the lowest entity id runs and the rest
are ignored.

Fix: keep one rig, on the camera entity.",
    },
    CodeEntry {
        code: code::CAMERA_RIG_PARENTED,
        severity: Severity::Error,
        title: "a camera rig on an entity with a parent",
        explanation: "A `CameraRig` entity has a `Parent`. The rig computes a world-space pose and writes it as the
camera's `Transform`, which would be read as local to the parent. At runtime the rig is reported once
and holds.

Fix: make the camera a root entity.",
    },
    CodeEntry {
        code: code::CAMERA_RIG_COLLISION_QUERY,
        severity: Severity::Error,
        title: "a camera rig collision cast was rejected",
        explanation: "The sphere cast of a `CameraRig`'s collision stage was refused by the physics world, most often
because a name in `collision.include_layers` or `collision.ignore_layers` is not a declared
`physics.layers` entry. The diagnostic carries the physics error and points at `/rig/collision`. It is
reported once per rig and the collision stage is skipped, so the eye is not pulled in.

Fix: declare the layer in the project's `physics.layers`, or correct the name.",
    },
    CodeEntry {
        code: code::CAMERA_RIG_COLLISION_NO_PHYSICS,
        severity: Severity::Warning,
        title: "a camera rig collides in a world with no physics",
        explanation: "A `CameraRig` has `collision.enabled` set, but the world has no `PhysicsWorld`, so there is nothing
to cast against. The rig reports this once and runs without the collision stage.

Fix: add physics to the game, or set `collision.enabled` to false.",
    },
    CodeEntry {
        code: code::CAMERA_RIG_COLLISION_STUCK,
        severity: Severity::Warning,
        title: "a camera rig pivot and target are inside geometry",
        explanation: "The collision stage found both the boom's origin (the pivot) and the target overlapping a collider,
so no cast can tell which way is out. The boom is held at `collision.min_distance`. It is reported once
per rig. The usual cause is a camera pivot or `focus_offset` placed inside a wall, or a collider on
the target's own children, which the cast does not exclude.

Fix: move the pivot out of the geometry, or put the target's child colliders on a layer listed in
`collision.ignore_layers`.",
    },
    CodeEntry {
        code: code::CAMERA_ZONE_NO_PHYSICS,
        severity: Severity::Warning,
        title: "a camera zone exists in a world with no physics",
        explanation: "A `CameraZone` is in the world, but there is no `PhysicsWorld`, so the overlap query that decides
which zone the rig's target stands in cannot run. The rig reports this once and runs on its own
`rig` as if there were no zones.

Fix: add physics to the game, or remove the `CameraZone` entities.",
    },
    CodeEntry {
        code: code::CAMERA_ZONE_WITHOUT_SENSOR,
        severity: Severity::Error,
        title: "a camera zone has no sensor collider",
        explanation: "A `CameraZone` entity has no `Collider` with `sensor: true`. A zone is the volume of its sensor:
the rig takes the zone's `rig` while the target's origin is inside it, so with no sensor the zone
never activates.

Fix: add `Collider` with `sensor: true` and the zone's shape beside the `CameraZone`.",
    },
    CodeEntry {
        code: code::CAMERA_ZONE_EQUAL_PRIORITY,
        severity: Severity::Warning,
        title: "two camera zones of the same priority overlap the target",
        explanation: "The rig's target stands inside two `CameraZone`s that share the highest `priority`. The zone on
the lower entity id wins, which is deterministic but easy to get wrong when a zone is respawned. It
is reported once per pair.

Fix: give the zones distinct `priority` values, or stop them overlapping.",
    },
    CodeEntry {
        code: code::PREFAB_ASSET_PATH,
        severity: Severity::Error,
        title: "a path in a prefab is not a relative path",
        explanation: "An asset path in a `kind: prefab` document (a part's component, an instance entry's override, or
a nested `prefab:` entry) is absolute, carries a drive, scheme or stream prefix (`:`), contains a
NUL byte, or has a malformed `..` segment. Every path in a prefab resolves against the directory of
the prefab file itself, so `meshes/a.mesh.yaml` names a file beside the prefab and `../x.png` climbs
one level; the engine rebases it to the content root when the prefab is read.

Fix: write the path relative to the prefab file's directory; `..` may climb.",
    },
    CodeEntry {
        code: code::LOCAL_LIGHT_INVALID_SHADOW,
        severity: Severity::Error,
        title: "local light shadow near plane is invalid",
        explanation: "\
A `PointLight` or `SpotLight` has a `shadow_near` that cannot form a shadow view (pointer
`/shadow_near`). Either it is non-finite or not above 0, or it is not below the light's `range`
(the shadow view's far plane), which leaves no depth span to render. The range check applies when
the light casts shadows or authors a `shadow_near`; a non-casting light with a tiny `range` and no
`shadow_near` is not refused.

Fix: give `shadow_near` a finite value above 0 and below `range` (default 0.05), or lower it to fit
a small `range`.",
    },
    CodeEntry {
        code: code::DIRECTIONAL_LIGHT_EXTENT_UNUSED,
        severity: Severity::Warning,
        title: "directional light shadow extent is unused under cascades",
        explanation: "\
A `DirectionalLight` sets `cascades` and also a `shadow_extent` other than the default 40.0 (pointer
`/entities/{i}/components/DirectionalLight/shadow_extent`). Under cascades each shadow frustum is
fitted to the active camera, so the extent and the light's `Transform` anchor are not read; the
value does nothing.

Fix: remove `shadow_extent`, or drop `cascades` to keep the anchored box.",
    },
    CodeEntry {
        code: code::PREFAB_CLIP_TARGET,
        severity: Severity::Error,
        title: "a prefab clip animates a part the prefab does not have",
        explanation: "A clip bound in a `kind: prefab` document's `clips:` map has a track whose `target` is not the name
of one of the prefab's parts (pointer `/clips/<name>`). A track binds by part name when the clip
plays on an instance, so an unmatched track moves nothing: the part it meant to pose is drawn at rest
and the clip looks partly dead, with no error at play time. A nested instance's parts bind by their
composed path (`turret/barrel`).

Fix: rename the track's `target` to a part name (`entities[].name`); the message names the nearest one.",
    },
    CodeEntry {
        code: code::DOC_SINGLETON_COUNT,
        severity: Severity::Error,
        title: "a single-doc kind does not hold exactly one doc",
        explanation: "A game registered a kind with `EngineBuilder::doc_singleton`, which promises the content
root holds exactly one `kind:` doc of it, and the count is not one (pointer `<kind>`).

With two or more, every read of the store would answer with a stray copy or an arbitrary one, so the
edit the author meant to apply may silently lose to a forgotten file; the message names every
declaring file. With none, the game would boot into nothing: the rules, tuning or clock it expects are
absent and no system could say so.

The engine counts at every point the doc validators run: at build, at `scene.load`/`restore`/district
switch, under `--check-docs`, and after a live `asset.reload` (a reload that would add a second doc is
refused and the store keeps its contents and generation).

Fix: delete or rename the stray copy, or add the missing doc under the content root. If a set (or no
doc) is legitimate, register the kind with `doc_kind` instead.",
    },
    CodeEntry {
        code: code::UI_RECT_FILL_IGNORES_PLACEMENT,
        severity: Severity::Warning,
        title: "a filled UiRect ignores its offset or size",
        explanation: "A `UiRect` with `fill: true` covers the whole render target (`[0, 0, width, height]` pixels at any
extent or aspect), so its `anchor`, `offset` and `size` play no part in placing it. The rect also
authors a non-default `offset` or `size` (pointer `/offset` or `/size`), which does nothing.

The scene loads and the rect fills the screen; the warning flags a value that looks like it should
matter. Fix: delete the `offset` or `size` from the filled rect, or drop `fill: true` to place the
rect by its anchor again. (`anchor` is not checked: the default is indistinguishable from an
authored `top_left`.)",
    },
    CodeEntry {
        code: code::ANIMATION_HOLD_ZERO,
        severity: Severity::Error,
        title: "Animation hold is zero",
        explanation: "\
An `Animation` component's `hold` is `0`.

`hold` is how many sim ticks each pose is shown: `1` (the default) poses every tick, `2` gives 30 poses
per second at 60 Hz, `5` gives 12 (stop-motion \"on twos\" at 24 fps). Zero ticks per pose has no
meaning, so the component is refused rather than read as `1`.

The JSON Pointer names the field: `/hold`. Fix: a whole number of at least `1`, or remove the field to
play every tick.",
    },
    CodeEntry {
        code: code::PARENT_JOINT_NOT_SKINNED,
        severity: Severity::Error,
        title: "ParentJoint's parent is not a skinned mesh or an assembly",
        explanation: "\
A `ParentJoint` component is on an entity whose `Parent` is not a skinned mesh or an assembly, or
that has no `Parent` at all.

`ParentJoint` attaches an entity to one joint of its parent's skeleton, or one part of its parent's
voxel assembly, so the parent must carry a `Mesh` whose `shape` is `asset` naming a mesh document with
`joints:`, `character`, which resolves to one (the rule `SkinBone` uses), or `assembly`. An
`assembly_part`, a `voxel` model or a primitive has nothing to attach to. The message names the
reason.

The JSON Pointer names the component: `/entities/{i}/components/ParentJoint`. Fix: point `Parent` at
the skinned or assembly entity, or remove `ParentJoint`.",
    },
    CodeEntry {
        code: code::PARENT_JOINT_UNKNOWN_JOINT,
        severity: Severity::Error,
        title: "ParentJoint's joint is not a joint of its parent's skeleton or a part of its assembly",
        explanation: "\
A `ParentJoint` component's `joint` does not name any joint of its parent's mesh document, or any
part of its parent's assembly.

The help names the nearest joint or part name when one is close enough to suggest a typo, or lists
every name the document declares.

The JSON Pointer names the field: `/entities/{i}/components/ParentJoint/joint`. Fix: spell `joint`
exactly as one of the parent mesh's `joints[].name` entries — `ludion check-scene <mesh> --bounds`
prints the joint tree — or as a part name of the parent assembly (`parts[].name`).",
    },
    CodeEntry {
        code: code::PRESENTATION_INSTANCE_SIM_COMPONENT,
        severity: Severity::Error,
        title: "a presentation prefab instance has a part only the sim reads",
        explanation: "\
A prefab instance entry with `presentation: true` spawns every part presentation, but one of its
parts carries a component that only the sim reads (a `Collider`, a `Velocity`, a joint, a character
motor, a path follower, a camera rig, or a component a game marked sim-only). A presentation entity
is outside the sim's world hash and no sim system can see it, so the component would be dead weight.
The message names the part and the component. An `overrides` entry that adds the component to a part
the key turned presentation is caught; a part the prefab already makes presentation is exempt.

Raised at expansion (load, `check-scene`) with the pointer `/entities/{i}/presentation`, and at a
runtime presentation spawn (`spawn_prefab(.., true)`, `prefab.spawn` with `presentation: true`) with
the pointer `/entities/{k}/components/{Component}`. Fix: drop `presentation: true` and keep
the instance sim, or keep the sim part in a separate instance and parent the presentation body to it
through `overrides` (`Parent` on the body's root part).",
    },
    CodeEntry {
        code: code::LOCAL_LIGHT_INVALID_SHADOW_DISTANCE,
        severity: Severity::Error,
        title: "local light shadow distance or fade is invalid",
        explanation: "\
A `PointLight` or `SpotLight` has a `shadow_distance` or `shadow_fade` that cannot describe a
distance fade. The message names which case:

- `shadow_distance` is non-finite or not above 0 (pointer `/shadow_distance`).
- `shadow_fade` is non-finite or negative (pointer `/shadow_fade`).
- `shadow_fade` is above 0 with no `shadow_distance`: the fade band sits before the distance, so it
  needs one (pointer `/shadow_fade`).
- `shadow_fade` is longer than `shadow_distance`, which would start the band behind the camera
  (pointer `/shadow_fade`).

Fix: give `shadow_distance` a finite value above 0 (the camera distance, to the light's footprint
surface, where its shadow ends), and keep `shadow_fade` between 0 and `shadow_distance`; remove
`shadow_fade` to cut the shadow off hard at the distance.",
    },
    CodeEntry {
        code: code::LOCAL_LIGHT_SHADOW_FADE_UNUSED,
        severity: Severity::Warning,
        title: "local light shadow distance is unused without cast_shadows",
        explanation: "\
A `PointLight` or `SpotLight` sets `shadow_distance` or `shadow_fade` but not `cast_shadows: true`
(pointer `/entities/{i}/components/{PointLight|SpotLight}/shadow_distance`, or `/shadow_fade` when
only that is set). The light has no shadow to fade, so the field does nothing.

Fix: set `cast_shadows: true`, or remove `shadow_distance` and `shadow_fade`.",
    },
    CodeEntry {
        code: code::CAMERA_DOF_FOCUS_ZERO,
        severity: Severity::Warning,
        title: "depth of field is on but focus_distance is 0",
        explanation: "\
A `PostProcess` has `dof` above 0 and an authored `focus_distance` of 0 (pointer
`/entities/{i}/components/PostProcess/focus_distance`). The focal plane is then at the lens, so every
surface in front of the camera is defocused and the scene blurs.

Fix: set `focus_distance` to the subject's distance from the camera, or set `dof` to 0.",
    },
    CodeEntry {
        code: code::SUN_DISC_SIZE_MISMATCH,
        severity: Severity::Warning,
        title: "sun disc size and light angular radius disagree",
        explanation: "\
`Environment.sun_disc_size` is the sky disc's full angle and `DirectionalLight.angular_radius` is
the sun's half angle, both in degrees. Both are authored and positive but `sun_disc_size` is not
twice `angular_radius` (pointer `/entities/{i}/components/Environment/sun_disc_size`), so the
disc in the sky and the specular highlight on glossy surfaces differ in size.

Fix: set `sun_disc_size` to twice `angular_radius`, or drop one of the two.",
    },
    CodeEntry {
        code: code::CAMERA_OFFSET_INVALID,
        severity: Severity::Error,
        title: "camera offset field is not finite",
        explanation: "\
A `CameraOffset` (pointer `/entities/{i}/components/CameraOffset/{field}`) has a `translation`,
`rotation_deg` or `fov_delta` that is NaN or infinite. A scene with one is refused; at runtime the
whole offset is ignored and the rig's pose is drawn.

Fix: write finite numbers.",
    },
    CodeEntry {
        code: code::CAMERA_OFFSET_ON_SIM_ENTITY,
        severity: Severity::Error,
        title: "camera offset on a sim entity",
        explanation: "\
A `CameraOffset` is on a sim entity (pointer `/entities/{i}/components/CameraOffset`). A sim
entity's components are hashed, and the offset exists to move the rendered camera without moving
`WorldHash`, so it is never applied there. `ludion check-scene` and `scene.check` refuse it; a scene
that loads anyway, or a runtime insert, is ignored and reported once.

Fix: put the `CameraOffset` on a presentation entity (`presentation: true` in a scene,
`spawn_presentation` in code).",
    },
    CodeEntry {
        code: code::CAMERA_OFFSET_FOV_CLAMPED,
        severity: Severity::Warning,
        title: "camera offset field of view was clamped",
        explanation: "A `CameraOffset.fov_delta` takes a perspective camera's vertical field of view outside 1 to 179
degrees, so the drawn field of view is clamped to that range. Reported once; the frame still draws.

Fix: keep `Camera.fov_y_deg + fov_delta` within 1 to 179, for instance by clamping the tween.",
    },
    CodeEntry {
        code: code::VIEWMODEL_NO_CAMERA,
        severity: Severity::Warning,
        title: "a viewmodel has no first-person camera",
        explanation: "\
A mesh is flagged `Viewmodel` (directly or through a `Parent` ancestor), but the scene has no single
camera that is both perspective and a first-person `CameraRig`, so there is no camera to hold it in
front of. Reported once; the mesh draws as an ordinary world mesh, shadows included.

Fix: give exactly one perspective camera a `CameraRig` in `first_person` mode, or remove the
`Viewmodel` flag.",
    },
    CodeEntry {
        code: code::VIEWMODEL_LENS_NOT_ON_CAMERA,
        severity: Severity::Warning,
        title: "a viewmodel lens is not on a camera",
        explanation: "\
A `ViewmodelLens` is on an entity that has no `Camera`, so nothing reads it. It is ignored and
reported once.

Fix: move the `ViewmodelLens` onto the first-person camera entity, or delete it.",
    },
    CodeEntry {
        code: code::VIEWMODEL_LENS_INVALID,
        severity: Severity::Error,
        title: "a viewmodel lens field is out of range",
        explanation: "\
A `ViewmodelLens` field is out of range (pointer `/entities/{i}/components/ViewmodelLens/{field}`):
`fov_y_deg` must be finite and in 1 to 179, `near` finite and above 0, `depth_slice` finite and in
(0, 0.5]. A scene with one is refused; at runtime the defaults are used and it is reported once.

Fix: write a value in range, or omit the field to take its default.",
    },
    CodeEntry {
        code: code::SPATIAL_VOLUME_INVALID,
        severity: Severity::Error,
        title: "a spatial volume is invalid",
        explanation: "\
A `SpatialVolume` has a size that is not a positive finite `f32` (pointer
`/entities/{i}/components/SpatialVolume/shape/sphere/radius` or `.../shape/aabb/half_extents/{k}`), or
`layers` is `0` (pointer `.../layers`), a mask no query filter can ever match. A scene with one is
refused.

Fix: give the radius and every half extent a value above 0, and set at least one bit in `layers`.",
    },
    CodeEntry {
        code: code::NAV_AGENT_INVALID,
        severity: Severity::Error,
        title: "a nav agent is invalid",
        explanation: "\
A `NavAgent` field is out of range (pointer `/entities/{i}/components/NavAgent/{field}`): `target` has a
non-finite component, `speed`, `arrive_radius`, `separation_radius` or `separation_strength` is negative or
not finite, or `separation_neighbors` is above 16. A scene with one is refused.

Fix: write a finite, non-negative value, and at most 16 separation neighbours, or omit the field to take
its default.",
    },
    CodeEntry {
        code: code::NAV_ROUTE_INVALID,
        severity: Severity::Error,
        title: "a nav route is invalid",
        explanation: "\
A `NavRoute` field is out of range (pointer `/entities/{i}/components/NavRoute/{field}`): `goal`, a
waypoint or `cost` is not finite, or `index` is past the end of `waypoints`. `NavRoute` is written by `nav_steer`; a scene that carries one is a snapshot.

Fix: delete the component and let `nav_steer` plan it again from the entity's `NavGoal`.",
    },
    CodeEntry {
        code: code::NAV_GOAL_INVALID,
        severity: Severity::Error,
        title: "a nav goal is invalid",
        explanation: "\
A `NavGoal` has a non-finite component in `at` (pointer `/entities/{i}/components/NavGoal/at/{k}`). A scene
with one is refused.

Fix: write three finite numbers for `at`.",
    },
    CodeEntry {
        code: code::PATROL_DOCUMENT_STRUCTURE,
        severity: Severity::Error,
        title: "a patrol document is malformed",
        explanation: "\
A `kind: patrol` document is structurally invalid: an empty or non-mapping root, a missing `format`, `kind`
or `points`, an unknown key (the message names the nearest valid one), a `mode` that is not `loop`,
`ping_pong` or `once`, or a point, `look` entry or number of the wrong shape. The pointer names the spot,
such as `/points/2/look/0/hold`. Data rules (a negative `wait`) are the separate `L1523`.

Fix: a minimal patrol is `format: 1`, `kind: patrol` and `points:` with one `- at: [x, y, z]` entry; see
`ludion docs patrol`.",
    },
    CodeEntry {
        code: code::PATROL_DATA_CONSISTENCY,
        severity: Severity::Error,
        title: "a patrol document is inconsistent",
        explanation: "\
A well-shaped `kind: patrol` document breaks a data rule: it has no points, a number is not finite or is
beyond the f32 range, a `wait` or `hold` is negative, a `look` entry has a `hold` of 0, or a `tag` is
empty. The pointer names the field. The document's shape problems are the separate `L1522`.

Fix: add a point, keep every number finite, give each look a positive `hold`, or drop the empty `tag`.",
    },
    CodeEntry {
        code: code::PATROL_FOLLOWER_INVALID,
        severity: Severity::Error,
        title: "a patrol follower is invalid",
        explanation: "\
A `PatrolFollower` field is invalid (pointer `/entities/{i}/components/PatrolFollower/{field}`): `route`
is an absolute path or climbs out of the content root with `..`. A scene with one is refused.

Fix: name the patrol as a content-root-relative path such as `patrols/guard.patrol.yaml`.",
    },
    CodeEntry {
        code: code::PATROL_STATE_INVALID,
        severity: Severity::Error,
        title: "a patrol state is invalid",
        explanation: "\
A `PatrolState` field is out of range (pointer `/entities/{i}/components/PatrolState/{field}`):
`stop_left` or `stop_elapsed` is negative or not finite, or `look_deg` is not finite. `PatrolState` is
written by `patrol_update`; a scene that carries one is a snapshot.

Fix: delete the component and let `patrol_update` start the patrol again.",
    },
    CodeEntry {
        code: code::NAV_LINK_INVALID,
        severity: Severity::Error,
        title: "a nav link is invalid",
        explanation: "A `NavLink` has a non-finite component in `from` or `to`, a negative or non-finite `cost`, or a
non-positive or non-finite `snap_radius` (pointers `/entities/{i}/components/NavLink/from/{k}`, `/to/{k}`,
`/cost`, `/snap_radius`). A scene with one is refused.

Fix: write finite endpoints, a `cost` of 0 or more metres and a `snap_radius` above 0.",
    },
    CodeEntry {
        code: code::NAV_BLOCKER_INVALID,
        severity: Severity::Error,
        title: "a nav blocker is invalid",
        explanation: "A `NavBlocker` has a non-finite or non-positive half-extent in `shape`, a negative or non-finite
`cost`, or an empty `classes` entry (pointers `/entities/{i}/components/NavBlocker/shape/box/half_extents/{k}`,
`/cost`, `/classes/{k}`). A scene with one is refused.

Fix: write finite half-extents above 0, a `cost` of 0 or more metres and a named class in every `classes` entry.",
    },
    CodeEntry {
        code: code::NAV_AVOIDANCE_INVALID,
        severity: Severity::Error,
        title: "nav avoidance values are invalid",
        explanation: "A `NavAgent.avoidance` has a non-finite or non-positive `radius` or `horizon_s`, a
`max_neighbors` of 0 or above 16, or sits on a flying agent (pointers
`/entities/{i}/components/NavAgent/avoidance/{field}`, `/avoidance`). A scene with one is refused.

Fix: write a `radius` and `horizon_s` above 0, a `max_neighbors` from 1 to 16, and drop `avoidance` from flying agents
(avoidance steers walkers only).",
    },
    CodeEntry {
        code: code::PERCEIVER_INVALID,
        severity: Severity::Error,
        title: "a perceiver is invalid",
        explanation: "\
A `Perceiver` field is out of range (pointer `/entities/{i}/components/Perceiver/{field}`): there are no
sight zones or more than 8; a zone's `range` is not finite and above 0, its `half_angle_deg` is outside
(0, 180] or its `vertical_half_angle_deg` outside (0, 90]; two zones share a `name`; a
`visibility_clamp` has `lo > hi`; `see_through_max_range` is negative; or the `sweep` has a
`period_s` that does not exceed `2 * dwell_s`, a sine with a `dwell_s`, or a non-finite value; or
`period_ticks` is 0. A scene with one is refused. A `SightSampleRecord` with a non-finite `distance`,
`angle_deg` or `effective_range` is refused the same way.

Fix: correct the named field. A zone with the default `vertical_half_angle_deg` (45) is a wedge, not a cone.",
    },
    CodeEntry {
        code: code::SIGHT_TARGET_INVALID,
        severity: Severity::Error,
        title: "a sight target is invalid",
        explanation: "\
A `SightTarget` field is out of range (pointer `/entities/{i}/components/SightTarget/{field}`): `class`
is empty, there are no sample `points` or more than 8, an `offset` or `visibility` is not finite, a
`visibility` is negative, or `require: { fraction: f }` has an `f` outside (0, 1]. A scene with one is
refused.

Fix: correct the named field; give the target at least one point (`offset: [0, 1.6, 0]` is a head).",
    },
    CodeEntry {
        code: code::PERCEPTION_NAME_UNKNOWN,
        severity: Severity::Warning,
        title: "perception names an unknown class or layer",
        explanation: "\
`perception_update` met a `detects` class the `PerceptionConfig` does not declare, or a `blocked_by` /
`see_through` layer the physics layer table does not declare. The message names the nearest declared
name. Conservatively, the zone detects nothing of that class, and a sample point with an unknown
`blocked_by` layer is not seen; an unknown `see_through` layer is dropped. Reported once per name.

`check-scene` reports it too, once per occurrence at its JSON Pointer: layers everywhere, and classes (`detects`,
`SightTarget.class`) and noise kinds (`hears`, `NoiseZone.kinds`) only under a game's `PerceptionConfig` (a game's
`--check-scene`, the wire's `scene.check` on a game host), not in `ludion check-scene`.

This is a **warning**.

Fix: declare the class in `PerceptionConfig.classes`, or the layer under `physics.layers`.",
    },
    CodeEntry {
        code: code::PERCEIVER_MEMORY_INVALID,
        severity: Severity::Error,
        title: "a perceiver's memory is invalid",
        explanation: "\
A `Perceiver` `memory` field is out of range (pointer `/entities/{i}/components/Perceiver/memory/{field}`):
`forget_sight_s` or `forget_hearing_s` is negative or not finite, or `max_records` is 0 or above 64. A
`StimulusRecord` with a non-finite `last_position`, `last_direction` or `strength` is refused the same
way. A scene with one is refused.

Fix: correct the named field. To keep no memory at all, omit `memory` instead of writing a zero.",
    },
    CodeEntry {
        code: code::STIMULUS_REPORT_INVALID,
        severity: Severity::Warning,
        title: "a reported stimulus was dropped",
        explanation: "\
`report_stimulus` was called for an entity that is not alive, that has no `Perceiver` with a `memory`,
or with an `at` or `strength` that is not finite. The stimulus is dropped and nothing is remembered.
Reported once per cause.

This is a **warning**.

Fix: give the perceiver `memory:` (see `ludion docs perception`), and pass finite numbers.",
    },
    CodeEntry {
        code: code::PERCEPTION_CONFIG_INVALID,
        severity: Severity::Error,
        title: "perception config is invalid",
        explanation: "\
`Engine::builder().perception(config)` was given a `classes` or `noise_kinds` list that holds more than
32 names, an empty name or a repeated name (pointer `/classes/{j}`, `/noise_kinds/{j}`). Each name is
one bit of a mask, so a list is capped at 32. It is also refused for a `surface_noise` entry with an
empty or repeated `surface` or a negative or non-finite `multiplier` (`/surface_noise/{j}/...`), an
`occlusion_factor` outside [0, 1] or a `max_occluders` outside 1 to 8, or a
`max_sight_rays_per_tick` of 0 (`/max_sight_rays_per_tick`; leave it unset for no budget). `build`
refuses the config.

Fix: list each name once, at most 32, and keep the numbers in range.",
    },
    CodeEntry {
        code: code::PERCEPTION_DEFERRED,
        severity: Severity::Warning,
        title: "a perceiver's sight is starved by the ray budget",
        explanation: "\
`perception_update` has not evaluated a perceiver's sight for more than four of its `period_ticks`
(so at least five ticks for the default period of 1) because `PerceptionConfig.max_sight_rays_per_tick`
served other perceivers first. Its carried samples go stale: `evaluated_tick` stops advancing and its
guard acts on old sight. Reported once per run; `engine.describe` `perception.deferred` lists the
perceivers waiting.

This is a **warning**.

Fix: raise `max_sight_rays_per_tick` (2048 is the recommended value), raise the `period_ticks` of
far or idle perceivers, or give them fewer zones and sample points.",
    },
    CodeEntry {
        code: code::NOISE_ZONE_INVALID,
        severity: Severity::Error,
        title: "a noise zone is invalid",
        explanation: "\
A `NoiseZone` field is out of range (pointer `/entities/{i}/components/NoiseZone/{field}`): a box
`half_extents` entry or a sphere `radius` that is not finite and above 0, a `multiplier` that is
negative or not finite, or an empty or repeated `kinds` entry. A scene with one is refused.

Fix: correct the named field. An empty `kinds` list already means every kind.",
    },
    CodeEntry {
        code: code::HEARING_INVALID,
        severity: Severity::Error,
        title: "a perceiver's hearing is invalid",
        explanation: "\
A `Perceiver` `hearing` field is out of range (pointer `/entities/{i}/components/Perceiver/hearing/{field}`):
an `ear` that is not finite, a `sensitivity` that is negative or not finite, or a `hears` list that is
empty or holds an empty or repeated kind. A scene with one is refused.

Fix: correct the named field. To hear nothing, omit `hearing`.",
    },
    CodeEntry {
        code: code::NOISE_INVALID,
        severity: Severity::Warning,
        title: "an emitted noise was dropped",
        explanation: "\
`emit_noise` was given an `at`, `radius` or `loudness` that is not finite or is negative, so the noise
is dropped and nothing hears it. Reported once. (An undeclared `kind` is `L1572`.)

This is a **warning**.

Fix: pass finite, non-negative numbers; see `ludion docs perception`.",
    },
    CodeEntry {
        code: code::PATROL_ROUTE_UNLOADED,
        severity: Severity::Warning,
        title: "a patrol route could not be loaded",
        explanation: "\
`patrol_update` could not load the `kind: patrol` document named by an entity's `PatrolFollower.route`: the
file is missing or unreadable, or it fails validation (`L1522`, `L1523`; the loader's diagnostics follow as
related notes). The guard is skipped and keeps whatever `NavGoal` it had. Reported once per entity.

Fix: correct the `route` path or the document; `ludion check-scene <file>` shows the document's errors.",
    },
    CodeEntry {
        code: code::PATROL_POINT_UNREACHABLE,
        severity: Severity::Warning,
        title: "a patrol stop is unreachable",
        explanation: "\
A patrol stop has no walkable path from the guard on the nav grid, so `patrol_update` skipped it and went on
to the next stop (the message names the point index). Reported once per entity and point.

Fix: move the point onto walkable ground, or clear what blocks it; `ludion docs nav` covers the grid.",
    },
    CodeEntry {
        code: code::PATROL_WITHOUT_AGENT,
        severity: Severity::Warning,
        title: "a patrol follower has no nav agent",
        explanation: "\
An entity carries a `PatrolFollower` but no `NavAgent`, so `patrol_update` has nothing to steer and skips
it. Reported once per entity.

Fix: add a `NavAgent` to the entity.",
    },
    CodeEntry {
        code: code::NAV_LINK_UNSNAPPED,
        severity: Severity::Warning,
        title: "a nav link end is off the walkable grid",
        explanation: "An enabled `NavLink` has an end with no walkable node within `snap_radius`, so the link is left out of the
navigation table and no route crosses it. Reported once per link per grid build; the help names the
nearest node's distance when one exists.

Fix: move the end onto walkable ground, or raise `snap_radius`.",
    },
    CodeEntry {
        code: code::NAV_NOT_AT_LINK,
        severity: Severity::Error,
        title: "the agent is not stopped at a nav link",
        explanation: "`nav_link_done` was called for an entity whose `NavRoute` is missing or not `at_link`. Only a routed
agent that reached a link start waits for it.

Fix: call `nav_link_done` only while the agent's `NavRoute.status` is `at_link`.",
    },
    CodeEntry {
        code: code::NAV_CLASS_UNKNOWN,
        severity: Severity::Warning,
        title: "the nav agent class is not defined",
        explanation: "A `NavAgent.class`, a `PathRequest.class` or a `NavLink.classes` entry names a class that
`NavConfig.classes` does not define. A query returns this as an error; `nav_steer` leaves the agent standing
still and `nav_update` skips the link entry, each reporting once per offender. The help lists the class names.

Fix: use a name from `NavConfig.classes`, or leave the class empty for the first class.",
    },
    CodeEntry {
        code: code::INPUT_BINDING_SOURCES,
        severity: Severity::Error,
        title: "a binding has zero or several sources",
        explanation: "\
An `input-actions` binding must name exactly one source (`key`, `mouse`, `pad`, `trigger`, `wheel`, `stick`, `keys`, `key_pair`, `pad_pair` or `mouse_delta`); this one names none or more than one (pointer `/actions/{name}/bindings/{i}`).

Fix: keep one source per binding and move the others into their own bindings.",
    },
    CodeEntry {
        code: code::INPUT_BINDING_SOURCE_KIND,
        severity: Severity::Error,
        title: "a binding source does not fit its action kind",
        explanation: "\
The binding's source cannot drive the action's `kind` (pointer `/actions/{name}/bindings/{i}/{field}`). A `button` takes `key`, `mouse`, `pad`, `trigger` or `wheel: up|down`; an `axis1` takes `wheel: x|y`, `key_pair` or `pad_pair`; an `axis2` takes `keys`, `stick` or `mouse_delta`.

Fix: change the action's `kind`, or use a source of that kind.",
    },
    CodeEntry {
        code: code::INPUT_BINDING_MODIFIER,
        severity: Severity::Error,
        title: "a binding modifier does not fit its source",
        explanation: "\
A modifier is set on a source it does not apply to (pointer `/actions/{name}/bindings/{i}/{field}`): `gesture` belongs on button bindings (not a wheel button), `press` and `release` only on `trigger`, `deadzone` only on `stick`, `scale` and `invert_x`/`invert_y` only on analog sources (`stick`, `mouse_delta`, `wheel` x|y).

Fix: remove the modifier or move it to a binding it applies to.",
    },
    CodeEntry {
        code: code::INPUT_BINDING_RANGE,
        severity: Severity::Error,
        title: "a binding value is out of range",
        explanation: "\
A binding or gesture number is out of range (pointer `/actions/{name}/bindings/{i}/{field}` or `/gestures/{field}`): non-finite, a trigger `release` above `press` or outside `[0, 1]`, a `deadzone` outside `[0, 1)`, or a gesture time that is not above zero.

Fix: write a finite value inside the stated range.",
    },
    CodeEntry {
        code: code::INPUT_BINDING_DUPLICATE,
        severity: Severity::Error,
        title: "two bindings share a source and gesture",
        explanation: "\
Two bindings in one `context` name the same physical input with the same gesture (pointer `/actions/{name}/bindings/{i}`), so one press would fire both actions with no way to tell them apart.

Fix: change one binding's source or gesture, or move it to another `context`.",
    },
    CodeEntry {
        code: code::INPUT_ACTION_INVALID,
        severity: Severity::Error,
        title: "an action is empty or badly named",
        explanation: "\
An action has no bindings, or its name is not made of `a-z`, `0-9` and `_` (pointer `/actions/{name}`).

Fix: give the action at least one binding and a lower-case snake_case name.",
    },
    CodeEntry {
        code: code::INPUT_ACTION_UNKNOWN,
        severity: Severity::Warning,
        title: "an input action name is unknown",
        explanation: "\
A game read an action name (`ActionName`) that the loaded `kind: input-actions` document does not define, so the read answered inactive/zero. Reported once per name.

Fix: add the action to the input map, or correct the name in the game code.",
    },
    CodeEntry {
        code: code::INPUT_STATE_INVALID,
        severity: Severity::Error,
        title: "an input-action state does not fit the map",
        explanation: "\
A restored input-action state holds an entry the loaded map cannot continue (a press with no ticks
down, or a pending tap on an input with no `double_tap` binding). Entries for inputs the map no
longer binds are dropped silently; this is for the ones that are corrupt.

Fix: restore a state saved from the same input map, or drop the state and let the gestures restart.",
    },
    CodeEntry {
        code: code::INPUT_ACTION_LOCKED,
        severity: Severity::Error,
        title: "an input action is locked",
        explanation: "\
A rebind, capture, unbind or reset named an action whose `locked: true` flag forbids player changes
(also: a conflict resolution that would swap or replace a binding of a locked action).

Fix: leave the action alone, or drop `locked` from its `kind: input-actions` entry. Resolve a conflict
with a locked action by keeping the existing binding.",
    },
    CodeEntry {
        code: code::INPUT_OVERRIDE_UNKNOWN,
        severity: Severity::Warning,
        title: "an input-binding override names an unknown action",
        explanation: "\
An input-bindings override (pointer `/actions/{name}`) names an action the loaded input map does not
define, so it is dropped. A renamed or removed action leaves such entries in a player's saved bindings.

Fix: nothing for a player's file (the entry is ignored); otherwise correct the action name.",
    },
    CodeEntry {
        code: code::INPUT_OVERRIDE_INVALID,
        severity: Severity::Warning,
        title: "an input-binding override does not compile",
        explanation: "\
An override (pointer `/actions/{name}`) holds a binding that does not fit the action's kind, names no or
several sources, or duplicates another action's source and gesture in the same context. That action
keeps its default bindings; the other overrides still apply.

Fix: correct or delete the entry, or call `reset` on the action.",
    },
    CodeEntry {
        code: code::INPUT_CAPTURE_IMPOSSIBLE,
        severity: Severity::Error,
        title: "a binding capture is impossible",
        explanation: "\
Capture needs a `button` action and a slot index at most the action's binding count. Resolving a
conflict with Replace, or with Swap that appended, is refused when it would leave the other action with
no bindings.

Fix: capture a button action with a valid slot; resolve with Swap (when we replaced a binding) or Keep.",
    },
    CodeEntry {
        code: code::INPUT_CAPTURE_IDLE,
        severity: Severity::Error,
        title: "no binding capture or conflict is pending",
        explanation: "\
`resolve_conflict` was called while no capture was waiting on a conflict.

Fix: call it only when `capture_result()` is `Conflict`.",
    },
    CodeEntry {
        code: code::INPUT_BINDINGS_ENVELOPE,
        severity: Severity::Warning,
        title: "input bindings file envelope is malformed",
        explanation: "\
The player's input bindings file (`save/input-bindings.yaml`) has no valid `format`/`kind` envelope:
`format` is missing or not an integer, `kind` is missing or not `input-bindings`, or the document is
not a mapping. The engine backs the file up (`input-bindings.yaml.bak.<hash>`), starts from the
default bindings and reports `L4277`.

Fix: nothing is required; the next `Actions::save()` writes a fresh file. To recover the old
bindings, open the backup and copy them across; a minimal file is:

  format: 1
  kind: input-bindings",
    },
    CodeEntry {
        code: code::INPUT_BINDINGS_RESET,
        severity: Severity::Warning,
        title: "input bindings file could not be loaded — defaulted",
        explanation: "\
The player's input bindings file (`save/input-bindings.yaml`) could not be honored at all: it is not
parseable, its envelope is malformed (`L1636`), a value has the wrong shape, or its `format` is newer
than this engine reads. The engine renamed it aside as `input-bindings.yaml.bak.<hash>` and started
from the default bindings. The game still boots and the player's bytes are kept. A single bad action
never resets the file: it is dropped with `L4273` or `L4274` and the rest applies.

Fix: nothing is required. To recover the old bindings, reconcile the backup with the new file by hand.",
    },
    CodeEntry {
        code: code::INPUT_BINDINGS_SAVE_FAILED,
        severity: Severity::Warning,
        title: "input bindings file could not be written to disk",
        explanation: "\
A game system called `Actions::save()` and the engine could not write `save/input-bindings.yaml` at
the frame boundary: the save directory is missing or not writable, or the disk is full. The write is
staged to a hidden `.tmp` sibling and renamed, so the previous whole file stays on disk. The new
bindings still apply to this session.

Fix: make `save/` writable and call `save()` again.",
    },
    CodeEntry {
        code: code::RUMBLE_EFFECT_INVALID,
        severity: Severity::Warning,
        title: "a rumble request had a non-finite value",
        explanation: "\nA game system called `Rumble::play()` with a `low`, `high` or `duration` that is NaN or infinite.
The value played as 0, so a request with no other motor or duration left is dropped. Finite values
out of range are clamped silently (motors to 0..=1, `duration` to 0..=2 seconds). It is one warning
per frame boundary, and rumble is host feel: it never touches the simulation or the `WorldHash`.

Fix: compute the motor strengths and the duration from finite values, for example guard a division
that can reach zero before it feeds `RumbleEffect`.",
    },
    CodeEntry {
        code: code::UI_TWEEN_INVALID,
        severity: Severity::Error,
        title: "a UI tween track is invalid",
        explanation: "\
A `UiTween` track (pointer `/entities/{i}/components/UiTween/tracks/{n}/{field}`) has a `from` or `to`
whose length does not match its `field` (`opacity` and `scale` take 1 number, `offset` and `size` 2,
`color` 4), a non-finite number, or a negative or non-finite `duration`. A scene with one is refused.

Fix: write the right number of finite values, and a `duration` of zero or more seconds.",
    },
    CodeEntry {
        code: code::UI_WORLD_ANCHOR_INVALID,
        severity: Severity::Error,
        title: "a UI world anchor is invalid",
        explanation: "\
A `UiWorldAnchor` (pointer `/entities/{i}/components/UiWorldAnchor/{field}`) has a non-finite
`world_offset` or `margin`, a negative `margin`, or a `fade` that is not `[near, far]` with
`0 <= near < far`. A scene with one is refused.

Fix: write finite numbers, a `margin` of zero or more canvas units, and a `fade` whose `far` is past
its `near`.",
    },
    CodeEntry {
        code: code::UI_MOTION_UNSUPPORTED,
        severity: Severity::Warning,
        title: "a UI tween or world anchor does not apply to its element",
        explanation: "\
A `UiTween` track names a field its element cannot take (`scale` and `color` need a `UiRect` or
`UiText`, `size` needs a `UiRect`), or a `UiWorldAnchor` sits on a `fill: true` rect, which ignores its
anchor. The track or anchor is skipped and the element draws as if it were absent. Reported once per
entity and field, as a degraded warning at render time.

Fix: move the track to an element that takes it, or remove it.",
    },
    CodeEntry {
        code: code::UI_RECT_SAMPLING_INVALID,
        severity: Severity::Error,
        title: "a UI rect sampling field is invalid",
        explanation: "\
A `UiRect` `uv_rect` (pointer `/entities/{i}/components/UiRect/uv_rect/{n}`) has a non-finite value, a
value outside 0..1, or `u0 >= u1` / `v0 >= v1`; a `slice.border` value is negative or non-finite; or
`slice.scale` is non-finite or not above zero. A scene with one is refused.

Fix: write `uv_rect: [u0, v0, u1, v1]` inside 0..1 with the minimum below the maximum, borders of zero
or more texels, and a positive `scale`.",
    },
    CodeEntry {
        code: code::UI_RECT_SAMPLING_IGNORED,
        severity: Severity::Warning,
        title: "a UI rect sampling field is ignored",
        explanation: "\
A `UiRect` has both `uv_rect` and `slice` (`slice` wins and `uv_rect` is ignored, pointer
`/entities/{i}/components/UiRect/uv_rect`), or has either field with neither `texture` nor `atlas`, so
there is nothing to sample.

Fix: keep one of `uv_rect` and `slice`, and give the rect a `texture` or an `atlas` and `frame`.",
    },
    CodeEntry {
        code: code::UI_SLICE_CLAMPED,
        severity: Severity::Warning,
        title: "a UI nine-slice was drawn clamped",
        explanation: "\
A `UiRect` `slice` has borders larger than the texture or atlas region, or corners (border x `scale`
x the canvas pixel scale) larger than the rect. The borders shrink proportionally so the corners fit.
Reported once per entity, as a degraded warning at render time.

Fix: shrink the `border` or `scale`, or enlarge the rect.",
    },
    CodeEntry {
        code: code::UI_GROUP_INVALID,
        severity: Severity::Error,
        title: "a UI group has an out-of-range number",
        explanation: "\
A `UiGroup` `scale` (pointer `/entities/{i}/components/UiGroup/scale`) is non-finite or not above zero,
or its `opacity` is non-finite or outside 0..1, or a `clip: { rounded_rect: { corner_radius } }` radius
(pointer `.../UiGroup/clip/rounded_rect/corner_radius`) is non-finite or negative. A scene with one is
refused.

Fix: write a positive finite `scale`, an `opacity` between 0 and 1 and a `corner_radius` of 0 or more.",
    },
    CodeEntry {
        code: code::UI_GROUP_CYCLE,
        severity: Severity::Error,
        title: "UI group references form a cycle",
        explanation: "\
A chain of `group` references on `UiRect`, `UiText`, `UiList` or `UiGroup` comes back to an entity it
already passed through (pointer `/entities/{i}/components/{Component}/group`). A scene with one is
refused.

Fix: break the loop; a group's `group` must point at an outer group.",
    },
    CodeEntry {
        code: code::UI_GROUP_TOO_DEEP,
        severity: Severity::Error,
        title: "a UI group chain is too deep",
        explanation: "\
An element sits inside more than 8 nested `UiGroup`s (pointer
`/entities/{i}/components/{Component}/group`). A scene with one is refused.

Fix: flatten the nesting to 8 groups or fewer.",
    },
    CodeEntry {
        code: code::UI_GROUP_IGNORED,
        severity: Severity::Warning,
        title: "a UI group field is ignored",
        explanation: "\
A `UiRect` has `fill_group: true` without a `group` (pointer
`/entities/{i}/components/UiRect/fill_group`), or has `fill: true` beside `group` or `fill_group`
(`fill` wins and the group fields are ignored).

Fix: set `group`, or drop `fill_group`; keep either `fill` or the group fields.",
    },
    CodeEntry {
        code: code::UI_TEXT_OUTLINE_INVALID,
        severity: Severity::Error,
        title: "a UI text outline is invalid",
        explanation: "\
A `UiText` or `UiList` `outline` has a `width` that is not finite or is below 0 (pointer
`/entities/{i}/components/{Component}/outline/width`), or a `color` with a non-finite channel
(`.../outline/color`). A scene with one is refused.

Fix: give `width` a finite value of 0 or more, in canvas units (0 draws no outline), and `color` finite
linear straight RGBA.",
    },
    CodeEntry {
        code: code::UI_RADIAL_INVALID,
        severity: Severity::Error,
        title: "a UiRadial is invalid",
        explanation: "\
A `UiRadial` has one of: a non-finite number; a `radius_inner` below 0 or a `radius_outer` that is not
above it; a `gap_deg` or `hysteresis_deg` outside `[0, 360 / items)`; a `deadzone` outside `[0, 1)`; a
`size` or `icon_size` that is not positive and finite; or more than 16 `items`. The pointer
(`/entities/{i}/components/UiRadial/{field}`) names the field. A scene with one is refused.

Fix: give the radii `0 <= radius_inner < radius_outer`, keep `gap_deg` and `hysteresis_deg` under one
sector's span, `deadzone` below 1, and at most 16 items (the game builds items from what it owns).",
    },
    CodeEntry {
        code: code::UI_ROTATION_INVALID,
        severity: Severity::Error,
        title: "a UI rotation is invalid",
        explanation: "\
A `UiRect`, `UiText` or `UiGroup` has a `rotation` that is not finite (pointer
`/entities/{i}/components/{Component}/rotation`). A scene with one is refused.

Fix: give `rotation` a finite value in degrees (positive is clockwise on screen; 0 draws unrotated).",
    },
    CodeEntry {
        code: code::UI_SHAPE_INVALID,
        severity: Severity::Error,
        title: "a UiShape is invalid",
        explanation: "\
A `UiShape` has one of: a non-finite number; a `radius` that is not above 0; a `ring` `thickness` that is
not in `(0, radius]`; a `wedge` `inner_radius` outside `[0, radius)`; a `sweep_deg` outside `[0, 360]`; a
negative `size` component or `corner_radius`; a stroke `width` that is not above 0; a `fill` on a `line` or
`arc` (they draw only their stroke); a `line` or `arc` with no `stroke`; a `polygon` point that is not
finite, more than 256 `polygon` points, or a `stroke` on a `polygon`. The pointer
(`/entities/{i}/components/UiShape/{field}`) names the field. A scene with one is refused.

Fix: correct the field; give a `line` or `arc` a `stroke` and drop its `fill`.",
    },
    CodeEntry {
        code: code::UI_GROUP_UNRESOLVED,
        severity: Severity::Warning,
        title: "a UI element's group could not be resolved",
        explanation: "\
A UI element's `group` names a dead entity or one without `UiGroup`, or its chain runs past 8 groups
(a wire spawn can build either). The element draws ungrouped. Reported once per entity, as a degraded
warning at render time; `check-scene` reports a static reference to an entity without `UiGroup`.

Fix: point `group` at a live entity that carries `UiGroup`, within 8 levels.",
    },
    CodeEntry {
        code: code::UI_CLIP_NESTING,
        severity: Severity::Warning,
        title: "a UI element sits under more than two clipping groups",
        explanation: "\
A UI element's `group` chain holds more than two groups whose `clip` is not `none`. The renderer
intersects at most two clips per element: the two innermost are kept and the outer ones are dropped.
Reported once per element as a degraded warning at render time; `check-scene` reports the chain
statically.

Fix: merge clipping groups, or move the element so at most two clipping groups enclose it.",
    },
    CodeEntry {
        code: code::UI_TEXT_OUTLINE_CLAMPED,
        severity: Severity::Warning,
        title: "a UI text outline is wider than the face can draw",
        explanation: "\
A `UiText` or `UiList` `outline.width` exceeds what its face can draw, so it is clamped. An SDF face
draws up to its baked spread less one screen pixel; a bitmap face (the embedded 8x8 among them)
draws 1 to 4 whole source pixels. Reported once per element as a degraded warning at render time; the
message names the face and the maximum width in canvas units.

Fix: lower `outline.width` to the stated maximum, or bake the face with a larger SDF spread.",
    },
    CodeEntry {
        code: code::UI_POLYGON_DEGENERATE,
        severity: Severity::Warning,
        title: "a UiShape polygon is degenerate",
        explanation: "\
A `UiShape` `polygon` has fewer than 3 distinct points, zero area, or edges that cross or touch, so it
cannot be filled and draws nothing that frame. Reported once per entity as a degraded warning at render
time; the message names the fault and, for a self-intersection, the two edge indices.

Fix: write a simple polygon of 3+ distinct points whose edges do not cross.",
    },
    CodeEntry {
        code: code::STATIC_INELIGIBLE,
        severity: Severity::Warning,
        title: "a static entity cannot take the static path",
        explanation: "\
An entity carries `Static` (pointer `/entities/{i}/components/Static`) but has something the static
path cannot hold: a skinned `asset`, an `assembly`, `assembly_part` or `character` shape, a blended
`Mesh` (`blend`), or one of
`SkinBone`, `ParentJoint`, `Viewmodel`, `Sprite`, `ParticleEmitter`, `Text`, `Decal`, `PointLight`,
`SpotLight`, `DirectionalLight`, `GroundMarker`, `Volley`, `PatternPlayer`. The message names the
reason. The entity still draws, on the dynamic path. An entity that is posed every frame, by its
prefab root's `Animation` or under an ancestor carrying `ParentJoint` or `SkinBone`, is ineligible
too; the prefab case is detected only at runtime. A `Static` with no renderable is a transform
group: it is never reported and lets its `Static` children stay static.

Fix: remove `Static` from the entity, or remove what makes it ineligible.",
    },
    CodeEntry {
        code: code::STATIC_PARENT_NOT_STATIC,
        severity: Severity::Warning,
        title: "a static entity has a parent that is not static",
        explanation: "\
An entity carries `Static` but a `Parent` ancestor does not (pointer
`/entities/{i}/components/Static`; the message names the ancestor). A static under a moving parent would
go stale, so the entity takes the dynamic path.

Fix: mark every ancestor `Static` too, or remove `Static` from the entity.",
    },
    CodeEntry {
        code: code::STATIC_REWRITTEN,
        severity: Severity::Warning,
        title: "a static entity is rewritten every frame",
        explanation: "\
An entity carrying `Static` was written on 30 consecutive frames. Writes are legal and the entity is
re-placed, but a hot entity defeats the point of the static path. Reported once per entity.

Fix: remove `Static` from the entity.",
    },
    CodeEntry {
        code: code::PREFAB_EMPTY,
        severity: Severity::Error,
        title: "a prefab has no parts",
        explanation: "A prefab document's `entities` list is empty, so an instance of it would spawn nothing.

Fix: add a named part (`- name: root` with `components: {}`), or delete the file.",
    },
    CodeEntry {
        code: code::GENERATION_JOBS_UNKNOWN,
        severity: Severity::Error,
        title: "a generation run has no jobs the server knows",
        explanation: "\
`gen fetch` or `gen status` read a `submitted` run folder and found nothing to collect: the server
knows none of the run's jobs (every one is lost), or the report lists no job at all because the
submit was interrupted before the server accepted the first one.

The usual cause of the first is a wrong `--url` or `LUDION_COMFY_URL`, so the verb refuses and
leaves `run.json` as it was: the run stays `submitted`, and a fetch against the right server still
collects it. A server restart or a cleared history forgets its jobs for good; then the folder holds
nothing to recover. A run whose jobs are only partly lost is not this code: it finalizes `partial`
and names each lost job with `L1169`.

Fix: point `--url` at the server that took the run; otherwise delete the run folder and submit the
recipe again with `ludion gen run <recipe>`.",
    },
    CodeEntry {
        code: code::ENTITY_REF_IN_COLLECTION,
        severity: Severity::Error,
        title: "a marked entity reference inside a list or map",
        explanation: "\
A component registration marks an entity-reference field (`EntityRefSchema`) that sits inside a
list, a map or a tuple.

An entity reference is addressed by one JSON Pointer into the component's own value. The range
check at scene load, the dangling-reference check and remap at `scene.save`, and the remap when
entities carry across a scene switch all read that pointer, so none of them can follow a reference
into a collection of unknown length. The registry refuses the component rather than let such a
field go unchecked.

Fix: hold each reference in its own named field, or derive the collection from a query at run time
instead of storing it.",
    },
    CodeEntry {
        code: code::PATTERN_SYSTEM_NOT_RUN,
        severity: Severity::Warning,
        title: "a pattern system never ran",
        explanation: "\
The world holds a `PatternPlayer` naming a pattern, or a `Volley`, but the schedule never ran
`play_patterns` (so the player fires nothing) or `advance_volleys` (so spent volleys are never
reaped). Raised once per system, after a tick, in a world with game systems.

Fix: register them on the `EngineBuilder`, `advance_volleys` after the game's hit systems:

  .system(\"play_patterns\", play_patterns)
  .system(\"advance_volleys\", advance_volleys)

Known limit: a wrapper that calls `play_patterns` only behind its own guard, such as a menu state,
warns until its first call.",
    },
    CodeEntry {
        code: code::MESH_LOD_CHAIN,
        severity: Severity::Error,
        title: "LOD chain is malformed",
        explanation: "\
A mesh asset's or voxel model's `lods:` list is not a valid level-of-detail chain. A chain lists up
to three coarser levels after the base, each `- {mesh: <path>, screen_size: <number>}` on a mesh. A
voxel level is generated at load as a 2x downsample of the previous one, so it carries only
`- screen_size: <number>`.

Specifically, one of:

  - more than three entries (four levels in all);
  - a `screen_size` that is not finite, not above zero, or not strictly smaller than the previous
    level's (a level takes over when the mesh covers less than its `screen_size` of the viewport
    height, so the sizes fall as the levels get coarser);
  - a `mesh` path that does not end in `.mesh.yaml`, is absolute, or names a drive or reserved device;
  - a level document that carries its own `lods` (chains do not nest);
  - a voxel level with a key besides `screen_size`, or none.

Fix: write the levels coarsest-last with falling sizes, e.g.

  lods:
    - mesh: crate.lod1.mesh.yaml
      screen_size: 0.25
    - mesh: crate.lod2.mesh.yaml
      screen_size: 0.08",
    },
    CodeEntry {
        code: code::MESH_LOD_LEVEL_MISMATCH,
        severity: Severity::Error,
        title: "mesh LOD level does not match its base",
        explanation: "\
A level listed in a mesh's `lods:` draws through the base mesh's pipeline and bone palette, so it
must match the base: the same `alpha_mode`, `alpha_cutoff`, `double_sided` and `rim`, the same set of
texture slots, and either no skin on both or an identical `joints` list (names, parents, rest
transforms and inverse binds). The message names the level file and the first key that differs.

Fix: make the level document agree with the base on the named key (re-run `ludion import` for a
generated level), or drop the level from `lods`.",
    },
    CodeEntry {
        code: code::CAMERA_LOD_INVALID,
        severity: Severity::Error,
        title: "camera LOD knob out of range",
        explanation: "\
A `Camera`'s `lod_bias` must be a finite number in `-4..=4` (each unit halves, or at a negative
value doubles, every mesh LOD switch distance) and its `shadow_bias_levels` at most 3 (the longest
chain has four levels). The JSON Pointer names the offending field.

Fix: bring the value into range, or drop the field to keep the chain's own thresholds.",
    },
    CodeEntry {
        code: code::VOXEL_LOD_EMPTY_LEVEL,
        severity: Severity::Warning,
        title: "voxel LOD level is empty",
        explanation: "\
A `kind: voxel` document's `lods:` level is made by halving the grid again, and a coarse cell is
solid only when at least four of its eight fine cells are. A thin or sparse model can lose every
cell that way, so the level draws nothing and the model vanishes once the camera passes that
level's `screen_size`. The message names the level. It is a **warning**: the document loads and
the level draws as an empty mesh.

Fix: drop that level and the ones after it, so the chain stops before the model thins out. A model
one cell thick is better left unchained.",
    },
    CodeEntry {
        code: code::CAMERA_OFFSET_DUPLICATE,
        severity: Severity::Error,
        title: "more than one camera offset",
        explanation: "\
More than one presentation entity carries a `CameraOffset`. A scene has one camera, so one offset:
only the one on the lowest entity id is applied and the rest are ignored, so the frame still
draws. `ludion check-scene` names the extra entity.

Fix: keep one offset entity and drive it from your presentation systems.",
    },
    CodeEntry {
        code: code::CONTENT_PATH_OUTSIDE_ROOT,
        severity: Severity::Error,
        title: "a content or payload path leaves the content root",
        explanation: "\
The general rule for any path that came from content or a payload and is joined to the content
root: a component asset field, a path nested in a document, or a recorded prefab source. After its
`..` segments are folded, the path must be a plain relative path below the root. An absolute path
(`/dev/zero`, `//host/share`), a drive, scheme or stream prefix (`C:x`), a reserved device name
(`nul.png`), or a `..` that climbs above the project root is refused before any file is opened.
The project root is the nearest directory above the content root holding `ludion.toml` (the content
root itself when none does), so a scene in `scenes/` may name `../assets/town/...`, an `[[import]]`
tree beside it; a check with no root at hand (scene validation, the wire) refuses only the spellings
and leaves the climb to the read. A `..` that only cancels a segment (`meshes/../t.png`) is fine. The per-kind codes (the fonts table, a prefab
asset path, a wire outer parameter) stay for the places that check earlier.

Fix: name the file relative to the content root (the directory of the scene the engine was started
with); a document's own paths may climb with `..` only while they stay inside it.",
    },
    // --- L2xxx: render ---
    CodeEntry {
        code: code::RENDER_GRAPH_CYCLE,
        severity: Severity::Error,
        title: "render graph has a dependency cycle",
        explanation: "\
The render graph's passes form a dependency cycle, so no valid execution order exists.

A frame graph schedules passes by their data dependencies: a pass that reads a resource must run
after the pass that writes it. The scheduler derives the order by repeatedly running whichever pass
has no unscheduled dependencies left (ties broken by declaration order, so the order is
deterministic). If a group of passes all depend on each other — directly, or around a loop — none of
them can ever become ready, and that group is reported here.

The most common form is a pass that reads a resource it also writes (a self-loop), or two passes
that each sample the other's output. Fix: break the cycle. Introduce a separate resource for the
second use so the read and the write name different textures, or reorder the passes so the producer
truly precedes the consumer. A frame graph must be acyclic.",
    },
    CodeEntry {
        code: code::RENDER_GRAPH_UNWRITTEN_READ,
        severity: Severity::Error,
        title: "render pass reads a resource no pass writes",
        explanation: "\
A render pass declared a read of a virtual resource that no pass in the graph writes.

Every resource a pass samples must be produced earlier in the same graph — the scheduler has no
notion of externally supplied textures (the only external target is the graph's
output, which is written by a pass and copied out, never read). A read with no matching writer is
therefore either a missing producer pass or a mistyped resource handle: the consumer would sample
an uninitialized texture.

Fix: add the pass that writes the resource before the pass that reads it, or correct the read to
name a resource that is actually produced. The message names both the reading pass and the
unwritten resource.

The same code covers a contested read: when several passes write what a pass reads, declaration
order is the write order, so the reader must be declared after at least one of those writers.",
    },
    CodeEntry {
        code: code::RENDER_NO_ADAPTER,
        severity: Severity::Error,
        title: "no compatible GPU adapter found",
        explanation: "\
Headless rendering could not find any GPU adapter to run on.

The renderer requests an adapter with no surface (it always targets an offscreen texture — there is
never a window). It first asks for a high-performance hardware adapter, then
retries allowing a fallback software adapter, so that machines with no GPU can still render through
a software rasterizer. This diagnostic means even the fallback request found nothing.

Fix: install or update GPU drivers, or make a software rasterizer available — WARP ships with
Windows; Mesa's llvmpipe provides one on Linux. On CI runners with no GPU, ensure the software
rasterizer is installed. Commands that only inspect the graph (`ludion dump-graph`) need no adapter
and are unaffected.",
    },
    CodeEntry {
        code: code::RENDER_DEVICE_FAILED,
        severity: Severity::Error,
        title: "GPU device could not be created",
        explanation: "\
A GPU adapter was found, but a logical device and queue could not be created from it.

After selecting an adapter the renderer requests a device with the engine's pinned limit set (the
WebGPU floor plus the few raises shipped content needs) and (optionally) timestamp queries. A failure
here is unusual and typically means a driver problem or that the adapter cannot satisfy the pinned
limits. The message includes the backend's reason.

Fix: update the GPU driver. If the failure mentions limits or features, the adapter is weaker than
the renderer requires; a different adapter or a software fallback may succeed.",
    },
    CodeEntry {
        code: code::RENDER_READBACK_FAILED,
        severity: Severity::Error,
        title: "frame readback failed",
        explanation: "\
A rendered frame — or its per-pass GPU timings — could not be read back from the GPU.

Readback copies the final target (or the resolved timestamp queries) into a staging buffer, then
maps that buffer to CPU-visible memory. For `ludion render` and any lockstep caller this is a
deliberate blocking wait, because the result must correspond to an exact frame. This diagnostic
means the device poll or the buffer map did not complete successfully — almost always a lost device
or an out-of-memory condition.

Fix: retry; if it persists, the GPU driver or hardware is unstable, or the render is exhausting GPU
memory (try a smaller resolution). Note this is distinct from a PNG write failure (L2006), which is
about the file on disk, not the GPU.",
    },
    CodeEntry {
        code: code::RENDER_PNG_IO,
        severity: Severity::Error,
        title: "render output could not be written",
        explanation: "\
A rendered output file could not be produced — either the PNG could not be encoded, or a render
output file could not be written to disk.

The renderer encodes the read-back RGBA pixels as a PNG (deterministically — no timestamp chunks, so
identical pixels always produce identical bytes) and writes it to the output path. The same code
also covers writing the optional `--dump-timings` JSON sidecar. This diagnostic covers the encode
step and every filesystem write; the message includes the underlying reason.

Fix: check that the output location is writable, that the path is not a directory, and
that there is free disk space. The GPU render itself already succeeded — this failure is purely on
the CPU/disk side.",
    },
    CodeEntry {
        code: code::WINDOW_PRESENTATION_UNAVAILABLE,
        severity: Severity::Error,
        title: "windowed presentation could not start",
        explanation: "\
The native windowed viewer could not open a window to present streamed frames in.

Two presenters raise this code, for the same window-system reasons.

`ludion-window` is a peer protocol client: it connects to a running server, subscribes to the frame
stream, and paints the decoded frames into an OS window through its own small wgpu surface. Before
any frame can be shown it must build a winit event loop, create a window, and create a GPU surface
for that window. `ludion-render-graph`'s present target is the in-process counterpart: the engine
renders to its offscreen texture as always and then blits it into a window surface it owns directly.
This diagnostic means one of those window-system steps failed:

  - the OS event loop could not be built or run — there is no display / window server available
    (a headless session, or a bare CI runner), or `run` was not called on the process's main
    thread (winit requires the event loop to live on the main thread);
  - a GPU surface could not be created for the window, or reported no format the adapter can
    present;
  - an already-running surface failed permanently while acquiring an image to draw into (a lost or
    outdated swapchain is not this — it is recovered silently on the next frame).

This is distinct from L2003 (no GPU adapter) and L2004 (GPU device creation failed), which are about
the graphics device rather than the window itself.

Fix: run the windowed host or viewer from an interactive desktop session, on the main thread. On a
headless machine there is no window to open — attach the streaming viewer over the wire protocol
instead (the same frames are available to any peer client), or use `render.screenshot` for one-shot
images.

A surface that dies once the game is running ends a windowed game with this printed; a headless host
stops rendering and keeps simulating, saying why once on the `log` channel.",
    },
    CodeEntry {
        code: code::SHADER_RELOAD_DISABLED,
        severity: Severity::Error,
        title: "shader reload is not enabled on this host",
        explanation: "\
An `asset.reload` named a renderer shader (a `.wgsl` file such as `tonemap.wgsl` or `mesh.wgsl`), but
the host is not running with shader hot-reload enabled.

Shaders are compiled into the binary (`include_str!`) for every shipping, test, golden, and `ludion
render` path — those never read shader source from disk, so their output is fixed by the build. Live
shader editing is a dev-only capability, enabled explicitly so the reload overlay can never perturb a
default render: `ludion serve --dev-shaders`, a game binary run with `--dev-shaders`, or `ludion dev`
(which forwards it to the child and watches the engine's shader sources).

Fix: restart the host with the dev flag. Without it, a shader change needs a rebuild — the shader is
part of the compiled `ludion-renderer` crate.",
    },
    CodeEntry {
        code: code::SHADER_RELOAD_INVALID,
        severity: Severity::Error,
        title: "reloaded shader was rejected",
        explanation: "\
A dev-flagged shader reload could not be applied, so the previous shader stays live — a bad edit
never kills the running session; the next frame renders with the last good pipeline.

There are three causes, all carried in the message:

  - the new WGSL failed to parse or validate (naga runs the exact front-end and validator wgpu's
    `create_shader_module` runs, so the error is the one the GPU would report) — a syntax slip or a
    type error in the edit;
  - the WGSL parsed but the recreated pipeline is incompatible with the existing bind group layouts:
    the reload changed a binding shape (a uniform struct's size, or a group/binding slot), which the
    fixed, size-pinned layouts reject;
  - the shader, or a snippet it composes, is compiled once per renderer (the GI bake and inject, the
    probe relight; see `crates/ludion-renderer/src/shader_reload.rs`), so nothing was applied.

Fix: for the first cause, correct the WGSL and save again — the watcher (or an explicit
`asset.reload`) retries, and a clean edit takes effect on the next frame. A deliberate change to a
shader's *interface* (binding layout, uniform size) cannot hot-reload; rebuild the engine for that. For
the compile-once cause, rebuild the engine and restart the host, as the message says.",
    },
    CodeEntry {
        code: code::RENDER_GPU_ERROR,
        severity: Severity::Error,
        title: "the GPU rejected the renderer's work",
        explanation: "\
The GPU (through wgpu) reported a validation, out-of-memory or internal error while the renderer was
recording or submitting a frame. Instead of aborting the process, the renderer collects the error
and returns it here; the message carries the backend's own text.

This is almost always an engine bug — the renderer issued a command the pinned device limits or the
GPU's rules reject — or a resource too large for the device (for example a texture past the pinned
2D limit). The frame in flight was not produced; the next render starts clean.

Fix: attach the message when reporting it. If it names a size or limit, lower the render extent
(`render.size`, `--scale`, `--width/--height`) or the asset that exceeds it.",
    },
    CodeEntry {
        code: code::RENDER_DEVICE_LOST,
        severity: Severity::Error,
        title: "the GPU device was lost",
        explanation: "\
The GPU device the renderer was using is gone: a driver reset (Windows TDR), the machine slept, an
external GPU was removed, or the driver crashed. Every later render on that device fails with this
same code — the loss is permanent for the process, and recovery is not supported. A windowed game
ends itself with this printed; a headless host stops rendering and keeps simulating, saying why once
on the `log` channel.

Fix: restart the host (`ludion serve`, the game, or `ludion dev`). If it recurs under load, the
GPU driver or hardware is unstable, or another process is starving the GPU long enough to trip the
watchdog; close it and retry.",
    },
    CodeEntry {
        code: code::RENDER_EXTENT_INVALID,
        severity: Severity::Error,
        title: "render extent out of range",
        explanation: "\
A render extent cannot be allocated. Two bounds apply, checked on the output extent and on the
render extent (`output x scale`, the size the scene is actually drawn at):

  - every axis is at least 1 px;
  - every render axis is at most 8192 px, the WebGPU-floor 2D texture limit the engine pins its
    device to (iOS devices have exactly that limit, so the workstation GPUs do not hide it).

The message names the output extent, the scale-derived render extent and the limit.

Fix: lower `render.size`, `--scale` (`render.scale`), or `--width/--height` until `output x scale`
fits. Scale 4 on a 4K output is 15360 px and does not.",
    },
    CodeEntry {
        code: code::RENDER_MULTIPLE_CAMERAS,
        severity: Severity::Error,
        title: "scene has more than one camera",
        explanation: "\
The scene contains more than one entity with a `Camera` and no `RenderView`, so the renderer cannot
decide which one frames the shot. A `Camera` beside a `RenderView` is a secondary view, not the main camera.

Rendering needs exactly one camera. The camera's pose comes from its entity's `Transform`, and its
`projection` (perspective or orthographic) comes from the `Camera` component. With two or more
cameras there is no defined choice, so extraction fails deterministically rather than picking one
arbitrarily; the JSON Pointer names the second camera's entity.

Fix: keep a single `Camera` component in the scene. (A scene with *no* camera is fine — the renderer
falls back to a fixed default framing that looks down the -Z axis at the origin from a small
distance, so an early scene still renders something sensible.)",
    },
    CodeEntry {
        code: code::RENDER_NO_CAMERA,
        severity: Severity::Error,
        title: "render.bounds needs a camera",
        explanation: "\
A `render.bounds` query ran against a scene that has no `Camera` component.

`render.bounds` answers 'what did I draw, and where' — for every renderable entity it returns a
world-space AABB and the pixel rect that AABB projects to under the current camera. The screen rect
is the whole point of the query (it is the editor's pick primitive and an agent's text-legible way
to confirm a sprite landed where it meant), and a screen rect only exists relative to a real camera.

Unlike a *render*, which falls back to a fixed default framing when a scene declares no camera so an
early scene still produces an image, `render.bounds` refuses to invent a viewpoint: a rect projected
against a made-up camera would silently mislead. So a camera-less scene is reported here instead.

Fix: add exactly one entity with a `Camera` component (its pose comes from that entity's
`Transform`). A scene with more than one camera is the separate error L2101.",
    },
    CodeEntry {
        code: code::GI_VOLUME_CELL_BUDGET_EXCEEDED,
        severity: Severity::Error,
        title: "GiVolume cell size implies an unbuildable grid",
        explanation: "\
A scene authors a `GiVolume`, and the `cell_size` it asks for splits the scene's world bounds into a
grid the global-illumination voxelizer will not allocate. The grid becomes a 3D texture, so it is
under two separate limits and the message says which one was hit.

**Too many cells in total.** The message names the authored `cell_size`, the scene's world extent,
the cell count that combination implies, and the ceiling. The volume's origin and extent are not
authored — they come from the scene's own world bounds — so `cell_size` alone decides the count, and
the count grows as the **cube** of its reciprocal: halving `cell_size` multiplies both the grid's
memory and the cost of building it by eight. A district-sized scene at half-unit cells is a few
million cells and entirely fine; three orders of magnitude below that is a mistyped number, not a
resolution choice, and the ceiling exists to say so before the process tries to allocate tens of
gigabytes.

**Too many cells along one axis.** The message names the breaching axis, how many cells it carries,
and the limit. A grid can sit well inside the total ceiling and still be unbuildable if it is long
and thin — one entity placed a couple of thousand units from the rest is enough. The per-axis limit
is WebGPU's *guaranteed minimum* `max_texture_dimension_3d`, deliberately in place of whatever the
local GPU reports: a scene checked against the spec floor is a scene that renders on every conformant
adapter, where a scene checked against one machine's larger limit would only render on that machine.
Without this check the grid would reach texture creation and fail wgpu's own validation mid-frame,
which is a panic rather than a diagnostic.

Both checks run on the computed extent, before a single cell is allocated, so hitting either costs
nothing but the error.

The same code also covers a `cell_size` that is not a positive, finite length, and world bounds that
are not finite — in both cases there is no lattice to snap the scene to at all.

Fix: raise `GiVolume.cell_size` (the message suggests a value that fits), or shrink the scene's
world bounds — one stray entity placed thousands of units from the rest stretches the bounds, and
therefore the grid, around empty space. Removing the `GiVolume` component entirely turns the
voxelizer off; nothing else in the scene depends on it.",
    },
    CodeEntry {
        code: code::GI_VOLUME_MISSING,
        severity: Severity::Error,
        title: "scene has no GiVolume to dump",
        explanation: "\
`ludion render --dump-voxels` or `--dump-gi` was asked to render the scene's global-illumination
grid or baked probe volume as a slice sheet, but the scene authors no `GiVolume`, so extraction never
built one — there is nothing to dump.

A `GiVolume` is the voxelizer's opt-in switch (`Plan/gi-wave0-voxelizer.md`): its absence takes the
historical, voxelizer-free path deterministically, exactly like an absent `Environment` or
`PostProcess`. `--dump-voxels`/`--dump-gi` are reads of that grid and its bake, not a way to conjure
one, so a scene that never opted in reports this instead of writing an empty or all-background image
that would look like a bug in the dump rather than an absent volume.

Unlike a render-time `--set`, which can tune a field a component already has, `--set` never creates a
component (`L4008`'s own contract) — it cannot add a `GiVolume` to a scene that lacks one.

Fix: add a `GiVolume` entity to the scene (minimally, just `cell_size:`), or point `--dump-voxels`/
`--dump-gi` at a scene that already authors one; `--set /entities/<n>/components/GiVolume/cell_size=<value>`
tunes the resolution of a `GiVolume` the scene already has.",
    },
    CodeEntry {
        code: code::GI_VOLUME_PROBE_BUDGET_EXCEEDED,
        severity: Severity::Error,
        title: "GiVolume probe spacing implies an unbuildable volume",
        explanation: "\
A scene authors a `GiVolume`, and the `probe_spacing` it asks for lays an irradiance probe lattice
over the global-illumination grid's world box that the sky-visibility bake will not allocate. The
lattice becomes a 3D texture, so it is under two separate limits and the message says which one was
hit.

Probes are a lattice aligned to the grid's own origin, one probe every `probe_spacing` world units
along each axis plus a closing probe on the far face, so `probe_spacing` alone decides the count.

**Too many probes in total.** The message names the authored `probe_spacing`, the box it covers, the
probe count that combination implies, and the ceiling. The count grows as the **cube** of the
spacing's reciprocal: halving it multiplies both the volume's memory and the bake's cost by eight.
The volume is interpolated in the shader — it is a coarse lattice of irradiance samples, not a second
voxel grid — so a spacing near or below `cell_size` is asking for a resolution the format was never
meant to carry.

**Too many probes along one axis.** The message names the breaching axis, how many probes it carries,
and the limit. A lattice can sit far inside the total ceiling and still be unbuildable if the grid it
covers is long and thin. The per-axis limit is WebGPU's *guaranteed minimum*
`max_texture_dimension_3d`, deliberately in place of whatever the local GPU reports: a volume checked
against the spec floor bakes on every conformant adapter, where a volume checked against one
machine's larger limit would only bake on that machine. Without this check the lattice would reach
texture creation and fail wgpu's own validation mid-frame, which is a panic rather than a diagnostic.

Both checks run on the computed lattice, before a single probe is allocated, so hitting either costs
nothing but the error.

The same code also covers a `probe_spacing` that is not a positive, finite length — there is no
lattice to lay at all.

Fix: raise `GiVolume.probe_spacing` (the message suggests a value that fits), raise `cell_size` (the
grid's box is what the lattice covers), or shrink the scene's world bounds — one stray entity placed
thousands of units from the rest stretches the bounds, and therefore the volume, around empty space.
Removing the `GiVolume` component entirely turns global illumination off.",
    },
    CodeEntry {
        code: code::GROUND_MARKER_NON_POSITIVE_DIMENSION,
        severity: Severity::Error,
        title: "GroundMarker shape dimension is not positive",
        explanation: "\
A `GroundMarker` (or a `DropShadow`) authors a shape whose `radius`, `length`, or `width` is zero,
negative, or not a finite number; a `Decal` raises the same code for a `size` element. The message
names the field and the value; the JSON Pointer points at it.

A ground marker is drawn from an analytic signed-distance function over its footprint, so every
dimension is a real length in world units and there is no distance to evaluate against a degenerate
one. A zero or negative dimension is therefore a mis-authored marker, not an invisible one, and
saying so at check time beats shipping a telegraph that silently never appears -- which, for a
component whose whole job is warning the player about an incoming attack, is the most expensive kind
of quiet failure.

Note that the authored dimension is the marker's *maximum* extent: the entity's uniform
`Transform.scale.x` multiplies it at extract, which is how a telegraph fills in as its warmup counts
down. Author the full-size shape and animate the scale; do not author a zero-size shape and grow it.

Fix: give every dimension of the marker's shape a value greater than 0.

    GroundMarker:
      shape:
        type: disc
        radius: 3.0

    GroundMarker:
      shape:
        type: segment
        length: 8.0
        width: 2.0",
    },
    CodeEntry {
        code: code::PARTICLE_EMITTER_INVALID,
        severity: Severity::Error,
        title: "ParticleEmitter cannot be evaluated",
        explanation: "\
A `ParticleEmitter` authors numbers the analytic evaluator cannot turn into particles. The message
names the offending field and value; the JSON Pointer points at it.

Particles are a pure function of the emitter and the current sim time -- there is no pool and no
per-frame state -- so every number here is read fresh on every extracted frame, and a degenerate one
would either produce nothing or produce a set nobody can bound. The four families this code covers:

- **A negative or non-finite dimension.** `extent`, `life`, `speed`, and the scalars whose bound the
  schema cannot express are all real lengths, seconds, or rates.
- **An inverted range.** `life` and `speed` are `[min, max]` pairs drawn from uniformly; a `min`
  above its `max` names an empty interval.
- **A live population above the cap.** The live set is `rate * life[1] + burst` particles, iterated
  in full on every frame. Past the cap that is a mis-typed `rate` far more often than an intended
  effect, and a frozen frame is a worse answer than a diagnostic.
- **A degenerate emission cone or falloff** -- `spread_deg` outside `[0, 180]` and `softness`
  outside `[0, 1]` carry schema bounds, so those arrive as `L1014` at the field rather than here.

Fix: give every dimension a finite, non-negative value; order each `[min, max]` pair; and lower
`rate`, `life`, or `burst` until the live population fits the cap.

    ParticleEmitter:
      rate: 40.0
      life:
        - 0.8
        - 1.4
      speed:
        - 1.0
        - 2.5",
    },
    CodeEntry {
        code: code::VOLLEY_INVALID,
        severity: Severity::Warning,
        title: "PatternPlayer or Volley cannot be evaluated as authored",
        explanation: "\
A `PatternPlayer` or `Volley` cannot be evaluated as authored; the sim degrades once per volley
and keeps running rather than crashing.

Four cases share this code: a `PatternPlayer.pattern` names a document that is not loaded; a
`Volley.bullet` names a bullet kind its own document lacks; an aimed volley's `target` entity is
missing or dead, so it falls back to its captured absolute angle; or a `Volley` is authored with
`count` above 256, the kill bitset's width.

Fix: load the named pattern document, add the missing bullet kind, keep the aim target alive (or
accept the absolute-angle fallback), or lower `count` to 256 or less. Raised by the engine's
`play_patterns` system, which the game registers in its own schedule.",
    },
    CodeEntry {
        code: code::RENDER_PLACEMENT_NON_FINITE,
        severity: Severity::Warning,
        title: "drawable placement is not finite",
        explanation: "\
A sprite's world placement (centre or axes) or a mesh instance's model matrix is NaN or infinite,
so extract skips that drawable instead of handing the renderer NaN geometry, which draws nothing
and says nothing. The rest of the frame renders as usual.

The usual cause is a non-finite `Transform` written by game code, or one up its `Parent` chain
(`L4024` catches a hashed one). The diagnostic's pointer names the component, once per component
per session.

Fix: guard the maths that wrote the transform (a division by zero, a normalise of a zero vector,
an unclamped `sqrt`), then check the entity's `Transform` and its parents.",
    },
    CodeEntry {
        code: code::DROP_SHADOW_INVALID_DISTANCE,
        severity: Severity::Error,
        title: "DropShadow distance is invalid",
        explanation: "\
A `DropShadow` authors a `max_distance` or `fade_distance` the shadow cannot be evaluated with. The
message names the field and the value; the JSON Pointer points at it.

`max_distance` is how far below the shadow's anchor a receiving surface may be and still be darkened:
past it the shadow is gone. It must be a positive, finite number of metres. `fade_distance` is the
length of the linear fade-out ending at `max_distance` (the shadow is full strength until
`max_distance - fade_distance` below the anchor); it must be finite and not negative. It also cannot
exceed `max_distance`, because the fade would then start above the anchor. A field left out is checked
at its default (`max_distance: 20.0`, `fade_distance: 4.0`), so `fade_distance: 30` alone is refused.

Fix: give `max_distance` a value greater than 0, and `fade_distance` a value between 0 and
`max_distance`.

    DropShadow:
      max_distance: 20.0
      fade_distance: 4.0",
    },
    CodeEntry {
        code: code::GI_VOLUME_EMPTY,
        severity: Severity::Error,
        title: "GiVolume covers no voxelizable geometry",
        explanation: "\
`ludion render --dump-voxels` or `--dump-gi` was asked to dump the scene's global-illumination grid
or baked probe volume, and the scene does author a `GiVolume` — but nothing placed in it voxelizes,
so extraction built no grid. The message counts what was skipped by reason and the help points at the
first entity of each.

What voxelizes: cubes, planes, spheres, voxel models and unskinned asset meshes. What is skipped:
a mesh carrying `GiDynamic` (its light is injected per frame, so it never enters the static grid);
skinned meshes and characters (their shape moves with the pose); an instance whose placement is not
finite; and sculpts, assemblies and asset meshes with no triangles, which are not voxelized. When the
message says the scene places no mesh, there is simply no geometry to fill the grid.

This differs from `L2104`, which means the scene authors no `GiVolume` at all. Here the opt-in is
present and the content is the problem.

Fix: place static geometry inside the scene, or drop `GiDynamic` from a mesh that should occlude. A
skinned mesh never voxelizes; give the scene static stand-in geometry if it should shape the bounce.",
    },
    CodeEntry {
        code: code::DECAL_INVALID_ANGLE_FADE,
        severity: Severity::Error,
        title: "Decal angle fade is invalid",
        explanation: "\
A `Decal` authors an `angle_fade` the projection cannot be faded with. The message names the value;
the JSON Pointer points at the element, or at `/angle_fade` when the pair is out of order.

θ is the angle, in degrees, between the receiving surface's normal and the decal box's local +Y (the
direction back towards the projector). The decal's weight is 1 at or below `start` and 0 at or above
`end`, fading between, so a decal does not smear across surfaces that face away from it. Both values
must be finite and within `[0, 90]`: past 90 degrees a surface faces away and is never painted, so a
larger bound means nothing. `start` must not exceed `end`, or the fade would run backwards.

Fix: give `angle_fade` two finite angles with `0 <= start <= end <= 90`.

    Decal:
      angle_fade: [60.0, 80.0]",
    },
    CodeEntry {
        code: code::GROUND_MARKER_WEDGE_INVALID,
        severity: Severity::Error,
        title: "GroundMarker wedge is not drawable",
        explanation: "A `GroundMarker` authors a `type: wedge` shape that cannot be drawn: `half_angle_deg` is outside
`(0, 180]`, `inner_radius` is negative or not below `outer_radius`, or `fill` is outside `[0, 1]`.
A `DropShadow` raises the same code when it authors `type: wedge`, because a drop shadow is a disc
or a segment. The message names the field and the value; the JSON Pointer points at it.

Fix: give the wedge a half-angle in `(0, 180]`, an `inner_radius` in `[0, outer_radius)` and a `fill`
in `[0, 1]`; use a `disc` or `segment` shape on a `DropShadow`.

    GroundMarker:
      shape: { type: wedge, half_angle_deg: 30.0, outer_radius: 7.0, inner_radius: 3.0, fill: 0.5 }",
    },
    CodeEntry {
        code: code::GROUND_MARKER_RING_INVALID,
        severity: Severity::Error,
        title: "GroundMarker ring is not drawable",
        explanation: "A `GroundMarker` authors a `type: ring` shape that cannot be drawn: `thickness` is greater
than `radius` (the band would pass through the centre), or `fill` is outside `[0, 1]`. A `DropShadow`
raises the same code when it authors `type: ring`, because a drop shadow is a disc or a segment. A
wedge's optional `inner_half_angle_deg` outside `(0, half_angle_deg]` is `L2113`, not this code.
The message names the field and the value; the JSON Pointer points at it.

Fix: give the ring a `thickness` in `(0, radius]` and a `fill` in `[0, 1]`; use a `disc` or `segment`
shape on a `DropShadow`.

    GroundMarker:
      shape: { type: ring, radius: 3.0, thickness: 0.2, dashes: 12 }",
    },
    CodeEntry {
        code: code::GROUND_MARKER_STYLE_INVALID,
        severity: Severity::Error,
        title: "GroundMarker style is not valid",
        explanation: "A `GroundMarker.style` field is out of range: `outline_px` or `grid_px` is not positive and
finite, or one of `grid_alpha`, `interior_alpha`, `inner_alpha`, `fill_alpha` or `flicker` is
outside `[0, 1]`. The message names the field and the value; the JSON Pointer is `/style/<field>`.

Fix: give the pixel widths a value above 0 and every alpha and `flicker` a value in `[0, 1]`; omit the
field to take its default.

    GroundMarker:
      style: { outline_px: 2.0, flicker: 0.5 }",
    },
    CodeEntry {
        code: code::GROUND_MARKER_CLIP_INVALID,
        severity: Severity::Error,
        title: "GroundMarkerClip is not valid",
        explanation: "A `GroundMarkerClip` field is out of range: `bins` is outside `[8, 512]`, `height` is not
finite, or `partial_alpha` is outside `[0, 1]` (JSON Pointer `/bins`, `/height`, `/partial_alpha`). Scene
check also raises it for a layer name the `physics.layers` table does not declare, for a clip on a
`segment` marker (a segment has no apex to cast from), and for a clip on an entity with no
`GroundMarker`.

Fix: give `bins` a value in `[8, 512]`, `partial_alpha` a value in `[0, 1]`, name only declared layers,
and put the clip beside a `disc`, `ring` or `wedge` `GroundMarker`.

    GroundMarkerClip:
      bins: 128
      layers: [sight_blocker]
      partial_layers: [low_cover]",
    },
    CodeEntry {
        code: code::GROUND_MARKER_CLIP_NO_PHYSICS,
        severity: Severity::Warning,
        title: "GroundMarkerClip has no physics to cast through",
        explanation: "A `GroundMarkerClip` is authored but the world has no physics, so there is nothing to
cast the visibility profile through. The marker is drawn unclipped.

Fix: declare a `physics:` block (with the layers the clip names) in the scene, or remove the
`GroundMarkerClip`.

    physics:
      layers: [sight_blocker, low_cover]",
    },
    CodeEntry {
        code: code::OCCLUDED_SILHOUETTE_INVALID,
        severity: Severity::Error,
        title: "OccludedSilhouette is not valid",
        explanation: "An `OccludedSilhouette` field is out of range: a `color` channel is outside `[0, 1]`,
`outline.px` is outside `(0, 8]`, `outline.alpha` is outside `[0, 1]`, or `bias` is negative or not
finite (JSON Pointer `/color/<i>`, `/outline/px`, `/outline/alpha`, `/bias`).

Fix: give each field a value in its range.

    OccludedSilhouette:
      color: [1, 1, 1, 0.55]
      outline: { px: 2, alpha: 1 }",
    },
    CodeEntry {
        code: code::OCCLUDED_SILHOUETTE_ON_VIEWMODEL,
        severity: Severity::Warning,
        title: "OccludedSilhouette on a Viewmodel mesh is ignored",
        explanation: "An `OccludedSilhouette` resolves onto a mesh that carries a `Viewmodel`, directly or by
inheritance. A viewmodel is drawn on its own near depth slice and is never in the silhouette mask, so
nothing is drawn for it. Raised once per entity by extract.

Fix: remove the `OccludedSilhouette` from the viewmodel's entity or its ancestors, or put it on a
sibling that is not a viewmodel.",
    },
    CodeEntry {
        code: code::OCCLUDED_SILHOUETTE_STYLE_LIMIT,
        severity: Severity::Warning,
        title: "more than 8 silhouette styles in one frame",
        explanation: "A frame resolves more than 8 distinct `OccludedSilhouette` styles (a style is the exact
colour, outline, `when` and `bias`). The first 8 in scene order are drawn and meshes with a ninth or later
style get no silhouette. Raised once by extract.

Fix: share styles between entities, for example one on a common `Parent`, so that a frame holds at most 8.",
    },
    CodeEntry {
        code: code::MESH_BLEND_ON_VIEWMODEL,
        severity: Severity::Warning,
        title: "Mesh.blend on a Viewmodel mesh is ignored",
        explanation: "A `Mesh` with `blend` is also a `Viewmodel` (directly or by inheritance). A viewmodel is
drawn on its own near depth slice and is never blended, so the mesh draws opaque. Raised once per entity
by extract.

Fix: remove `blend` from the viewmodel's mesh, or move the blended mesh onto an entity that is not a
viewmodel.",
    },
    CodeEntry {
        code: code::RENDER_VIEW_INVALID,
        severity: Severity::Error,
        title: "RenderView is not valid",
        explanation: "A `RenderView` has an empty `name`, a `size` axis outside `16..=2048`, or sits on an entity without a
`Camera` (JSON Pointer `/name`, `/size/<i>`, or the entity's components). A secondary view renders through the
camera on its own entity.

Fix: name the view, keep each size axis in `16..=2048`, and put the `RenderView` beside a `Camera`.

    RenderView: { name: radar, size: [256, 256] }",
    },
    CodeEntry {
        code: code::RENDER_VIEW_NAME_DUPLICATE,
        severity: Severity::Error,
        title: "two RenderViews share a name",
        explanation: "Two `RenderView` components carry the same `name`, so a `UiRect.view` cannot say which one it shows
(JSON Pointer `/name` of the second).

Fix: give every view its own name.",
    },
    CodeEntry {
        code: code::UI_RECT_VIEW_UNRESOLVED,
        severity: Severity::Error,
        title: "UiRect.view names no RenderView",
        explanation: "A `UiRect` has a `view` that no `RenderView` carries (JSON Pointer `/view`). The rect draws nothing.

Fix: spell the name as the `RenderView` does, or add the view.",
    },
    CodeEntry {
        code: code::UI_RECT_VIEW_CONFLICT,
        severity: Severity::Error,
        title: "UiRect.view beside a texture source",
        explanation: "A `UiRect` sets `view` together with `texture`, `atlas`, `frame`, `slice` or `uv_rect`. A view is the
rect's whole image (JSON Pointer `/view`).

Fix: keep `view` and remove the other texture fields, or drop `view`.",
    },
    CodeEntry {
        code: code::RENDER_VIEW_BUDGET,
        severity: Severity::Warning,
        title: "more than 4 enabled RenderViews",
        explanation: "More than 4 enabled `RenderView`s are in the scene. The first 4 in `(order, entity)` order render;
each further one is skipped and a `UiRect` that shows it draws nothing. Raised once per skipped view by extract.

Fix: disable views that are not on screen (`enabled: false`) or merge them.",
    },
    CodeEntry {
        code: code::RENDER_LAYERS_INVALID,
        severity: Severity::Error,
        title: "render layers are not valid",
        explanation: "A `RenderLayers` list, `Camera.layers` or `RenderView.layers` is empty, holds an empty name, or repeats a
name (JSON Pointer `/layers` or `/layers/<i>`). Extract raises it too when a scene uses more than 31 layer
names besides `default`; the extra names are ignored.

The scene's top-level `render_layers:` block raises it too, at `/render_layers/<i>/name`, for an empty name, the
reserved name `default`, a repeated name, or more than 31 entries (`/render_layers`).

Fix: list each layer name once, at least one per list, and keep the scene to 31 names besides `default`.

    RenderLayers: { layers: [radar] }",
    },
    CodeEntry {
        code: code::RENDER_LAYER_UNKNOWN,
        severity: Severity::Error,
        title: "render layer is not declared",
        explanation: "A layer name in `RenderLayers`, `Camera.layers`, `Camera.lights`, `RenderView.layers` or
`RenderView.lights` is not listed in the scene's top-level `render_layers:` block (JSON Pointer
`/entities/<i>/components/<Component>/<field>/<j>`). `check-scene` and `scene.check` report it, with the nearest
declared name when one is close; load and extract do not, so a name inserted at runtime still renders. `default` is
implicit and needs no declaration.

Fix: declare the layer once at the top of the scene, or correct the spelling.

    render_layers:
      - name: radar",
    },
    CodeEntry {
        code: code::RAIN_INVALID,
        severity: Severity::Error,
        title: "Rain field out of range",
        explanation: "A `Rain` field is non-finite or outside its range (the JSON Pointer names the field). `intensity`,
`gust`, `medium` and each `color` channel take [0, 1]; `density` (0, 16]; `radius` [2, 64]; `cell` [1, 8];
`height` is `[low, high]` with low < high and a span of at most 64; `fall_speed` is `[min, max]` with
0 < min <= max <= 40; `wind.y` must be 0 and |x|, |z| at most 20; `drop_width` (0, 0.05]; `streak` [0, 8].

Fix: bring the field into range; `ludion docs weather` lists every field.",
    },
    CodeEntry {
        code: code::RAIN_DUPLICATE,
        severity: Severity::Warning,
        title: "more than one Rain",
        explanation: "More than one entity carries a `Rain`. Rain is a singleton: the lowest entity id is used and each
further one is ignored. Raised by extract and by check-scene.

Fix: keep one `Rain` entity.",
    },
    CodeEntry {
        code: code::RAIN_STREAK_BUDGET,
        severity: Severity::Warning,
        title: "Rain exceeds the streak budget",
        explanation: "`density * intensity * cell^3` streaks per tile, over the tiles around the camera, exceeds the
65,536-streak ceiling. The per-tile count is clamped so the total fits; the rain is thinner than authored. Raised once
per load by extract and by check-scene.

Fix: lower `density`, `radius` or the `height` span.",
    },
    CodeEntry {
        code: code::CUBE_LUT_MALFORMED,
        severity: Severity::Error,
        title: "malformed .cube LUT",
        explanation: "A `.cube` LUT file does not parse; the message names the line. The accepted grammar is: empty
lines and `#` comments; header keywords before the first data row (`TITLE \"name\"`, `LUT_3D_SIZE n`,
`DOMAIN_MIN r g b`, `DOMAIN_MAX r g b`, `LUT_3D_INPUT_RANGE lo hi`); then exactly n^3 rows of three finite floats,
red varying fastest. Also raised for an unknown keyword, a keyword after the first data row, a missing
`LUT_3D_SIZE`, a domain whose max is not above its min, or a file that is not UTF-8.

Fix: re-export the LUT as a 3D `.cube`, or write one with `ludion lut identity`.",
    },
    CodeEntry {
        code: code::CUBE_LUT_UNSUPPORTED,
        severity: Severity::Error,
        title: "unsupported .cube LUT",
        explanation: "The `.cube` file is well formed but the engine cannot use it: it is a 1D table (`LUT_1D_SIZE`) or its
`LUT_3D_SIZE` is not 17, 33 or 65.

Fix: re-export the LUT as a 3D table of size 17, 33 or 65.",
    },
    CodeEntry {
        code: code::POST_PROCESS_INVALID_METERING,
        severity: Severity::Error,
        title: "invalid metered-exposure setting",
        explanation: "A `PostProcess` metering field is out of range. `metering_clip` is `[low, high]`, the share of
the metered pixels (by weight) trimmed off the dark and bright ends before the mean: each element must be in `[0, 1)`
and the two must sum to less than 1. `exposure_range` is `[min, max]` in EV, each within `[-8, 8]`, with min not
above max.

Fix: keep each clip share below 1 and the pair's sum below 1 (the default is `[0.02, 0.02]`), and give
`exposure_range` as `[min, max]` inside `[-8, 8]`.",
    },
    CodeEntry {
        code: code::SCREEN_EFFECT_INVALID_TARGET,
        severity: Severity::Error,
        title: "invalid screen-effect region",
        explanation: "A `ScreenEffect` with a `region` target has a size element that is zero, negative or not
finite, or an offset element that is not finite. The region is laid out in the UI's canvas units like a `UiRect`:
`anchor` picks the corner or edge, `offset` moves it, `size` is `[width, height]` and must be positive. An effect
with an unusable region is not applied.

Fix: give `size` two positive numbers and `offset` two finite numbers, or use `target: fill`.",
    },
    CodeEntry {
        code: code::SCREEN_EFFECT_CAP,
        severity: Severity::Warning,
        title: "too many screen effects",
        explanation: "More than four `ScreenEffect`s would apply to the frame. At most four are applied, lowest
`order` first and then lowest entity id; the rest are not drawn. An effect with every stage off (for example
`preset: none` and no override) is not applied and is not counted. Raised once per load by extract and by
check-scene.

Fix: remove an effect, merge two into one region, or leave the extra one's stages off.",
    },
    CodeEntry {
        code: code::RENDER_GOLDEN_MISSING,
        severity: Severity::Error,
        title: "golden image is missing",
        explanation: "\
A golden-test comparison was requested, but no golden image exists yet at the expected path (or it
could not be read from disk).

`ludion test` compares a fresh render against a committed golden PNG (`<scene-stem>.png` in the
scene's `goldens/` directory). The very first time a scene gets a golden test, there is nothing to
compare against yet — this is expected, not a bug — and the same message covers the rarer case of a
golden path that exists but cannot be opened (permissions, or the path names a directory).

Fix: run `ludion test --bless` to render and commit a golden for this scene (review the written PNG
before committing it — blessing is not automatic approval of correctness).",
    },
    CodeEntry {
        code: code::RENDER_IMAGE_DIMENSION_MISMATCH,
        severity: Severity::Error,
        title: "rendered image size does not match its golden",
        explanation: "\
A freshly rendered image's width/height do not match its committed golden's width/height, so no
pixel-wise SSIM comparison is possible — the two images do not even have the same number of pixels.

This usually means the golden test's resolution changed (a different `--width`/`--height`, or a
different default) since the golden was last blessed, not a rendering regression.

Fix: re-bless the golden at the intended resolution (`ludion test --bless`) if the resolution change
was deliberate, or fix the test's configured width/height back to what the golden was blessed at.",
    },
    CodeEntry {
        code: code::RENDER_SSIM_BELOW_THRESHOLD,
        severity: Severity::Error,
        title: "golden comparison scored below the SSIM threshold",
        explanation: "\
A golden-test comparison's global SSIM (structural similarity) score fell below the configured
threshold, or one tile fell below the tile floor, so the render is considered to have regressed.

The harness writes a structured diff report (`<scene-stem>.diff.json`, next to a `<scene-stem>
.actual.png`) that localizes the damage: the image is divided into a tile grid, each tile's score is
that of its worst sliding SSIM window, and the report lists every tile below threshold plus the
single worst tile — with
pixel-rect coordinates and a coarse region name (e.g. \"top-left\") — so the *where* is readable
without opening an image viewer.

Fix: open the report and the `.actual.png`/golden pair for the named region. If the change is an
intentional visual update, re-bless (`ludion test --bless`) after reviewing it; otherwise it is a
rendering regression to fix.",
    },
    CodeEntry {
        code: code::RENDER_GOLDEN_PNG_DECODE,
        severity: Severity::Error,
        title: "golden image is not a valid PNG",
        explanation: "\
A committed golden image file was read from disk, but its bytes are not a decodable PNG.

Golden images are always written by `ludion test --bless` through the engine's own deterministic PNG
encoder, so this usually means the file was corrupted (a bad merge, an LFS/checkout problem, or
manual editing) rather than never having been a valid golden.

Fix: re-bless the golden (`ludion test --bless`) to write a fresh, valid PNG.",
    },
    CodeEntry {
        code: code::RENDER_GOLDEN_PLATFORM_UNPINNED,
        severity: Severity::Error,
        title: "this platform has no pinned golden tier",
        explanation: "\
A golden test ran on an operating system that has no pinned golden tier, so there is no committed
golden set to compare against (and blessing one here would gate every other machine on this
machine's GPU).

Bit-exact image goldens only hold on a pinned (OS, GPU) pair, so each pinned platform gets its own
blessed set in a per-OS subdirectory of `goldens/`: `goldens/windows/<stem>.png` (pinned RTX 3090)
and `goldens/macos/<stem>.png` (pinned Apple M4). Tests resolve the subdirectory from the host OS
at runtime; an OS outside the pinned matrix lands here instead of silently comparing against
another platform's pixels.

Fix: run golden tests on one of the pinned platforms (Windows or macOS). If a new platform is
joining the pinned matrix, bless its set on that platform's pinned GPU — `ludion test --bless` for
directory suites, `LUDION_BLESS=1 cargo test` for a game project — into `goldens/<os>/`, review the
images, and commit them alongside the existing per-OS sets.",
    },
    CodeEntry {
        code: code::RENDER_GOLDEN_BLESS_ADAPTER_UNPINNED,
        severity: Severity::Error,
        title: "refusing to bless a pinned golden set on a non-reference GPU",
        explanation: "\
A bless was asked to write a golden into a per-OS pinned directory (`goldens/windows/` or
`goldens/macos/`), but the GPU that rendered it is not that platform's pinned reference adapter
(RTX 3090 on Windows, Apple M4 on macOS).

Bit-exact goldens only hold on their pinned (OS, GPU) pair. Blessing from another GPU — a laptop, a
different desktop card — would overwrite the committed set with pixels the reference box then fails
against, or worse, would make every other machine gate on this one's GPU. The refusal is the backstop
for a stray `LUDION_BLESS` in a shell session.

Fix: bless on the pinned reference machine. A game project whose pinned GPU is its own card sets
`LUDION_BLESS_ANY_ADAPTER=1` beside `LUDION_BLESS=1` to opt out; review the images and commit them.",
    },
    CodeEntry {
        code: code::RENDER_GOLDEN_COLOR_ABOVE_CEILING,
        severity: Severity::Error,
        title: "golden comparison's colour error exceeded the ceiling",
        explanation: "\
A golden-test comparison passed its structural (luma SSIM) gates, but one tile's colour error — the
mean absolute difference per R, G or B channel over that tile, 0–255, the worst channel — exceeded
the colour ceiling. SSIM is computed on luma only, so a change that keeps brightness structure but
moves hue (a grade, a palette or tint change, swapped channel order, a fog or sky colour change)
is invisible to it; this gate catches it.

The diff report names the tile: `worst_color_tile` is the tile with the highest colour error, and
`damaged_tiles` lists every tile over the ceiling alongside the below-threshold ones, with pixel
rects and a coarse region name. The ceiling itself is the report's `color_ceiling`.

Fix: open the `.actual.png`/golden pair for the named region and compare the colours. If the change
is an intentional visual update, re-bless (`ludion test --bless`) after reviewing it; otherwise it is
a rendering regression to fix.",
    },
    CodeEntry {
        code: code::GOLDEN_MANIFEST_INVALID,
        severity: Severity::Error,
        title: "golden manifest is invalid",
        explanation: "\
A golden suite's `goldens/manifest.json` is not valid JSON or does not match the manifest shape.

The manifest sits at the `goldens/` root and maps a scene stem to optional overrides; every field
(`width`, `height`, `ssim_threshold`, `frames`) is optional:

  { \"breakout-mock\": { \"ssim_threshold\": 0.97 }, \"sprite-flipbook\": { \"frames\": 23 } }

The manifest is strict: a field outside those four is refused, and so is a key naming no scene in
the suite (a misspelled stem would otherwise silently leave its scene at the defaults). The check
runs before `--filter`, so a filtered run still catches it.

Fix: correct the JSON or the stem, or delete the file — with no manifest every scene uses the suite
defaults.",
    },
    CodeEntry {
        code: code::TEST_CHECKED_NOTHING,
        severity: Severity::Error,
        title: "`ludion test` checked no scene",
        explanation: "\
`ludion test` renders every top-level `*.yaml`/`*.yml` scene in `--dir` and compares it to its
golden, and none was selected: either `--filter <substr>` matched no file stem, or the directory
holds no scene file. A run that checked nothing must not read as a pass: a typo in the filter or a
wrong directory would otherwise turn a CI gate green without rendering a single scene.

The help lists the stems the directory does hold. A game project's own golden runs through
`cargo test` (`tests/golden.rs`), not `ludion test`.

Fix: correct the filter, or point `--dir` at the directory holding the suite's scenes.",
    },
    CodeEntry {
        code: code::RENDER_GOLDEN_COMPARE_ADAPTER_UNPINNED,
        severity: Severity::Error,
        title: "skipping a bit-exact golden comparison on a non-reference GPU",
        explanation: "\
A byte-exact comparison (SSIM threshold 1.0) against a committed per-OS golden set
(`goldens/windows/` or `goldens/macos/`) ran on a GPU that is not that platform's pinned reference
adapter (RTX 3090 on Windows, Apple M4 on macOS), so the test skips instead of failing.

Bit-exact goldens only hold on their pinned (OS, GPU) pair: another card rasterises the same scene
to different bytes, and a red run there says nothing about the code. Suites that compare at
SSIM 0.99 still run on any GPU.

Fix: run the bit-exact suites on the reference machine. A project whose pinned GPU is its own card
sets `LUDION_BLESS_ANY_ADAPTER=1` to compare anyway.",
    },
    // --- L3xxx: protocol / server ---
    CodeEntry {
        code: code::PROTOCOL_MALFORMED_ENVELOPE,
        severity: Severity::Error,
        title: "malformed protocol envelope",
        explanation: "\
A protocol request could not be parsed as a well-formed envelope.

Every request is a JSON object of the form:

  {\"v\": 1, \"id\": <number>, \"method\": \"<dotted name>\", \"params\": {…}}

`v` is the protocol version (see L3003); `id` is a client-chosen correlation number echoed in the
response; `method` is one of the defined dotted names (see L3002); `params` is the method's argument
object — omitted entirely for parameterless methods. This diagnostic covers the structural failures:
the payload is not a JSON object, the `v` or `method` field is missing, or the `params` object does
not match the named method's shape (a wrong field type, a missing required field). Unknown fields
and unknown envelope keys are refused too, never ignored. The message includes the underlying parse
error, and the JSON Pointer — when present — names the offending part.

Fix: correct the envelope to the shape above. If the failure is inside `params`, compare it against
the method's schema in the exported protocol schema (or `engine.describe`).",
    },
    CodeEntry {
        code: code::PROTOCOL_UNKNOWN_METHOD,
        severity: Severity::Error,
        title: "unknown protocol method",
        explanation: "\
A request named a `method` that this protocol version does not define.

The method is a valid string but is not in the fixed set of dotted method names the engine exposes
(for example `entity.spawn`, `sim.step`, `render.screenshot`). This usually means a typo — the
diagnostic suggests the nearest known method when one is close — or a client built against a newer or
older protocol version than the engine is running.

Fix: correct the method name, or regenerate the client from this engine's exported schema. The full
method list is in the protocol schema and in the `engine.describe` response; a version mismatch is
also flagged at connect time by the handshake's schema hash.",
    },
    CodeEntry {
        code: code::PROTOCOL_UNSUPPORTED_VERSION,
        severity: Severity::Error,
        title: "unsupported protocol version",
        explanation: "\
A request envelope declared a protocol version this engine build does not speak.

This engine speaks exactly one protocol version. A different `v` means the client was generated
against another version of the schema; protocol versions are breaking by design — there is no
best-effort cross-version translation — so the request is rejected up front rather than being
misinterpreted.

Fix: regenerate the client from this engine's exported schema (the handshake announces the engine's
protocol version and schema hash, so drift is detectable at connect time), or run the engine build
the client was generated against.",
    },
    CodeEntry {
        code: code::PROTOCOL_MALFORMED_FRAME,
        severity: Severity::Error,
        title: "malformed binary frame message",
        explanation: "\
A binary frame-streaming message could not be decoded.

Frames on the `frames` subscription are binary WebSocket messages: a fixed 36-byte little-endian
header — the magic `LFR1`, a format byte, the subscription id, the frame index, width, height, and a
payload length — followed by exactly that many encoded pixel bytes. This diagnostic means the bytes
are shorter than a header, do not begin with the magic, name a pixel format the reader does not know
(the valid formats are QOI, LZ4_RGBA and RAW_RGBA), or carry a payload length that disagrees with
the bytes that follow, or declare a width or height past the largest extent a host renders.

Fix: this indicates a framing bug or a corrupted/misrouted message, not user input — the sender and
receiver disagree on the frame layout. Ensure both use this protocol version's frame encoding.",
    },
    CodeEntry {
        code: code::PROTOCOL_INVALID_INPUT,
        severity: Severity::Error,
        title: "invalid input event",
        explanation: "\
An `input.inject` command carried an `event` value that could not be interpreted as a sim input
event.

The engine deserializes the opaque `event` JSON in an `input.inject` request into the simulation's
input-event type — a key, mouse or gamepad event, for example:

  {\"kind\": \"key_down\", \"key\": \"W\"}
  {\"kind\": \"gamepad_axis\", \"axis\": \"left_stick_x\", \"value_bits\": 1056964608}

This diagnostic means the value was not one of those shapes: a missing or unknown `kind`, a missing
`key`, or a key name the engine does not define. The message includes the underlying serde error.

Analog values travel as IEEE-754 bit patterns in `value_bits`, `position_bits` and `delta_bits`.
They must decode to finite numbers; a stick axis must lie in [-1, 1] and a trigger in [0, 1]
(`1.0` is `1065353216`). Anything else is refused here rather than clamped.

The same code refuses an `input.inject` whose `frame` the sim has already run — the input could
not take effect there, and clamping it to now would acknowledge a frame that never saw it. Nothing
is recorded.

Fix: match the input-event shape above. The valid key names are the variants of the engine's `Key`
type (letters `A`–`Z`, digits `Digit0`–`Digit9`, `Up`/`Down`/`Left`/`Right`, `Space`, `Enter`,
`Escape`); check the spelling and that `kind` is `key_down` or `key_up`. For a past `frame`, omit
`frame` to apply at the next tick, or address the current frame or later.",
    },
    CodeEntry {
        code: code::PROTOCOL_COMPONENT_ABSENT,
        severity: Severity::Error,
        title: "entity does not have the requested component",
        explanation: "\
A `component.get` or `component.remove` named a live entity and a registered component, but the
entity does not currently carry that component.

`component.get` reads one component value off one entity; `component.remove` deletes one. Either way
the entity is alive and the component name is valid (an unknown name is L0004, a dead entity is
L0002) — the entity simply has no value of that component right now. Returning `null` from
`component.get` would be ambiguous with a component whose value is literally null, so the absence is
reported explicitly instead.

Fix: query the entity's components first (`query.entities` with the entity's filter, or read the
help text — the diagnostic lists the components the entity does have), or set the component with
`component.set` before reading or removing it.",
    },
    CodeEntry {
        code: code::PROTOCOL_SPAWN_NAME_CONFLICT,
        severity: Severity::Error,
        title: "entity.spawn named an entity two ways",
        explanation: "\
An `entity.spawn` supplied both a top-level `name` and a `Name` entry in its `components` map.

`Name` is a field-backed component with exactly one authoring channel, so an entity's name is given
one way, not two. `entity.spawn` accepts a convenience `name` field that becomes the entity's `Name`
component; the same component may instead be supplied explicitly inside `components`. Supplying both
is ambiguous — the two could disagree — so the spawn is rejected up front, before any entity is
created (no half-built entity is left behind).

Fix: pass the name exactly one way — either the top-level `name` field, or a `Name` entry in
`components`, but not both.",
    },
    CodeEntry {
        code: code::PROTOCOL_UNKNOWN_SUBSCRIPTION,
        severity: Severity::Error,
        title: "unknown subscription id",
        explanation: "\
A `stream.unsubscribe` named a subscription id that is not currently open.

Subscription ids are minted by `stream.subscribe` and freed by `stream.unsubscribe`. This diagnostic
means the id was never issued, or was already unsubscribed — a double-free or a stale id. It is
reported rather than silently ignored so a client that has lost track of its subscriptions finds out.

A connection may only close subscriptions it opened: an id that is open but belongs to another
connection is refused with this same code, and the owner's subscription is untouched.

Fix: unsubscribe each id exactly once, using the `sub` value returned by the matching
`stream.subscribe`.",
    },
    CodeEntry {
        code: code::PROTOCOL_SNAPSHOT_INVALID,
        severity: Severity::Error,
        title: "invalid engine snapshot",
        explanation: "\
An `engine.restore` snapshot document could not be applied.

`engine.restore` takes a document produced by `engine.snapshot`: a versioned object carrying the
serialized world (entities + components), the RNG state, the frame index, and the scene reference and
seed. This diagnostic means the document is not a well-formed snapshot — a missing or unrecognized
`format`, a missing field, or a component/world value that does not deserialize — or that it was
written by an incompatible engine build. The message names the specific problem.

Fix: restore from a snapshot produced by this engine build's `engine.snapshot`. Snapshots are not a
long-term storage format across engine versions; regenerate the snapshot if the engine has changed.",
    },
    CodeEntry {
        code: code::PROTOCOL_ENGINE_UNAVAILABLE,
        severity: Severity::Error,
        title: "engine is unavailable",
        explanation: "\
The engine could not answer, because the sim thread that owns the world is not reachable.

The server bridges each WebSocket request to the single sim thread over a command queue and waits
for a one-shot reply. This diagnostic covers every way that bridge can be absent: on the server, the
command queue disconnected or the sim thread ended before replying — in practice, the server is
shutting down (or has already shut down) while a request was in flight; on the client, the server
could not be reached at all, or the connection dropped mid-request. It is never caused by anything
in the request itself; the same request against a running, reachable server is dispatched normally.

A call that merely waited too long is not this code: a client whose per-call timeout elapsed raises
L3054, because the connection is still up and the command may still run.

Fix: ensure the server is running and reachable at the address you connected to, then reconnect. If
the server crashed rather than shut down cleanly, inspect its logs — a panic on the sim thread would
end it and surface here for any request that raced the teardown.",
    },
    CodeEntry {
        code: code::REPLAY_FORMAT,
        severity: Severity::Error,
        title: "malformed replay session",
        explanation: "\
A replay session file is not well-formed.

A replay is the magic `LRP1`, a `u16` format version, a length-prefixed compressed header, then a
sequence of `{kind, frame, body}` records — each body an independent zstd frame. This diagnostic
covers the structural failures: the wrong magic, a format version this engine does not read, a file
truncated inside a header or record, an unknown record kind, or a header/record body that is not
valid JSON once decompressed. (A zstd frame that fails to *inflate* is reported separately as L3011.)

The same code refuses a replay recorded against a different protocol version. Its header also
records a description-stripped wire schema hash and a replay content hash (the record schema, the
input events and the snapshot document); doc-comment edits move neither, a new field or command
moves them. A header without a content hash predates it — that is a format age, not a determinism
failure.

Fix: re-record the session with a compatible engine build (`LUDION_BLESS=1` on the owning test, or
`--record`). A version mismatch means the replay was written by a different engine version; replay
formats, like the protocol, evolve by explicit, breaking version bumps.",
    },
    CodeEntry {
        code: code::REPLAY_COMPRESSION,
        severity: Severity::Error,
        title: "replay decompression failed",
        explanation: "\
A zstd frame inside a replay session could not be decompressed.

Each replay header and record body is stored as an independent zstd frame. This diagnostic means one
of those frames is not valid zstd data — the file is corrupt (a bad copy, a truncated write, a
merge that mangled binary content) or was produced by an incompatible tool. It is distinct from
L3010, which is about the *structure* around the frames; here the framing was found but the
compressed bytes themselves are bad.

Fix: re-record the session. If the file was stored in version control, ensure it was treated as
binary (not text-normalized), which can corrupt zstd frames.",
    },
    CodeEntry {
        code: code::REPLAY_SCENE_UNRESOLVED,
        severity: Severity::Error,
        title: "replay scene reference could not be resolved",
        explanation: "\
A replay header names a scene by *path*, and that path could not be loaded while replaying.

A replay reconstructs its starting world from the header's scene reference. That reference is one of
two shapes: an inline document (the full scene text, carried in the replay itself) or a path (a scene
file the replayer must find on disk, resolved relative to the replay's project root — the replay
file's own directory). Recordings this engine makes always embed the scene inline, so they are
self-contained and portable: move or copy the replay anywhere and it still reproduces the same
`WorldHash` sequence. This diagnostic means the replay carried a *path* instead, and the file it
names is not present (or not readable) beside the replay — typically a hand-authored replay, or one
produced by a tool that referenced an external scene which did not travel with it.

A path-named scene that holds prefab instances cannot replay even when the file is present: the
prefab texts it read were never recorded (`not recorded`), so the replay refuses them rather than
reading the disk.

Fix: place the referenced scene file where the replay's project root expects it (next to the replay,
matching the relative path in the header), or re-record the session with this engine so the scene is
embedded inline and the replay becomes self-contained (required when the scene holds prefab
instances).",
    },
    CodeEntry {
        code: code::RESTORE_COMPONENT_UNREGISTERED,
        severity: Severity::Error,
        title: "restore dropped an unregistered component",
        explanation: "\
An `engine.restore` migration dropped a component because this engine build no longer registers it.

Restore is a migration, not a strict round-trip (engine-architecture.md §8): the dev loop restores a
snapshot taken from one build into a freshly recompiled binary, so a snapshot can name a component
the running build no longer has — a component that was renamed or removed since the snapshot was
taken. Rather than fail the whole restore (which would strand the designer with no world), the
migration drops just that component from just that entity and records the drop as a report entry.
The entity restores with all its other components; only the unregistered one is gone.

This is reported, not fatal — the `engine.restore` response carries a migration report, and a
non-empty report is normal after a schema change. If you did not expect a component to disappear,
the component name in the snapshot no longer matches any registered name: it was renamed (register it
under the new name and re-snapshot), or removed on purpose (the drop is correct). A game system that
relied on the dropped component will simply find no entities carrying it.",
    },
    CodeEntry {
        code: code::RESTORE_COMPONENT_UNDESERIALIZABLE,
        severity: Severity::Error,
        title: "restore dropped an undeserializable component value",
        explanation: "\
An `engine.restore` migration dropped a component because its snapshotted value no longer
deserializes into the component type this build defines.

The component name is still registered, but its shape changed incompatibly since the snapshot: a
field's type changed, a required field with no serde default was added, an enum variant was removed,
or the value is otherwise no longer valid for the current struct. A migration fills *added* fields
from their serde defaults automatically (reported as an informational field-level entry), but a value
that cannot deserialize *at all* has no honest default — filling the whole component from defaults
would hand a game system invented state it would then act on. So the migration drops the component
from that entity and records the entity id and the underlying serde error in the report entry.

This is reported, not fatal. Fix the incompatibility if the drop was not intended: give the changed
or added field a `#[serde(default)]` so old values still load, keep the field's type
round-trip-compatible, or accept the drop and re-snapshot against the new shape. The report's serde
error names the exact field that failed. (Adding a `#[serde(default)]` to every game component field
is the drift-safety discipline the testkit's `assert_components_drift_safe` probe enforces.)",
    },
    CodeEntry {
        code: code::REPLAY_DECOMPRESSED_TOO_LARGE,
        severity: Severity::Error,
        title: "replay frame decompresses past the size ceiling",
        explanation: "\
A replay record's zstd frame decompresses to more than the reader's ceiling for that frame — 256x
its compressed size, at least 1 MiB and at most 256 MiB — so the reader refused to inflate it. The
writer applies the same ceiling after compressing, before it writes: a header or record whose JSON
is past it is refused with this code, and nothing of it is written, because the file could not be
read back.

Every body in an LRP1 replay — the header and each record — is an independent zstd frame. The
in-the-clear length fields (`header_len`, `body_len`) bound the *compressed* bytes on disk (L3010),
but a tiny compressed frame can still expand enormously: a few hundred bytes of zstd can inflate to
megabytes of zeros. To keep an untrusted or corrupt `.lrp` from exhausting memory before the JSON
parse even runs, the reader decompresses each frame through a hard cap and stops the moment the
output would exceed it, allocating no more than the ceiling. This diagnostic names the byte offset
of the offending frame within the file.

A decompression bomb is a ratio attack, so the ceiling scales with the compressed size: real
snapshots compress at most ~26:1, well under the 256:1 bound, while a bomb inflates thousands to
one. A legitimate keyframe of any size up to 256 MiB therefore records and replays. Hitting the
ceiling when reading means the frame is a decompression bomb or the file is corrupt; hitting it
when recording means one snapshot is past 256 MiB or compresses better than 256:1.

A whole read call (`records`, `seek`) is also capped: the frames it decodes may total at most 256x
the file's size, within 1 MiB and 2 GiB, so many small frames cannot add up to an allocation no
single frame's ceiling would allow. That message names the total, the cap and the record offset
where it tripped.

Fix: re-record the session (or a smaller scene), or verify the `.lrp` came from a trusted `ludion`
run.",
    },
    CodeEntry {
        code: code::TRANSITION_TARGET_UNRESOLVED,
        severity: Severity::Error,
        title: "scene-switch target district could not be loaded",
        explanation: "\
A mid-run scene switch named a target district that could not be loaded or validated, so the switch
was rejected.

A game requests an in-run district change by setting the `SceneSwitch` resource; the engine performs
it at the frame boundary by rebuilding the world from the target scene while continuing the session
(the frame counter, the RNG stream, and the session logs all carry across). The switch is
transactional: the target district is instantiated into a throwaway world first, and only swapped in
on full success. This diagnostic means that build failed — the target scene file was missing under the
project root, unreadable, or did not pass validation (a malformed document, an unknown component, a
bad value). Because the failed switch touched nothing, the live world is untouched and the simulation
runs on in the current district; the failure never crashes or stalls the sim.

Fix: check the related diagnostics for the underlying load/validation error, confirm the district's
scene file exists at the requested path relative to the project root (the directory of the scene the
engine was started with) and passes `ludion check-scene`, then request the switch again.

A `SceneSwitch::restart` raises the same code when the scene text the session holds or a prefab it
names can no longer be built, or when the session holds no scene text at all (a data doc gone bad
reports the doc walk's own code). A restart rebuilds held text, never the scene file:
edit-then-retry means `scene.load`.",
    },
    CodeEntry {
        code: code::TRANSITION_ANCHOR_MISSING,
        severity: Severity::Error,
        title: "scene-switch spawn anchor not found",
        explanation: "\
A mid-run scene switch loaded its target district but found no entity named by the requested spawn
anchor, so the switch was rejected.

A switch drops the carried entities (typically the player) at a named anchor in the target district:
a plain entity whose `name:` field matches the switch's `spawn_anchor`, whose `Transform.translation`
is the drop point. The engine scans the freshly-built target world for that name and, finding none,
rejects the switch rather than guess a location. The switch is transactional, so the live world is
untouched and the simulation continues in the current district — this never crashes or stalls the sim.

Fix: add an entity to the target district whose `name:` equals the anchor the switch requests (place
it at the intended entrance), or correct the `spawn_anchor` string in the switch request to match an
existing name. Anchor names are matched exactly. A `SceneSwitch::restart` names no anchor and never
raises this.",
    },
    CodeEntry {
        code: code::TRANSITION_CARRY_DEAD,
        severity: Severity::Warning,
        title: "scene-switch carried a dead entity",
        explanation: "\
A mid-run scene switch named a carried entity that was not alive at the switch frame, so it was
skipped and the switch proceeded without it.

A switch carries an explicit list of entities (typically the player) from the current district into
the target one, serializing each out of the live world and respawning it at the anchor. This warning
means one of the requested carry ids did not name a live entity when the switch drained — the game
captured a stale `EntityId`, typically an entity that was despawned in the same frame it was named for
the carry. Rather than reject the whole switch (and strand the player), the engine skips just the dead
id and completes the switch with the entities that were live; the diagnostic names the skipped id. The
switch **still happened** — this is a warning on `Engine::last_transition_error`, not a rejection.

Fix: build the carry list in the same frame the switch is requested, from live queries (e.g. the
entity your gate system just found), so every id names an entity that is still alive at the frame
boundary. If the skipped entity was meant to travel, ensure it is not despawned before the switch is
requested.",
    },
    CodeEntry {
        code: code::TRANSITION_CARRY_ORPHANED,
        severity: Severity::Warning,
        title: "scene-switch carried an entity without what it references",
        explanation: "\
A mid-run scene switch carried an entity whose `Parent` is alive but was not carried with it, so the
entity arrived un-parented.

A switch carries each listed entity together with its whole subtree, and the carried children keep
their `Parent` links, remapped to the respawned ids. A carried entity whose parent stays behind in the
departing district has nothing to attach to. The engine un-parents it keeping its world pose: its
`Transform` rotation and scale become the world ones it had at the switch frame, and like every carried root its translation is then
the spawn anchor's. The switch **still happened** — this is a warning on
`Engine::last_transition_error`, not a rejection. The message names the orphaned entities.

Fix: carry the parent too (its subtree comes along), or carry the child's whole family from the
parent id, if the child is meant to stay attached.

The same warning covers a carried entity's other entity references (a marked `EntityId` field of any
component). A reference to a carried entity follows it to its respawned id. A reference to an entity
that stayed behind arrives null when the field is optional, and the component is left behind when the
field is required. The message names each owner, component/field and target.

Fix: carry the referenced entity too.",
    },
    CodeEntry {
        code: code::PROTOCOL_UNEXPECTED_RESPONSE,
        severity: Severity::Error,
        title: "a reply's body does not match the command",
        explanation: "\
The server answered a request with a response kind this client did not expect for that method. The
request itself was well-formed; the reply was not what the method promises.

This is usually a host and a client from different engine versions, whose method tables disagree.

Fix: run a `ludion` and a host built from the same engine version. The message names the method, the
expected and the received kind.",
    },
    CodeEntry {
        code: code::PROTOCOL_MESSAGE_TOO_LARGE,
        severity: Severity::Error,
        title: "a wire message exceeds the size limit",
        explanation: "\
A wire message was over the 256 MiB limit (`ludion_protocol::MAX_MESSAGE_BYTES`), so it was refused
rather than sent or read. Frames and screenshots up to the 8K render ceiling always fit, so this is
usually a very large `engine.snapshot` / `engine.restore` or `query.entities` result.

Fix: narrow the query, or snapshot a smaller world. The message names the size and the limit.

An event over the limit is not sent either: a `log` event with this code takes its place, keeping its
`sub` and `seq`, so the stream shows no gap.",
    },
    CodeEntry {
        code: code::PROTOCOL_INVALID_MAX_FPS,
        severity: Severity::Error,
        title: "a subscription's max_fps is out of range",
        explanation: "\
A `stream.subscribe` carried `mode.max_fps` that is not a finite number between 0.1 and 1000. The
subscription was not opened. `max_fps` caps how often frames are delivered; below 0.1 the interval
between frames is longer than ten seconds, and above 1000 the cap could never bind.

Fix: pass a `max_fps` in `[0.1, 1000]`, or omit it for no cap. The message names the value.",
    },
    CodeEntry {
        code: code::RECORDING_ARM_REFUSED,
        severity: Severity::Error,
        title: "recording cannot be armed after the first tick or with no scene",
        explanation: "\
`Engine::set_recording` was called when the engine had no scene loaded, or after the sim had already
advanced past frame 0. A replay is the scene's initial state plus the inputs applied from frame 0; a
recording armed later would stamp a header that does not describe the state its first frame starts
from, and the replay would diverge when played back. The engine is left untouched and nothing is
recorded.

Fix: arm the recording right after loading the scene, before the first tick (a scene load resets
the frame to 0). To record a session that is already running, load the scene again, then arm.",
    },
    CodeEntry {
        code: code::RESTORE_ENGINE_VERSION_DIFFERS,
        severity: Severity::Warning,
        title: "restored snapshot came from a different engine version",
        explanation: "\
An `engine.restore` snapshot records the engine version that captured it, and it differs from the
running build's. The restore went ahead and the world is intact; this entry only says the snapshot
and the engine may disagree on details a version change can move (component shapes, simulation
behavior).

This is reported, not fatal. If the world looks wrong after the restore, re-create the snapshot from
this build; otherwise nothing needs doing. Component-level drift, if any, is reported separately.",
    },
    CodeEntry {
        code: code::RESTORE_SNAPSHOT_FIELD_UNKNOWN,
        severity: Severity::Warning,
        title: "restored snapshot carried an unknown top-level field",
        explanation: "\
An `engine.restore` snapshot document carried a top-level field this engine build does not define.
The restore ignored it and went ahead; whatever state the field held was not restored. A field like
this usually means the snapshot came from a newer engine version that captures more state.

This is reported, not fatal. Restore with the engine version that took the snapshot to keep that
state, or take a fresh snapshot from this build. The entry names the field.",
    },
    CodeEntry {
        code: code::REPLAY_PANIC_NOT_REPRODUCED,
        severity: Severity::Error,
        title: "replay no longer reproduces its recorded system panic",
        explanation: "\
A recording marks a tick on which a game system panicked (L4007), and the replay ticked that frame
without a panic.

A recorded session that ended in a panic carries a `system_panic` record right after the `sim.step`
whose last tick panicked. Replaying it reproduces that panic and only that one; a build that no longer
panics there is no longer running the recorded session, so its per-frame hashes prove nothing about it.

Fix: if the panic was fixed on purpose, the recording is stale — re-record the session with this
build. If it was not, a behavior change moved or removed the panic; diff the systems the panic message
named when it was recorded.",
    },
    CodeEntry {
        code: code::PROTOCOL_MALFORMED_REPLY,
        severity: Severity::Error,
        title: "a server's message could not be parsed",
        explanation: "`ludion-client` read a message from the server — the handshake reply, a response or a pushed event —
that is not JSON or not the shape this protocol version defines.

The far end is not a Ludion host, or is one built from a different engine version (L3003 is the
version-drift check at the handshake). The message carries the parse error. A malformed *request*
envelope the client sent is L3001, not this.

Fix: connect to a host built from the same engine version as the client.",
    },
    CodeEntry {
        code: code::REPLAY_TORN_TAIL,
        severity: Severity::Warning,
        title: "a replay ends in a torn record",
        explanation: "\
The last record of a replay is cut short: the file ends inside a record header, or the record's
body runs past the end of the file. The reader dropped that record and the session replays up to
the last whole record.

This is what a recording stopped mid-write (L3042) or a killed host leaves behind: the writer emits
each record with one write, so a failed or interrupted write can tear only the final record. Only
the final record is forgiven — a cut inside the header, or an unknown record kind in a complete
record header, is still L3010. The message names the byte offset where the torn record starts and
how many trailing bytes were dropped.

Fix: if the full session matters, re-record it. Otherwise nothing: the replay is valid as far as
it goes.",
    },
    CodeEntry {
        code: code::RESIDUE_REF_UNRESOLVED,
        severity: Severity::Warning,
        title: "a residue record's entity reference could not be followed",
        explanation: "\
A `Residue`-marked entity holds an entity reference (`Parent`, or any field the registry marks as
one) to something the scene's residue cannot identify again, so the reference was cleared or its
component left behind. The switch itself happened.

Residue records an entity reference by identity, not by id: a reference to another residue record
follows that record to its respawned entity, and a reference to an entity with a `Name` follows the
first entity of that name in the scene it returns to. Anything else cannot be found again — an
anonymous entity that is not itself residue, an entity that was carried out of the scene, or one that
was already despawned. An optional reference arrives empty; a component whose required reference is
unresolved is left behind, the way a carried component's is (L3043).

Fix: mark the referenced entity `Residue` too, or give it a unique `name:`, so the reference has an
identity to follow.",
    },
    CodeEntry {
        code: code::REPLAY_FRAME_WINDOW,
        severity: Severity::Error,
        title: "replay-to-frames window is out of range",
        explanation: "\
An offline replay-to-frames render was asked for a frame window that falls outside the replay's
recorded range.

The replay-to-frames render (`cargo run -p <game> -- --replay-frames run.lrp --frames-out <dir>`)
drives a committed `.lrp` frame-by-frame and writes each stepped sim frame to a numbered PNG for
ffmpeg to assemble. Its `--from`/`--to` window is inclusive on both ends and is measured in absolute
sim frames: frame 0 is the initial post-load world, and the replay's final frame is the sum of every
recorded `sim.step`'s ticks. This diagnostic means the window does not fit that range — either
`--from` was greater than `--to` (an empty window), or `--to` named a frame past the replay's last.

Fix: pass a `--from` no greater than `--to`, and a `--to` no greater than the replay's final frame
(the diagnostic reports that frame's index). Omit `--to` to render through the final frame, or omit
both to render the whole replay.

The same code also refuses three cases the frame tools cannot answer. A replay with an
`engine.restore`, or a `scene.load` after its first tick, resets or jumps the sim frame, so frame
numbers repeat and a window or divergence frame is ambiguous: window or bisect a recording that has
neither (record the session after the load), or replay it whole with `--replay-verify`. A
`--profile-replay` window that measures no frame (`--from 0 --to 0`: frame 0 is the loaded world, no
tick produces it) has nothing to report: widen it to include a ticked frame. A bisect frame the
replay never reaches is named with the replay's last frame: bisect against a frame in `0..=<last>`.
`ReplayReader::seek` refuses the same two resets: seek a recording that has neither, or replay it
whole.",
    },
    CodeEntry {
        code: code::SERVER_ACCEPT_FAILED,
        severity: Severity::Error,
        title: "the host could not accept a connection",
        explanation: "\
The server's listening socket returned an error instead of an incoming connection.

Most accept failures are about one client and nothing else — a peer that vanished between the TCP
handshake and the accept — so the host logs the error class once and keeps listening, briefly backing
off so a socket that fails instantly cannot spin a core. A failure that never clears is a different
condition: the process is out of file descriptors (`EMFILE`/`ENFILE`), a system-wide descriptor limit
is exhausted, or the listening socket itself has gone bad. None of those recover by retrying, so after
a sustained run of consecutive failures the host stops accepting and submits an `engine.shutdown`
through the ordinary command queue — the same stop path a wire client uses — rather than run on
invisibly, unable to serve anyone. Connections already open are unaffected while it retries; the
message names the operating system's reason and, when the host gives up, how many accepts failed in a
row.

Fix: read the reported OS error first. For descriptor exhaustion, raise the process limit (`ulimit
-n` on macOS, the handle quota on Windows) or find what is holding descriptors open — a client that
connects in a loop without closing, or a watcher over a large tree — then restart the host. A single
logged failure with no shutdown needs no action; it is reported so a recurring one is visible.",
    },
    CodeEntry {
        code: code::PROTOCOL_SIM_STALLED,
        severity: Severity::Error,
        title: "the sim thread did not answer in time",
        explanation: "\
A request reached the sim thread but no reply came back within the server's wait bound.

Every request crosses one bridge: the server queues the command, the sim thread runs it through
dispatch and answers on a one-shot channel. That wait is bounded. This diagnostic means the bridge is
intact — the queue is connected and the sim thread is alive, otherwise the answer would be L3009 —
and the reply simply never arrived: a game system looping without terminating, a handler blocking on
something that never completes, or a `sim.step` whose `ticks` cannot finish inside the bound. The
bound sits far above any command this engine performs in one go, so it never fires on work that is
merely slow. The server answers this rather than waiting forever, so the connection keeps serving and
the host can still be shut down; the diagnostic names the method that stalled. The embedded host's
`suspend` and `resume` raise it too, after a 2 s wait; the transition stays queued and applies when
the sim next reaches a pass boundary.

Fix: the stalled method names the code to look at. For a game system, attach a debugger to the
`ludion-sim` thread and find the loop that is not terminating — an unbounded `while` on a condition
the sim never reaches is the usual shape. For a deliberately huge `sim.step`, split it into several
smaller steps: from outside, one step that cannot finish is indistinguishable from a wedge. Restart
the host to clear a wedged sim thread; the world it owns cannot be recovered from another thread.",
    },
    CodeEntry {
        code: code::REPLAY_KEYFRAME_DRIFT,
        severity: Severity::Error,
        title: "replay no longer reproduces its recorded keyframe",
        explanation: "\
A committed replay was re-driven with the current build and the live state stopped matching a
keyframe the recording embedded.

A `.lrp` is not only a list of inputs: every 300 frames it embeds a full world snapshot as a seek
target. Those keyframes are also an oracle. Re-driving the replay with today's code and comparing the
live world against each keyframe answers a question a per-frame hash mismatch cannot: *what* changed.
The verifier reports the first keyframe that fails to reproduce, the entities that only one side has,
the components whose values differ, and — separately, because the world hash deliberately excludes
them — whether the RNG state or the entity free list drifted. A drift in those two has not surfaced
in component data yet but will: the next draw or the next spawn diverges.

Two causes, and the report tells them apart by what it names. A **deliberate gameplay change** (a
retuned constant, a new system, a different draw order) makes the replay stale: the recorded
keyframes describe code that no longer exists. An **unintended behavior change** shows up the same
way, which is the point — before this check, a re-record was a rubber stamp and both looked
identical.

Fix: read the named component first and decide which of the two you are looking at. If the change was
intended, re-record the replay so its keyframes match the code that produced them (a game's own
recording flag — breakout's is `LUDION_BLESS=1 cargo test -p breakout --test replay`), and commit the new
`.lrp` in the same change as the gameplay edit. If it was not intended, the named entity and component
are where the regression is; the frame narrows it further.",
    },
    CodeEntry {
        code: code::PROTOCOL_RENDER_SIZE_INVALID,
        severity: Severity::Error,
        title: "render.size did not name a renderable extent",
        explanation: "\
A `render.size` request named something the host cannot render at, so the session's extent was left
exactly as it was.

`render.size` sets the resolution every live stream frame is produced at (and the default a
dimensionless `render.screenshot` uses), or reports it. Both `width` and `height` are optional so the
same verb sets and queries — but they are optional *together*: an extent is a pair, and a request
carrying exactly one axis is refused rather than guessed at, because the two plausible guesses (keep
the other axis, or preserve the aspect ratio) give different pictures and neither is what the client
asked for. Each axis must be a whole pixel count in 1..=7680 x 1..=4320: past that ceiling the
readback ring's three staging buffers and the frame graph's physical cache stop being a stream and
become an allocation failure several layers below the place that could explain it.

Send `{}` to read the extent without changing it; the reply carries the pair in force either way, so
a client never has to assume its request was taken verbatim.

The extent is session state shared by every attached client — the last writer wins. That is what
keeps `render.bounds` (which reports the same number as the pixel space its screen rects live in)
true for everyone at once, and it is why the right way to learn a session's extent is to read it,
never to hardcode 1280x720.

Fix: send both axes, in whole pixels, inside the range — `{\"width\": 960, \"height\": 540}`. The
`path` on this diagnostic points at the offending field. To pick a startup extent instead, so no
client has to ask, use a host's `--render-size` flag or `LUDION_RENDER_SIZE` (`L0017` covers those).",
    },
    CodeEntry {
        code: code::RESIDUE_DUPLICATE_NAME,
        severity: Severity::Warning,
        title: "two Residue entities share one name",
        explanation: "\
Two or more `Residue`-marked entities in the scene a switch left carry the same `Name`, so only the
first was captured and the others were skipped.

Scene residue is the state a scene keeps while you are away: mark an entity `Residue: {}` and the
engine captures its components when the run switches out of that scene, then re-applies them when the
run comes back. Identity is the entity's `Name` — that is what lets a record find its way back onto
the right entity in the freshly instantiated scene — so one name can stand for exactly one record.
Faced with two, the engine keeps the first in entity order and skips the rest rather than pick a
winner silently; the skipped entities' state is lost for this visit, but the switch itself happened
and the sim runs on in the target scene.

The mirror image raises the same code: on the way *back* in, a record whose name is carried by two
entities of the freshly instantiated scene addresses both. The record lands on the first in entity
order — the pre-existing behaviour — and the diagnostic names every id it could have chosen, because
the entity that misses out is rebuilt from the scene text and silently loses the state it kept.

Fix: give each `Residue`-marked entity a unique `name:` in that scene (generated maps already name
markers per id, e.g. `nest-site-12`), or drop the marker from the entities whose state does not need
to survive the visit. An entity that genuinely has no identity of its own — a dropped pickup, one of
many — should carry **no** `Name` at all: an unnamed `Residue` entity is anonymous, never matched,
and always respawned on return, which is exactly the intent.",
    },
    CodeEntry {
        code: code::RESIDUE_CARRY_CONFLICT,
        severity: Severity::Warning,
        title: "an entity was both carried and Residue-marked",
        explanation: "\
A scene switch named an entity in its carry list that also carries the `Residue` marker, so the entity
was asked to travel and to stay behind at once. The carry wins: the entity crossed into the target
scene and no residue was captured for it.

The two facilities are opposites by design. A carried entity is serialized out of the departing world
and respawned at the target scene's anchor, so it *is* the thing that moves; residue is what the
departing scene keeps while you are away, re-applied on your return. An entity cannot be both without
also being duplicated on the next return — the carried one and the remembered one. The engine resolves
it in favor of the carry, because that is the entity the player is looking at, and reports it here.

Fix: decide which the entity is. Run state that must follow the player (health, inventory, quest log)
lives on the carried entity and needs no marker. State that belongs to the place (a destroyed nest, a
thrown switch, a dropped pile) stays behind with `Residue: {}` and must not appear in the carry list
the switch requests.",
    },
    CodeEntry {
        code: code::RESIDUE_EVICTED,
        severity: Severity::Warning,
        title: "scene residue hit a bound and dropped state",
        explanation: "\
The session's scene residue is bounded, and a switch pushed it past one of those bounds, so the engine
dropped the oldest state rather than let the map grow without limit.

Residue is what a scene keeps while the run is away. A session holds at most 32 scenes' worth
(`RESIDUE_MAX_SCENES`), and one scene holds at most 4096 records (`RESIDUE_MAX_RECORDS_PER_SCENE`).
Past the scene bound the **least recently visited** scene is evicted — the one whose last departure
sits on the lowest frame, ties broken by ascending scene name — so the choice is the same on every
machine and in every replay. Past the record bound the scene keeps the first 4096 records in capture
order and drops the rest. Either way the switch itself happened and the sim runs on in the target
scene; what was dropped is simply forgotten, and the affected scene rebuilds that part of itself from
its text on the next return.

Fix: visit fewer distinct named scenes in one session, or mark fewer entities `Residue: {}`. Anonymous
records are the usual cause of the record bound — an unnamed `Residue` entity is never matched and
always respawns, so a system that drops one per event accumulates across a long visit. Despawn them, or
give the state one named entity that counts instead of one entity per event.",
    },
    CodeEntry {
        code: code::TRANSITION_ANCHOR_NO_TRANSFORM,
        severity: Severity::Error,
        title: "the switch anchor carries no Transform",
        explanation: "\
A mid-run scene switch found an entity carrying the anchor's `Name` in the target district, but that
entity has no `Transform`, so it names no point to drop the carried entities at. The switch was
rejected and the sim runs on in the current district.

An anchor is an ordinary named entity whose `Transform.translation` is the drop point — that
translation is the entire contract, and it is the one field a carried entity does not keep. An anchor
without one is an authoring mistake of the same kind as an anchor that does not exist (`L3019`), and
it is refused for the same reason: the alternative is dropping the player at the world origin, which
looks like a physics bug several systems away from the scene file that caused it.

Fix: give the anchor entity a `Transform` in the target district's scene text, at the point the player
should arrive:

    - name: gate-from-plaza
      components:
        Transform:
          translation: [ 40.0, 0.0, -10.0 ]

A generated map authors this for you — `kind: map` gate anchors always carry a transform, so an anchor
missing one usually means the switch named a decorative entity that happens to share the name.",
    },
    CodeEntry {
        code: code::RESIDUE_RECORD_UNMIGRATABLE,
        severity: Severity::Warning,
        title: "a residue record no longer reads into this build",
        explanation: "\
A scene's residue holds a component bag that this engine build can no longer deserialize — the
component was unregistered, or its shape changed since the bag was captured — so the whole record was
dropped and the scene's authored version of that entity stands. The switch itself happened.

Residue outlives more than a visit: a snapshot carries it, so `ludion dev`'s restart-with-state and a
replay seek both restore bags captured by the *previous* build of the game. A component whose fields
moved in between is exactly the case: the bag is real, it simply is not this build's shape any more.
Records are migrated the way a snapshot's world entities are (`L3015`/`L3016`) — checked before they
touch the world, because a named record replaces its counterpart in place and a half-applied bag would
leave the authored entity stripped. Dropping the one record rather than rejecting the switch is what
keeps a district reachable across a component change: the alternative is a gate that fails forever.

Fix: nothing, if you meant to change the component — the scene rebuilds that entity from its text and
the run keeps going. If the state mattered, restart the session (`scene.load` clears every district's
residue) rather than resuming from a snapshot taken before the change.",
    },
    CodeEntry {
        code: code::RESIDUE_NAME_UNMATCHED,
        severity: Severity::Warning,
        title: "a renamed residue record matched no entity",
        explanation: "\
A residue record captured off an entity the **scene document authored** came back to its scene and
found no entity carrying that `Name`, so the record was dropped for this visit. The switch itself
happened.

A named record's whole purpose is to land back on its own entity, in place, keeping the `EntityId` the
scene minted. When an authored counterpart is gone — the scene text renamed it, or a generated map
re-indexed its markers — the scene still authors that entity, under its new name, and spawning the
record beside it would duplicate the thing it was supposed to *be*: two nests on one site, one
authored and one remembered. The engine drops it and says so instead.

This is only ever raised for records the document authored. A marked entity a *system* spawned has no
counterpart in the scene text and never will, so it is respawned with its `Name` on every return —
that is how a game keeps per-district state it created itself (a census of what a sector has left)
across a visit. Provenance travels on the record, including through a snapshot; residue restored from
a snapshot written before the distinction existed is read as code-spawned, i.e. respawned.

Fix: keep authored `Residue` entities' `name:` stable across scene edits — that name is their
identity, not a label. If you renamed one deliberately, this fires once per return until the session
ends and is harmless; the state it carried is gone with the old name. An entity with no identity of its
own should carry no `Name` at all: an unnamed record is anonymous, never matched, and always
respawns.",
    },
    CodeEntry {
        code: code::PROTOCOL_RENDER_VIEW_INVALID,
        severity: Severity::Error,
        title: "render.view did not name a pose a camera can look through",
        explanation: "A `render.view` request carried a pose the host cannot build a camera from, so the view override was
left exactly as it was.

`render.view` points this host's *stream* at a free camera pose instead of the scene's own `Camera`
- what an editor's fly and orbit gestures write, at the display's cadence. The pose is a world-space
`translation`, a `rotation` in Euler-XYZ degrees, and an optional `fov_y_deg`. Every coordinate and
every angle must be finite: a NaN or an infinity does not aim a camera somewhere strange, it produces
a view matrix whose every product is NaN, and the frame that comes back is blank with nothing in it
to say why. A vertical field of view must be strictly inside 0 and 180 degrees, the open interval a
perspective projection is defined on - at either end the projection matrix degenerates.

A refused request changes nothing, which is what makes the override safe to drive from a gesture
loop: one bad frame of input leaves the human looking at the last good pose instead of at nothing.

Send `{\"pose\": null}` to clear the override and return the stream to the scene's camera, or `{}` to
read the pose in force without touching it; the reply carries it either way.

Fix: send finite numbers, and an `fov_y_deg` inside `0..180` - the `path` on this diagnostic points at
the offending field. The override is host render state and never reaches the world: it is not hashed,
not snapshotted, and not recorded into a replay, so nothing about the simulation depends on getting
this right.",
    },
    CodeEntry {
        code: code::PHYSICS_SNAPSHOT_INVALID,
        severity: Severity::Error,
        title: "the snapshot's physics state could not be read or written",
        explanation: "An engine snapshot carries the rigid-body solver's state as one additive `physics` field:
base64 of a binary codec, opaque because a structured JSON value cannot hold every field the solver
keeps. This code says that blob could not be turned into a solver state, or the live one could not
be turned into a blob.

The overwhelmingly likely cause is a *document/build mismatch*. The bytes are the pinned solver's own
serialization, so they are readable only by a build with the same solver pin and the same binary
codec major version - a pin move is deliberately a snapshot-format event. A snapshot taken before
such a move, replayed into a build after it, lands here, and so does a `.lrp` whose keyframes were
recorded then: a replay keyframe *is* an engine snapshot.

Fix: re-take the snapshot, or re-record the replay corpus, against the build you are running. If
neither the pin nor the codec has moved, the document is corrupt - a truncated file, or a `physics`
value that is not valid base64 - and the honest repair is to produce it again rather than to patch
the text.

A session with no physics world writes no `physics` field at all and never raises this. The field is
invisible to `WorldHash` either way: the solver is a world *resource*, and what reaches the hash is
the pose and `Velocity` the mirror writes back into components.",
    },
    CodeEntry {
        code: code::PHYSICS_SNAPSHOT_NO_SETTINGS,
        severity: Severity::Warning,
        title: "restored solver state has no scene physics settings",
        explanation: "\
A restored snapshot carries the rigid-body solver's state (its additive `physics` field) but no
`scene_physics` — the scene's `physics:` block as it stood when the snapshot was taken.

`scene_physics` is what tells a restored engine to keep stepping physics at all: without it there is
no space, gravity or solver tunables to install the write/step/mirror triple with, so the restored
solver state sits there and is never advanced. This is the shape of a snapshot hand-edited to drop
the field, or taken by a build old enough to predate it — a document produced by the current
`engine.snapshot` from a physics scene always carries both fields together.

A restore replaces the engine's physics settings with the document's own — whatever scene was
loaded before the restore, and its `physics:` block, do not carry over.

Fix: re-take the snapshot from a session whose loaded scene declares a `physics:` block, so
`engine.snapshot` writes both fields together the way it always does for a physics scene.",
    },
    CodeEntry {
        code: code::JOINT_BODY_MISSING,
        severity: Severity::Warning,
        title: "a joint's body is dead or unclaimed",
        explanation: "\
A `Joint`'s `a` or `b` names an entity that is dead, or alive but with no rigid, kinematic or static
body — a `Collider` never claimed by `ludion-physics` (`ludion-spatial`'s 2D index owns it instead),
or one whose shape the solver rejected (`L1181`/`L1182`).

The joint is left inert rather than deleted: its rapier joint is removed, the `Joint` component
stays on its carrier untouched, and it re-forms the moment both bodies exist again — the shape a
runtime spawn order, a delayed claim, or a temporarily rejected shape all produce. This is a warning
because nothing about the simulation is wrong; the joint is simply not doing anything yet.

Fix: nothing is required if this is expected (a body that spawns later, or a shape you are about to
fix). Otherwise, check that both `a` and `b` name live entities carrying a `Collider` the solver can
build a body from.",
    },
    CodeEntry {
        code: code::RAGDOLL_SWITCH_INVALID,
        severity: Severity::Error,
        title: "ragdoll switch refused",
        explanation: "\
`switch_to_ragdoll` refused to switch an entity from a posed assembly into one rigid body per part.

The switch validates before it mutates anything, and is all-or-nothing: if any check below fails,
the world is left exactly as it was. The refusal names which check failed:

  - the entity is not alive, or has no `Transform`;
  - the entity's `Mesh.shape` is not `Assembly` (the switch only applies to a whole-assembly entity,
    never to a single `assembly_part` entity or one already switched);
  - the entity's `Transform.scale` is not uniform (its x, y and z components are not exactly equal) —
    a non-uniform scale has no single factor to size each part's collider and mass by;
  - one of the assembly's part meshes is empty (no vertices to build a bounds collider from);
  - a part's mass at that scale is zero or not finite (a flat part with no extent on an axis, or a
    scale large enough to overflow) — a rigid body needs a strictly positive, finite mass.

Fix: switch an entity that still draws the whole assembly with a uniform scale, and make sure every
part file the assembly places has geometry.

`switch_skin_to_ragdoll` raises the same code when it refuses to turn a skinned mesh into its
`SkinBone` ragdoll. Besides the dead / no-`Transform` / scale cases above, it refuses when:

  - the entity's `Mesh` is not a skinned `asset` or `character` mesh (no `joints:` in its document);
  - the parts prefab's last part is not named `$skin` (a `$skin` elsewhere gets a hint to move it last);
  - a part's `SkinBone` binds another skin than `$skin`;
  - any other entity reference in the parts names `$skin`;
  - the `$skin` part's mesh document differs from the live skin's.

Nothing is mutated when it fires. Fix: end the parts prefab with a `$skin` part that carries the same
skinned `Mesh` as the entity, and bind every `SkinBone.skin` to it.",
    },
    CodeEntry {
        code: code::JOINT_MOTOR_MISMATCH,
        severity: Severity::Error,
        title: "a joint motor does not fit its joint",
        explanation: "\
A `JointMotor` sits on the same carrier as a `Joint` it cannot drive. The motor is not applied: the
joint stays passive, exactly as if it had no motor, until the `JointMotor` or the `Joint` changes.

Three causes raise it:

  - the joint is `fixed`, which has no free axis to drive;
  - the `target` has the wrong number of elements for the joint kind — one for a `hinge` or
    `slider`, three for a `ball`;
  - a `stiffness`, `damping`, `max_force` or `target` value written by game code, bypassing scene and
    wire validation, is non-finite or negative (`L1188` catches the same values on the validated
    paths).

A passive motor (stiffness and damping both 0) is not checked, so a mismatch reports when a system
first makes it active.

A `JointMotor` on a carrier with no `Joint` — never added, or broken off — is silently inert and does
not raise this: a ragdoll whose joint breaks reports nothing.

Fix: match `target` to the joint kind, keep the values finite and non-negative, and drop the
`JointMotor` from a `fixed` joint's carrier.",
    },
    CodeEntry {
        code: code::IMPULSE_TARGET_INVALID,
        severity: Severity::Warning,
        title: "an impulse's target has no rigid body",
        explanation: "\
An at-centre or at-point impulse (`PhysicsWorld::apply_impulse`, `apply_impulse_at_point`) named an
entity that has no rigid body when the physics write system applied it: the entity is dead, its
`Collider` was rejected by the solver (`L1181`/`L1182`/`L1186`), or its `Collider` is `static` or
`kinematic`. The impulse is dropped and nothing else changes.

It is a warning because the tick is otherwise well-formed; the call did nothing. A radial impulse
never raises this: it skips static and kinematic bodies silently, since a blast is expected to
overlap them.

Fix: apply impulses only to entities whose `Collider` has `body: rigid`. The physics write system
creates a body before it drains the queue, so an entity spawned earlier the same tick is a valid
target.",
    },
    CodeEntry {
        code: code::WIRE_ORIGIN_REFUSED,
        severity: Severity::Error,
        title: "WebSocket origin refused",
        explanation: "\
The WebSocket handshake carried an `Origin` header that is neither a loopback address
(`localhost`, `127.0.0.1`, `[::1]`) nor the editor's own (`tauri://localhost`,
`http://tauri.localhost`), so the server answered HTTP 403 and did not upgrade the connection.
`Origin` is set by browsers, so this stops a web page from another site from driving the engine.
Native clients (the CLI, `ludion-client`, the MCP adapter, Node) send no `Origin` and are not
affected. More than one `Origin` header is refused too.

Fix: connect from a native client, or from a page served from a loopback address.",
    },
    CodeEntry {
        code: code::WIRE_PATH_OUTSIDE_ROOT,
        severity: Severity::Error,
        title: "scene path is outside the content root",
        explanation: "\
`scene.load`, `scene.save` and `scene.check` take a path relative to the content root — the
directory of the scene the engine was started with. The path given is absolute (a leading `/` or
`\\`), carries a drive, scheme or stream prefix (any `:`), climbs out with a `..` segment, is
empty, or contains a NUL byte. Nothing is read or written. `scene.check` reports this as an invalid
result rather than an error.

Fix: pass a path below the content root, e.g. `levels/arena.yaml`.",
    },
    CodeEntry {
        code: code::RESTORE_ENTITIES_MISMATCH,
        severity: Severity::Error,
        title: "snapshot entity list disagrees with its allocator state",
        explanation: "\
`engine.restore` rebuilt the entity allocator from the snapshot's `allocator` field and found its
live slots are not exactly the snapshot's `entities` list: an entity is missing, extra, or at a
different generation.

The restore changed nothing — the engine is exactly as it was before the call.

This comes from a hand-edited snapshot; an engine-written one keeps the two in step.

Fix: re-take the snapshot with `engine.snapshot`, or delete the `allocator` field to fall back to
restoring from the `entities` list alone.",
    },
    CodeEntry {
        code: code::RECORDING_STOPPED,
        severity: Severity::Error,
        title: "a recording stopped before the session ended",
        explanation: "\
A write to the replay sink failed mid-session — the sink itself failed (L3010) or a frame was past
its replay ceiling (L3017) — so the engine stopped recording. The command that triggered the write
still applied; only the recording ended.

The `.lrp` is valid up to the last whole record: everything written before the failure replays,
and a torn final record is dropped with L3052. `Engine::stop_recording` returns this diagnostic once; a second call is
`Ok`. The message names the frame the recording died at and the underlying cause.

Fix: remove the cause (free disk space, record a smaller scene) and re-record.",
    },
    // --- L4xxx: sim ---
    CodeEntry {
        code: code::INVALID_SIM_SPEED,
        severity: Severity::Error,
        title: "invalid simulation speed",
        explanation: "\
A simulation speed factor was rejected because it is not a finite number in `[1/64, 64]`.

`sim.set_speed` scales only *when* the live scheduler performs ticks — wall-clock pacing. It never
changes the fixed timestep `dt` or the number of ticks a step performs; that would break
determinism. A speed of `0`, a negative number, `NaN`, or an infinity has no meaningful pacing
interpretation, so `Speed::new` (and deserialization of a speed value) rejects it up front instead
of letting the scheduler divide by it later. The range is bounded because the loop sleeps on
`tick period / factor`: a tiny factor overflows the sleep duration, and a huge one only spins the
loop without ticking any faster than the host can.

Fix: pass a finite factor between `1/64` and `64` — `1.0` is real time, `2.0` is double speed, `0.5`
is half speed. To stop ticking entirely, use `sim.pause` rather than a zero speed. Headless runs
ignore speed altogether and tick as fast as possible.",
    },
    CodeEntry {
        code: code::INPUT_SCRIPT_IO,
        severity: Severity::Error,
        title: "input script could not be read",
        explanation: "\
An input-script file could not be read from disk.

`ludion sim-hash --script <file>` loads a JSON input
script — the serializable list of frame-addressed input events that drives a deterministic run.
The message includes the operating system's reason — typically the file does not exist, the path
is a directory, or permissions deny access.

Fix: check the path (it is resolved relative to the current working directory) and the file's
permissions.",
    },
    CodeEntry {
        code: code::INPUT_SCRIPT_INVALID,
        severity: Severity::Error,
        title: "input script is not valid",
        explanation: "\
An input-script file was read but could not be parsed as an input script.

An input script is a JSON array of frame-addressed input events, each a record of the form:

  [
    {\"frame\": 10, \"event\": {\"kind\": \"key_down\", \"key\": \"W\"}},
    {\"frame\": 30, \"event\": {\"kind\": \"key_up\",   \"key\": \"W\"}}
  ]

`frame` is the tick at whose boundary the event applies (before that frame's systems run);
`kind` is `key_down` or `key_up`; `key` is a key name such as `\"W\"`, `\"Left\"`, or `\"Space\"`.
Events may appear in any order — the script canonicalizes to frame order on load. The message
includes the underlying JSON error with its line and column.

Fix: correct the JSON at the reported position and match the record shape above.",
    },
    CodeEntry {
        code: code::BUDGET_FILE_INVALID,
        severity: Severity::Error,
        title: "profile budget file is invalid",
        explanation: "\
A profile budget file could not be read, or its contents are not valid budget TOML.

`ludion profile --check <budget.toml>` compares the measured medians against a budget file:
optional `frame_total_ms`, `frame_total_max_ms`, `post_total_ms`, `cluster_build_ms`,
`extract_ms` and `load_total_ms` scalars, a `[systems]` table of per-system ceilings, a `[systems_max]` table of
per-system worst-frame ceilings, a `[passes]` table of per-GPU-pass ceilings, and a
`[post_groups]` table of per-effect ceilings. Every value is a number of milliseconds.

  frame_total_ms   = 1.0
  frame_total_max_ms = 4.0
  post_total_ms    = 2.0
  cluster_build_ms = 0.3
  extract_ms       = 0.5
  load_total_ms    = 5000.0

  [systems]
  move_paddle = 0.20

  [systems_max]
  move_paddle = 0.80

  [passes]
  scene     = 3.0
  composite = 1.5

  [post_groups]
  bloom = 1.2

A `[post_groups]` entry budgets an *effect* rather than a pass: its measured value is the sum of
its member passes' medians, so one ceiling covers however many passes that effect's implementation
spends (bloom is three passes as a Gaussian blur and eleven as a mip pyramid). `post_total_ms` is
that sum across every group. `cluster_build_ms` is the odd one out: a CPU cost, not a GPU one — the
clustered-forward light binning that runs before the frame is drawn — and it is skipped rather than
failed when the report carries no cluster section, which is every live (`--attach`) capture.
`extract_ms` is the other CPU line — the sim thread's per-frame extract cost, the one line that
scales with visual population rather than draw cost — and unlike `cluster_build_ms` it is checked
wherever the report carries an `extract` sequential timing, which is both the offline driver and a
live capture. `load_total_ms` is checked against the report's `load` section (one run's total load
wall time, not a median), and is `L4025` when the report has none.

`frame_total_max_ms` and `[systems_max]` budget the *worst* frame rather than the median — the
spike a median hides. They check a report's tail (`max_ms`), which a `--profile-replay` run and
the offline driver carry, and are skipped rather than failed when the report has none (a live
capture, or a report predating tails).

The message includes the operating system's read error or the TOML parser's position. A budget is
a regression tripwire, not a performance goal: set each ceiling to roughly twice the observed
median so a failure means a real regression, never noise.

Fix: check the path (resolved relative to the current directory) and its permissions, or correct
the TOML at the reported position to match the shape above.",
    },
    CodeEntry {
        code: code::BUDGET_EXCEEDED,
        severity: Severity::Error,
        title: "profile budget exceeded",
        explanation: "\
A measured timing exceeded the ceiling its budget file set for it.

`ludion profile --check` raises one of these per over-budget line — a system, a GPU pass, a
post-process effect group, the frame total, the post total, the cluster build, or the load total —
naming the measured value and the ceiling it crossed, and exits non-zero. Budgets are
release-profile, pinned-hardware regression tripwires with generous headroom (about twice the
observed median); crossing one means the timing genuinely regressed, not that the machine was busy.

The message says which statistic the number is. A single line reports a `median` — or, for a
`frame_total_max_ms` / `[systems_max]` ceiling, a `max` with the frame it occurred at; a group or a
total reports `summed medians` over the member passes measured this run, with that count in
parentheses:

  post group `bloom` (11 passes): summed medians 0.212 ms exceeds budget 0.180 ms
  system `fire` (worst at frame 412): max 3.100 ms exceeds budget 0.800 ms

That is an aggregate breach. It does not say which member grew — the per-pass medians that make it
up are in the report the same run printed, so read them to find the one that moved.

Fix: profile again to confirm the regression reproduces (`ludion profile --scene … --frames N`),
then find and fix the code that got slower. Only if the new cost is intended and permanent should
you re-calibrate the budget file to the new measured median.",
    },
    CodeEntry {
        code: code::BUDGET_UNKNOWN_KEY,
        severity: Severity::Error,
        title: "profile budget names an unproducible system, pass, or group",
        explanation: "\
A budget file's `[systems]`, `[systems_max]`, `[passes]`, or `[post_groups]` table names a key that
does not match anything the profiled engine could ever measure. Each table has its own set of producible names:

  - `[systems]` and `[systems_max]`: the names the profiled engine registered its systems under.
  - `[passes]`: the frame graph's fixed pass names — `shadow`, `prepass`, `ssao`, `ssao_blur`,
    `scene`, `shafts`, `shafts_blur`, `dof_coc`, `dof`, `bloom_threshold`,
    `bloom_down_1`..`bloom_down_5`, `bloom_up_0`..`bloom_up_4`, `composite`, `resolve`, `overlay`.
  - `[post_groups]`: the post-process effect groups — `bloom`, `ssao`, `shafts`, `dof`, `lens`.

This is different from a budgeted name that is simply absent from *this* report — a GPU pass a
UI-free scene never schedules (`overlay`), one a native-scale run never schedules (`resolve`), the
conditional effects' passes a scene did not ask for, or every pass when the run has no GPU adapter,
are producible but unmeasured, and stay quiet. A key matching nothing producible can only
mean the system, pass, or group was renamed, deleted, or the budget entry has a typo — left
unchecked, that budget line would never fire again
(the tripwire it names is permanently disarmed) while `ludion profile --check` still exits 0.

Fix: rename the budget key to the current system/pass/group name (the message names the nearest
match, if one is close), or remove the entry if what it named was intentionally deleted.",
    },
    CodeEntry {
        code: code::SYSTEM_PANICKED,
        severity: Severity::Error,
        title: "a game system panicked",
        explanation: "\
A system's body panicked. The schedule caught it at that system's unwind boundary, skipped the rest
of the tick, and paused the sim; the process is still running and still answering.

Every other authoring mistake in this engine already arrives as a message — a malformed scene names
its JSON Pointer, a bad data-doc edit is L1119 with the store untouched. A panicking system used to
be the exception: game code panics readily by design (an `unwrap` on a component the world no longer
has is one line), and that panic killed the sim thread, after which every request answered L3009 and
the process was a zombie with nothing to say about the cause. This diagnostic is that message. It
names the system by its registration name, quotes the panic payload, and gives the panic site's
file:line:column; set RUST_BACKTRACE=1 to get the full backtrace with it.

This is not L3023. L3023 means a request reached the sim thread and no reply came back — a system
that never terminates, or a handler blocked forever. This one means a system ran and failed
outright. If you are seeing L3023, nothing panicked; look for a loop, not an unwrap.

The auto-pause is deliberate and is what keeps this to one diagnostic instead of an endless stream:
a half-completed system can leave the world in a shape that makes the next frame panic in the same
place, so the sim stops at the failure rather than ticking into it again. The world is left exactly
as the panic left it — that shape is the evidence — and every non-ticking command still works, so
`entity.query`, `component.get`, `sim.hash`, and `render.screenshot` all inspect the failing state
directly. The systems after the panicking one did not run this tick, so their effects are absent by
design, not by corruption. The pause lands after frame N's tick (the frame the message names), so
`sim.hash` and the frame counter report N+1.

The same code covers a system that removed the `InputState` resource: the engine restores an empty
one and pauses, so held input is lost. Reset it in place instead of removing it.

Fix: read the panic message and location; it is ordinary Rust, and the usual shapes are an `unwrap`
on a `get_mut`/`resource` that returned `None` (the entity was despawned, or the component was never
inserted), or an index out of bounds. Handle the missing case rather than unwrapping it — a system
that no longer has its target should return, not assume. After fixing, restart (or `ludion dev`
recompiles and restarts for you). `sim.resume` does not retry the failed tick: it continues at frame
N+1 from the first system, so the systems before the panicking one run again on a world they
already changed, and the panicking system will, in nearly every case, panic again. To retry,
restore a snapshot (`engine.restore`) or reload the scene.

A recording marks the panicking tick with a `system_panic` record. A replay reproduces L4007 only on
that tick: a panic anywhere else fails the replay with L4007 itself, and a marked tick that no longer
panics is L3050. `drive_hashes` on a live engine has no marks, so it propagates every L4007.",
    },
    CodeEntry {
        code: code::SCENE_OVERRIDE_INVALID,
        severity: Severity::Error,
        title: "render-time scene override could not be applied",
        explanation: "\
A `--set <json-pointer>=<value>` override could not be applied to the scene it was given.

`ludion render --set` replaces one value in the loaded scene, in memory, and never writes to disk.
It exists so a look can be swept without editing content: every registered component's data feeds
the WorldHash, so changing a value on disk to try it out invalidates every committed replay, while
an override costs nothing. It is deliberately narrow: it never creates a component or an entity, and
never conjures an intermediate node, because a pointer that could grow structure would let a typo
silently render a scene nobody authored.

The one thing it does add is a missing *final* field inside a component map the scene already
carries — every component field has a behavior-preserving default, so refusing that would mean the
only way to tune a knob a file left unspelled is to override the whole component map, and every
sweep over a newly-added field would be unusable. That addition is safe because it is still the
schema that has the last word: a misspelled field name comes back as `L1014` at the same pointer,
naming the nearest declared field, exactly as the same typo written into the file would.

`ludion render --sweep <json-pointer>=<v,v,…>` is the same primitive with a list: one pointer, one
render per value, stitched into a labelled contact sheet. It raises this code for everything below,
plus its own two spellings — a spec with no values after `=` or an empty value between two commas,
and a list longer than the sheet's 16-cell cap (past that the cells are too small to judge, which is
all a sheet is for; split the range across several sheets). The comma split is literal with no
escape, so a swept value cannot itself contain a comma.

Five things raise this code:

- A malformed spec: no `=` between the pointer and the value, an empty pointer, or a pointer that
  does not start with `/`. Spell it `--set /entities/0/components/Transform/translation/1=2.5`. The
  value is read as JSON when it parses as JSON and as a plain string otherwise, so `=0.8` is a
  number, `=night` is the string \"night\", and `=\"7\"` is the string \"7\".
- A pointer that does not resolve. The help names the deepest prefix that did resolve and, where the
  failing segment is a key, the nearest match among that node's own keys. Addressable nodes are
  `/name`, `/generated_from`, `/seed`, `/entities/<i>/name`,
  `/entities/<i>/components/<Component>`, and any path inside a component's value (object keys and
  sequence indices, RFC 6901, `~0`/`~1` escapes included). Only the pointer's *last* segment may name
  a field the component map does not have; a miss any earlier does not resolve. Pointers at a whole
  entity, at `/entities`, or at an entity's whole `components` map are refused: those change the
  document's shape rather than tune a value.
- A scene-level field given a value it cannot hold — `/name` and `/generated_from` take a string
  (or null to clear), `/seed` a non-negative integer (or null). These are the document skeleton and
  have no schema behind them.
- A `--sweep` value list that is empty, or that carries an empty value between two commas.
- A `--sweep` value list longer than 16.

A wrong-typed or out-of-range *component* value is not this code. The override is applied before
validation runs, so it comes back as the ordinary `L1014` naming the same JSON Pointer and the
schema's own expectation — a better message than this code could give, and the same one an edit to
the file would produce.

## Against a model (`render --model --set` / `--sweep`)

Both spellings also tune a **voxel model's palette** for the render alone, which is what a value
calibration pass adjusts. The addressable domain there is exactly four shapes:

  /palette/<glyph>/color/<0..3>     0 red, 1 green, 2 blue, 3 alpha
  /palette/<glyph>/emissive
  /palette/<glyph>/metallic
  /palette/<glyph>/roughness

Anything else raises this code, as does a glyph the palette does not have (the help lists the ones
it does), a non-numeric value, or a `.mesh.yaml` model, which carries per-vertex colors and no
palette at all — override the scene that places it instead. A `--model --sweep` renders one view per
value, so requesting more than one `--views` entry is this code too.

A scalar the palette entry left unset is created and echoed as `unset -> <value>`, on the same
reasoning as the scene case. Range and finiteness are still the document's own word: the overridden
model is re-emitted and re-validated before it renders, so an out-of-range `metallic` comes back as
the ordinary `L1114`, exactly as writing it into the file would.

## Against an atlas flipbook (`render --atlas --clip`)

The same code covers a flipbook request the atlas instruments cannot serve — the flags there address
a clip rather than a pointer, and each misshape is refused instead of quietly ignored:

- `--time`, `--strip` or `--onion` with no `--clip` to modify. Name the clip too, e.g.
  `--clip ship/bank --time 0.25`.
- `--strip` alongside `--onion`: the phases side by side and the phases stacked are two answers to
  one question, and an image shows one of them. Run the other into a second `--out`.
- A `--strip N` outside `2..=16`. One phase is `--clip <name> --time <t>` spelled the long way, and
  past sixteen a phase is too narrow to read the art in.
- A `--clip` name the atlas carries no flipbook for. The help lists the names it does carry, or says
  it carries none — a different mistake, fixed by declaring one under `flipbooks:` in the
  `kind: atlas-source` document and re-running `generate-atlas`.

Fix: correct the spec as the help suggests, then re-run. `ludion check-scene <file>` prints the
document the pointers address.",
    },
    CodeEntry {
        code: code::STATS_THRESHOLD_FILE_INVALID,
        severity: Severity::Error,
        title: "image-stats threshold file is invalid",
        explanation: "\
An `image-stats --check <file>` threshold file could not be read, is not valid TOML, or names a key
outside the four `ImageStats` metrics.

`ludion image-stats --check` compares every measured metric against a ceiling file: any of the four
metric names at the top level (a ceiling applied to every image), plus an optional `[images.\"<file
name>\"]` table of the same four names overriding the default for one image (keyed by the PNG's
bare file name, so it resolves the same run from any working directory). Every metric is
\"higher is worse\", so every entry is a ceiling — a measured value at or below it passes.

  banding_score = 0.90
  edge_aliasing = 1.00
  clipped_high  = 0.05

  [images.\"night-sky.png\"]
  banding_score = 0.95

An omitted metric is simply not checked, at either level.

Unlike a profile budget's `[systems]`/`[passes]` tables — whose keys name dynamic, producible
system/pass names, so an unknown one is a distinct mistake worth its own code (`L4006`) — the metric
set here is a fixed four (`banding_score`, `edge_aliasing`, `clipped_low`, `clipped_high`). A key
outside that set can only be a typo in a small, closed vocabulary, so it collapses into this same
code rather than earning a second one.

Fix: check the path (resolved relative to the current directory) and its permissions, or correct
the TOML at the reported position — the parser's own message names the offending key when it is one
outside the four metrics.",
    },
    CodeEntry {
        code: code::STATS_THRESHOLD_EXCEEDED,
        severity: Severity::Error,
        title: "image-stats threshold exceeded",
        explanation: "\
A measured `ImageStats` metric exceeded the ceiling its threshold file set for it.

`ludion image-stats --check` raises one of these per over-threshold metric, naming the image, the
metric, the measured value, and the ceiling it crossed, and exits non-zero. These four numbers are
defect *gates*, not a target to optimise toward zero — a still image legitimately carries some
banding, some hard edges, some rail-clipped pixels, and the ceiling exists to catch a regression, not
to chase a perfect score.

Fix: run `ludion image-stats <file>` alone to see the current numbers, confirm the regression is
real (a rendering change that plausibly moved this metric), then fix the code that made it worse.
Only if the new value is intended and permanent should you re-calibrate the threshold file to match.",
    },
    CodeEntry {
        code: code::INVALID_RENDER_SCALE,
        severity: Severity::Error,
        title: "invalid render scale",
        explanation: "\
A render scale was rejected because it is not a finite number between 0.25 and 4.0.

`--scale` is a *client* quality setting, never scene content: the whole chain renders at N x the
requested output resolution and one resampling pass carries the result back to that resolution after
tonemapping — an area-weighted box filter down above `1.0`, a bilinear filter up below it. Either way
it happens before the UI overlay, so screen-space text is drawn at the requested resolution and never
softens, and the caller gets back the extent it asked for. A scale change is therefore a quality
change, never a look change.

Both ends of the range are closed deliberately. Above `4.0` the cost grows as the square while the
visible gain has long since stopped: 16x the pixels of native is already past the point where the
resolve's box filter has anything left to average. Below `0.25` the frame is under a sixteenth of
native's pixels — one source texel per 4x4 block of output pixels — and what arrives is the
magnifier's own filter rather than the image.

Fix: pass a finite factor in `[0.25, 4.0]` — `1.0` is native (no resampling pass at all,
byte-identical to a run with no flag), `1.5` and `2.0` buy quality, `0.75` and `0.5` buy frame time
on a machine that cannot afford native.",
    },
    CodeEntry {
        code: code::PROFILE_CAPTURE_INVALID,
        severity: Severity::Error,
        title: "invalid profile capture request",
        explanation: "\
A `profile.capture` named a frame count the engine will not measure.

The capture measures the next N *live* frames — the ones the host is already producing — and its
reply is deferred until that window closes. Two ends of the range are therefore closed:

- **Zero frames** measures nothing and would never close a window, so the request would hang until
  the transport's reply timeout and then be reported as a stalled sim thread. It is refused up
  front instead, where the message can say what went wrong.
- **Too many frames** has the same failure mode more slowly. The absolute ceiling is one minute of
  frames at the default 60 Hz tick rate; past it the wait is indistinguishable from a wedged sim
  thread from outside the sim thread, which is exactly how it would be reported (`L3023`).

A window that fits the frame range but not the current speed or tick rate is `L4023`, not this code.

Fix: ask for a frame count in range — `{\"frames\": 60}` is one second at the default tick rate and
is the default when the parameter is omitted. To watch cost continuously instead of capturing a
fixed window, subscribe to the `profile` channel: windows arrive about once a second for as long as
the subscription is open, and no capture is involved.",
    },
    CodeEntry {
        code: code::PROFILE_CAPTURE_BUSY,
        severity: Severity::Error,
        title: "a profile capture is already in flight",
        explanation: "\
A `profile.capture` arrived while an earlier capture was still measuring, or while its finished
report had not yet been handed back.

One capture at a time is the whole rule, and it is deliberate. A capture arms a window over the
frames the host is *already* producing; two overlapping windows would measure the same frames twice
and report two answers to the same question, and the second request has no way to say which frames
it meant. There is no queue behind this either — a capture that waited its turn would describe
frames the caller never asked about.

This is not a busy engine or a resource limit: nothing is being contended for. It only ever means a
second capture was issued before the first one's reply came back.

Fix: wait for the in-flight capture's reply — it arrives when its window closes, which is its
`frames` count of live frames away — then issue the next one. If several observers each want a
running picture, they do not need captures at all: any number of clients may subscribe to the
`profile` channel at once and every one of them receives every window.",
    },
    CodeEntry {
        code: code::PROFILE_CAPTURE_CANNOT_CLOSE,
        severity: Severity::Error,
        title: "a profile capture cannot close its window",
        explanation: "\
A `profile.capture` measures the next N *live* sim frames and answers when the last one has run. A
paused sim produces none, and a slow one produces too few to finish inside the wall-clock bound
(`MAX_CAPTURE_WALL_SECONDS`, half the transport's reply budget), so such a capture would otherwise
hang until the transport called the sim stalled (`L3023`). It is answered at once instead. Three
cases:

- **Armed while paused** — refused up front; nothing is armed.
- **Window too long at the current speed and tick rate** — refused up front on `/params/frames`;
  the message names the largest frame count that fits now.
- **Became unclosable mid-flight** — a `sim.pause` (a `sim.step` that finishes pauses too, and so
  does a system panic), or a `sim.set_speed` that slows the remaining frames past the bound. This is
  re-evaluated only when the run state or speed changes, against the wall budget left at that moment;
  a host that is merely late is not cancelled. The held reply is answered with this code and the
  capture is cancelled, so the next capture arms (no `L4013`).

Fix: `sim.resume`, capture, then `sim.pause` again; or raise the speed with `sim.set_speed`; or ask
for fewer frames. To watch cost on a paused or slow sim, subscribe to the `profile` channel instead.",
    },
    CodeEntry {
        code: code::SPRITE_IMPORT_NEEDS_QUANTIZE,
        severity: Severity::Error,
        title: "PNG has more colours than a sprite doc can name",
        explanation: "\
A PNG offered to the sprite-document importer holds more distinct RGBA colours than a sprite
document has glyphs to name them with, and no `--quantize N` was given.

A `kind: sprite` document is a palette-indexed glyph grid: one printable-ASCII character per palette
entry, `0x21..=0x7E` minus the reserved `.`, which is 93 colours. That budget is the format's whole
point — the document has to stay *readable and editable as text*, and a grid whose glyphs run out of
distinct characters is neither. So the importer refuses rather than inventing a second character
class or silently dropping colours.

The usual cause is a source image that was never pixel art: a photograph, an anti-aliased render, or
a gradient exported at 8 bits per channel, where a handful of visible tones are actually thousands
of near-identical colours.

Fix: pass `--quantize N` (N at most the glyph budget) to merge colours by deterministic median cut —
the import then reports the loss as `L4015`, colours before and after plus the largest per-channel
error, so you can judge it. Or reduce the palette in the source image first, which keeps the import
lossless and puts the artistic decision where an artist can see it.",
    },
    CodeEntry {
        code: code::SPRITE_IMPORT_QUANTIZED,
        severity: Severity::Warning,
        title: "PNG import quantized its palette",
        explanation: "\
A sprite-document import ran with `--quantize N` and merged colours to fit the glyph budget, so the
written document is not a lossless copy of its PNG.

This is a warning, not a failure: the import asked for quantization and got it. It exists because a
lossy conversion that reports nothing is the kind of thing that is discovered three edits later, on
screen, with no way to tell whether the tone that looks wrong was authored wrong or merged away. The
message names the colour count before and after and the largest per-channel error, which is enough
to decide without opening an image editor.

Fix: nothing is required. If the reported error is larger than you want, raise `N` (up to the glyph
budget), or reduce the palette in the source image so the import can run lossless. Remember the
written document is now the source of truth — it is meant to be hand-edited from here, so the PNG's
extra colours are gone for good.",
    },
    CodeEntry {
        code: code::COMMAND_PANICKED,
        severity: Severity::Error,
        title: "a command handler panicked",
        explanation: "\
A command's handler panicked while the live loop was executing it. The loop's per-command unwind
boundary caught the panic, answered the request with this diagnostic instead of a reply, and paused
the sim; the process is still running and still answering.

The world is left exactly as the panic left it — a handler may have applied half its edit — because
that shape is the evidence. The command was not recorded: a recording that included it would
diverge from the world on replay. The message names the method and the frame and quotes the panic
payload; the panic's file:line:column and any backtrace are on the host's stderr (set
RUST_BACKTRACE=1 for the full one).

This is not L4007, which is a panic inside a game *system* during a tick, and it is not L3023, which
means a request reached the sim thread and no reply came back at all.

Fix: fix the handler — the panic site is on stderr. To get back to a known world without
restarting the host, `engine.restore` a snapshot or `scene.load` the scene, then `sim.resume`.",
    },
    CodeEntry {
        code: code::SIM_THREAD_PANICKED,
        severity: Severity::Error,
        title: "the sim thread panicked",
        explanation: "\
A panic outside any command or system boundary ended the sim thread. Commands and systems each run
inside their own unwind boundary (L4016, L4007), so this is a panic in the live loop itself —
between those boundaries — and there is no sim left to answer requests. The host stopped and exits
non-zero rather than staying up as a zombie that answers L3009 to everything.

The message quotes the panic payload; the panic's file:line:column and any backtrace are on the
host's stderr (set RUST_BACKTRACE=1 for the full one).

Fix: this is an engine defect, not a content mistake — the stderr location names the site. Restart
the host to get a fresh sim.",
    },
    CodeEntry {
        code: code::SYSTEM_NAME_DUPLICATE,
        severity: Severity::Error,
        title: "a system name is registered twice",
        explanation: "\
Two systems in one schedule carry the same name, or a game system took a name the engine reserves.
A system's name is its identity: the recording of component access, the query-drift check, the
profiling spans and the L4007 panic report all key by name, so two systems sharing one would merge
into a single entry and hide which of them did the work.

The engine builder refuses the duplicate at build time and names both registration sites; a bare
`Schedule` panics with this code instead. The names `physics_write`, `physics_step` and
`physics_mirror` are reserved — a scene's `physics:` block installs systems under them.

Fix: give every system a distinct name (the function's own name is the convention).",
    },
    CodeEntry {
        code: code::BUDGET_GPU_UNMEASURED,
        severity: Severity::Error,
        title: "profile budget gates GPU timings but none were measured",
        explanation: "\
A budget file names GPU lines (`[passes]`, `[post_groups]`, `post_total_ms`, `gpu_frame_ms` or
`gpu_frame_max_ms`), but the profile run measured none of them: the host had no GPU adapter, the
adapter has no timestamp queries, the graph schedules more passes than the timing ring holds (they are
named and untimed), or — for the two frame keys — the backend cannot prove one queue
timeline for a frame, so the report carries no `gpu.frame_median_ms`. Nothing
measured cannot be over budget, so without this error the check would exit 0 with every GPU line
silently unchecked — a green gate that gated nothing.

This is different from one named pass absent from a run that did measure the GPU (a UI-free scene
never schedules `overlay`): that stays quiet, because the scene, not the machine, decided it.

Fix: run the check on the pinned box whose adapter the budget was calibrated on, or — for a
CPU-only budget — remove the GPU lines from the file.",
    },
    CodeEntry {
        code: code::BUDGET_QUALITY_MISMATCH,
        severity: Severity::Error,
        title: "a reduced-quality, anti-aliased or thread-mismatched profile report cannot be checked against a budget",
        explanation: "\
A profile report measured below full quality, with anti-aliasing on, or at a different sim worker
count than the budget names, was handed to a budget check, which refuses it.

A budget is calibrated against everything the scene authors, so its ceilings describe frames a
reduced-quality run never rendered: the comparison would pass for a reason nobody chose, which is
worse than not running it. The refusal reads off the report's own `quality` descriptor, so it covers
an `--attach` capture of someone else's host exactly as it covers a local run. A report carrying no
descriptor predates the field and is compared as before.

An anti-aliased capture (`quality.anti_aliasing` other than `off`) is refused the same way: it pays
passes the budget never measured.

A budget's optional `threads` key names the sim worker count its CPU ceilings were calibrated at; a
report at another count is refused, because the ceilings only hold there. A report carrying no count
predates the field and is compared as before.

Fix: re-measure at `--quality full` (or with `LUDION_QUALITY` unset), with `--aa off`, and with
`--threads <N>` set to the budget's `threads`, and check that report.",
    },
    CodeEntry {
        code: code::PROFILE_SAMPLES_MISSING,
        severity: Severity::Error,
        title: "the profile report carries no per-frame samples",
        explanation: "\
`ludion profile --perfetto` builds a timeline from the report's per-frame `samples`, and this report
has none.

A fresh `ludion profile --scene ...` or `--attach <url> --perfetto <out>` capture always requests
them, so the report is either an older committed report JSON that predates the field, or a
`profile.capture` reply taken without `include_samples`.

Fix: re-run the capture with `--perfetto` set, so the report carries its samples.",
    },
    CodeEntry {
        code: code::SIM_THREAD_STUCK,
        severity: Severity::Error,
        title: "the sim thread did not stop in time",
        explanation: "\
The host asked the sim thread to stop and it was still busy after the grace period (5 seconds). The
message names what it was doing: running a command (a huge `sim.step` is the usual one), running a
sim tick, or rendering a frame. The sim loop only looks at the stop request between those steps, so
one that never returns holds the thread for good. Two other answers mean a slow stop, not a wedged
sim: still booting (loading the scene, building the renderer), or still winding down after its last
step (dropping the engine, flushing a recording, releasing the GPU).

The host does not wait for it: it leaves the thread detached and exits non-zero, because waiting
would hang the process on a world nobody can reach any more. A recording that was armed is
incomplete.

Fix: find what never returns — a loop in a system, a `sim.step` with a tick count that cannot finish.
Split long steps into bounded ones.",
    },
    CodeEntry {
        code: code::HASHED_NON_FINITE_FLOAT,
        severity: Severity::Warning,
        title: "a hashed component holds a non-finite float",
        explanation: "\
A component the world hash walked holds a NaN or an infinite float. The message names the component
type, the first entity found with one and how many non-finite values that hash met.

A non-finite float in sim state is almost always the first visible symptom of a bug: a division by
zero, the normalisation of a zero vector, an unclamped `acos`. The hash does not fail on it — it
encodes NaN, `+inf` and `-inf` as three distinct values, so a run that diverges into one of them
shows up as a hash mismatch — but every later frame inherits the poisoned value.

It is reported once per component type per session, by the first hash that meets one (`sim.step`
with `hash`, `sim.hash`, a replay hash trace, `ludion sim-hash`).

Fix: find the writer — `ludion replay` the session and bisect for the first frame whose hash moves
— and guard the maths there.",
    },
    CodeEntry {
        code: code::BUDGET_LOAD_UNMEASURED,
        severity: Severity::Error,
        title: "profile budget gates the load total but the report has no load section",
        explanation: "\
A budget file sets `load_total_ms`, but the profile report carries no `load` section: this run
measured no scene load. A live `--attach` capture joins a world that is already loaded, a
`--profile-replay` run replays recorded frames, and a host that installs no load observer records
nothing. Nothing measured cannot be over budget, so without this error the check would exit 0 with
the load line silently unchecked — a green gate that gated nothing.

Fix: measure the load with `ludion profile --scene <scene>` or a game's `--profile`, both of which
observe it, and check that report; or remove `load_total_ms` from a budget that is applied to live
captures.",
    },
    // --- L5xxx: audio ---
    CodeEntry {
        code: code::PROJECTILE_SPAWN_REFUSED,
        severity: Severity::Warning,
        title: "projectile spawn refused",
        explanation: "\
`spawn_projectile` did not fire a shot. The message names the cause, which is reported once per
cause per session:

- no pool: the game never called `Engine::builder().projectiles(..)`;
- pool full: every slot holds a live projectile;
- an invalid field: the description has a non-finite position or velocity, a negative or
  non-finite radius, drag or lifetime, or a gravity scale that is not finite. The message names the
  field.

The shot is dropped and the game continues.

Fix: add `.projectiles(ProjectileConfig { .. })` to the builder, raise `ProjectileConfig::capacity`
or shorten the shots' `lifetime`, or guard the maths that built the description (a division by
zero, a normalise of a zero vector).",
    },
    CodeEntry {
        code: code::PROJECTILE_CONFIG_INVALID,
        severity: Severity::Error,
        title: "projectile config is invalid",
        explanation: "\
A `ProjectileConfig` cannot build a pool. The message names the field:

- `capacity` must be in `1..=32768`;
- `gravity` must be three finite numbers (m/s^2);
- `ignore_layers` must name layers declared in the scene's `physics.layers`, and no name may be
  empty.

Raised by `Engine::builder().build()` for the first two and for an empty name, by a snapshot
restore whose recorded config fails the same check, and, for an unknown layer name, by
`projectiles_step` (reported once; the pool does not move until the name is fixed).

Fix: correct the field the message names. An unknown layer is a typo against `physics.layers`.",
    },
    CodeEntry {
        code: code::PROJECTILE_VISUALS_INVALID,
        severity: Severity::Error,
        title: "projectile visuals are invalid",
        explanation: "\
A `ProjectileVisuals` table given to `Engine::builder().projectile_visuals(..)` cannot be drawn.
The path names the field (`/tags/3/size`, `/fallback/color/0`):

- `size` must be a finite number > 0 (metres);
- `stretch`, `emissive`, each `color` channel and `converge_distance` must be finite and >= 0;
- a billboard's `softness` must be in `0..=1`.

Raised by `Engine::builder().build()`.

Fix: correct the field the path names.",
    },
    CodeEntry {
        code: code::PROJECTILE_MUZZLE_UNRESOLVED,
        severity: Severity::Warning,
        title: "projectile muzzle is unresolved",
        explanation: "\
`ProjectileVisuals.muzzle` names an entity, but the frame cannot use it as the muzzle a shot is
drawn from. One of:

- no entity has that `Name`;
- the entity is not a `Viewmodel` and has no `Viewmodel` ancestor;
- the scene has no single perspective first-person camera, or a `render.view` override is in force.

Shots draw at their simulated position, with no convergence. The warning is latched once per name.

Fix: name a `Viewmodel` entity (or a child of one) in a scene whose camera has a first-person
`CameraRig`, or set `muzzle` to `None`.",
    },
    CodeEntry {
        code: code::PROJECTILE_STEP_NOT_RUN,
        severity: Severity::Warning,
        title: "projectiles_step never ran",
        explanation: "\
The projectile pool holds live shots, but no `projectiles_step` system ever ran, so the shots
never move, hit or expire and the pool fills until spawns are refused (`L4026`).

Raised once per session, after a tick.

Fix: register `.system(\"projectiles_step\", projectiles_step)` on the `EngineBuilder`, after
`spatial3d_rebuild`.

Known limit: a wrapper that calls `projectiles_step` only behind its own guard, such as a menu
state, warns until its first call.",
    },
    CodeEntry {
        code: code::PROJECTILE_RESTORE_CAPACITY,
        severity: Severity::Warning,
        title: "projectile pool too small for the snapshot",
        explanation: "\
A snapshot restore found the build's `ProjectileConfig::capacity` below the number of slots the
snapshot's pool had ever used (its high-water mark). Shrinking would drop live shots and leave a
free list no run produced, so the snapshot's pool was not restored: the engine starts a fresh,
empty pool at the build's capacity. The rest of the restore proceeds.

A restore otherwise takes the build's `ProjectileConfig` (gravity, targets, layers, capacity), not
the one the snapshot was taken under.

Fix: raise `ProjectileConfig::capacity` to at least the number the message names, or restore with
the old build.",
    },
    CodeEntry {
        code: code::NAV_NOT_READY,
        severity: Severity::Error,
        title: "a navigation query ran before the grid exists",
        explanation: "\
`nav_path`, `nav_distance`, `nav_reachable` or `nav_project` was called on a world with no `Nav` resource, or
before `nav_update` built the first grid.

Fix: opt in with `Engine::builder().nav(NavConfig { .. })`, and run `nav_update` (it builds the grid on its
first tick) before querying.",
    },
    CodeEntry {
        code: code::NAV_PATH_REQUEST_INVALID,
        severity: Severity::Error,
        title: "a navigation path request is invalid",
        explanation: "\
A `PathRequest` has a non-finite `from` or `to`, a negative or NaN `max_cost`, a negative or non-finite
`snap_radius`, or a `max_expanded` of 0. `nav_distance`, `nav_reachable` and `nav_project` check their own
point and radius the same way.

Fix: build the request with `PathRequest::new(from, to)` and override only finite, non-negative fields.",
    },
    CodeEntry {
        code: code::NAV_CONFIG_INVALID,
        severity: Severity::Error,
        title: "nav config is invalid",
        explanation: "\
A `NavConfig` cannot build a walkability grid. The message names the field:

- `origin`, `cell_size`, `y_min`, `y_max`, `agent_radius`, `agent_height`, `max_step`,
  `max_slope_deg` and `flow_radius` must be finite; the sizes and `flow_radius` must be positive;
- `nx` and `nz` must be at least 1, and `nx * nz` at most 4194304;
- `y_min` must be below `y_max`;
- `max_slope_deg` must be in `(0, 90)`;
- `max_path_requests_per_tick` must be at least 1;
- `ignore_layers` must name layers declared in the scene's `physics.layers`, and no name may be
  empty.

Raised by `Engine::builder().build()` for everything but an unknown layer name, and by `nav_update`
for that (reported once; the grid keeps its last build until the name is fixed).

Fix: correct the field the message names.",
    },
    CodeEntry {
        code: code::PROJECTILE_LOOK_ASSET,
        severity: Severity::Error,
        title: "projectile look mesh or voxel model cannot be drawn",
        explanation: "\
A `ProjectileLook::Mesh { asset }` or `ProjectileLook::Voxel { model }` names a document the engine
cannot draw.

- At `Engine::builder().build()` the file `asset` or `model` (relative to the project root) does not exist; the
  build is refused and the help names the nearest asset.
- At extract the file exists but failed to load or parse. The diagnostic is reported once per asset,
  that tag draws nothing, and the frame continues.

Fix: point `asset` at an existing `*.mesh.yaml` (or `model` at a `*.voxel.yaml`), or repair the document the message names.",
    },
    CodeEntry {
        code: code::PHYSICS_SCALE_UNGATED,
        severity: Severity::Warning,
        title: "physics scale past the gated scale",
        explanation: "\
The scene has more non-static physics bodies than the gated scale of 512. The count is every rigid
and kinematic solver body, character motors included; static bodies are never counted.

512 is the scale the pinned budgets gate; `ludion docs physics-scale` lists which scene shapes are
gated and how. Past the gated scale the step cost is unmeasured on the pinned boxes.

Raised by `check-scene` (and the wire `scene.check`) for the authored count, and once per session at
runtime for the live count; `ludion profile` and `--profile` print it. It only warns: nothing is
dropped and the exit code does not change.

Fix: build a stress scene at the game's shape and measure it against a budget on each pinned box
(`ludion devdocs physics-pin`, `ludion docs physics-scale`).",
    },
    CodeEntry {
        code: code::POSE_BLEND_POSE_MISSING,
        severity: Severity::Warning,
        title: "pose blend names a pose no part of its group carries",
        explanation: "\nA `PoseBlend` on a ragdoll group root has a weight above 0, but none of the `JointPoses` parts grouped
to that root has a pose of that name, so no part is driven: they all stay limp.

A part is grouped to a root by `JointPoses.group`, else by its `SkinBone.skin`. Reported once per
(group, pose) while the weight stays above 0; a weight of 0 never warns, so a root may carry a
`PoseBlend` before its parts exist. It only warns: nothing is dropped and the exit code does not
change.

Fix: add the pose to the parts' `JointPoses.poses`, correct the `PoseBlend.pose` spelling, or check
that the parts reach the root through `group:` or `SkinBone.skin`.",
    },
    CodeEntry {
        code: code::LIGHT_EXPOSURE_INVALID,
        severity: Severity::Error,
        title: "light exposure query is malformed",
        explanation: "\
`light_exposure` refuses a query it cannot answer. Three cases, each with a JSON Pointer:

  - `/points`: the list is empty or holds more than 16 points (`MAX_EXPOSURE_POINTS`);
  - `/points/<i>`: point `i` has a NaN or infinite coordinate;
  - `/facing`: a `Facing::Normal` that is zero-length or non-finite.

Fix: pass between 1 and 16 finite points, and a non-zero finite normal (or `Facing::Omni`).
See `ludion docs light-exposure`.",
    },
    CodeEntry {
        code: code::LIGHT_EXPOSURE_PRESENTATION_LIGHT,
        severity: Severity::Warning,
        title: "presentation light never counts toward light exposure",
        explanation: "\
A scene with a `LightExposure` subject has a `PointLight`, `SpotLight` or `DirectionalLight` on an
entity marked `presentation: true` (pointer `/entities/{i}/components/{PointLight|SpotLight|DirectionalLight}`).
The light is drawn, but `light_exposure_update` reads sim entities only, so the light is never
counted in any reading: what the player sees and what a guard perceives disagree.

Fix: drop `presentation: true` from the light if it should count (it then joins the world hash), or
leave it when the light is decoration and add a sim-side light for what should count.",
    },
    CodeEntry {
        code: code::LIGHT_EXPOSURE_OCCLUDERS_UNMATCHED,
        severity: Severity::Warning,
        title: "light exposure occluder layers match no collider",
        explanation: "\
A `LightExposure` sets `occluder_layers`, but no 3D-claimed collider in the scene is on any of
those layers (through `Collider.layer`, `default` when absent, or `QueryLayers`), so a shadow ray
can never be blocked (pointer `/entities/{i}/components/LightExposure/occluder_layers`).

Fix: put the occluders on the named layer (`Collider.layer` or `QueryLayers`), or correct the
names; remove `occluder_layers` to count static colliders only.",
    },
    CodeEntry {
        code: code::AUDIO_DEVICE_UNAVAILABLE,
        severity: Severity::Warning,
        title: "no audio output device available",
        explanation: "\
The audio playback sink could not open an output device, so it plays silent.

Sound is a peripheral concern of the engine, never a determinism-bearing one: audio events are a
pure function of the simulation, logged and verifiable in-process, and playback happens on a
separate thread outside every determinism gate. So a machine with no audio device — a CI runner, a
headless server, a container — is not an error. The sink reports this once as a warning, then keeps
routing every audio event, warning of each broken sound and track as playback would (`L1101`/
`L1104`/`L5003`), so the run proceeds exactly as it would with sound, just inaudibly. It keeps
retrying the open with backoff, so a device that appears later is picked up with the declared
loops and the music. Nothing fails, and no test gates on it.

Fix: nothing is required — the warning is informational. If you *expected* to hear sound on a
desktop session, check that an output device is connected and selected as the system default, and
that no other application is holding exclusive access to it.",
    },
    CodeEntry {
        code: code::AUDIO_STREAM_FAILED,
        severity: Severity::Warning,
        title: "the audio output stream failed",
        explanation: "\
An output stream that had opened successfully was lost while playing, so the run is silent until the
default output reopens.

This is the running counterpart of L5001, and it is a warning for the same reason: playback sits
outside every determinism gate, so losing a device mid-run cannot make a simulation wrong. It
usually means the device went away underneath the process — unplugged, switched by the OS,
reconfigured to another sample rate, or grabbed by another application in exclusive mode.

A stream counts as lost when cpal reports an error, when a stream that had been rendering stops
calling back for 500 ms, when a stream never calls back within 5 s of starting, or when its
callback is gone. One-shots above half the command queue are dropped at any time, so a burst never
reads as a lost stream. The sink reports this once, drops the stream, keeps routing audio events so
nothing upstream ever blocks, and retries the system
default output with backoff (250 ms, doubling, up to 5 s, never giving up; the wait starts over
once a stream has played for 5 s). The declared loops and the music come back on the new stream,
fading in; one-shots fired meanwhile are dropped. A line notes the reopen.

Fix: nothing is required — no restart. If no sound returns, reconnect or reselect the output device.",
    },
    CodeEntry {
        code: code::MUSIC_TRACK_UNPLAYABLE,
        severity: Severity::Warning,
        title: "a music track could not be streamed",
        explanation: "A music track named by `music_play` could not be streamed, so it is skipped and nothing plays.

The sink opens music as an Ogg Vorbis file under the content root and streams it from disk. The
file may be missing or unreadable, may not be Ogg Vorbis at all (a WAV renamed `.ogg`, or Opus in an
Ogg container, which is not supported), may have failed to decode partway through, or may be encoded
below the 8000 Hz floor (`ludion_assets::MIN_SAMPLE_RATE`) — a rate that low is refused rather than
played at the wrong pitch. This is a warning for the same reason as L5001 and L5002: playback sits
outside every determinism gate, so a broken track cannot make a simulation wrong, and audio never
gates a run. The sink warns once per file version — a track that is rewritten is tried again on the
next `music_play`.

Fix: encode the track as Ogg Vorbis, e.g. `ffmpeg -i in.wav -c:a libvorbis -q:a 5 out.ogg`; for a
track below the floor, resample it (`ffmpeg -i in.ogg -ar 44100 out.ogg`); and check the path is
relative to the content root.",
    },
    CodeEntry {
        code: code::AUDIO_VOICE_CAPS_INVALID,
        severity: Severity::Error,
        title: "the `[audio]` voice caps are invalid",
        explanation: "The `[audio]` table of `ludion.toml` holds a key or a value the engine cannot use, so the manifest is refused.

The playback sink bounds how many voices sound at once: at most `voices_per_sound` live one-shots
of one sound (default 4) and `voices_total` live voices overall (default 64). When either cap is
full, the oldest one-shot is stolen with a short fade; loops count but are never stolen. The
manifest overrides both defaults, and nothing else — `voices_per_sound` and `voices_total` are the
only legal keys, each an integer from 1 to 256. The upper bound exists because the mixer reserves
its voices up front, so a huge total costs memory before a single sound plays. A `voices_per_sound`
above `voices_total` is refused too: a per-sound cap the total always reaches first can never
bind, which is almost certainly a mistake. The pair is judged after defaults fill the absent key,
so `voices_per_sound = 100` alone fails against the default total of 64.

Fix: write the table with integers in range, e.g.

    [audio]
    voices_per_sound = 6
    voices_total = 96",
    },
    CodeEntry {
        code: code::AUDIO_GAIN_NON_FINITE,
        severity: Severity::Warning,
        title: "an audio gain is not a finite number",
        explanation: "A game queued a sound, a loop or the music with a gain that is NaN or infinite, so it plays at gain 0.

A non-finite gain would poison everything the mixer sums it with: one NaN voice makes the whole
output NaN until it retires, and a looping voice never retires. The audio queue therefore replaces
the gain with 0 before the message leaves the engine. A loop stays declared at 0, so its next valid
gain slews it back in, and nothing else in the message stream changes. A negative finite gain is
left alone: it does not poison a mix. This is a warning
for the same reason as the rest of L5xxx: audio sits outside every determinism gate. It is reported
once per sound per session.

Fix: look for a division by zero in a computed volume, such as `hp / max_hp` with `max_hp` 0, or a
distance falloff that divides by a zero radius, and clamp or guard it before it reaches the audio
verb.",
    },
    CodeEntry {
        code: code::MUSIC_STEM_MISMATCHED,
        severity: Severity::Warning,
        title: "a music stem does not match its set",
        explanation: "A music set named a stem that cannot share the set's playhead, so the sink left it out and played the rest.

The stems of a set are decoded side by side and mixed from one sample counter, so they stay locked
only while they are the same length at the same sample rate. The first stem that opens sets the
reference; a later stem with another sample rate or another length (the last Ogg page's granule
position) is dropped, and so is every stem past the eighth. This is a warning for the same reason
as the rest of L5xxx: audio sits outside every determinism gate. It is raised once per set
(not once per file, because the set is what is wrong), and again if the files change.

Fix: export all stems of a set from one session at one sample rate and one length (pad the shorter
ones with silence), and keep a set to 8 stems or fewer.",
    },
    CodeEntry {
        code: code::AUDIO_DUCK_RULE_INVALID,
        severity: Severity::Error,
        title: "an audio duck rule is invalid",
        explanation: "\
An `Engine::builder().audio_duck(rule)` rule cannot run, so `build` refuses it. The message names the
rule's index and the bad field.

A rule ducks the buses in `duck` by `depth_db` while any sound on its `when` bus is live. The engine
only knows when a sound is live for the voice-like buses, so `when` may not be `Master` (it is the
sum of the rest) or `Music` (streamed music has no tracked end). `depth_db` must be finite and at
most 0: a duck never boosts. `duck` must name at least one bus, and may not name `Master` or the
`when` bus itself, which would duck the very sound that triggers it.

Fix: write the rule in the valid shape, e.g.

    DuckRule { when: Bus::Voice, duck: &[Bus::Music, Bus::Ambient],
               depth_db: -9.0, attack_ticks: 6, release_ticks: 30 }",
    },
    CodeEntry {
        code: code::AUDIO_OCCLUSION_INVALID,
        severity: Severity::Error,
        title: "an audio occlusion config is invalid",
        explanation: "\
An `Engine::builder().audio_occlusion(config)` config cannot run, so `build` refuses it. The message
names the bad field.

Occlusion casts `rays` (1, 3 or 5) from the listener to each positional source. `spread_m` is the
side-ray offset at the source and must be finite and at least 0. `per_hit_db` is the loss per distinct
blocker on one ray and `max_db` the floor; both must be finite and at most 0, and `max_db` may not be
above `per_hit_db`. `occluded_low_pass_hz` is the cutoff when every ray is blocked and must lie
strictly between the filter's open ends (20 Hz and 20 kHz). `interval_ticks` must be at least 1, and
`max_rays_per_tick` at least `rays`, or no source could ever be cast.

Fix: start from `AudioOcclusion::default()` and change one field at a time.",
    },
    CodeEntry {
        code: code::AUDIO_OCCLUSION_QUERY_REJECTED,
        severity: Severity::Warning,
        title: "the audio occlusion query was refused",
        explanation: "\
The physics query behind audio occlusion refused the `QueryFilter` in `AudioOcclusion::filter`, most
often because it names a physics layer the game never declared. This is a warning, raised once per
engine: every source then reads as unoccluded, so the game keeps playing.

Fix: declare the layer in `physics.layers`, or change `include_layer` / `ignore_layer` in the filter
to a declared name.",
    },
    CodeEntry {
        code: code::DIALOGUE_CONFIG_INVALID,
        severity: Severity::Error,
        title: "the dialogue config is invalid",
        explanation: "\
`Engine::builder().dialogue(config)` was given a config the engine cannot run: an empty
`default_locale`, or a reading-time constant (`base_s`, `per_char_s`, `min_s`) that is negative or
not finite. `build` refuses it.

Fix: give `default_locale` a locale tag such as `en`, and finite non-negative reading constants.",
    },
    CodeEntry {
        code: code::STRINGS_DUPLICATE_KEY,
        severity: Severity::Error,
        title: "a string key is defined twice",
        explanation: "\
Two `kind: strings` docs of one locale define the same key (pointer
`strings/{doc path}/strings/{key}`, at the later document). A string table is one flat map per locale,
so the second definition would silently shadow or lose the first.

Fix: delete one definition, or rename the key.",
    },
    CodeEntry {
        code: code::DIALOGUE_DUPLICATE_LINE,
        severity: Severity::Error,
        title: "a dialogue line id is defined twice",
        explanation: "\
Two `kind: dialogue` docs define a line with the same id (pointer
`dialogue/{doc path}/lines/{id}`, at the later document). `say` resolves a line by id across every
dialogue doc, so the id must be unique.

Fix: rename one line.",
    },
    CodeEntry {
        code: code::DIALOGUE_DUPLICATE_SPEAKER,
        severity: Severity::Error,
        title: "a speaker id is defined twice",
        explanation: "\
Two `kind: dialogue` docs define a speaker with the same id (pointer
`dialogue/{doc path}/speakers/{id}`, at the later document).

Fix: rename one speaker, or delete the duplicate.",
    },
    CodeEntry {
        code: code::DIALOGUE_UNKNOWN_SPEAKER,
        severity: Severity::Error,
        title: "a dialogue line names an unknown speaker",
        explanation: "\
A line's `speaker` is not the id of any speaker in the loaded `kind: dialogue` docs (pointer
`dialogue/{doc path}/lines/{id}/speaker`).

Fix: add the speaker under `speakers:`, or correct the id.",
    },
    CodeEntry {
        code: code::STRING_KEY_MISSING_DEFAULT,
        severity: Severity::Error,
        title: "a string key is missing from the default locale",
        explanation: "\
A dialogue line `text` or a speaker `name` is a string-table key, and the default locale has no
text for it (pointer `dialogue/{doc path}/lines/{id}/text` or `.../speakers/{id}/name`). The default
locale is the fallback for every other one, so it must hold every key.

Fix: add the key to a `kind: strings` doc whose `locale` is the config's `default_locale`.",
    },
    CodeEntry {
        code: code::STRINGS_PLACEHOLDER_MISMATCH,
        severity: Severity::Warning,
        title: "a translation uses different placeholders",
        explanation: "\
A string in a non-default locale uses a different set of `{name}` placeholders than the default
locale's text (pointer `strings/{doc path}/strings/{key}`). The line is filled from one list of
arguments, so a translation that drops a placeholder loses the value and one that adds a placeholder
shows it unfilled.

Fix: make the translation use the same placeholders. Literal braces are written `{{` and `}}`.",
    },
    CodeEntry {
        code: code::DIALOGUE_LINE_INVALID,
        severity: Severity::Error,
        title: "a dialogue line is invalid",
        explanation: "\
An authored line cannot play as written: `duration_ticks` is 0 (a line lasts at least one tick), or
a cue's `at` is at or past the line's `duration_ticks` (pointer
`dialogue/{doc path}/lines/{id}/duration_ticks` or `.../cues/{name}`), so the cue could never fire.

Fix: lengthen the line, or move the cue earlier.",
    },
    CodeEntry {
        code: code::STRINGS_LOCALE_INVALID,
        severity: Severity::Error,
        title: "a strings doc has an empty locale",
        explanation: "\
A `kind: strings` doc has an empty `locale` (pointer `strings/{doc path}/locale`), so its keys
belong to no locale.

Fix: set `locale:` to a tag such as `en` or `fr`.",
    },
    CodeEntry {
        code: code::DIALOGUE_UNKNOWN_LINE,
        severity: Severity::Error,
        title: "a dialogue line is unknown",
        explanation: "\
`dialogue::say` (or `say_with`, `say_sequence`) named a line id that no loaded `kind: dialogue` doc
defines, or a line whose speaker no doc defines. Nothing was queued.

Fix: check the id against the `lines:` of your dialogue docs, and that the docs sit under the content
root.",
    },
    CodeEntry {
        code: code::DIALOGUE_NO_SPEAKER,
        severity: Severity::Error,
        title: "a world line has no speaker",
        explanation: "\
A line spoken by a `world`-channel speaker was said without a live speaker entity. Its slot is
per speaker entity, so without one it has nowhere to queue. Nothing was queued.

Fix: pass `Some(entity)` to `say_with` (or the sequence's speakers list), or give the speaker the
`radio` or `ui` channel.",
    },
    CodeEntry {
        code: code::STRING_FELL_BACK,
        severity: Severity::Warning,
        title: "a string fell back to the default locale",
        explanation: "\
A string key has no text in the active locale, so the default locale's text is shown instead. A
warning, reported once per locale and key.

Fix: add the key to a `kind: strings` doc for the active locale.",
    },
    CodeEntry {
        code: code::STRING_MISSING,
        severity: Severity::Warning,
        title: "a string key is missing everywhere",
        explanation: "\
A string key is in no locale's table, so the key itself is shown in its place. A warning, reported
once per key.

Fix: add the key to a `kind: strings` doc, or correct the key.",
    },
    CodeEntry {
        code: code::STRING_ARG_MISSING,
        severity: Severity::Warning,
        title: "a string placeholder has no argument",
        explanation: "\
A string has a `{name}` placeholder that the caller passed no argument for, so it stays as written.
A warning, reported once per key and placeholder.

Fix: pass the argument in `say_with`'s `args` or `format_text`'s, or remove the placeholder. Literal
braces are written `{{` and `}}`.",
    },
    CodeEntry {
        code: code::DIALOGUE_SEQUENCE_INVALID,
        severity: Severity::Error,
        title: "a dialogue sequence is invalid",
        explanation: "\
`dialogue::say_sequence` was given no lines, or a speakers list whose length differs from the lines
list. Nothing was queued.

Fix: pass one speaker slot per line (`None` for a line with no speaker entity).",
    },
    CodeEntry {
        code: code::DIALOGUE_NOT_ENABLED,
        severity: Severity::Error,
        title: "dialogue is not enabled",
        explanation: "\
A `dialogue::say` call (or `say_with`, `say_sequence`) reached an engine that did not opt in, so
there is no config, no bank and no queue.

Fix: add `.dialogue(DialogueConfig::default())` to `Engine::builder()`.",
    },
    CodeEntry {
        code: code::SEQUENCE_INVALID,
        severity: Severity::Error,
        title: "a sequence document is invalid",
        explanation: "\
A `kind: sequence` document breaks a structural rule (pointer `/tracks/{track}/{n}/...`): `length_ticks`
is 0, a key's `at` is past `length_ticks`, the keys of one track are not in `at` order, a camera key
has neither or both of `rig` and `restore`, or a `fade`, `gain` or `range` is not finite. The build
refuses it.

Fix: put the keys of each track in ascending `at` order inside `0..=length_ticks`, give each camera
key exactly one of `rig:` or `restore: true`, and use finite numbers.",
    },
    CodeEntry {
        code: code::SEQUENCE_UNKNOWN_ROLE,
        severity: Severity::Error,
        title: "a sequence key names an unknown role",
        explanation: "\
A sequence key's `role` is not in the document's `roles:` list, a role is declared twice, or a
camera, animation or ui key carries no `role` at all. Roles are how the game binds entities to a
beat, so a key with no declared role could never play.

Fix: declare the role under `roles:` and spell it the same in the key, and give every camera,
animation and ui key a `role`.",
    },
    CodeEntry {
        code: code::SEQUENCE_EVENT_NAME,
        severity: Severity::Error,
        title: "a sequence event has an invalid name",
        explanation: "\
An `event` key has an empty `name`, or the reserved name `end` (the engine's own `Ended` event).

Fix: give the event a non-empty name other than `end`, such as `radar_online`.",
    },
    CodeEntry {
        code: code::SEQUENCE_UNKNOWN_LINE,
        severity: Severity::Error,
        title: "a sequence dialogue key names an unknown line",
        explanation: "\
A `dialogue` key names a line id no loaded `kind: dialogue` doc defines, or the game never opted in
to dialogue with `.dialogue()`, so no line can be said.

Fix: check the id against the `lines:` of your dialogue docs, and add `.dialogue(..)` to the
engine builder.",
    },
    CodeEntry {
        code: code::SEQUENCE_HOLD_CLOCK,
        severity: Severity::Error,
        title: "a holding sequence uses the game clock",
        explanation: "\
A sequence sets `holds_gameplay: true` with `clock: game`. A held game clock never advances, so the
sequence's playhead would stall for as long as it holds.

Fix: drop `clock:` (a holding sequence runs on the sim clock) or write `clock: sim`.",
    },
    CodeEntry {
        code: code::SEQUENCE_WAIT_CONFLICT,
        severity: Severity::Error,
        title: "two sequence wait keys share one tick",
        explanation: "\
Two `wait` keys, on the dialogue and event tracks together, sit at the same `at`. A player holds at
most one wait at a time.

Fix: move one wait key to a different `at`, or drop one of the two.",
    },
    CodeEntry {
        code: code::SEQUENCE_UNKNOWN,
        severity: Severity::Error,
        title: "no such sequence is loaded",
        explanation: "\
`sequences::play` was given a key that no loaded `kind: sequence` document has. A sequence is keyed
by its path under the content root, such as `sequences/radar.yaml`.

Fix: pass the document's content-root path exactly.",
    },
    CodeEntry {
        code: code::SEQUENCE_BIND_INVALID,
        severity: Severity::Error,
        title: "a sequence binding is invalid",
        explanation: "\
`sequences::play` bound a role the document does not declare under `roles:`, or bound one role to
two entities. Nothing was started.

Fix: bind each declared role at most once, spelled as in the document.",
    },
    CodeEntry {
        code: code::SEQUENCE_ROLE_MISSING,
        severity: Severity::Warning,
        title: "a sequence role has no live entity",
        explanation: "\
A role the document declares was left unbound by `play`, or its entity has despawned, so every key
of that role is inert. Reported once per player and role; the rest of the sequence plays.

Fix: bind every role the beat uses, and keep the entity alive until the beat ends.",
    },
    CodeEntry {
        code: code::SEQUENCE_KEY_DEGRADED,
        severity: Severity::Warning,
        title: "a sequence key degraded",
        explanation: "\
A key could not do its job at run time: the role entity lacks the component the key writes
(`CameraRig`, `Animation`), a `dialogue::say` was refused, or the document was unloaded while the
sequence played (the sequence is cancelled). Reported once per player and reason; the other keys
play.

Fix: put the component on the bound entity, fix the line, and keep the document loaded.",
    },
    CodeEntry {
        code: code::SEQUENCES_NOT_ENABLED,
        severity: Severity::Error,
        title: "sequences are not enabled",
        explanation: "\
A `sequences::play` call reached an engine that did not opt in, so there is no `kind: sequence`
document type and no player system data.

Fix: add `.sequences()` to `Engine::builder()` and schedule `sequence_update`.",
    },
    CodeEntry {
        code: code::PAIRED_UNKNOWN,
        severity: Severity::Error,
        title: "no paired document loads at that path",
        explanation: "\
`paired_fit` was given a content-root path that is not a loadable `kind: paired` document: the engine has
no project root, the file is missing, or it fails the checks `ludion check-scene` runs on it. The loader's
own diagnostics (`L1752`–`L1754`, or the asset error) follow this one. A failed load is remembered until the
next world build.

Fix: pass the document's path relative to the project's content root, and fix what the diagnostics after it
name; see `ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::PAIRED_CAST_INVALID,
        severity: Severity::Error,
        title: "a paired cast cannot play its role",
        explanation: "\
The cast given to `paired_fit` does not match the document: a role name the document lacks (the message
names the nearest), a role cast twice or not at all, or an entity that is dead, has no `Transform`, is
`Parent`ed, or whose `Mesh` is not the voxel assembly the role names. A cast entity whose motor cannot be
tested at its target (an unknown `posture`) is refused here too. The pointer is `/roles/<role>`.

Fix: cast exactly the document's roles, each with a live, unparented entity that draws the role's assembly;
see `ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::PAIRED_SCALE_MISMATCH,
        severity: Severity::Error,
        title: "a paired cast entity has the wrong scale",
        explanation: "\
A cast entity's `Transform.scale` is not uniform, or differs from the role's `scale` by more than 1e-5
relative. The document's offsets are metres and its contacts are measured at the role's scale, so a cast
at another scale would align and measure wrongly. The pointer is `/roles/<role>`.

Fix: set the entity's uniform scale to the role's `scale`, or change the role's `scale`; see
`ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::PAIRED_UNSUPPORTED,
        severity: Severity::Error,
        title: "a paired document is not playable yet",
        explanation: "\
The runtime fits and plays paired documents of exactly two roles. A document with more roles checks fine
with `ludion check-scene`, but `paired_fit` refuses it.

Fix: split the action into pairs of roles; see `ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::PAIRED_LOCKSTEP_BROKEN,
        severity: Severity::Warning,
        title: "a game write broke a paired action's lockstep",
        explanation: "A playing paired action keeps its clip roles in lockstep: each role's `Animation` has the clip, `started`
and speed `1` that `start_paired` wrote. `paired_update` found one changed (a `crossfade`, a clip cut or a
speed write by a game system), so it ended the whole action on that tick and warned once per entity. The
guard does not name the writer.

Fix: end the action with `abort_paired` before writing the participant's `Animation`, or stop the system
that writes it while the action plays; see `ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::PAIRED_NOT_PLAYING,
        severity: Severity::Error,
        title: "the entity is not in a paired action",
        explanation: "`abort_paired` was given an entity that carries no `PairedAction`: the action already ended, or the entity
never took part in one.

Fix: check for `PairedAction` before aborting; see `ludion docs paired-actions`.",
    },
    CodeEntry {
        code: code::ANIM_GRAPH_STATE_INVALID,
        severity: Severity::Error,
        title: "an animation graph cannot step this entity",
        explanation: "\
`AnimGraph::step` could not read the entity's state: it has no `AnimGraphState`, the state's name is not
one the graph declares, it has no `Animation`, or a transition names a layer its `AnimationLayers` lacks.
Nothing was written.

Fix: spawn `AnimGraphState { state: \"<a declared name>\", .. }` beside an `Animation` already playing
that state's clip, and give the entity every layer the graph's states list.",
    },
    CodeEntry {
        code: code::ANIM_GRAPH_INVALID,
        severity: Severity::Error,
        title: "an animation graph table is malformed",
        explanation: "\
An `AnimGraph` declares two states with the same `name` or `id`, or an edge's `from`/`to` names a state the
table does not declare. `step` refuses the graph and writes nothing.

Fix: give each state a unique name and id and declare every state an edge uses; `describe()` prints the table.",
    },
    CodeEntry {
        code: code::PROTOCOL_CALL_TIMED_OUT,
        severity: Severity::Error,
        title: "a call timed out waiting for its reply",
        explanation: "\
A client's per-call timeout elapsed before the host answered. The connection was alive when the
client gave up, so this is not L3009: the host may be busy, wedged, or stopped without closing the
socket.

The command may still run. The client stopped waiting; it did not cancel anything on the host. Never
resend a mutating command blindly — it can apply twice. A timed-out connection still occupies a
server pipeline slot, so a long-lived caller should drop it and reconnect.

The server answers its own, more precise L3023 when the sim thread takes a request and never replies
within its bound; an adapter's timeout sits above that bound so L3023 arrives first. This code means
no answer of any kind came back.

Fix: check that the host process is running and not stopped; read back the state (for example
`sim.hash`) before deciding whether to retry; raise the timeout only for a command that is expected
to run long.",
    },
    CodeEntry {
        code: code::SERVER_CONNECTION_LIMIT,
        severity: Severity::Error,
        title: "the server is at its connection limit",
        explanation: "\
The server refused this connection because it already serves its maximum number of clients.

The refusal arrives as the error reply to the handshake, after which the server closes the socket.
Connections that are already open are unaffected. The cap exists so a runaway local peer cannot
exhaust the server's threads.

Fix: close a client that is no longer needed (a stale adapter, a forgotten harness) and reconnect.
If the clients are all live, a host serving more than the cap at once is outside what this server is
built for.",
    },
    CodeEntry {
        code: code::SERVER_EVENTS_DROPPED,
        severity: Severity::Warning,
        title: "events were dropped on a full connection",
        explanation: "\
A subscription's events were dropped because the connection's outbound queue was full: the client
stopped reading, or reads slower than the engine emits.

Each event carries a `seq` that the engine advances whether or not the event is delivered, so a drop
is normally visible as a gap. When the dropped events are the last ones the subscription emits, no
later event exists to show the gap; this log takes the last dropped event's `seq` and names how many
were lost, so the tail is not silent. It is sent once the queue has room again.

Fix: read the connection continuously, or subscribe with a narrower channel. A client that must see
every event should treat a `seq` gap, or this notice, as a reason to read the state back.",
    },
    CodeEntry {
        code: code::SIM_STEP_TOO_LONG,
        severity: Severity::Error,
        title: "sim.step asks for too many ticks",
        explanation: "\
A `sim.step` named a tick count above the limit of 216,000 (one hour at the default 60 Hz).

The command runs synchronously on the sim thread, so a very large count holds every other client
and the host's frame loop hostage until it finishes; past the transport's reply timeout the wait
is indistinguishable from a wedged sim thread. The request is refused before anything runs: the
frame does not move and nothing is recorded.

Fix: step in smaller batches. `ticks: 0` stays valid.",
    },
    CodeEntry {
        code: code::SCENE_SAVE_CONFLICT,
        severity: Severity::Error,
        title: "scene file changed since this session saw it",
        explanation: "\
`scene.save` found a file at the destination whose text differs from what this session last loaded
or saved there, or that this session never loaded. Writing would overwrite text the session never
saw, so nothing was written. The message says which case it is.

Fix: `scene.load` the destination to see it (its text becomes the session's base), or pass
`force: true` to overwrite it knowingly. `force` skips only this check.",
    },
    CodeEntry {
        code: code::SCENE_SAVE_WRONG_KIND,
        severity: Severity::Error,
        title: "scene destination is not a scene",
        explanation: "\
`scene.save` found a `kind:` document (a prefab, mesh, sculpt and so on) at the destination.
Saving the world there would replace that asset with a scene, so nothing was written, even with
`force`.

Fix: pick another destination. To change a prefab from the world, use `prefab.apply`.",
    },
];

/// Codes that once existed and are gone from [`CODES`], each with why. A retired code is never
/// reused, so `ludion explain` answers it from here instead of calling it a typo.
pub const RETIRED: &[(&str, &str)] = &[
    ("L0003", "the removed CommandBuffer's dead-target skip"),
    (
        "L1193",
        "a `SkinBone`'s out-of-range or self-referencing `skin`, which the generic entity-reference \
         check already refuses as L1017 on every reachable path",
    ),
    (
        "L1216",
        "a `PathFollower` spawned into a world without `path_follow`; the system is now always \
         installed",
    ),
    (
        "L1310",
        "a `CameraRig` spawned into a world without `camera_rig`; the system is now always installed",
    ),
    (
        "L3012",
        "the refused acked subscribe mode, removed from the wire",
    ),
];

/// Why `code` was retired, if it is in [`RETIRED`].
///
/// ```
/// use ludion_core::codes;
///
/// assert!(codes::retired("L0003").is_some());
/// assert!(codes::retired("L0001").is_none());
/// ```
pub fn retired(code: &str) -> Option<&'static str> {
    RETIRED
        .iter()
        .find(|(retired, _)| *retired == code)
        .map(|(_, note)| *note)
}

/// Looks up a code string (e.g. `"L0001"`) in the table.
///
/// Returns `None` for unregistered or malformed codes — the caller (typically `ludion explain`)
/// turns that into an [`code::UNKNOWN_DIAGNOSTIC_CODE`] diagnostic.
pub fn explain(code: &str) -> Option<&'static CodeEntry> {
    CODES.iter().find(|entry| entry.code.as_str() == code)
}

/// Looks up a [`DiagnosticCode`] in the table.
pub fn explain_code(code: DiagnosticCode) -> Option<&'static CodeEntry> {
    CODES.iter().find(|entry| entry.code == code)
}

/// The severity `code` is registered with — [`Severity::Error`] when it has no row.
///
/// This is what makes the table the source of truth for error-vs-lint:
/// [`crate::diagnostics::Diagnostic::new`] calls it, so a code's severity is decided once here
/// rather than by whichever container a call site drops the diagnostic into.
///
/// # Example
///
/// ```
/// use ludion_core::codes::{self, code};
/// use ludion_core::diagnostics::Severity;
///
/// assert_eq!(codes::severity(code::SCENE_YAML_SYNTAX), Severity::Error);
/// assert_eq!(codes::severity(code::MESH_VERTEX_BUDGET_EXCEEDED), Severity::Warning);
/// ```
pub fn severity(code: DiagnosticCode) -> Severity {
    match explain_code(code) {
        Some(entry) => entry.severity,
        None => Severity::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `DiagnosticCode` constant declared in this crate. Keep in sync with `mod code`; the
    /// test below asserts each one is registered in [`CODES`].
    const DECLARED_CODES: &[DiagnosticCode] = &[
        code::UNKNOWN_DIAGNOSTIC_CODE,
        code::DEAD_ENTITY,
        code::UNKNOWN_COMPONENT,
        code::DUPLICATE_COMPONENT_REGISTRATION,
        code::COMPONENT_DESERIALIZE_FAILED,
        code::COMPONENT_SERIALIZE_FAILED,
        code::DOCS_DIR_NOT_FOUND,
        code::DOCS_UNKNOWN_TOPIC,
        code::DOCS_MALFORMED,
        code::NEW_TARGET_IN_WORKSPACE,
        code::ENGINE_LINK_MISSING,
        code::ENGINE_LINK_TARGET_INVALID,
        code::ENGINE_LINK_OCCUPIED,
        code::NEW_ENGINE_LINK_FAILED,
        code::DEV_NOT_A_GAME,
        code::DEV_CHILD_FAILED,
        code::THREADS_ENV_INVALID,
        code::DOCS_UNKNOWN_SECTION,
        code::QUALITY_SETTING_INVALID,
        code::RENDER_SIZE_INVALID,
        code::WINDOW_FEATURE_OFF,
        code::NO_DEFAULT_SCENE,
        code::EMBED_ARGUMENT_INVALID,
        code::TOUCH_LAYOUT_INVALID,
        code::EXPORT_TOOLCHAIN_UNAVAILABLE,
        code::MANIFEST_INVALID,
        code::EXPORT_FLAGS_INVALID,
        code::ENTITY_ALLOCATOR_STATE_INVALID,
        code::LIFTED_COMPONENT_READ,
        code::COMPONENT_NOT_REGISTERED,
        code::PRESENTATION_BOUNDARY,
        code::OPERATION_FAILED_SILENTLY,
        code::CREATE_TARGET_EXISTS,
        code::KIND_CHECKED_ELSEWHERE,
        code::DEV_CARGO_FAILED,
        code::DEV_DEGRADED,
        code::EXPORT_FAILED,
        code::NEW_NAME_INVALID,
        code::GUIDE_STALE,
        code::FULLSCREEN_SETTING_INVALID,
        code::DEFAULT_SCENE_MISMATCH,
        // --- L1xxx: scene / assets ---
        code::SCENE_YAML_SYNTAX,
        code::SCENE_YAML_ANCHOR,
        code::SCENE_YAML_TAG,
        code::SCENE_YAML_MULTIPLE_DOCUMENTS,
        code::SCENE_YAML_FLOW_COLLECTION,
        code::SCENE_YAML_BLOCK_SCALAR,
        code::SCENE_YAML_DUPLICATE_KEY,
        code::SCENE_YAML_COMPLEX_KEY,
        code::SCENE_TYPE_MISMATCH,
        code::SCENE_UNKNOWN_KEY,
        code::SCENE_MISSING_KEY,
        code::SCENE_UNSUPPORTED_FORMAT,
        code::SCENE_UNKNOWN_COMPONENT,
        code::SCENE_COMPONENT_MISMATCH,
        code::SCENE_IO,
        code::SCENE_RESERVED_COMPONENT,
        code::SCENE_INVALID_ENTITY_REF,
        code::SCENE_SAVE_DANGLING_REF,
        code::TILEMAP_RAGGED_ROWS,
        code::TILEMAP_INVALID_GLYPH,
        code::MAP_DOCUMENT_STRUCTURE,
        code::MAP_KIT_INVALID,
        code::MAP_MARKER_OUT_OF_BOUNDS,
        code::MAP_UNKNOWN_KIND,
        code::MAP_NAME_COLLISION,
        code::MAP_FOOTPRINT_MISFIT,
        code::MAP_MIRROR_FRAGMENTS,
        code::MAP_SPAWN_INVALID,
        code::MAP_DANGLING_GATE_ANCHOR,
        code::MAP_GATE_SEALED,
        code::MAP_MARKER_BLOCKED,
        code::SCENE_YAML_KEY_TOO_LONG,
        code::MAP_GATES_DISCONNECTED,
        code::MAP_RESIDUE_MARKER_UNNAMED,
        code::SCENE_PHYSICS_INVALID,
        code::SCENE_YAML_TOO_DEEP,
        code::SCENE_PARENT_CYCLE,
        code::SCENE_PRESENTATION_REF,
        code::SCENE_YAML_DIRECTIVE,
        code::OUTPUT_IO,
        code::MAP_SCENE_DRIFT,
        code::MAP_EDGE_LEAK,
        code::MAP_DRIFT_FAULT,
        code::ASSET_FILE_IO,
        code::ASSET_PNG_DECODE,
        code::WATCHER_START_FAILED,
        code::ASSET_WAV_DECODE,
        code::MESH_DOCUMENT_STRUCTURE,
        code::MESH_DATA_CONSISTENCY,
        code::MESH_VERTEX_BUDGET_EXCEEDED,
        code::VOXEL_DOCUMENT_STRUCTURE,
        code::VOXEL_DATA_CONSISTENCY,
        code::VOXEL_SIZE_BUDGET_EXCEEDED,
        code::VOXEL_DISCONNECTED_COMPONENTS,
        code::VOXEL_OPS_DOCUMENT_STRUCTURE,
        code::VOXEL_OP_INAPPLICABLE,
        code::VOXEL_GEN_INTERMEDIATE,
        code::VOXEL_OPS_SHADOWED_BY_MIRROR,
        code::VOXEL_GEN_DOUBLE_ENCODED,
        code::ASSEMBLY_DOCUMENT_STRUCTURE,
        code::ASSEMBLY_DATA_CONSISTENCY,
        code::PART_DOCUMENT_STRUCTURE,
        code::PART_DATA_CONSISTENCY,
        code::ANIMATION_DOCUMENT_STRUCTURE,
        code::ANIMATION_DATA_CONSISTENCY,
        code::ASSEMBLY_UNEXPECTED_OVERLAP,
        code::DOC_REFERENCE_UNRESOLVED,
        code::DOC_NAME_DUPLICATE,
        code::DOC_SET_OVER_CAPACITY,
        code::SPRITE_PALETTE_STRUCTURE,
        code::PATTERN_BULLET_UNKNOWN,
        code::PATTERN_ENTRY_INVALID,
        code::PATTERN_PLAY_UNRESOLVED,
        code::SPRITE_ATLAS_UNRESOLVED,
        code::SPRITE_FRAME_UNKNOWN,
        code::SPRITE_DOCUMENT_STRUCTURE,
        code::FONT_DOCUMENT_STRUCTURE,
        code::FONT_SOURCE_DOCUMENT_STRUCTURE,
        code::FONT_FILE,
        code::FONTS_TABLE_STRUCTURE,
        code::FONT_NAME_UNKNOWN,
        code::FONT_LOAD,
        code::FONT_GLYPH_MISSING,
        code::DOC_TYPE_RESERVED_FIELD,
        code::VOXEL_COMMENT_DROPPED,
        code::COMFY_REPLY_MALFORMED,
        code::SPRITE_EFFECT_INVALID,
        code::COLLIDER_NON_POSITIVE_EXTENT,
        code::AUDIO_EMITTER_NON_POSITIVE_RANGE,
        code::AUDIO_LISTENER_DUPLICATE,
        code::MODEL_SHEET_INPUT,
        code::UI_ELEMENT_NON_POSITIVE_SIZE,
        code::DOC_ENVELOPE_INVALID,
        code::DOC_SCHEMA_MISMATCH,
        code::DOC_KIND_NOT_REGISTERED,
        code::SCULPT_DOCUMENT_STRUCTURE,
        code::SCULPT_DOCUMENT_DATA,
        code::SCULPT_PART_UNRESOLVED,
        code::SCULPT_DISCONNECTED,
        code::SCULPT_BONE_TREE,
        code::PROFILE_LOAD_FAILED,
        code::PROFILE_SAVE_FAILED,
        code::SETTINGS_ENVELOPE,
        code::SETTINGS_RESET,
        code::SETTINGS_KEY_DROPPED,
        code::SETTINGS_VALUE_CLAMPED,
        code::SETTINGS_SAVE_FAILED,
        code::RUN_SAVE_WRITE_FAILED,
        code::RUN_SAVE_RESUME_REFUSED,
        code::RUN_RECORDING_FAILED,
        code::CHECKPOINT_CONFIG_INVALID,
        code::CHECKPOINT_NO_SLOT,
        code::CHECKPOINT_SLOTS_FULL,
        code::CHECKPOINT_NAME_INVALID,
        code::CHECKPOINT_CAPTURE_FAILED,
        code::CHECKPOINT_RESTORE_FAILED,
        code::CHECKPOINT_SWITCH_DROPPED,
        code::CHECKPOINT_KEYFRAME_SKIP,
        code::CHECKPOINT_RECORDING_ARM_REFUSED,
        code::CHECKPOINT_DISK_WRITE_FAILED,
        code::CHECKPOINT_DISK_LOAD_REFUSED,
        code::GAME_SCALE_INVALID,
        code::RUN_RECORDING_CONFIG_INVALID,
        code::REPLAY_GAME_MISMATCH,
        code::REPLAY_PLAYBACK_FAILED,
        code::DIRECTIONAL_LIGHT_INVALID_FRUSTUM,
        code::SCHEDULE_MODULE_CONCENTRATION,
        code::QUERY_DECL_UNKNOWN_COMPONENT,
        code::QUERY_DECL_NO_SYSTEM,
        code::QUERY_DRIFT_UNDECLARED,
        code::QUERY_DRIFT_UNOBSERVED,
        code::ENVIRONMENT_INVALID_FIELD,
        code::POST_PROCESS_INVALID_TRIPLE,
        code::GENERATION_STRUCTURE,
        code::GENERATION_DATA,
        code::WORKFLOW_NOT_API,
        code::WORKFLOW_UNKNOWN_NODE,
        code::WORKFLOW_INPUT_INVALID,
        code::WORKFLOW_TITLE_UNRESOLVED,
        code::COMFY_UNREACHABLE,
        code::COMFY_REJECTED,
        code::GENERATION_RUN_FAILED,
        code::GENERATION_RUN_TIMED_OUT,
        code::GENERATION_OUTPUT_KIND,
        code::GENERATION_ACCEPT_REFUSED,
        code::MESH_EDIT_STRUCTURE,
        code::MESH_EDIT_DATA,
        code::MESH_EDIT_PICK_UNRESOLVED,
        code::MESH_EDIT_INAPPLICABLE,
        code::MESH_EDIT_OP_PAINTED_NOTHING,
        code::MESH_EDIT_MATCH_FAILED,
        code::COLLIDER_DEGENERATE_HULL,
        code::COLLIDER_HEIGHTFIELD_GRID,
        code::JOINT_INVALID,
        code::ASSEMBLY_JOINT_INVALID,
        code::MESH_PART_UNKNOWN,
        code::COLLIDER_LAYER_UNKNOWN,
        code::COLLIDER_LAYER_UNCLAIMED,
        code::QUERY_LAYERS_INVALID,
        code::QUERY_LAYERS_UNCLAIMED,
        code::SURFACE_UNKNOWN,
        code::SURFACES_INVALID,
        code::SURFACE_TAG_UNCLAIMED,
        code::SURFACE_OVERLAY_NO_SENSOR,
        code::SURFACE_PART_INVALID,
        code::SURFACE_PART_UNMATCHED,
        code::MESH_SURFACES_INVALID,
        code::MESH_SURFACE_UNDECLARED,
        code::AI_MACHINE_HIERARCHY,
        code::AI_MACHINE_INITIAL,
        code::AI_MACHINE_VARIANTS,
        code::AI_EVENT_REF_NOT_NULLABLE,
        code::JOINT_MOTOR_INVALID,
        code::JOINT_ZERO_INVALID,
        code::POSE_SNAPSHOT_INVALID,
        code::PAIRED_DOCUMENT_STRUCTURE,
        code::PAIRED_DATA_CONSISTENCY,
        code::PAIRED_ROLE_MISMATCH,
        code::PAIRED_CONTACT_GAP,
        code::COLLIDER_COMPOUND_INVALID,
        code::MESH_SKIN_STRUCTURE,
        code::MESH_SKIN_DATA,
        code::QUEST_SAFE_ZONE_TOO_SMALL,
        code::SKIN_BONE_NOT_SKINNED,
        code::SKIN_BONE_UNKNOWN_BONE,
        code::SKIN_BONE_DUPLICATE,
        code::SKIN_BONE_ANIMATED_SKIN,
        code::ANIMATION_EVENT_INVALID,
        code::CHARACTER_DOCUMENT_STRUCTURE,
        code::CHARACTER_DATA_CONSISTENCY,
        code::ANIMATION_LAYERS_TOO_MANY,
        code::ANIMATION_LAYER_INVALID,
        code::ANIMATION_LAYERS_ON_SKIN_BONE_SKIN,
        code::IK_TARGET_INVALID,
        code::IMPORT_INVALID_GLTF,
        code::IMPORT_UNSUPPORTED_FEATURE,
        code::IMPORT_OUTPUT_IO,
        code::IMPORT_URI_REFUSED,
        code::IMPORT_SKIN_UNWEIGHTED,
        code::IMPORT_EXTENSION_IGNORED,
        code::PLACEMENT_FLOATING,
        code::PLACEMENT_SUNK,
        code::PLACEMENT_OVERLAP,
        code::MESH_GRID_ORIENT,
        code::MESH_GRID_ANIMATED_SHAPE,
        code::IMPORT_SYNC_REFUSED,
        code::IMPORT_CLIP_ERROR_BOUND,
        code::IMPORT_CLIP_EXTRAS_INVALID,
        code::IMPORT_CLIP_AT_REST,
        code::ANIMATION_SPRITE_CROSSFADE,
        code::ANIMATION_MODEL_UNLOADED,
        code::COLLIDER_MESH_ON_RIGID,
        code::COLLIDER_MESH_UNAVAILABLE,
        code::PATH_DOCUMENT_STRUCTURE,
        code::PATH_DATA_CONSISTENCY,
        code::PATH_FOLLOWER_UNLOADED,
        code::GENERATION_IMAGE_UNBOUND,
        code::HIERARCHY_CYCLE,
        code::HIERARCHY_PARENT_INVALID,
        code::HIERARCHY_TOO_DEEP,
        code::HIERARCHY_RIGID_BODY,
        code::HIERARCHY_STATIC_COLLIDER,
        code::HIERARCHY_KINEMATIC_SHEAR,
        code::HIERARCHY_REPARENT_CYCLE,
        code::HIERARCHY_REPARENT_PRESENTATION,
        code::HIERARCHY_REPARENT_TOO_DEEP,
        code::PREFAB_DOCUMENT_STRUCTURE,
        code::PREFAB_PART_NAME,
        code::PREFAB_PART_DUPLICATE,
        code::PREFAB_SINGLETON,
        code::PREFAB_UNRESOLVED,
        code::PREFAB_INSTANCE_STRUCTURE,
        code::PREFAB_INSTANCE_CYCLE,
        code::PREFAB_INSTANCE_PART,
        code::PREFAB_INSTANCE_SAVE,
        code::PREFAB_NOT_INSTANCE,
        code::PREFAB_CREATE,
        code::PREFAB_APPLY,
        code::PREFAB_REVERT,
        code::COLLIDER_NO_PHYSICS_SCENE,
        code::COLLIDER_NO_SPATIAL_INDEX,
        code::COLOR_COMPONENT_INVALID,
        code::CAMERA_INVALID_DEPTH_RANGE,
        code::PATTERN_AUTHORING_LINT,
        code::GENERATION_IDENTICAL_CANDIDATES,
        code::SCENE_SAVE_GENERATED,
        code::VOXEL_VIEW_OUT_OF_RANGE,
        code::PATH_FOLLOWER_INVALID,
        code::COOK_ENCODE_FAILED,
        code::COOK_ROOT_INVALID,
        code::COOK_MAP_LEAVES_ROOT,
        code::TEXTURE_NOT_COOKED,
        code::SPOT_LIGHT_INVALID_CONE,
        code::POST_PROCESS_INVALID_SSR,
        code::REFLECTION_PROBE_INVALID,
        code::REFLECTION_PROBE_EQUAL_RANK,
        code::REFLECTION_PROBE_SHADOWED,
        code::REFLECTION_PROBE_DUPLICATE,
        code::REFLECTION_PROBE_OVER_CAP,
        code::REFLECTION_PROBE_NOT_STORED,
        code::REFLECTION_PROBE_UNKNOWN,
        code::REFLECTION_PROBE_NOT_BUNDLED,
        code::DIRECTIONAL_LIGHT_INVALID_ANGULAR_RADIUS,
        code::TEXTURE_STRUCTURE,
        code::TEXTURE_DATA,
        code::TEXTURE_GRAPH,
        code::TEXTURE_DRIFT,
        code::TEXTURE_CHECK_WARNING,
        code::TEXTURE_CHECK_FAILED,
        code::TEXTURE_RULES_INVALID,
        code::COLLIDER_SCALE_IGNORED,
        code::COLLIDER_SENSOR_ON_RIGID,
        code::COLLIDER_SENSOR_NO_INTERIOR,
        code::COLLIDER_SENSOR_UNCLAIMED,
        code::CHARACTER_MOTOR_INVALID,
        code::CHARACTER_MOTOR_WITH_COLLIDER,
        code::CHARACTER_MOTOR_NO_PHYSICS_SCENE,
        code::CHARACTER_MOTOR_PARENTED,
        code::CHARACTER_MOTOR_POSTURE_UNKNOWN,
        code::CHARACTER_MOTOR_POSTURE_FALLBACK,
        code::SURFACE_VELOCITY_INVALID,
        code::SURFACE_VELOCITY_NO_SURFACE,
        code::LANE_CONSTRAINT_INVALID,
        code::LANE_PATH_UNLOADED,
        code::LANE_WITHOUT_MOTOR,
        code::LANE_PATH_VERTICAL,
        code::CAMERA_RIG_INVALID,
        code::CAMERA_RIG_TARGET_MISSING,
        code::CAMERA_RIG_PATH_UNLOADED,
        code::CAMERA_RIG_WITHOUT_CAMERA,
        code::CAMERA_RIG_DUPLICATE,
        code::CAMERA_RIG_PARENTED,
        code::CAMERA_RIG_COLLISION_QUERY,
        code::CAMERA_RIG_COLLISION_NO_PHYSICS,
        code::CAMERA_RIG_COLLISION_STUCK,
        code::CAMERA_ZONE_NO_PHYSICS,
        code::CAMERA_ZONE_WITHOUT_SENSOR,
        code::CAMERA_ZONE_EQUAL_PRIORITY,
        code::PREFAB_ASSET_PATH,
        code::LOCAL_LIGHT_INVALID_SHADOW,
        code::DIRECTIONAL_LIGHT_EXTENT_UNUSED,
        code::PREFAB_CLIP_TARGET,
        code::DOC_SINGLETON_COUNT,
        code::UI_RECT_FILL_IGNORES_PLACEMENT,
        code::ANIMATION_HOLD_ZERO,
        code::PARENT_JOINT_NOT_SKINNED,
        code::PARENT_JOINT_UNKNOWN_JOINT,
        code::PRESENTATION_INSTANCE_SIM_COMPONENT,
        code::CONTENT_PATH_OUTSIDE_ROOT,
        code::LOCAL_LIGHT_INVALID_SHADOW_DISTANCE,
        code::LOCAL_LIGHT_SHADOW_FADE_UNUSED,
        code::CAMERA_DOF_FOCUS_ZERO,
        code::SUN_DISC_SIZE_MISMATCH,
        code::CAMERA_OFFSET_INVALID,
        code::CAMERA_OFFSET_ON_SIM_ENTITY,
        code::CAMERA_OFFSET_DUPLICATE,
        code::CAMERA_OFFSET_FOV_CLAMPED,
        code::VIEWMODEL_NO_CAMERA,
        code::VIEWMODEL_LENS_NOT_ON_CAMERA,
        code::VIEWMODEL_LENS_INVALID,
        code::SPATIAL_VOLUME_INVALID,
        code::NAV_AGENT_INVALID,
        code::NAV_ROUTE_INVALID,
        code::NAV_GOAL_INVALID,
        code::PATROL_DOCUMENT_STRUCTURE,
        code::PATROL_DATA_CONSISTENCY,
        code::PATROL_FOLLOWER_INVALID,
        code::PATROL_STATE_INVALID,
        code::NAV_LINK_INVALID,
        code::NAV_BLOCKER_INVALID,
        code::NAV_AVOIDANCE_INVALID,
        code::PERCEIVER_INVALID,
        code::SIGHT_TARGET_INVALID,
        code::PERCEPTION_NAME_UNKNOWN,
        code::PERCEIVER_MEMORY_INVALID,
        code::STIMULUS_REPORT_INVALID,
        code::NOISE_ZONE_INVALID,
        code::AUDIO_ZONE_INVALID,
        code::HEARING_INVALID,
        code::NOISE_INVALID,
        code::INPUT_BINDING_SOURCES,
        code::INPUT_BINDING_SOURCE_KIND,
        code::INPUT_BINDING_MODIFIER,
        code::INPUT_BINDING_RANGE,
        code::INPUT_BINDING_DUPLICATE,
        code::INPUT_ACTION_INVALID,
        code::INPUT_ACTION_UNKNOWN,
        code::INPUT_STATE_INVALID,
        code::INPUT_ACTION_LOCKED,
        code::INPUT_OVERRIDE_UNKNOWN,
        code::INPUT_OVERRIDE_INVALID,
        code::INPUT_CAPTURE_IMPOSSIBLE,
        code::INPUT_CAPTURE_IDLE,
        code::INPUT_BINDINGS_ENVELOPE,
        code::INPUT_BINDINGS_RESET,
        code::INPUT_BINDINGS_SAVE_FAILED,
        code::RUMBLE_EFFECT_INVALID,
        code::UI_TWEEN_INVALID,
        code::UI_WORLD_ANCHOR_INVALID,
        code::UI_MOTION_UNSUPPORTED,
        code::UI_RECT_SAMPLING_INVALID,
        code::UI_RECT_SAMPLING_IGNORED,
        code::UI_SLICE_CLAMPED,
        code::UI_GROUP_INVALID,
        code::UI_GROUP_CYCLE,
        code::UI_GROUP_TOO_DEEP,
        code::UI_GROUP_IGNORED,
        code::UI_TEXT_OUTLINE_INVALID,
        code::UI_RADIAL_INVALID,
        code::UI_ROTATION_INVALID,
        code::UI_SHAPE_INVALID,
        code::UI_GROUP_UNRESOLVED,
        code::UI_CLIP_NESTING,
        code::UI_TEXT_OUTLINE_CLAMPED,
        code::UI_POLYGON_DEGENERATE,
        code::STATIC_INELIGIBLE,
        code::STATIC_PARENT_NOT_STATIC,
        code::STATIC_REWRITTEN,
        code::PREFAB_EMPTY,
        code::GENERATION_JOBS_UNKNOWN,
        code::ENTITY_REF_IN_COLLECTION,
        code::PATTERN_SYSTEM_NOT_RUN,
        code::MESH_LOD_CHAIN,
        code::MESH_LOD_LEVEL_MISMATCH,
        code::CAMERA_LOD_INVALID,
        code::VOXEL_LOD_EMPTY_LEVEL,
        // --- L2xxx: render ---
        code::RENDER_GRAPH_CYCLE,
        code::RENDER_GRAPH_UNWRITTEN_READ,
        code::RENDER_NO_ADAPTER,
        code::RENDER_DEVICE_FAILED,
        code::RENDER_READBACK_FAILED,
        code::RENDER_PNG_IO,
        code::WINDOW_PRESENTATION_UNAVAILABLE,
        code::SHADER_RELOAD_DISABLED,
        code::SHADER_RELOAD_INVALID,
        code::RENDER_GPU_ERROR,
        code::RENDER_DEVICE_LOST,
        code::RENDER_EXTENT_INVALID,
        code::RENDER_MULTIPLE_CAMERAS,
        code::RENDER_NO_CAMERA,
        code::GI_VOLUME_CELL_BUDGET_EXCEEDED,
        code::GI_VOLUME_MISSING,
        code::GI_VOLUME_PROBE_BUDGET_EXCEEDED,
        code::GROUND_MARKER_NON_POSITIVE_DIMENSION,
        code::PARTICLE_EMITTER_INVALID,
        code::VOLLEY_INVALID,
        code::RENDER_PLACEMENT_NON_FINITE,
        code::DROP_SHADOW_INVALID_DISTANCE,
        code::GI_VOLUME_EMPTY,
        code::DECAL_INVALID_ANGLE_FADE,
        code::GROUND_MARKER_WEDGE_INVALID,
        code::GROUND_MARKER_RING_INVALID,
        code::GROUND_MARKER_STYLE_INVALID,
        code::GROUND_MARKER_CLIP_INVALID,
        code::GROUND_MARKER_CLIP_NO_PHYSICS,
        code::OCCLUDED_SILHOUETTE_INVALID,
        code::OCCLUDED_SILHOUETTE_ON_VIEWMODEL,
        code::OCCLUDED_SILHOUETTE_STYLE_LIMIT,
        code::MESH_BLEND_ON_VIEWMODEL,
        code::RENDER_VIEW_INVALID,
        code::RENDER_VIEW_NAME_DUPLICATE,
        code::UI_RECT_VIEW_UNRESOLVED,
        code::UI_RECT_VIEW_CONFLICT,
        code::RENDER_VIEW_BUDGET,
        code::RENDER_LAYERS_INVALID,
        code::RENDER_LAYER_UNKNOWN,
        code::RAIN_INVALID,
        code::RAIN_DUPLICATE,
        code::RAIN_STREAK_BUDGET,
        code::CUBE_LUT_MALFORMED,
        code::CUBE_LUT_UNSUPPORTED,
        code::POST_PROCESS_INVALID_METERING,
        code::SCREEN_EFFECT_INVALID_TARGET,
        code::SCREEN_EFFECT_CAP,
        code::RENDER_GOLDEN_MISSING,
        code::RENDER_IMAGE_DIMENSION_MISMATCH,
        code::RENDER_SSIM_BELOW_THRESHOLD,
        code::RENDER_GOLDEN_PNG_DECODE,
        code::RENDER_GOLDEN_PLATFORM_UNPINNED,
        code::RENDER_GOLDEN_BLESS_ADAPTER_UNPINNED,
        code::RENDER_GOLDEN_COLOR_ABOVE_CEILING,
        code::GOLDEN_MANIFEST_INVALID,
        code::TEST_CHECKED_NOTHING,
        code::RENDER_GOLDEN_COMPARE_ADAPTER_UNPINNED,
        // --- L3xxx: protocol / server ---
        code::PROTOCOL_MALFORMED_ENVELOPE,
        code::PROTOCOL_UNKNOWN_METHOD,
        code::PROTOCOL_UNSUPPORTED_VERSION,
        code::PROTOCOL_MALFORMED_FRAME,
        code::PROTOCOL_INVALID_INPUT,
        code::PROTOCOL_COMPONENT_ABSENT,
        code::PROTOCOL_SPAWN_NAME_CONFLICT,
        code::PROTOCOL_UNKNOWN_SUBSCRIPTION,
        code::PROTOCOL_SNAPSHOT_INVALID,
        code::PROTOCOL_ENGINE_UNAVAILABLE,
        code::REPLAY_FORMAT,
        code::REPLAY_COMPRESSION,
        code::REPLAY_SCENE_UNRESOLVED,
        code::RESTORE_COMPONENT_UNREGISTERED,
        code::RESTORE_COMPONENT_UNDESERIALIZABLE,
        code::REPLAY_DECOMPRESSED_TOO_LARGE,
        code::TRANSITION_TARGET_UNRESOLVED,
        code::TRANSITION_ANCHOR_MISSING,
        code::TRANSITION_CARRY_DEAD,
        code::TRANSITION_CARRY_ORPHANED,
        code::REPLAY_FRAME_WINDOW,
        code::SERVER_ACCEPT_FAILED,
        code::PROTOCOL_SIM_STALLED,
        code::REPLAY_KEYFRAME_DRIFT,
        code::PROTOCOL_RENDER_SIZE_INVALID,
        code::RESIDUE_DUPLICATE_NAME,
        code::RESIDUE_CARRY_CONFLICT,
        code::RESIDUE_EVICTED,
        code::TRANSITION_ANCHOR_NO_TRANSFORM,
        code::RESIDUE_RECORD_UNMIGRATABLE,
        code::RESIDUE_NAME_UNMATCHED,
        code::PROTOCOL_RENDER_VIEW_INVALID,
        code::PHYSICS_SNAPSHOT_INVALID,
        code::PHYSICS_SNAPSHOT_NO_SETTINGS,
        code::JOINT_BODY_MISSING,
        code::RAGDOLL_SWITCH_INVALID,
        code::JOINT_MOTOR_MISMATCH,
        code::IMPULSE_TARGET_INVALID,
        code::WIRE_ORIGIN_REFUSED,
        code::WIRE_PATH_OUTSIDE_ROOT,
        code::RESTORE_ENTITIES_MISMATCH,
        code::RECORDING_STOPPED,
        code::PROTOCOL_UNEXPECTED_RESPONSE,
        code::PROTOCOL_MESSAGE_TOO_LARGE,
        code::PROTOCOL_INVALID_MAX_FPS,
        code::RECORDING_ARM_REFUSED,
        code::RESTORE_ENGINE_VERSION_DIFFERS,
        code::RESTORE_SNAPSHOT_FIELD_UNKNOWN,
        code::REPLAY_PANIC_NOT_REPRODUCED,
        code::PROTOCOL_MALFORMED_REPLY,
        code::REPLAY_TORN_TAIL,
        code::RESIDUE_REF_UNRESOLVED,
        code::PROTOCOL_CALL_TIMED_OUT,
        code::SERVER_CONNECTION_LIMIT,
        code::SERVER_EVENTS_DROPPED,
        code::SIM_STEP_TOO_LONG,
        code::SCENE_SAVE_CONFLICT,
        code::SCENE_SAVE_WRONG_KIND,
        // --- L4xxx: sim ---
        code::INVALID_SIM_SPEED,
        code::INPUT_SCRIPT_IO,
        code::INPUT_SCRIPT_INVALID,
        code::BUDGET_FILE_INVALID,
        code::BUDGET_EXCEEDED,
        code::BUDGET_UNKNOWN_KEY,
        code::SYSTEM_PANICKED,
        code::AI_TRANSITION_CHAIN_CAPPED,
        code::AI_INBOX_OVERFLOW,
        code::AI_GROUP_MISSING,
        code::AI_SEND_NO_MIND,
        code::AI_STATE_NOT_LISTED,
        code::SCENE_OVERRIDE_INVALID,
        code::STATS_THRESHOLD_FILE_INVALID,
        code::STATS_THRESHOLD_EXCEEDED,
        code::INVALID_RENDER_SCALE,
        code::PROFILE_CAPTURE_INVALID,
        code::PROFILE_CAPTURE_BUSY,
        code::PROFILE_CAPTURE_CANNOT_CLOSE,
        code::SPRITE_IMPORT_NEEDS_QUANTIZE,
        code::SPRITE_IMPORT_QUANTIZED,
        code::COMMAND_PANICKED,
        code::SIM_THREAD_PANICKED,
        code::SYSTEM_NAME_DUPLICATE,
        code::BUDGET_GPU_UNMEASURED,
        code::BUDGET_QUALITY_MISMATCH,
        code::PROFILE_SAMPLES_MISSING,
        code::SIM_THREAD_STUCK,
        code::HASHED_NON_FINITE_FLOAT,
        code::BUDGET_LOAD_UNMEASURED,
        code::PROJECTILE_SPAWN_REFUSED,
        code::PROJECTILE_CONFIG_INVALID,
        code::PROJECTILE_VISUALS_INVALID,
        code::PROJECTILE_MUZZLE_UNRESOLVED,
        code::PROJECTILE_STEP_NOT_RUN,
        code::PROJECTILE_RESTORE_CAPACITY,
        code::NAV_CONFIG_INVALID,
        code::NAV_NOT_READY,
        code::NAV_PATH_REQUEST_INVALID,
        code::PATROL_ROUTE_UNLOADED,
        code::PATROL_POINT_UNREACHABLE,
        code::PATROL_WITHOUT_AGENT,
        code::NAV_LINK_UNSNAPPED,
        code::NAV_NOT_AT_LINK,
        code::NAV_CLASS_UNKNOWN,
        code::PERCEPTION_CONFIG_INVALID,
        code::PERCEPTION_DEFERRED,
        code::PROJECTILE_LOOK_ASSET,
        code::PHYSICS_SCALE_UNGATED,
        code::POSE_BLEND_POSE_MISSING,
        code::LIGHT_EXPOSURE_INVALID,
        code::LIGHT_EXPOSURE_PRESENTATION_LIGHT,
        code::LIGHT_EXPOSURE_OCCLUDERS_UNMATCHED,
        // --- L5xxx: audio ---
        code::AUDIO_DEVICE_UNAVAILABLE,
        code::AUDIO_STREAM_FAILED,
        code::MUSIC_TRACK_UNPLAYABLE,
        code::AUDIO_VOICE_CAPS_INVALID,
        code::AUDIO_GAIN_NON_FINITE,
        code::MUSIC_STEM_MISMATCHED,
        code::AUDIO_DUCK_RULE_INVALID,
        code::AUDIO_OCCLUSION_INVALID,
        code::AUDIO_OCCLUSION_QUERY_REJECTED,
        code::DIALOGUE_CONFIG_INVALID,
        code::STRINGS_DUPLICATE_KEY,
        code::DIALOGUE_DUPLICATE_LINE,
        code::DIALOGUE_DUPLICATE_SPEAKER,
        code::DIALOGUE_UNKNOWN_SPEAKER,
        code::STRING_KEY_MISSING_DEFAULT,
        code::STRINGS_PLACEHOLDER_MISMATCH,
        code::DIALOGUE_LINE_INVALID,
        code::STRINGS_LOCALE_INVALID,
        code::DIALOGUE_UNKNOWN_LINE,
        code::DIALOGUE_NO_SPEAKER,
        code::STRING_FELL_BACK,
        code::STRING_MISSING,
        code::STRING_ARG_MISSING,
        code::DIALOGUE_SEQUENCE_INVALID,
        code::DIALOGUE_NOT_ENABLED,
        code::SEQUENCE_INVALID,
        code::SEQUENCE_UNKNOWN_ROLE,
        code::SEQUENCE_EVENT_NAME,
        code::SEQUENCE_UNKNOWN_LINE,
        code::SEQUENCE_HOLD_CLOCK,
        code::SEQUENCE_WAIT_CONFLICT,
        code::SEQUENCE_UNKNOWN,
        code::SEQUENCE_BIND_INVALID,
        code::SEQUENCE_ROLE_MISSING,
        code::SEQUENCE_KEY_DEGRADED,
        code::SEQUENCES_NOT_ENABLED,
        code::PAIRED_UNKNOWN,
        code::PAIRED_CAST_INVALID,
        code::PAIRED_SCALE_MISMATCH,
        code::PAIRED_UNSUPPORTED,
        code::PAIRED_LOCKSTEP_BROKEN,
        code::PAIRED_NOT_PLAYING,
        code::ANIM_GRAPH_STATE_INVALID,
        code::ANIM_GRAPH_INVALID,
    ];

    #[test]
    fn retired_codes_are_not_registered() {
        for (retired, _) in RETIRED {
            assert!(
                explain(retired).is_none(),
                "{retired} is retired and registered"
            );
        }
    }

    #[test]
    fn declared_codes_are_registered() {
        for &c in DECLARED_CODES {
            assert!(
                explain_code(c).is_some(),
                "declared code {c} is missing from the CODES table",
            );
        }
    }

    #[test]
    fn table_codes_are_unique() {
        for (i, a) in CODES.iter().enumerate() {
            for b in &CODES[i + 1..] {
                assert_ne!(a.code, b.code, "duplicate code {} in table", a.code);
            }
        }
    }

    #[test]
    fn table_codes_have_valid_shape() {
        for entry in CODES {
            let s = entry.code.as_str();
            // Re-parsing enforces the `^L\d{4}$` shape independently of the const constructor.
            assert!(
                crate::diagnostics::DiagnosticCode::parse(s).is_some(),
                "code {s} does not match L####",
            );
            assert!(!entry.title.is_empty(), "code {s} has an empty title");
            assert!(
                !entry.explanation.is_empty(),
                "code {s} has an empty explanation",
            );
        }
    }

    #[test]
    fn codes_fall_in_their_documented_domain() {
        // The table is centralized: codes from every crate live here, but each must fall in one
        // of the six documented domain ranges (L0xxx–L5xxx).
        for entry in CODES {
            let digit = entry.code.domain_digit();
            assert!(
                (b'0'..=b'5').contains(&digit),
                "code {} is outside the documented domain ranges L0xxx-L5xxx",
                entry.code,
            );
        }
    }

    #[test]
    fn domain_table_agrees_with_l0001_explanation() {
        // The domain-range table is deliberately triplicated in prose (this module's top doc
        // comment, `DiagnosticCode::domain_digit`'s doc comment in diagnostics.rs, and L0001's
        // user-facing `explanation` below) rather than unified into one shared source — see the
        // architecture doc's "regularity over cleverness" note on avoiding indirection for its own
        // sake. Doc comments aren't introspectable at runtime, so this test pins the one site that
        // is: L0001's explanation text. If you add/rename a domain, update all three sites.
        const DOMAINS: [&str; 6] = [
            "L0xxx  core / ecs",
            "L1xxx  scene / assets",
            "L2xxx  render",
            "L3xxx  protocol / server",
            "L4xxx  sim",
            "L5xxx  audio",
        ];
        let explanation = explain("L0001").expect("L0001 is registered").explanation;
        for line in DOMAINS {
            assert!(
                explanation.contains(line),
                "L0001's explanation is missing the domain line {line:?} — keep it, this module's \
                 top doc comment, and diagnostics.rs's `domain_digit` doc comment in sync",
            );
        }
    }

    #[test]
    fn the_registered_lints_are_exactly_these() {
        // Severity is a property of the code, declared once here — so the set of codes that do NOT
        // fail their command is pinned. Adding a row to this list is the deliberate act of saying
        // "raising this never stops the caller"; anything not listed is fatal.
        const LINTS: &[DiagnosticCode] = &[
            code::COMPONENT_NOT_REGISTERED,
            code::DEV_DEGRADED,
            code::GUIDE_STALE,
            code::MAP_GATE_SEALED,
            code::MAP_MARKER_BLOCKED,
            code::MAP_GATES_DISCONNECTED,
            code::MAP_RESIDUE_MARKER_UNNAMED,
            code::MAP_EDGE_LEAK,
            code::MESH_VERTEX_BUDGET_EXCEEDED,
            code::VOXEL_SIZE_BUDGET_EXCEEDED,
            code::VOXEL_DISCONNECTED_COMPONENTS,
            code::VOXEL_OPS_SHADOWED_BY_MIRROR,
            code::VOXEL_GEN_DOUBLE_ENCODED,
            code::ASSEMBLY_UNEXPECTED_OVERLAP,
            code::DOC_SET_OVER_CAPACITY,
            code::FONT_GLYPH_MISSING,
            code::VOXEL_COMMENT_DROPPED,
            code::AUDIO_LISTENER_DUPLICATE,
            code::DOC_KIND_NOT_REGISTERED,
            code::SCULPT_PART_UNRESOLVED,
            code::SCULPT_DISCONNECTED,
            code::SETTINGS_ENVELOPE,
            code::SETTINGS_RESET,
            code::SETTINGS_KEY_DROPPED,
            code::SETTINGS_VALUE_CLAMPED,
            code::SETTINGS_SAVE_FAILED,
            code::PROFILE_LOAD_FAILED,
            code::RUN_SAVE_WRITE_FAILED,
            code::RUN_SAVE_RESUME_REFUSED,
            code::RUN_RECORDING_FAILED,
            code::CHECKPOINT_NO_SLOT,
            code::CHECKPOINT_SLOTS_FULL,
            code::CHECKPOINT_NAME_INVALID,
            code::CHECKPOINT_CAPTURE_FAILED,
            code::CHECKPOINT_RESTORE_FAILED,
            code::CHECKPOINT_SWITCH_DROPPED,
            code::CHECKPOINT_KEYFRAME_SKIP,
            code::CHECKPOINT_RECORDING_ARM_REFUSED,
            code::CHECKPOINT_DISK_LOAD_REFUSED,
            code::GAME_SCALE_INVALID,
            code::REPLAY_PLAYBACK_FAILED,
            code::PROFILE_SAVE_FAILED,
            code::SCHEDULE_MODULE_CONCENTRATION,
            code::QUERY_DRIFT_UNOBSERVED,
            code::SURFACE_PART_UNMATCHED,
            code::MESH_SURFACE_UNDECLARED,
            code::AI_TRANSITION_CHAIN_CAPPED,
            code::AI_INBOX_OVERFLOW,
            code::AI_GROUP_MISSING,
            code::AI_SEND_NO_MIND,
            code::AI_STATE_NOT_LISTED,
            code::PAIRED_CONTACT_GAP,
            code::IMPORT_SKIN_UNWEIGHTED,
            code::IMPORT_EXTENSION_IGNORED,
            code::PLACEMENT_FLOATING,
            code::PLACEMENT_SUNK,
            code::PLACEMENT_OVERLAP,
            code::ANIMATION_SPRITE_CROSSFADE,
            code::HIERARCHY_CYCLE,
            code::HIERARCHY_PARENT_INVALID,
            code::HIERARCHY_TOO_DEEP,
            code::HIERARCHY_KINEMATIC_SHEAR,
            code::PATTERN_AUTHORING_LINT,
            code::GENERATION_IDENTICAL_CANDIDATES,
            code::COOK_MAP_LEAVES_ROOT,
            code::TEXTURE_NOT_COOKED,
            code::REFLECTION_PROBE_EQUAL_RANK,
            code::REFLECTION_PROBE_SHADOWED,
            code::REFLECTION_PROBE_NOT_STORED,
            code::TEXTURE_CHECK_WARNING,
            code::COLLIDER_SCALE_IGNORED,
            code::CHARACTER_MOTOR_POSTURE_FALLBACK,
            code::LANE_PATH_VERTICAL,
            code::CAMERA_RIG_TARGET_MISSING,
            code::CAMERA_RIG_COLLISION_NO_PHYSICS,
            code::CAMERA_RIG_COLLISION_STUCK,
            code::CAMERA_ZONE_NO_PHYSICS,
            code::CAMERA_ZONE_EQUAL_PRIORITY,
            code::DIRECTIONAL_LIGHT_EXTENT_UNUSED,
            code::UI_RECT_FILL_IGNORES_PLACEMENT,
            code::LOCAL_LIGHT_SHADOW_FADE_UNUSED,
            code::CAMERA_DOF_FOCUS_ZERO,
            code::SUN_DISC_SIZE_MISMATCH,
            code::CAMERA_OFFSET_FOV_CLAMPED,
            code::VIEWMODEL_NO_CAMERA,
            code::VIEWMODEL_LENS_NOT_ON_CAMERA,
            code::PERCEPTION_NAME_UNKNOWN,
            code::STIMULUS_REPORT_INVALID,
            code::PERCEPTION_DEFERRED,
            code::NOISE_INVALID,
            code::PATROL_ROUTE_UNLOADED,
            code::PATROL_POINT_UNREACHABLE,
            code::PATROL_WITHOUT_AGENT,
            code::NAV_LINK_UNSNAPPED,
            code::NAV_CLASS_UNKNOWN,
            code::INPUT_ACTION_UNKNOWN,
            code::INPUT_OVERRIDE_UNKNOWN,
            code::INPUT_OVERRIDE_INVALID,
            code::INPUT_BINDINGS_ENVELOPE,
            code::INPUT_BINDINGS_RESET,
            code::INPUT_BINDINGS_SAVE_FAILED,
            code::RUMBLE_EFFECT_INVALID,
            code::UI_MOTION_UNSUPPORTED,
            code::UI_RECT_SAMPLING_IGNORED,
            code::UI_SLICE_CLAMPED,
            code::UI_GROUP_IGNORED,
            code::UI_GROUP_UNRESOLVED,
            code::UI_CLIP_NESTING,
            code::UI_TEXT_OUTLINE_CLAMPED,
            code::UI_POLYGON_DEGENERATE,
            code::STATIC_INELIGIBLE,
            code::STATIC_PARENT_NOT_STATIC,
            code::STATIC_REWRITTEN,
            code::PATTERN_SYSTEM_NOT_RUN,
            code::VOXEL_LOD_EMPTY_LEVEL,
            code::VOLLEY_INVALID,
            code::RENDER_PLACEMENT_NON_FINITE,
            code::GROUND_MARKER_CLIP_NO_PHYSICS,
            code::OCCLUDED_SILHOUETTE_ON_VIEWMODEL,
            code::OCCLUDED_SILHOUETTE_STYLE_LIMIT,
            code::MESH_BLEND_ON_VIEWMODEL,
            code::RENDER_VIEW_BUDGET,
            code::RAIN_DUPLICATE,
            code::RAIN_STREAK_BUDGET,
            code::SCREEN_EFFECT_CAP,
            code::TRANSITION_CARRY_DEAD,
            code::TRANSITION_CARRY_ORPHANED,
            code::RESTORE_ENGINE_VERSION_DIFFERS,
            code::RESTORE_SNAPSHOT_FIELD_UNKNOWN,
            code::REPLAY_TORN_TAIL,
            code::RESIDUE_REF_UNRESOLVED,
            code::RESIDUE_DUPLICATE_NAME,
            code::RESIDUE_CARRY_CONFLICT,
            code::RESIDUE_EVICTED,
            code::RESIDUE_RECORD_UNMIGRATABLE,
            code::RESIDUE_NAME_UNMATCHED,
            code::PHYSICS_SNAPSHOT_NO_SETTINGS,
            code::JOINT_BODY_MISSING,
            code::IMPULSE_TARGET_INVALID,
            code::SPRITE_IMPORT_QUANTIZED,
            code::HASHED_NON_FINITE_FLOAT,
            code::PROJECTILE_SPAWN_REFUSED,
            code::PROJECTILE_MUZZLE_UNRESOLVED,
            code::PROJECTILE_STEP_NOT_RUN,
            code::PROJECTILE_RESTORE_CAPACITY,
            code::PHYSICS_SCALE_UNGATED,
            code::POSE_BLEND_POSE_MISSING,
            code::LIGHT_EXPOSURE_PRESENTATION_LIGHT,
            code::LIGHT_EXPOSURE_OCCLUDERS_UNMATCHED,
            code::AUDIO_DEVICE_UNAVAILABLE,
            code::AUDIO_STREAM_FAILED,
            code::MUSIC_TRACK_UNPLAYABLE,
            code::AUDIO_GAIN_NON_FINITE,
            code::MUSIC_STEM_MISMATCHED,
            code::AUDIO_OCCLUSION_QUERY_REJECTED,
            code::STRINGS_PLACEHOLDER_MISMATCH,
            code::STRING_FELL_BACK,
            code::STRING_MISSING,
            code::STRING_ARG_MISSING,
            code::SEQUENCE_ROLE_MISSING,
            code::SEQUENCE_KEY_DEGRADED,
            code::PAIRED_LOCKSTEP_BROKEN,
            code::SERVER_EVENTS_DROPPED,
        ];
        let registered: Vec<DiagnosticCode> = CODES
            .iter()
            .filter(|entry| entry.severity == Severity::Warning)
            .map(|entry| entry.code)
            .collect();
        assert_eq!(registered, LINTS);
        for lint in LINTS {
            assert_eq!(severity(*lint), Severity::Warning);
        }
    }

    #[test]
    fn severity_of_an_unregistered_code_is_error() {
        assert_eq!(
            severity(DiagnosticCode::new("L0099")),
            Severity::Error,
            "a code with no row must be fatal, never silently a lint",
        );
    }

    #[test]
    fn explain_lookup_works() {
        assert_eq!(explain("L0001").unwrap().title, "unknown diagnostic code");
        assert_eq!(explain("L0002").unwrap().title, "entity is not alive");
        assert!(explain("L0099").is_none());
        assert!(explain("nonsense").is_none());
    }

    // --- Source-parsing helpers for the bijection and reachability tests below. The table parses
    //     its own source so that a new `pub const` (or a stale `DECLARED_CODES` entry) cannot slip
    //     the hand-maintained registry list without failing a test. ---

    /// Every `pub const NAME: DiagnosticCode` declared in `mod code`, by name, parsed from source.
    fn code_module_const_names(src: &str) -> Vec<String> {
        src.lines()
            .filter_map(|line| {
                let rest = line.trim_start().strip_prefix("pub const ")?;
                let (name, ty) = rest.split_once(':')?;
                ty.trim_start()
                    .starts_with("DiagnosticCode")
                    .then(|| name.trim().to_string())
            })
            .collect()
    }

    /// Every `code::NAME` named inside the `DECLARED_CODES` array literal, parsed from source.
    fn declared_codes_list_names(src: &str) -> Vec<String> {
        let start = src
            .find("const DECLARED_CODES")
            .expect("DECLARED_CODES is present");
        let after = &src[start..];
        let open = after.find("&[").expect("DECLARED_CODES array opens");
        let close = after[open..]
            .find("];")
            .expect("DECLARED_CODES array closes")
            + open;
        after[open..close]
            .split("code::")
            .skip(1)
            .filter_map(|chunk| {
                let name: String = chunk
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                (!name.is_empty()).then_some(name)
            })
            .collect()
    }

    /// True if `ident` appears in `haystack` as a whole Rust identifier (not a sub-slice of a
    /// longer one — so `UNKNOWN_COMPONENT` does not match inside `SCENE_UNKNOWN_COMPONENT`).
    fn references_ident(haystack: &str, ident: &str) -> bool {
        let bytes = haystack.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let mut from = 0;
        while let Some(pos) = haystack[from..].find(ident) {
            let i = from + pos;
            let before_ok = i == 0 || !is_word(bytes[i - 1]);
            let end = i + ident.len();
            let after_ok = end >= bytes.len() || !is_word(bytes[end]);
            if before_ok && after_ok {
                return true;
            }
            from = i + 1;
        }
        false
    }

    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// Bijection: every `pub const` in `mod code` is registered in `DECLARED_CODES`, and every
    /// entry in `DECLARED_CODES` names a const that still exists. The forward direction closes the
    /// hand-maintained gap — a new const that skips the list fails here; the reverse catches a
    /// rename that left a stale entry (the compiler already rejects a truly nonexistent one).
    #[test]
    fn code_consts_and_declared_list_are_a_bijection() {
        let src = include_str!("codes.rs");
        let mut module = code_module_const_names(src);
        let mut declared = declared_codes_list_names(src);
        module.sort();
        module.dedup();
        declared.sort();
        declared.dedup();
        for name in &module {
            assert!(
                declared.contains(name),
                "code::{name} is declared in `mod code` but missing from DECLARED_CODES — add it \
                 to the list in the test module",
            );
        }
        for name in &declared {
            assert!(
                module.contains(name),
                "DECLARED_CODES names code::{name}, which no longer exists in `mod code`",
            );
        }
        assert_eq!(
            module, declared,
            "the code const set and DECLARED_CODES must be identical",
        );
    }

    /// The part of a source file before its first inline test module — a `#[cfg(…)]` whose
    /// predicate names `test` (`test`, `all(test, …)`) over `mod NAME {`; a heuristic, but the
    /// workspace convention puts the test module last, and a code only a test raises is dead.
    fn non_test_source(text: &str) -> &str {
        const ATTR: &str = "#[cfg(";
        let cut = text.match_indices(ATTR).map(|(i, _)| i).find(|&i| {
            let Some(end) = text[i..].find(")]") else {
                return false;
            };
            let predicate = &text[i + ATTR.len()..i + end];
            let names_test = predicate
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|word| word == "test");
            let rest = text[i + end + 2..].trim_start();
            let inline_mod = rest.starts_with("mod ")
                && rest
                    .find(['{', ';'])
                    .is_some_and(|j| rest.as_bytes()[j] == b'{');
            names_test && inline_mod
        });
        &text[..cut.unwrap_or(text.len())]
    }

    #[test]
    fn non_test_source_cuts_at_a_compound_test_cfg() {
        let text =
            "fn live() {}\n#[cfg(all(test, feature = \"wire\"))]\nmod x {\n    fn dead() {}\n}\n";
        assert_eq!(non_test_source(text), "fn live() {}\n");
    }

    #[test]
    fn non_test_source_ignores_an_out_of_line_test_mod() {
        let text = "#[cfg(test)]\nmod helpers;\nfn live() {}\n";
        assert_eq!(non_test_source(text), text);
    }

    /// Reachability: every diagnostic const is referenced by some workspace source *other* than this
    /// table — no dead codes. `tests/` directories and trailing `#[cfg(test)] mod` blocks do not
    /// count. A const only ever named in `codes.rs` is either an unused code (drop
    /// the entry) or one whose construction site was lost (a bug to restore).
    ///
    /// The corpus is `crates/` **and** `games/`, because a game raises codes too: a game's
    /// `validate_docs` pass owns every cross-reference rule it states (`L1144` and its family), so a
    /// code whose only construction site is a game is live, not dead.
    #[test]
    fn every_code_const_is_referenced_outside_codes_rs() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let crates_dir = manifest.parent().expect("crate lives under crates/");
        let workspace = crates_dir.parent().expect("crates/ lives at the root");
        let codes_rs = std::fs::canonicalize(manifest.join("src").join("codes.rs")).ok();

        let mut files = Vec::new();
        collect_rs_files(crates_dir, &mut files);
        collect_rs_files(&workspace.join("games"), &mut files);

        let mut corpus = String::new();
        for f in &files {
            if std::fs::canonicalize(f).ok() == codes_rs {
                continue;
            }
            if f.components().any(|c| c.as_os_str() == "tests") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(f) {
                corpus.push_str(non_test_source(&text));
                corpus.push('\n');
            }
        }
        assert!(
            !corpus.is_empty(),
            "found no sibling crate source to scan — is the workspace layout intact?",
        );

        // Codes raised only by the Lore-only game (games/vigil): exempt when it is absent (a public
        // clone), asserted still raised when present so the list cannot go stale.
        const VIGIL_ONLY: &[&str] = &[
            "DOC_NAME_DUPLICATE",
            "DOC_SET_OVER_CAPACITY",
            "QUEST_SAFE_ZONE_TOO_SMALL",
        ];
        let vigil_present = workspace.join("games/vigil").is_dir();
        for name in VIGIL_ONLY {
            assert!(
                !vigil_present || references_ident(&corpus, name),
                "code::{name} is listed as vigil-only but games/vigil no longer raises it — drop it from VIGIL_ONLY"
            );
        }

        let src = include_str!("codes.rs");
        for name in code_module_const_names(src) {
            if !vigil_present && VIGIL_ONLY.contains(&name.as_str()) {
                continue;
            }
            assert!(
                references_ident(&corpus, &name),
                "code::{name} is never referenced outside codes.rs — it is a dead diagnostic code \
                 (remove the const and its table entry, or restore its lost construction site)",
            );
        }
    }

    /// Severity belongs to the code table, not the call site: a `.with_severity(` override is
    /// legitimate only where a property of the *invocation* — not of the diagnostic class — decides
    /// fatality. Each site is named here; a new one fails this test.
    #[test]
    fn with_severity_is_called_only_at_named_sites() {
        // (path suffix, call count, why)
        const SITES: &[(&str, usize, &str)] = &[
            (
                "ludion-import/src/lib.rs",
                1,
                "L1202 downgrades under --allow-lossy",
            ),
            (
                "ludion-cli/src/gen_cmd.rs",
                1,
                "L1167: the live check is optional",
            ),
            (
                "ludion-engine/src/probe_bake.rs",
                1,
                "L1279: a failed coverage check is an error, the resolve's miss a warning",
            ),
            (
                "ludion-sim/src/rebind.rs",
                1,
                "L4272: a locked action refuses a rebind, but an override naming it is only dropped",
            ),
            ("ludion-core/src/diagnostics.rs", 1, "its own unit test"),
            ("ludion-protocol/src/lib.rs", 1, "a wire-shape test fixture"),
        ];
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let crates_dir = manifest.parent().expect("crate lives under crates/");
        let workspace = crates_dir.parent().expect("crates/ lives at the root");
        let codes_rs = std::fs::canonicalize(manifest.join("src").join("codes.rs")).ok();

        let mut files = Vec::new();
        collect_rs_files(crates_dir, &mut files);
        collect_rs_files(&workspace.join("games"), &mut files);
        assert!(
            !files.is_empty(),
            "found no source to scan — is the workspace layout intact?"
        );

        for file in &files {
            if std::fs::canonicalize(file).ok() == codes_rs {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(file) else {
                continue;
            };
            let count = text.matches(".with_severity(").count();
            let path = crate::paths::slash(file);
            let allowed = SITES
                .iter()
                .find(|(suffix, _, _)| path.ends_with(suffix))
                .map_or(0, |(_, count, _)| *count);
            assert_eq!(
                count, allowed,
                "{path} calls `.with_severity(` {count} time(s), {allowed} allowed: severity belongs \
                 to the code table (`CODES`), so fix the code's row — or, if an invocation truly \
                 decides fatality, name the site in SITES with its reason",
            );
        }
    }
}
