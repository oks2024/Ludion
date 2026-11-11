//! [`Command`] — the exhaustive method surface, and its per-method parameter structs.
//!
//! One `#[serde(tag = "method", content = "params")]` enum is the whole capability set of the engine
//! (engine-architecture.md §6). On the wire a command is the `method` + `params` pair inside a
//! [`Request`](crate::Request) envelope: `{"method": "entity.spawn", "params": {…}}`. Methods with no
//! parameters are unit variants and serialize with **no** `params` key.
//!
//! Every payload that would otherwise name a concrete component type is a [`serde_json::Value`] plus a
//! registry *name* — spawn components, `component.set` values, query results, injected input events,
//! snapshots, and graph/timing dumps. That is the boundary rule that keeps this crate depending on
//! `ludion-core` alone; the dispatcher validates those values against the component registry.

use std::collections::BTreeMap;

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::envelope::SubId;
use crate::frame::FrameFormat;

/// Every capability the engine exposes over the protocol, as one exhaustive enum.
///
/// The wire form is adjacently tagged: the dotted `method` name plus a `params` object (absent for
/// parameterless methods). Adding a variant here is the parity gate — a command with no dispatch
/// handler is a compile error in `ludion-engine`.
///
/// # Example
/// ```
/// use ludion_protocol::Command;
/// use ludion_protocol::command::EntityDestroyParams;
/// use ludion_core::ids::EntityId;
///
/// let cmd = Command::EntityDestroy(EntityDestroyParams { entity: EntityId::new(7, 0) });
/// assert_eq!(cmd.method(), "entity.destroy");
/// assert_eq!(
///     serde_json::to_value(&cmd).unwrap(),
///     serde_json::json!({
///         "method": "entity.destroy",
///         "params": { "entity": { "index": 7, "generation": 0 } },
///     }),
/// );
///
/// // A parameterless method serializes with no `params` key.
/// assert_eq!(
///     serde_json::to_value(&Command::SimPause).unwrap(),
///     serde_json::json!({ "method": "sim.pause" }),
/// );
/// ```
// Deliberately NOT `#[non_exhaustive]`, unlike the response-side `ResponseBody`/`Event`/`Response`:
// the exhaustive `match` over `Command` in `ludion-engine`'s dispatch IS the parity gate — a variant
// with no handler must stay a compile error. `#[non_exhaustive]` would force a wildcard arm there and
// silently delete that guarantee, so it must never be added here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Command {
    /// `scene.load` — replace the world with the scene at a path (validate → instantiate).
    #[serde(rename = "scene.load")]
    SceneLoad(SceneLoadParams),
    /// `scene.save` — write the current world to a path through the canonical YAML writer.
    #[serde(rename = "scene.save")]
    SceneSave(SceneSaveParams),
    /// `scene.check` — validate a scene file without loading it; returns its diagnostics.
    #[serde(rename = "scene.check")]
    SceneCheck(SceneCheckParams),

    /// `entity.spawn` — create an entity carrying the given components (registry name → value).
    #[serde(rename = "entity.spawn")]
    EntitySpawn(EntitySpawnParams),
    /// `entity.destroy` — despawn an entity.
    #[serde(rename = "entity.destroy")]
    EntityDestroy(EntityDestroyParams),
    /// `entity.reparent` — re-point an entity's `Parent` (or remove it), keeping its world pose or
    /// its local `Transform`.
    #[serde(rename = "entity.reparent")]
    EntityReparent(EntityReparentParams),
    /// `prefab.spawn` — instantiate a `kind: prefab` file at runtime; replies with the new entity ids
    /// in the prefab's part order.
    #[serde(rename = "prefab.spawn")]
    PrefabSpawn(PrefabSpawnParams),
    /// `prefab.create` — write the given live entities out as a new `kind: prefab` file, the first
    /// being its root; replies with the normalized path.
    #[serde(rename = "prefab.create")]
    PrefabCreate(PrefabCreateParams),
    /// `prefab.apply` — write a spawned instance's edits back into its prefab file; a runtime room
    /// (a `map.spawn` instance) is refused with `L1237`.
    #[serde(rename = "prefab.apply")]
    PrefabApply(PrefabApplyParams),
    /// `prefab.revert` — restore a spawned instance's parts (or one component or field) from its
    /// prefab file; a runtime room (a `map.spawn` instance) is refused with `L1238`.
    #[serde(rename = "prefab.revert")]
    PrefabRevert(PrefabRevertParams),
    /// `prefab.unpack` — turn a spawned instance into plain entities by removing its
    /// `PrefabInstance` links.
    #[serde(rename = "prefab.unpack")]
    PrefabUnpack(PrefabUnpackParams),
    /// `prefab.overrides` — what `scene.save` would write for each prefab instance: its overrides,
    /// removed parts and placement. A pure read; a runtime room (a `map.spawn` instance) reports
    /// `L1235` for its entry.
    #[serde(rename = "prefab.overrides")]
    PrefabOverrides(PrefabOverridesParams),
    /// `map.spawn` — instantiate a `kind: map` file as a room at runtime; replies with the new
    /// entity ids in part order, the instance root first.
    #[serde(rename = "map.spawn")]
    MapSpawn(MapSpawnParams),
    /// `prefab.despawn` — remove every part of a spawned prefab or map instance, given its root;
    /// replies with the removed ids.
    #[serde(rename = "prefab.despawn")]
    PrefabDespawn(PrefabDespawnParams),

    /// `component.set` — insert or overwrite one component value on an entity.
    #[serde(rename = "component.set")]
    ComponentSet(ComponentSetParams),
    /// `component.get` — read one component value from an entity.
    #[serde(rename = "component.get")]
    ComponentGet(ComponentGetParams),
    /// `component.remove` — remove one component from an entity (the undo inverse of the
    /// `component.set` that inserted it).
    #[serde(rename = "component.remove")]
    ComponentRemove(ComponentRemoveParams),

    /// `query.entities` — list entities that carry all of the filter components, returning the
    /// requested component values for each.
    #[serde(rename = "query.entities")]
    QueryEntities(QueryEntitiesParams),

    /// `sim.pause` — stop ticking; the engine keeps draining and answering the queue.
    #[serde(rename = "sim.pause")]
    SimPause,
    /// `sim.resume` — resume free-running ticks.
    #[serde(rename = "sim.resume")]
    SimResume,
    /// `sim.step` — run exactly `ticks` ticks, then pause (the agent's single-step debugger).
    #[serde(rename = "sim.step")]
    SimStep(SimStepParams),
    /// `sim.set_speed` — scale the live scheduler's wall-clock pacing (never `dt`).
    #[serde(rename = "sim.set_speed")]
    SimSetSpeed(SimSetSpeedParams),
    /// `sim.hash` — compute the current `WorldHash` (the determinism verification primitive).
    #[serde(rename = "sim.hash")]
    SimHash,

    /// `input.inject` — feed one input event into the sim (now, or addressed to a future frame).
    #[serde(rename = "input.inject")]
    InputInject(InputInjectParams),

    /// `render.screenshot` — render one guaranteed-frame image and return it as a base64 PNG.
    #[serde(rename = "render.screenshot")]
    RenderScreenshot(RenderScreenshotParams),
    /// `render.dump_graph` — return the render graph's structure as JSON (no GPU needed).
    #[serde(rename = "render.dump_graph")]
    RenderDumpGraph,
    /// `render.dump_timings` — return the last frame's per-pass GPU timings as JSON.
    #[serde(rename = "render.dump_timings")]
    RenderDumpTimings,
    /// `render.bounds` — the world AABB + screen rect of every renderable under the current camera
    /// (pure CPU, no GPU): "what did I just draw, and where". The editor's pick primitive.
    #[serde(rename = "render.bounds")]
    RenderBounds(RenderBoundsParams),
    /// `render.gi_rebake` — set this session's global-illumination rebake lever, ask for one rebake,
    /// or read both. Every field is optional, so the same verb sets, triggers, and queries: empty
    /// params (`{}`) change nothing and just report.
    #[serde(rename = "render.gi_rebake")]
    RenderGiRebake(RenderGiRebakeParams),
    /// `render.size` — set this session's render extent (the live stream's resolution, and the
    /// default a dimensionless `render.screenshot` uses), or read it. Both fields are optional, so
    /// the same verb sets and queries: empty params (`{}`) change nothing and just report the extent
    /// in force.
    #[serde(rename = "render.size")]
    RenderSize(RenderSizeParams),
    /// `render.view` — point this host's *stream* at a free camera pose instead of the scene's own
    /// `Camera`, clear that override (`{"pose": null}`), or read it (`{}`). Host render state, like
    /// `render.size`: never written to a scene, never in a `WorldHash`, never recorded into a replay,
    /// not part of `engine.snapshot`.
    #[serde(rename = "render.view")]
    RenderView(RenderViewParams),
    /// `render.debug` — read or set which debug-draw categories this host renders (`{}` reads,
    /// `{"categories": [...]}` sets), and report which categories the last tick drew. Host render
    /// state, like `render.view`: never written to a scene, never in a `WorldHash`, never recorded
    /// into a replay, not part of `engine.snapshot`.
    #[serde(rename = "render.debug")]
    RenderDebug(RenderDebugParams),

    /// `stream.subscribe` — open a subscription on a channel; returns a `sub` id.
    #[serde(rename = "stream.subscribe")]
    StreamSubscribe(StreamSubscribeParams),
    /// `stream.unsubscribe` — close a subscription.
    #[serde(rename = "stream.unsubscribe")]
    StreamUnsubscribe(StreamUnsubscribeParams),

    /// `asset.reload` — reload one asset (by path) or all changed assets.
    #[serde(rename = "asset.reload")]
    AssetReload(AssetReloadParams),

    /// `engine.describe` — return the full machine-readable surface (component + protocol schemas).
    #[serde(rename = "engine.describe")]
    EngineDescribe,
    /// `engine.snapshot` — serialize the whole engine state (world + RNG + frame + scene refs).
    #[serde(rename = "engine.snapshot")]
    EngineSnapshot,
    /// `engine.restore` — restore engine state from a snapshot produced by `engine.snapshot`.
    #[serde(rename = "engine.restore")]
    EngineRestore(EngineRestoreParams),
    /// `engine.shutdown` — gracefully stop the hosting process: flush any armed recording, close
    /// connections, exit 0 — exactly what Ctrl-C does. In-process (no host) it just flags the request
    /// and acks; nothing exits.
    #[serde(rename = "engine.shutdown")]
    EngineShutdown,

    /// `profile.capture` — measure the next N *live* frames and return one performance report.
    ///
    /// The reply is deferred until the window closes, so it arrives when the frames it describes
    /// have actually run — and a connection answers its requests in order, so a client that needs
    /// other commands answered meanwhile issues this on a connection of its own. Subscribing to the
    /// `profile` channel is the way to watch cost continuously instead; the two coexist.
    #[serde(rename = "profile.capture")]
    ProfileCapture(crate::perf::ProfileCaptureParams),
}

impl Command {
    /// The exact dotted method name this command carries on the wire.
    ///
    /// # Example
    /// ```
    /// use ludion_protocol::Command;
    /// assert_eq!(Command::SimHash.method(), "sim.hash");
    /// ```
    pub fn method(&self) -> &'static str {
        match self {
            Command::SceneLoad(_) => "scene.load",
            Command::SceneSave(_) => "scene.save",
            Command::SceneCheck(_) => "scene.check",
            Command::EntitySpawn(_) => "entity.spawn",
            Command::EntityDestroy(_) => "entity.destroy",
            Command::EntityReparent(_) => "entity.reparent",
            Command::PrefabSpawn(_) => "prefab.spawn",
            Command::PrefabCreate(_) => "prefab.create",
            Command::PrefabApply(_) => "prefab.apply",
            Command::PrefabRevert(_) => "prefab.revert",
            Command::PrefabUnpack(_) => "prefab.unpack",
            Command::PrefabOverrides(_) => "prefab.overrides",
            Command::MapSpawn(_) => "map.spawn",
            Command::PrefabDespawn(_) => "prefab.despawn",
            Command::ComponentSet(_) => "component.set",
            Command::ComponentGet(_) => "component.get",
            Command::ComponentRemove(_) => "component.remove",
            Command::QueryEntities(_) => "query.entities",
            Command::SimPause => "sim.pause",
            Command::SimResume => "sim.resume",
            Command::SimStep(_) => "sim.step",
            Command::SimSetSpeed(_) => "sim.set_speed",
            Command::SimHash => "sim.hash",
            Command::InputInject(_) => "input.inject",
            Command::RenderScreenshot(_) => "render.screenshot",
            Command::RenderDumpGraph => "render.dump_graph",
            Command::RenderDumpTimings => "render.dump_timings",
            Command::RenderBounds(_) => "render.bounds",
            Command::RenderGiRebake(_) => "render.gi_rebake",
            Command::RenderSize(_) => "render.size",
            Command::RenderView(_) => "render.view",
            Command::RenderDebug(_) => "render.debug",
            Command::StreamSubscribe(_) => "stream.subscribe",
            Command::StreamUnsubscribe(_) => "stream.unsubscribe",
            Command::AssetReload(_) => "asset.reload",
            Command::EngineDescribe => "engine.describe",
            Command::EngineSnapshot => "engine.snapshot",
            Command::EngineRestore(_) => "engine.restore",
            Command::EngineShutdown => "engine.shutdown",
            Command::ProfileCapture(_) => "profile.capture",
        }
    }

    /// Whether running this command can make the next rendered image differ.
    ///
    /// A live host re-renders for its frame subscribers when the world may have moved. Most of what
    /// a client sends cannot move it — inspector and overlay polls, and input, which is queued for
    /// the *next* tick — and a render submitted for an unchanged world is not merely wasted GPU
    /// work: it fills the readback ring, which pushes the host's collect toward blocking the sim
    /// thread past its tick deadline.
    ///
    /// Exhaustive with no wildcard arm, for the same reason the enum is not `#[non_exhaustive]`: a
    /// new command must be classified deliberately rather than defaulted. Classify an unclear one as
    /// `true` — a redundant render costs time, a missing one leaves a visibly stale viewport, and
    /// those are not symmetric.
    ///
    /// # Example
    /// ```
    /// use ludion_protocol::Command;
    /// use ludion_protocol::command::QueryEntitiesParams;
    ///
    /// assert!(!Command::QueryEntities(QueryEntitiesParams::default()).may_change_frame());
    /// assert!(Command::SimStep(ludion_protocol::command::SimStepParams { ticks: 1, hash: false })
    ///     .may_change_frame());
    /// ```
    pub fn may_change_frame(&self) -> bool {
        match self {
            // The image is a function of the world, and each of these replaces part or all of it.
            Command::SceneLoad(_)
            | Command::EntitySpawn(_)
            | Command::EntityDestroy(_)
            | Command::EntityReparent(_)
            | Command::PrefabSpawn(_)
            | Command::MapSpawn(_)
            | Command::PrefabDespawn(_)
            | Command::PrefabRevert(_)
            | Command::PrefabUnpack(_)
            | Command::ComponentSet(_)
            | Command::ComponentRemove(_)
            | Command::EngineRestore(_) => true,
            // Runs ticks: the world has already moved by the time the reply is sent.
            Command::SimStep(_) => true,
            // Touches no entity, yet a reloaded mesh, texture or shader is drawn.
            Command::AssetReload(_) => true,
            // A viewer attaching to a *paused* session is owed a first frame: the frame index never
            // advances there, so nothing else can trigger one for it.
            Command::StreamSubscribe(_) => true,
            // Nothing in the world moves, but a rebake relights it, and leaving `hold` lets the next
            // frame follow the world again. The query form (`{}`) moves no pixels and costs one
            // render — the classify-unclear-as-true rule, applied to the whole variant.
            Command::RenderGiRebake(_) => true,
            // A resize drops the readback ring and its render targets, so the next frame a
            // subscriber sees must be re-rendered at the new extent; the query form (`{}`) pays one
            // render for the same classify-unclear-as-true reason `render.gi_rebake` does.
            Command::RenderSize(_) => true,
            // The override *is* what the stream draws through, so setting or clearing it moves every
            // subscriber's next frame; the query form (`{}`) pays one render for the same
            // classify-unclear-as-true reason `render.size` does.
            Command::RenderView(_) => true,
            // Enabling a category adds lines to every subscriber's next frame; the query form pays
            // one render for the same classify-unclear-as-true reason `render.view` does.
            Command::RenderDebug(_) => true,

            // Pure reads of the world, the surface, or the last frame's timings.
            Command::SceneCheck(_)
            | Command::ComponentGet(_)
            | Command::QueryEntities(_)
            | Command::PrefabOverrides(_)
            | Command::SimHash
            | Command::RenderDumpGraph
            | Command::RenderDumpTimings
            | Command::RenderBounds(_)
            | Command::EngineDescribe
            | Command::EngineSnapshot => false,
            // Writes the world out to disk; nothing in the world moves.
            Command::SceneSave(_) => false,
            // Write a prefab file; the instance in the world is untouched.
            Command::PrefabCreate(_) | Command::PrefabApply(_) => false,
            // Queued into the input script and applied at the start of the next tick — a render on
            // receipt would draw the pre-input world.
            Command::InputInject(_) => false,
            // Renders into its own reply; what a stream subscriber would see is untouched.
            Command::RenderScreenshot(_) => false,
            // Run control and pacing decide *when* ticks happen; a tick that lands advances the
            // frame index, which is the host's other change signal.
            Command::SimPause | Command::SimResume | Command::SimSetSpeed(_) => false,
            // Closing a subscription cannot change an image; reopening one is what owes a frame.
            Command::StreamUnsubscribe(_) => false,
            // Measures the frames the sim produces; it produces none of its own.
            Command::ProfileCapture(_) => false,
            // The host returns from its loop rather than rendering again.
            Command::EngineShutdown => false,
        }
    }

    /// Every method name this protocol version defines, in enum-declaration order.
    ///
    /// Used by the envelope parser to distinguish an *unknown* method (`L3002`) from an otherwise
    /// malformed envelope (`L3001`). A unit test asserts this stays in sync with the enum.
    ///
    /// # Example
    /// ```
    /// use ludion_protocol::Command;
    /// assert!(Command::METHODS.contains(&"entity.spawn"));
    /// assert!(!Command::METHODS.contains(&"entity.teleport"));
    /// ```
    pub const METHODS: &'static [&'static str] = &[
        "scene.load",
        "scene.save",
        "scene.check",
        "entity.spawn",
        "entity.destroy",
        "entity.reparent",
        "prefab.spawn",
        "prefab.create",
        "prefab.apply",
        "prefab.revert",
        "prefab.unpack",
        "prefab.overrides",
        "map.spawn",
        "prefab.despawn",
        "component.set",
        "component.get",
        "component.remove",
        "query.entities",
        "sim.pause",
        "sim.resume",
        "sim.step",
        "sim.set_speed",
        "sim.hash",
        "input.inject",
        "render.screenshot",
        "render.dump_graph",
        "render.dump_timings",
        "render.bounds",
        "render.gi_rebake",
        "render.size",
        "render.view",
        "render.debug",
        "stream.subscribe",
        "stream.unsubscribe",
        "asset.reload",
        "engine.describe",
        "engine.snapshot",
        "engine.restore",
        "engine.shutdown",
        "profile.capture",
    ];
}

/// Parameters for `scene.load`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneLoadParams {
    /// Path to the scene file, resolved by the engine relative to its project root.
    pub path: String,
}

/// Parameters for `scene.save`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneSaveParams {
    /// Destination path; the engine writes canonical YAML there.
    pub path: String,
    /// Skip only the check that the destination still holds the text this session loaded or last
    /// saved there; the generated-scene and wrong-kind refusals still apply.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}

/// Parameters for `scene.check`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneCheckParams {
    /// Path to the scene file to validate.
    pub path: String,
}

/// Parameters for `entity.spawn`.
///
/// `components` maps registry component names to their JSON values — never a concrete Rust component
/// type (the boundary rule). The dispatcher validates each value against the named component's schema.
///
/// # Example
/// ```
/// use ludion_protocol::command::EntitySpawnParams;
/// use std::collections::BTreeMap;
///
/// let mut components = BTreeMap::new();
/// components.insert("Transform".to_string(), serde_json::json!({ "position": [0.0, 1.0, 0.0] }));
/// let params = EntitySpawnParams { name: Some("player".into()), components, presentation: false };
/// assert_eq!(params.components.len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntitySpawnParams {
    /// Optional human-facing name for the entity (for logs / the inspector).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The entity's components, keyed by registry name; each value is validated at dispatch against
    /// the component's schema, its semantic validator and the entity-reference checks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, Value>,
    /// Spawn from the presentation id range: the entity is drawn and never hashed. A sim spawn
    /// (the default) may not name a presentation entity.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub presentation: bool,
}

/// Parameters for `entity.destroy`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityDestroyParams {
    /// The entity to despawn.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub entity: EntityId,
}

/// Which pose `entity.reparent` preserves.
///
/// # Example
/// ```
/// use ludion_protocol::command::ReparentKeep;
/// assert_eq!(serde_json::to_value(ReparentKeep::World).unwrap(), serde_json::json!("world"));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReparentKeep {
    /// Rewrite the entity's local `Transform` so its world pose is unchanged. Lossy under a
    /// non-uniform scale with rotation, where the composed pose has a shear a `Transform` cannot hold.
    World,
    /// Leave the `Transform` bits alone; the entity moves with its new parent.
    Local,
}

/// Parameters for `entity.reparent`.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_protocol::command::{EntityReparentParams, ReparentKeep};
///
/// let params = EntityReparentParams {
///     entity: EntityId::new(4, 0),
///     parent: Some(EntityId::new(2, 0)),
///     keep: ReparentKeep::World,
/// };
/// assert_eq!(serde_json::to_value(&params).unwrap()["keep"], "world");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityReparentParams {
    /// The entity to move.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub entity: EntityId,
    /// The new parent, or `null` to detach the entity into a root.
    #[schemars(with = "Option<crate::schema_proxy::EntityIdSchema>")]
    pub parent: Option<EntityId>,
    /// Whether the entity keeps its world pose (`world`) or its local `Transform` (`local`).
    pub keep: ReparentKeep,
}

fn one3() -> [f32; 3] {
    [1.0; 3]
}

/// A pose baked onto every root part of a spawned prefab. Field defaults equal a `Transform`'s.
///
/// # Example
/// ```
/// use ludion_protocol::command::Placement;
/// let placement: Placement = serde_json::from_str(r#"{"translation": [5, 0, 0]}"#).unwrap();
/// assert_eq!(placement.scale, [1.0, 1.0, 1.0]);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// World-space translation.
    #[serde(default)]
    pub translation: [f32; 3],
    /// Euler XYZ rotation in degrees, as `Transform.rotation`.
    #[serde(default)]
    pub rotation: [f32; 3],
    /// Per-axis scale; defaults to `[1, 1, 1]`.
    #[serde(default = "one3")]
    pub scale: [f32; 3],
}

/// Parameters for `prefab.spawn`.
///
/// # Example
/// ```
/// use ludion_protocol::command::{Placement, PrefabSpawnParams};
///
/// let params = PrefabSpawnParams {
///     path: "prefabs/crate.prefab.yaml".into(),
///     transform: Some(Placement { translation: [5.0, 0.0, 0.0], rotation: [0.0; 3], scale: [1.0; 3] }),
///     name: Some("a".into()),
///     presentation: false,
/// };
/// assert_eq!(serde_json::to_value(&params).unwrap()["name"], "a");
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabSpawnParams {
    /// The prefab file, relative to the content root.
    pub path: String,
    /// A pose baked onto every root part of the prefab; omitted, the parts keep their authored pose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Placement>,
    /// An instance name: the parts become `<name>/<part>`; omitted, they keep their bare part names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Spawn every part from the presentation allocator, nested instances included; a sim-only
    /// component is `L1320`. Omitted, each part keeps its prefab's kind.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub presentation: bool,
}

/// Parameters for `prefab.create`.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_protocol::command::PrefabCreateParams;
///
/// let params = PrefabCreateParams {
///     entities: vec![EntityId::new(1, 0), EntityId::new(2, 0)],
///     path: "prefabs/lamp.prefab.yaml".into(),
/// };
/// assert_eq!(serde_json::to_value(&params).unwrap()["path"], "prefabs/lamp.prefab.yaml");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabCreateParams {
    /// The prefab's parts: the first is the root, the rest follow in order. Each needs a unique
    /// `Name` without `/`, and a reference between them must stay inside the set.
    #[schemars(with = "Vec<crate::schema_proxy::EntityIdSchema>")]
    pub entities: Vec<EntityId>,
    /// The file to write, relative to the content root; overwritten if it exists.
    pub path: String,
}

/// Parameters for `prefab.apply`.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_protocol::command::PrefabApplyParams;
///
/// let params = PrefabApplyParams { instance: EntityId::new(1, 0), parts: None };
/// assert!(serde_json::to_value(&params).unwrap().get("parts").is_none());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabApplyParams {
    /// Any part of the spawned instance.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub instance: EntityId,
    /// Narrows the apply to these part paths; omitted, every edited part is written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<String>>,
}

/// Parameters for `prefab.revert`.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_protocol::command::PrefabRevertParams;
///
/// let params = PrefabRevertParams {
///     instance: EntityId::new(1, 0),
///     pointer: Some("/bulb/Transform".into()),
/// };
/// assert_eq!(serde_json::to_value(&params).unwrap()["pointer"], "/bulb/Transform");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabRevertParams {
    /// Any part of the spawned instance.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub instance: EntityId,
    /// Narrows the revert to `/<part path>`, `/<part path>/<Component>` or
    /// `/<part path>/<Component>/<field>...`; omitted, the whole instance. A prefab root's
    /// `Transform` and `Parent` are never reverted, and removed parts are not re-spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
}

/// Parameters for `prefab.unpack`.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_protocol::command::PrefabUnpackParams;
///
/// let params = PrefabUnpackParams { instance: EntityId::new(1, 0) };
/// assert_eq!(serde_json::to_value(&params).unwrap()["instance"]["index"], 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabUnpackParams {
    /// Any part of the spawned instance.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub instance: EntityId,
}

/// Parameters for `prefab.overrides`.
///
/// # Example
/// ```
/// use ludion_protocol::command::PrefabOverridesParams;
///
/// let params = PrefabOverridesParams::default();
/// assert!(serde_json::to_value(&params).unwrap().get("instances").is_none());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabOverridesParams {
    /// Entities naming the instances to read, each by any of its parts; omitted, every live
    /// instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<crate::schema_proxy::EntityIdSchema>>")]
    pub instances: Option<Vec<EntityId>>,
}

/// Which axis `map.spawn` mirrors the room across; the same spelling as `generate-map --mirror`.
///
/// # Example
/// ```
/// use ludion_protocol::command::MapMirror;
/// assert_eq!(serde_json::to_value(MapMirror::X).unwrap(), "x");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MapMirror {
    /// Flip the room's x axis.
    X,
    /// Flip the room's z axis.
    Z,
}

/// Parameters for `map.spawn`.
///
/// # Example
/// ```
/// use ludion_protocol::command::{MapMirror, MapSpawnParams};
///
/// let params = MapSpawnParams {
///     map: "rooms/crypt.map.yaml".into(),
///     kit: None,
///     seed: Some(7),
///     mirror: Some(MapMirror::X),
///     placement: None,
///     name: Some("a".into()),
/// };
/// assert_eq!(serde_json::to_value(&params).unwrap()["mirror"], "x");
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MapSpawnParams {
    /// The `kind: map` file, relative to the content root.
    pub map: String,
    /// A kit file overriding the map's own `kit`, relative to the content root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kit: Option<String>,
    /// The seed for the map's random choices; omitted, the map's own seed. A JS client loses
    /// precision above 2^53.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Mirror the room across this axis; omitted, no mirror.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<MapMirror>,
    /// A pose baked onto every root part of the room; omitted, the parts keep their authored pose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// An instance name: the parts become `<name>/<part>`; omitted, they keep their bare part names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Parameters for `prefab.despawn`.
///
/// # Example
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_protocol::command::PrefabDespawnParams;
///
/// let params = PrefabDespawnParams { root: EntityId::new(1, 0) };
/// assert_eq!(serde_json::to_value(&params).unwrap()["root"]["index"], 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabDespawnParams {
    /// The instance's root: `entities[0]` of `prefab.spawn` or `map.spawn`.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub root: EntityId,
}

/// Parameters for `component.set`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentSetParams {
    /// The target entity.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub entity: EntityId,
    /// The registry name of the component to insert or overwrite.
    pub component: String,
    /// The component's JSON value, validated at dispatch against the component's schema, its
    /// semantic validator and the entity-reference checks.
    pub value: Value,
}

/// Parameters for `component.get`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentGetParams {
    /// The target entity.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub entity: EntityId,
    /// The registry name of the component to read.
    pub component: String,
}

/// Parameters for `component.remove`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentRemoveParams {
    /// The target entity.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub entity: EntityId,
    /// The registry name of the component to remove.
    pub component: String,
}

/// Parameters for `query.entities`.
///
/// `filter` selects entities that carry **all** of the named components; `include` names the
/// components whose values are returned for each match (empty `include` returns just the ids).
/// `entities` restricts the matches to those ids.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryEntitiesParams {
    /// Registry names an entity must all carry to match (empty = every live entity).
    #[serde(default)]
    pub filter: Vec<String>,
    /// Registry names whose values to return per matched entity (empty = ids only).
    #[serde(default)]
    pub include: Vec<String>,
    /// Also return each matched entity's composed world `Transform` (`world` on the match); an entity
    /// with no `Transform` has none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub world: bool,
    /// Restrict the matches to these ids (absent = every live entity). An id that is not live with
    /// the same generation is skipped silently, never an error; duplicates collapse, and matches
    /// come back in the canonical order (ascending index), not the request order. `[]` matches
    /// nothing. `filter` still applies on top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<crate::schema_proxy::EntityIdSchema>>")]
    pub entities: Option<Vec<EntityId>>,
}

/// The most ticks one `sim.step` may run: one hour at the default 60 Hz. The command runs
/// synchronously on the sim thread, so a larger count is refused with `L3057`.
pub const MAX_STEP_TICKS: u64 = 216_000;

/// Parameters for `sim.step`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimStepParams {
    /// Number of ticks to run before pausing; at most 216000 (`L3057` above it), 0 runs none.
    pub ticks: u64,
    /// Whether to compute and return the `WorldHash` of the resulting state.
    #[serde(default)]
    pub hash: bool,
}

/// Parameters for `sim.set_speed`.
///
/// The factor is validated at dispatch (`L4001` unless it is finite and within `[1/64, 64]`); the
/// protocol layer carries it as a plain `f64` and does not depend on `ludion-sim`'s `Speed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimSetSpeedParams {
    /// Wall-clock pacing factor: `1.0` real time, `2.0` double speed, `0.5` half speed. `L4001`
    /// unless finite and within `[1/64, 64]`.
    pub speed: f64,
}

/// Parameters for `input.inject`.
///
/// `event` is a `ludion-sim` input-event value (`{"kind": "key_down", "key": "W"}`) carried as an
/// opaque [`serde_json::Value`] — the protocol layer does not name the sim's `InputEvent` type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputInjectParams {
    /// The frame at whose tick boundary the event applies; omit to apply at the next tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<u64>")]
    pub frame: Option<FrameIndex>,
    /// The input event, validated at dispatch.
    pub event: Value,
}

/// Parameters for `render.screenshot`.
///
/// Omitted dimensions use the engine's configured render resolution.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderScreenshotParams {
    /// Optional output width in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Optional output height in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Which camera to render through; omit for the scene's own (see [`CameraChoice`]).
    #[serde(default, skip_serializing_if = "CameraChoice::is_scene")]
    pub camera: CameraChoice,
}

/// Whether a host's global-illumination caches follow the world or hold the baseline they already
/// have. The wire form is the bare lowercase name (`"auto"` / `"hold"`).
///
/// Host/session state, never world state: it is not hashed into a `WorldHash`, not captured by
/// `engine.snapshot`, and not recorded into a replay, so a held session simulates and records exactly
/// as an unheld one does.
///
/// # Example
/// ```
/// use ludion_protocol::command::GiRebakeMode;
/// assert_eq!(serde_json::to_value(GiRebakeMode::Hold).unwrap(), serde_json::json!("hold"));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GiRebakeMode {
    /// Both caches follow the world: a changed input rebuilds. The default, and what every engine
    /// did before the lever existed.
    #[default]
    Auto,
    /// Both caches hold what they have. A key miss serves the retained voxelization and probe
    /// volumes and reports staleness instead of rebuilding — the answer for a live session whose
    /// unmarked movers would otherwise force a full revoxelize plus a full bake every frame.
    Hold,
}

/// Parameters for `render.gi_rebake`.
///
/// Both fields are optional and independent, which is what lets one verb set, trigger, and query:
/// an omitted `mode` leaves the lever where it is, so `{}` is a pure read of the reply's
/// `mode` + `stale`.
///
/// # Example
/// ```
/// use ludion_protocol::Command;
/// use ludion_protocol::command::{GiRebakeMode, RenderGiRebakeParams};
///
/// // Hold the baseline *and* refresh it once, in one call.
/// let cmd = Command::RenderGiRebake(RenderGiRebakeParams {
///     mode: Some(GiRebakeMode::Hold),
///     rebake: true,
/// });
/// assert_eq!(
///     serde_json::to_value(&cmd).unwrap(),
///     serde_json::json!({
///         "method": "render.gi_rebake",
///         "params": { "mode": "hold", "rebake": true },
///     }),
/// );
///
/// // The query form changes nothing.
/// assert_eq!(RenderGiRebakeParams::default(), serde_json::from_str("{}").unwrap());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderGiRebakeParams {
    /// The lever's new setting; omit to leave it exactly as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<GiRebakeMode>,
    /// Whether to run one rebake: the next frame rebuilds both caches against the current world and
    /// makes that the new held baseline. A no-op at [`GiRebakeMode::Auto`], where nothing is held.
    #[serde(default)]
    pub rebake: bool,
}

/// Parameters for `render.size`.
///
/// Both fields are optional, which is what lets one verb set and query: omitting both (`{}`) is a
/// pure read of the reply's extent. They are not independent, though — an extent is a *pair*, so a
/// request naming exactly one axis is refused (`L3025`) rather than half-applied. Zero, and anything
/// past 7680 x 4320, is refused the same way.
///
/// **The extent is session state, shared by every attached client: the last writer wins.** That is
/// deliberate, not an oversight — `render.bounds` reports this same number as the pixel space its
/// screen rects are expressed in, so one extent per host is what keeps every viewer's picking and
/// gizmo math agreeing with the pixels it was sent.
///
/// Two consequences worth knowing before reaching for it. A resize *inside* an open
/// `profile.capture` window folds two extents into one median — harmless, but the report then
/// describes no single resolution. And a game's mouse input arrives as render-target pixel
/// positions, so a session played at a resized extent is a different input trace: committed
/// golden-session recordings (`games/*/run.lrp`) are made at the host's default extent.
///
/// # Example
/// ```
/// use ludion_protocol::Command;
/// use ludion_protocol::command::RenderSizeParams;
///
/// // Halve the pixel count a laptop has to move, in one call.
/// let cmd = Command::RenderSize(RenderSizeParams {
///     width: Some(960),
///     height: Some(540),
/// });
/// assert_eq!(
///     serde_json::to_value(&cmd).unwrap(),
///     serde_json::json!({
///         "method": "render.size",
///         "params": { "width": 960, "height": 540 },
///     }),
/// );
///
/// // The query form changes nothing.
/// assert_eq!(RenderSizeParams::default(), serde_json::from_str("{}").unwrap());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderSizeParams {
    /// The new extent's width in pixels; omit (with `height`) to leave the extent as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// The new extent's height in pixels; omit (with `width`) to leave the extent as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// Which camera a per-call render query answers through.
///
/// The default is [`Scene`](CameraChoice::Scene) — the scene's own `Camera` entity — so every call
/// written before `render.view` existed, and every agent that never sets an override, sees exactly
/// the framing the *game* has, whether or not a human is flying the stream somewhere else.
/// [`View`](CameraChoice::View) is the opt-in: a client that set an override asks its queries to
/// follow it. `View` with no override in force is the scene camera, not an error.
///
/// # Example
/// ```
/// use ludion_protocol::command::CameraChoice;
/// assert_eq!(CameraChoice::default(), CameraChoice::Scene);
/// assert_eq!(serde_json::to_value(CameraChoice::View).unwrap(), serde_json::json!("view"));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CameraChoice {
    /// The scene's own `Camera` entity — what a render has always drawn through.
    #[default]
    Scene,
    /// The host's `render.view` override, when one is in force; the scene camera when none is.
    View,
}

impl CameraChoice {
    /// Whether this is the default [`Scene`](CameraChoice::Scene) choice.
    ///
    /// The `skip_serializing_if` behind every `camera` field: a request that did not choose puts no
    /// `camera` key on the wire, so the bytes a client sends are the bytes it sent before the field
    /// existed.
    pub fn is_scene(&self) -> bool {
        matches!(self, CameraChoice::Scene)
    }
}

/// Which camera the camera cull view tests against, for `render.view`.
///
/// The image is always drawn through the rendered camera; this moves only the frustum the GPU cull
/// keeps instances by, so a flown view can show what the game camera would cull. Debug surface:
/// the cull is conservative, and a cull camera that is not the drawn one drops visible meshes.
///
/// # Example
/// ```
/// use ludion_protocol::command::CullFrom;
/// assert_eq!(CullFrom::default(), CullFrom::View);
/// assert_eq!(serde_json::to_value(CullFrom::Frozen).unwrap(), serde_json::json!("frozen"));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CullFrom {
    /// The rendered camera: the flown view, or the scene camera when no override is in force.
    #[default]
    View,
    /// The live scene camera, following the game while the view flies elsewhere.
    Game,
    /// The stream camera as it stood when the command applied, held until the next `view`/`game`.
    Frozen,
}

/// A free camera pose for `render.view`: where the view camera stands, where it looks, and how wide
/// it sees.
///
/// Geometry only — the clip planes, and the field of view when this pose omits one, come from the
/// scene's own `Camera`, so a flown view keeps the game's depth range rather than inventing one.
/// The override is always a *perspective* camera: an orthographic scene camera contributes its clip
/// planes and nothing else.
///
/// # Example
/// ```
/// use ludion_protocol::command::ViewPose;
///
/// let pose = ViewPose { translation: [0.0, 2.0, 6.0], rotation: [-15.0, 0.0, 0.0], fov_y_deg: None };
/// assert_eq!(
///     serde_json::to_value(&pose).unwrap(),
///     serde_json::json!({ "translation": [0.0, 2.0, 6.0], "rotation": [-15.0, 0.0, 0.0] }),
/// );
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewPose {
    /// World-space position of the view camera.
    pub translation: [f32; 3],
    /// Orientation as Euler-XYZ **degrees**, the same convention a `Transform`'s rotation carries.
    pub rotation: [f32; 3],
    /// Vertical field of view in degrees; omit to keep the scene camera's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov_y_deg: Option<f32>,
}

/// Parameters for `render.view`.
///
/// One verb that sets, clears and queries, the [`RenderSizeParams`] shape — but the three forms need
/// three states, so `pose` is a *double* option and each JSON form means something different:
///
/// - `{}` — absent: a pure read. The reply carries the override in force.
/// - `{"pose": null}` — the override is cleared and the stream returns to the scene's `Camera`.
/// - `{"pose": {…}}` — the stream flies to that pose.
///
/// The override is host render state, never world state: it is not hashed into a `WorldHash`, not
/// captured by `engine.snapshot`, and not recorded into a replay, so a session with a flown view
/// simulates and records exactly as one without it. It moves the *stream*; `render.screenshot` and
/// `render.bounds` choose per call through [`CameraChoice`], defaulting to the scene camera.
///
/// # Example
/// ```
/// use ludion_protocol::Command;
/// use ludion_protocol::command::{CullFrom, RenderViewParams, ViewPose};
///
/// // Fly the stream to a pose.
/// let cmd = Command::RenderView(RenderViewParams {
///     pose: Some(Some(ViewPose {
///         translation: [0.0, 2.0, 6.0],
///         rotation: [-15.0, 0.0, 0.0],
///         fov_y_deg: Some(50.0),
///     })),
///     // Keep culling against the live scene camera while the view flies.
///     cull_from: Some(CullFrom::Game),
/// });
/// assert_eq!(
///     serde_json::to_value(&cmd).unwrap(),
///     serde_json::json!({
///         "method": "render.view",
///         "params": { "pose": {
///             "translation": [0.0, 2.0, 6.0],
///             "rotation": [-15.0, 0.0, 0.0],
///             "fov_y_deg": 50.0,
///         }, "cull_from": "game" },
///     }),
/// );
///
/// // An explicit `null` clears it; an absent `pose` only reads.
/// let cleared: RenderViewParams = serde_json::from_str(r#"{"pose": null}"#).unwrap();
/// assert_eq!(cleared.pose, Some(None));
/// assert_eq!(RenderViewParams::default().pose, None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderViewParams {
    /// The pose to fly to, an explicit `null` to clear the override, or absent to read it.
    #[serde(
        default,
        deserialize_with = "explicit_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<ViewPose>")]
    pub pose: Option<Option<ViewPose>>,
    /// Which camera the camera cull view tests against (see [`CullFrom`]); absent keeps what is in
    /// force, `view` is the default. `frozen` captures the stream camera as the command applies and
    /// holds it; `game` follows the live scene camera. The image is still drawn through the view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cull_from: Option<CullFrom>,
}

/// Parameters for `render.debug`.
///
/// One verb that reads and sets. `categories` absent (`{}`) is a pure read; a list replaces the set
/// of enabled debug-draw categories: `[]` turns everything off, and a list containing `"all"` turns
/// everything on. Any string is accepted — a category nothing draws is simply never in the reply's
/// `drawn`, which is how a client checks what a scene actually emits.
///
/// `view` shows a debug view in place of the composite: a name sets it, an explicit `null`
/// clears it, and absent keeps what is in force. A view never turns its effect on — on a frame
/// without SSR, probes or a granted local-light shadow the normal frame is drawn.
///
/// The state is host render state, never world state: not hashed, not captured by `engine.snapshot`,
/// not recorded into a replay. It applies to the stream and to `render.screenshot` alike, so a
/// screenshot taken with `motor` enabled shows the motor's lines.
///
/// # Example
/// ```
/// use ludion_protocol::Command;
/// use ludion_protocol::command::{RenderDebugParams, RenderDebugView};
///
/// let cmd = Command::RenderDebug(RenderDebugParams {
///     categories: Some(vec!["path".to_string()]),
///     view: None,
/// });
/// assert_eq!(
///     serde_json::to_value(&cmd).unwrap(),
///     serde_json::json!({ "method": "render.debug", "params": { "categories": ["path"] } }),
/// );
///
/// // An absent `categories` only reads.
/// assert_eq!(RenderDebugParams::default().categories, None);
///
/// // A `view` sets an SSR debug view, an explicit `null` clears it, an absent one keeps it.
/// let set: RenderDebugParams = serde_json::from_str(r#"{"view": "ssr_confidence"}"#).unwrap();
/// assert_eq!(set.view, Some(Some(RenderDebugView::SsrConfidence)));
/// let cleared: RenderDebugParams = serde_json::from_str(r#"{"view": null}"#).unwrap();
/// assert_eq!(cleared.view, Some(None));
/// ```
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderDebugParams {
    /// The categories to enable (`[]` for none, `"all"` for every one), or absent to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    /// The debug view to show, an explicit `null` to clear it, or absent to keep it.
    #[serde(
        default,
        deserialize_with = "explicit_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<RenderDebugView>")]
    pub view: Option<Option<RenderDebugView>>,
}

/// A debug view shown in place of the composite: the SSR trace at its own resolution, the probe map,
/// the local-light shadow atlas, or the lit frame tinted by sun cascade.
/// The spelling matches `ludion render --debug-view`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderDebugView {
    /// The per-texel trace confidence as a grey ramp.
    SsrConfidence,
    /// The colour-pyramid level each hit read.
    SsrFootprint,
    /// The winning reflection probe per pixel, coloured by rank.
    ProbeIndex,
    /// The local-light shadow atlas, letterboxed: granted tiles as linearised depth, ungranted cells
    /// dark blue, a red line on every cell border.
    LocalShadowAtlas,
    /// Each mesh fragment tinted by the sun cascade that shades it.
    SunCascades,
}

/// Reads an optional field as a *present* value, so an explicit `null` survives as `Some(None)`.
///
/// Serde collapses a plain `Option<Option<T>>` field: a `null` and a missing key both deserialize to
/// `None`, which would make "clear the override" indistinguishable from "read it". `#[serde(default)]`
/// supplies the missing-key `None`; this is the other half.
fn explicit_null<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Parameters for `render.bounds`.
///
/// One optional field, so the wire form a client sent before it existed — a `render.bounds` envelope
/// with no `params` at all — still parses and still means the scene camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderBoundsParams {
    /// Which camera to project the screen rects through; omit for the scene's own.
    #[serde(default, skip_serializing_if = "CameraChoice::is_scene")]
    pub camera: CameraChoice,
}

impl<'de> Deserialize<'de> for RenderBoundsParams {
    /// Accepts the map form *and* an absent `params` key.
    ///
    /// `render.bounds` was a parameterless method, so clients send `{"method": "render.bounds"}` with
    /// no `params` — and an adjacently tagged enum answers a missing `content` by handing the variant
    /// a deserializer that serves exactly one thing: `deserialize_option`. Routing through
    /// [`Option`] is what reaches it, so an absent `params` reads as the default rather than as
    /// `missing field params`. Every other variant's params are required, as they have always been.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// The map form, exactly as a derived `Deserialize` would read it.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(default)]
            camera: CameraChoice,
        }
        let fields = Option::<Fields>::deserialize(deserializer)?;
        Ok(RenderBoundsParams {
            camera: fields.map(|f| f.camera).unwrap_or_default(),
        })
    }
}

/// Parameters for `stream.subscribe`.
///
/// # Example
/// ```
/// use ludion_protocol::command::StreamSubscribeParams;
/// use ludion_protocol::frame::FrameFormat;
/// use ludion_protocol::{Channel, SubscribeMode};
///
/// // A viewer subscribes to the frame stream, degrading fps under load instead of buffering latency.
/// let params = StreamSubscribeParams {
///     channel: Channel::Frames,
///     mode: SubscribeMode::LatestWins { max_fps: Some(30.0) },
///     format: FrameFormat::Qoi,
/// };
/// assert_eq!(
///     serde_json::to_value(&params).unwrap(),
///     serde_json::json!({
///         "channel": "frames",
///         "mode": { "mode": "latest_wins", "max_fps": 30.0 },
///         "format": "QOI",
///     }),
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreamSubscribeParams {
    /// Which channel to subscribe to.
    pub channel: Channel,
    /// Delivery mode (defaults to lossy `latest_wins`).
    #[serde(default)]
    pub mode: SubscribeMode,
    /// Payload codec for `frames` subscriptions. Ignored on every other channel.
    #[serde(default)]
    pub format: FrameFormat,
}

/// Parameters for `stream.unsubscribe`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreamUnsubscribeParams {
    /// The subscription to close (the `sub` id returned by `stream.subscribe`).
    pub sub: SubId,
}

/// Parameters for `asset.reload`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssetReloadParams {
    /// A single asset path to reload; omit to reload every changed asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Parameters for `engine.restore`.
///
/// `snapshot` is an opaque snapshot document produced by `engine.snapshot`; the protocol carries it
/// as a [`serde_json::Value`] and the engine interprets it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EngineRestoreParams {
    /// The snapshot document to restore from.
    pub snapshot: Value,
}

/// A subscription channel (engine-architecture.md §6).
///
/// The wire form is the dotted channel name. `frames` is delivered as binary [`frame`](crate::frame)
/// messages; every other channel is delivered as a JSON [`Event`](crate::Event).
///
/// # Example
/// ```
/// use ludion_protocol::Channel;
/// assert_eq!(serde_json::to_value(Channel::SceneChanged).unwrap(), serde_json::json!("scene.changed"));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Channel {
    /// Binary rendered frames (see [`frame`](crate::frame)).
    #[serde(rename = "frames")]
    Frames,
    /// Structured log records.
    #[serde(rename = "log")]
    Log,
    /// Gameplay/sim events emitted by systems.
    #[serde(rename = "sim.events")]
    SimEvents,
    /// Notifications that the world was replaced: a `scene.load`, a `scene.save`, an `engine.restore`,
    /// or a game-driven scene switch.
    #[serde(rename = "scene.changed")]
    SceneChanged,
    /// Notifications that an asset was reloaded.
    #[serde(rename = "asset.reloaded")]
    AssetReloaded,
    /// Live performance windows (see [`perf`](crate::perf)). Subscribing is what *arms* the
    /// profiler: an engine nobody is watching measures nothing and pays nothing, and the last
    /// unsubscribe (or disconnect) turns it back off. There is deliberately no start/stop verb.
    #[serde(rename = "profile")]
    Profile,
}

/// How a subscription delivers under load (engine-architecture.md §6).
///
/// - [`LatestWins`](SubscribeMode::LatestWins): at most one pending message per subscriber; unsent
///   messages are replaced (the viewer default — degrades fps instead of building latency).
///
/// The wire form is internally tagged by `mode`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum SubscribeMode {
    /// Lossy latest-wins delivery, with an optional frame-rate cap.
    LatestWins {
        /// Optional maximum delivery rate in frames per second (`None` = uncapped).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_fps: Option<f32>,
    },
}

impl Default for SubscribeMode {
    /// The viewer default: lossy latest-wins with no fps cap.
    fn default() -> Self {
        SubscribeMode::LatestWins { max_fps: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_table_matches_the_enum() {
        // The generated Command schema carries one `oneOf` variant per enum variant, in declaration
        // order, each with a `method` const. METHODS must be exactly those names in the same order —
        // a variant added to the enum without a METHODS entry would make the envelope parser reject
        // its (valid) method as L3002.
        let schema = crate::schema::protocol_schema();
        let variants: Vec<&str> = schema["command"]["oneOf"]
            .as_array()
            .expect("Command schema is a oneOf over its variants")
            .iter()
            .map(|v| {
                v["properties"]["method"]["const"]
                    .as_str()
                    .expect("every variant schema pins `method` to a const string")
            })
            .collect();
        assert_eq!(variants, Command::METHODS);
    }

    #[test]
    fn spawn_presentation_flag_is_omitted_when_false_and_round_trips() {
        let sim = EntitySpawnParams {
            name: None,
            components: BTreeMap::new(),
            presentation: false,
        };
        assert_eq!(serde_json::to_value(&sim).unwrap(), serde_json::json!({}));
        let absent: EntitySpawnParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!absent.presentation);
        let flagged = EntitySpawnParams {
            presentation: true,
            ..sim
        };
        let value = serde_json::to_value(&flagged).unwrap();
        assert_eq!(value, serde_json::json!({ "presentation": true }));
        assert_eq!(
            serde_json::from_value::<EntitySpawnParams>(value).unwrap(),
            flagged
        );
    }

    #[test]
    fn the_method_surface_is_the_size_it_is_declared_to_be() {
        // A tripwire, not a fact worth asserting for its own sake: the surface is deliberately small
        // and every addition is a protocol rev, so growing it should require editing this number and
        // saying so in the commit — not slip in unremarked.
        assert_eq!(Command::METHODS.len(), 40);
    }

    /// One instance of every variant, in declaration order. The method-name assertion below is what
    /// keeps it complete: a variant added to the enum and not to this list fails there.
    fn one_of_every_command() -> Vec<Command> {
        let entity = EntityId::new(1, 0);
        vec![
            Command::SceneLoad(SceneLoadParams {
                path: "a.yaml".into(),
            }),
            Command::SceneSave(SceneSaveParams {
                path: "a.yaml".into(),
                force: false,
            }),
            Command::SceneCheck(SceneCheckParams {
                path: "a.yaml".into(),
            }),
            Command::EntitySpawn(EntitySpawnParams {
                name: None,
                components: BTreeMap::new(),
                presentation: false,
            }),
            Command::EntityDestroy(EntityDestroyParams { entity }),
            Command::EntityReparent(EntityReparentParams {
                entity,
                parent: None,
                keep: ReparentKeep::World,
            }),
            Command::PrefabSpawn(PrefabSpawnParams {
                path: "a.prefab.yaml".into(),
                transform: None,
                name: None,
                presentation: false,
            }),
            Command::PrefabCreate(PrefabCreateParams {
                entities: vec![entity],
                path: "a.prefab.yaml".into(),
            }),
            Command::PrefabApply(PrefabApplyParams {
                instance: entity,
                parts: None,
            }),
            Command::PrefabRevert(PrefabRevertParams {
                instance: entity,
                pointer: None,
            }),
            Command::PrefabUnpack(PrefabUnpackParams { instance: entity }),
            Command::PrefabOverrides(PrefabOverridesParams::default()),
            Command::MapSpawn(MapSpawnParams {
                map: "rooms/crypt.map.yaml".into(),
                kit: Some("rooms/ossuary.kit.yaml".into()),
                seed: Some(7),
                mirror: Some(MapMirror::Z),
                placement: None,
                name: Some("a".into()),
            }),
            Command::PrefabDespawn(PrefabDespawnParams { root: entity }),
            Command::ComponentSet(ComponentSetParams {
                entity,
                component: "Transform".into(),
                value: Value::Null,
            }),
            Command::ComponentGet(ComponentGetParams {
                entity,
                component: "Transform".into(),
            }),
            Command::ComponentRemove(ComponentRemoveParams {
                entity,
                component: "Transform".into(),
            }),
            Command::QueryEntities(QueryEntitiesParams::default()),
            Command::SimPause,
            Command::SimResume,
            Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }),
            Command::SimSetSpeed(SimSetSpeedParams { speed: 1.0 }),
            Command::SimHash,
            Command::InputInject(InputInjectParams {
                frame: None,
                event: Value::Null,
            }),
            Command::RenderScreenshot(RenderScreenshotParams::default()),
            Command::RenderDumpGraph,
            Command::RenderDumpTimings,
            Command::RenderBounds(RenderBoundsParams::default()),
            Command::RenderGiRebake(RenderGiRebakeParams::default()),
            Command::RenderSize(RenderSizeParams::default()),
            Command::RenderView(RenderViewParams::default()),
            Command::RenderDebug(RenderDebugParams::default()),
            Command::StreamSubscribe(StreamSubscribeParams {
                channel: Channel::Frames,
                mode: SubscribeMode::default(),
                format: FrameFormat::Qoi,
            }),
            Command::StreamUnsubscribe(StreamUnsubscribeParams { sub: SubId::new(1) }),
            Command::AssetReload(AssetReloadParams {
                path: Some("mesh.gltf".into()),
            }),
            Command::EngineDescribe,
            Command::EngineSnapshot,
            Command::EngineRestore(EngineRestoreParams {
                snapshot: Value::Null,
            }),
            Command::EngineShutdown,
            Command::ProfileCapture(crate::perf::ProfileCaptureParams {
                frames: 1,
                include_samples: false,
            }),
        ]
    }

    #[test]
    fn only_world_or_asset_changing_commands_owe_a_frame() {
        let commands = one_of_every_command();
        let methods: Vec<&str> = commands.iter().map(Command::method).collect();
        assert_eq!(methods, Command::METHODS, "classify every variant");

        let owed: Vec<&str> = commands
            .iter()
            .filter(|command| command.may_change_frame())
            .map(Command::method)
            .collect();
        assert_eq!(
            owed,
            [
                "scene.load",
                "entity.spawn",
                "entity.destroy",
                "entity.reparent",
                "prefab.spawn",
                "prefab.revert",
                "prefab.unpack",
                "map.spawn",
                "prefab.despawn",
                "component.set",
                "component.remove",
                "sim.step",
                "render.gi_rebake",
                "render.size",
                "render.view",
                "render.debug",
                "stream.subscribe",
                "asset.reload",
                "engine.restore",
            ]
        );
    }

    #[test]
    fn the_traps_in_the_frame_classification_stay_classified() {
        // Each of these is where name-based or mutation-based intuition gets the answer wrong; the
        // list is the reason `may_change_frame` is worth a test at all.
        let cmd = |method: &str| {
            one_of_every_command()
                .into_iter()
                .find(|command| command.method() == method)
                .expect("every method has a sample")
        };

        // Queued into the input script, applied at the start of the next tick.
        assert!(!cmd("input.inject").may_change_frame());
        // A paused session never advances its frame index, so this is a subscriber's only trigger.
        assert!(cmd("stream.subscribe").may_change_frame());
        // No entity moves; the reloaded bytes are still drawn.
        assert!(cmd("asset.reload").may_change_frame());
        // The plain mutation, and the poll the editor issues over every registered component.
        assert!(cmd("component.set").may_change_frame());
        assert!(!cmd("query.entities").may_change_frame());
    }

    #[test]
    fn profile_capture_round_trips() {
        let cmd = Command::ProfileCapture(crate::perf::ProfileCaptureParams {
            frames: 120,
            include_samples: true,
        });
        assert_eq!(cmd.method(), "profile.capture");
        let value = serde_json::to_value(&cmd).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "method": "profile.capture",
                "params": { "frames": 120, "include_samples": true },
            })
        );
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), cmd);
    }

    #[test]
    fn gi_rebake_is_one_verb_that_sets_triggers_and_queries() {
        // The query form: no field present, nothing changed, and it still round-trips as itself.
        let query = Command::RenderGiRebake(RenderGiRebakeParams::default());
        assert_eq!(query.method(), "render.gi_rebake");
        let value = serde_json::to_value(&query).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "method": "render.gi_rebake",
                "params": { "rebake": false },
            })
        );
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), query);

        // A client may omit `params` fields entirely; both default.
        let bare: Command = serde_json::from_value(
            serde_json::json!({ "method": "render.gi_rebake", "params": {} }),
        )
        .unwrap();
        assert_eq!(bare, query);

        for (mode, name) in [(GiRebakeMode::Auto, "auto"), (GiRebakeMode::Hold, "hold")] {
            let cmd = Command::RenderGiRebake(RenderGiRebakeParams {
                mode: Some(mode),
                rebake: true,
            });
            let value = serde_json::to_value(&cmd).unwrap();
            assert_eq!(value["params"]["mode"], serde_json::json!(name));
            assert_eq!(value["params"]["rebake"], serde_json::json!(true));
            assert_eq!(serde_json::from_value::<Command>(value).unwrap(), cmd);
        }

        // A misspelled mode is ordinary params validation — no code of its own.
        assert!(
            serde_json::from_value::<Command>(serde_json::json!({
                "method": "render.gi_rebake",
                "params": { "mode": "held" },
            }))
            .is_err()
        );
    }

    #[test]
    fn render_size_is_one_verb_that_sets_and_queries() {
        // The query form: neither axis present, nothing changed, and the params serialize away
        // entirely — a pure read is literally `{}` on the wire.
        let query = Command::RenderSize(RenderSizeParams::default());
        assert_eq!(query.method(), "render.size");
        let value = serde_json::to_value(&query).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "method": "render.size", "params": {} })
        );
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), query);

        let set = Command::RenderSize(RenderSizeParams {
            width: Some(960),
            height: Some(540),
        });
        let value = serde_json::to_value(&set).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "method": "render.size",
                "params": { "width": 960, "height": 540 },
            })
        );
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), set);

        // A half-named extent parses fine here — the pair rule is the dispatcher's (`L3025`), not
        // the wire shape's, so the client gets a diagnostic naming the axis rather than a parse
        // error naming nothing.
        assert_eq!(
            serde_json::from_value::<Command>(serde_json::json!({
                "method": "render.size",
                "params": { "width": 960 },
            }))
            .unwrap(),
            Command::RenderSize(RenderSizeParams {
                width: Some(960),
                height: None,
            })
        );
    }

    /// The three forms of `render.view` are three distinct wire shapes, and the middle one — an
    /// explicit `null` — is the whole reason `pose` is a double option: it must not collapse into
    /// "field absent", which means *read*.
    #[test]
    fn render_view_tells_clear_apart_from_read() {
        let read = Command::RenderView(RenderViewParams::default());
        assert_eq!(read.method(), "render.view");
        let value = serde_json::to_value(&read).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "method": "render.view", "params": {} })
        );
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), read);

        let clear = Command::RenderView(RenderViewParams {
            pose: Some(None),
            cull_from: None,
        });
        let value = serde_json::to_value(&clear).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "method": "render.view", "params": { "pose": null } })
        );
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), clear);
        assert_ne!(
            clear, read,
            "an explicit null must not read as an absent key"
        );

        let set = Command::RenderView(RenderViewParams {
            pose: Some(Some(ViewPose {
                translation: [1.0, 2.0, 3.0],
                rotation: [-15.0, 45.0, 0.0],
                fov_y_deg: None,
            })),
            cull_from: None,
        });
        let value = serde_json::to_value(&set).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "method": "render.view",
                "params": { "pose": {
                    "translation": [1.0, 2.0, 3.0],
                    "rotation": [-15.0, 45.0, 0.0],
                }},
            }),
            "an omitted fov serializes away, so the scene camera's own is what the pose means",
        );
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), set);
    }

    /// `cull_from` is absent by default, parses its snake_case names, and refuses anything else.
    #[test]
    fn render_view_cull_from_parses_and_refuses_unknown() {
        let parse = |text: &str| serde_json::from_str::<RenderViewParams>(text);
        assert_eq!(parse("{}").unwrap().cull_from, None);
        assert_eq!(
            parse(r#"{"cull_from": "frozen"}"#).unwrap().cull_from,
            Some(CullFrom::Frozen)
        );
        assert!(parse(r#"{"cull_from": "bogus"}"#).is_err());
    }

    /// `render.debug`'s `view` has the same three forms as `render.view`'s `pose`: absent reads, an
    /// explicit `null` clears, a name sets. An unknown name is an error, not a silent no-op.
    #[test]
    fn render_debug_view_tells_clear_apart_from_read_and_set() {
        let parse = |text: &str| serde_json::from_str::<RenderDebugParams>(text);
        assert_eq!(parse("{}").unwrap().view, None);
        assert_eq!(parse(r#"{"view": null}"#).unwrap().view, Some(None));
        assert_eq!(
            parse(r#"{"view": "ssr_footprint"}"#).unwrap().view,
            Some(Some(RenderDebugView::SsrFootprint))
        );
        assert_eq!(
            parse(r#"{"view": "probe_index"}"#).unwrap().view,
            Some(Some(RenderDebugView::ProbeIndex))
        );
        assert!(parse(r#"{"view": "ssr_nonsense"}"#).is_err());

        let set = Command::RenderDebug(RenderDebugParams {
            categories: None,
            view: Some(Some(RenderDebugView::SsrConfidence)),
        });
        let value = serde_json::to_value(&set).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "method": "render.debug", "params": { "view": "ssr_confidence" } })
        );
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), set);
    }

    /// **The camera choice is invisible to every client that does not make one.** `render.bounds`
    /// was parameterless and `render.screenshot`'s params were two optional axes; both must keep
    /// parsing and serializing exactly as they did, or every existing agent call, CLI path and
    /// editor poll changes meaning on an engine that merely *can* fly a view.
    #[test]
    fn the_camera_choice_is_absent_from_a_request_that_did_not_make_one() {
        // The shape the editor's picking poll sends: no `params` key at all.
        let bare: Command =
            serde_json::from_value(serde_json::json!({ "method": "render.bounds" })).unwrap();
        assert_eq!(bare, Command::RenderBounds(RenderBoundsParams::default()));
        assert_eq!(
            serde_json::to_value(&bare).unwrap(),
            serde_json::json!({ "method": "render.bounds", "params": {} }),
        );
        // And the map form, which is what a client that *does* choose sends.
        assert_eq!(
            serde_json::from_value::<Command>(serde_json::json!({
                "method": "render.bounds",
                "params": { "camera": "view" },
            }))
            .unwrap(),
            Command::RenderBounds(RenderBoundsParams {
                camera: CameraChoice::View,
            }),
        );

        // A dimensionless screenshot is still literally `{}` on the wire.
        assert_eq!(
            serde_json::to_value(Command::RenderScreenshot(RenderScreenshotParams::default()))
                .unwrap(),
            serde_json::json!({ "method": "render.screenshot", "params": {} }),
        );
        let sized: Command = serde_json::from_value(serde_json::json!({
            "method": "render.screenshot",
            "params": { "width": 320, "height": 180 },
        }))
        .unwrap();
        assert_eq!(
            sized,
            Command::RenderScreenshot(RenderScreenshotParams {
                width: Some(320),
                height: Some(180),
                camera: CameraChoice::Scene,
            }),
        );
        assert_eq!(
            serde_json::to_value(Command::RenderScreenshot(RenderScreenshotParams {
                width: None,
                height: None,
                camera: CameraChoice::View,
            }))
            .unwrap(),
            serde_json::json!({ "method": "render.screenshot", "params": { "camera": "view" } }),
        );
    }

    #[test]
    fn the_profile_channel_is_its_bare_name() {
        assert_eq!(
            serde_json::to_value(Channel::Profile).unwrap(),
            serde_json::json!("profile")
        );
        assert_eq!(
            serde_json::from_value::<Channel>(serde_json::json!("profile")).unwrap(),
            Channel::Profile
        );
    }

    #[test]
    fn unit_variants_serialize_without_params() {
        for cmd in [
            Command::SimPause,
            Command::SimResume,
            Command::SimHash,
            Command::RenderDumpGraph,
            Command::RenderDumpTimings,
            Command::EngineDescribe,
            Command::EngineSnapshot,
            Command::EngineShutdown,
        ] {
            let value = serde_json::to_value(&cmd).unwrap();
            let obj = value.as_object().unwrap();
            assert!(obj.contains_key("method"), "{cmd:?}");
            assert!(
                !obj.contains_key("params"),
                "unit variant must omit params: {cmd:?}"
            );
            // Round-trips.
            assert_eq!(serde_json::from_value::<Command>(value).unwrap(), cmd);
        }
    }

    #[test]
    fn method_accessor_matches_serialized_tag() {
        let cmd = Command::SceneLoad(SceneLoadParams {
            path: "a.yaml".into(),
        });
        assert_eq!(cmd.method(), "scene.load");
        let value = serde_json::to_value(&cmd).unwrap();
        assert_eq!(value["method"], serde_json::json!("scene.load"));
    }

    #[test]
    fn subscribe_mode_defaults_to_latest_wins() {
        assert_eq!(
            SubscribeMode::default(),
            SubscribeMode::LatestWins { max_fps: None }
        );
        assert!(
            serde_json::from_value::<SubscribeMode>(serde_json::json!({ "mode": "acked" }))
                .is_err()
        );
    }
}
