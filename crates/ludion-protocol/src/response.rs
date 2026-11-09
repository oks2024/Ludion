//! [`ResponseBody`] — the `ok` payloads a command produces.
//!
//! A successful response's envelope is `{"id": …, "ok": <ResponseBody>, "frame": …}`
//! (engine-architecture.md §6). The `frame` — the sim frame at which the command applied — lives on
//! the [envelope](crate::envelope::WireResult), not in the body, so *every* success carries it
//! uniformly. The body is internally tagged by a `kind` discriminant so an `ok` payload is
//! self-describing and never ambiguous between similarly shaped variants (e.g. the empty
//! [`Ack`](ResponseBody::Ack) versus another all-optional payload).

use std::collections::BTreeMap;

use ludion_core::ids::EntityId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::WireHash;
use crate::envelope::SubId;

/// The success payload of a command response, tagged by `kind`.
///
/// # Example
/// ```
/// use ludion_protocol::ResponseBody;
/// use ludion_core::ids::EntityId;
///
/// let body = ResponseBody::Spawned { entity: EntityId::new(7, 0) };
/// assert_eq!(
///     serde_json::to_value(&body).unwrap(),
///     serde_json::json!({ "kind": "spawned", "entity": { "index": 7, "generation": 0 } }),
/// );
///
/// // The empty acknowledgement is just its tag.
/// assert_eq!(serde_json::to_value(ResponseBody::Ack).unwrap(), serde_json::json!({ "kind": "ack" }));
/// ```
// `#[non_exhaustive]`: a future additive protocol rev may add a response `kind`, so a downstream
// client that matches on this must carry a wildcard arm. Unlike `Command` — whose exhaustive dispatch
// match *is* the parity gate — response bodies are only ever *consumed* downstream, so reserving room
// to grow costs a client nothing but the wildcard. Kept as a plain comment, not a `///` doc: the
// enum's doc feeds schemars' `description`, and editing it would move the protocol schema hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResponseBody {
    /// A bare acknowledgement — the command succeeded and has no data to return (its effect is
    /// captured by the envelope `frame`). Used by `sim.pause`, `sim.resume`, `sim.set_speed`,
    /// `input.inject`, and `stream.unsubscribe`.
    Ack,

    /// Result of `scene.load`.
    SceneLoaded {
        /// How many entities the loaded scene instantiated.
        entity_count: u32,
    },
    /// Result of `scene.save`.
    SceneSaved {
        /// The path the canonical YAML was written to.
        path: String,
        /// What `scene.check` reports over the written text; the file was written regardless.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[schemars(with = "Vec<crate::schema_proxy::WireErrorSchema>")]
        diagnostics: Vec<ludion_core::diagnostics::Diagnostic>,
    },
    /// Result of `scene.check`.
    SceneChecked {
        /// Whether the scene validated with no errors.
        valid: bool,
        /// The diagnostics produced (empty when `valid`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[schemars(with = "Vec<crate::schema_proxy::WireErrorSchema>")]
        diagnostics: Vec<ludion_core::diagnostics::Diagnostic>,
    },

    /// Result of `entity.spawn`.
    Spawned {
        /// The id of the newly created entity.
        #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
        entity: EntityId,
    },
    /// Result of `entity.destroy` — the destroyed entity's full component state at the moment it was
    /// despawned (the undo inverse: `entity.spawn` with these components respawns it, under a fresh
    /// id). Keyed by registry name, sorted by name (`BTreeMap` order); includes `Name` if the entity
    /// carried one — it is a component like any other on the wire. Destroying an entity destroys its
    /// descendants too: `descendants` lists each, every parent before its children, so a client
    /// respawns the root and then each descendant in order, remapping the old ids its `Parent`
    /// components name to the fresh ones.
    EntityDestroyed {
        /// The destroyed entity's components, keyed by registry name.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        components: BTreeMap<String, Value>,
        /// The cascaded descendants, parents before children; empty when the entity had none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        descendants: Vec<DestroyedEntity>,
    },
    /// Result of `entity.reparent` — the entity's `Parent` and `Transform` immediately before (the
    /// undo inverse: `entity.reparent` with `parent: previous_parent` and `keep: local`, then
    /// `component.set Transform` with `previous_transform` when it is present).
    Reparented {
        /// The parent the entity had, or `null` for a root.
        #[schemars(with = "Option<crate::schema_proxy::EntityIdSchema>")]
        previous_parent: Option<EntityId>,
        /// The entity's `Transform` before the call, or `null` when it had none.
        previous_transform: Option<Value>,
    },
    /// Result of `prefab.spawn` — the spawned entities in the prefab's part order; the first is the
    /// instance root (`PrefabInstance.root`). The undo is `entity.destroy` on each root part.
    PrefabSpawned {
        /// The new entity ids, one per prefab part, in part order.
        #[schemars(with = "Vec<crate::schema_proxy::EntityIdSchema>")]
        entities: Vec<EntityId>,
    },
    /// Result of `prefab.create` — the file written.
    PrefabCreated {
        /// The prefab's normalized path, relative to the content root.
        path: String,
    },
    /// Result of `prefab.apply` — what was written into the prefab file.
    PrefabApplied {
        /// The prefab file written, relative to the content root.
        path: String,
        /// The part paths whose component overrides were written into the prefab.
        parts: Vec<String>,
        /// The part paths deleted from the prefab.
        removed: Vec<String>,
    },
    /// Result of `prefab.revert` — what the revert changed.
    PrefabReverted {
        /// The parts with at least one component written or removed, in live order.
        #[schemars(with = "Vec<crate::schema_proxy::EntityIdSchema>")]
        entities: Vec<EntityId>,
        /// The prefab's part paths with no live part: removed from the instance, and not re-spawned.
        not_restored: Vec<String>,
    },
    /// Result of `prefab.unpack` — the instance's parts, now plain entities.
    PrefabUnpacked {
        /// The parts' ids, in live order.
        #[schemars(with = "Vec<crate::schema_proxy::EntityIdSchema>")]
        entities: Vec<EntityId>,
    },
    /// Result of `prefab.overrides` — one entry per instance, in request order (all live instances
    /// in world order when none were named).
    PrefabOverrides {
        /// What `scene.save` would write for each instance.
        instances: Vec<PrefabInstanceOverrides>,
    },

    /// Result of `component.set` — the component's value immediately before the set (the undo
    /// inverse: `component.set` with this value restores it), or `None` when the set was a fresh
    /// insert (the entity did not carry the component before).
    ComponentSet {
        /// The value the component held immediately before this set, or `None` for a fresh insert.
        previous: Option<Value>,
    },
    /// Result of `component.get`.
    ComponentValue {
        /// The component's JSON value.
        value: Value,
    },
    /// Result of `component.remove` — the removed component's value (the undo inverse:
    /// `component.set` with this value re-inserts it).
    ComponentRemoved {
        /// The value the component held immediately before removal.
        previous: Value,
    },

    /// Result of `query.entities`.
    QueryResult {
        /// The matched entities and their requested component values.
        entities: Vec<EntityMatch>,
    },

    /// Result of `sim.step`.
    Stepped {
        /// The `WorldHash` of the resulting state, present iff the request set `hash: true`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<WireHash>,
    },
    /// Result of `sim.hash`.
    SimHash {
        /// The current `WorldHash`.
        hash: WireHash,
    },

    /// Result of `render.screenshot`.
    Screenshot {
        /// Image width in pixels.
        width: u32,
        /// Image height in pixels.
        height: u32,
        /// The image encoding, always "png".
        format: String,
        /// The image bytes as standard (RFC 4648) base64 of the PNG. The command path favours a
        /// self-contained JSON payload over the binary frame channel; agents decode this directly.
        data: String,
        /// Whether this image's global illumination was *held* rather than rebuilt for the world it
        /// shows (`render.gi_rebake` at `hold`). An agent that screenshots a stale world learns so
        /// from the same reply, instead of reasoning about light that is one edit out of date.
        stale: bool,
    },
    /// Result of `render.dump_graph` — the render graph structure as JSON.
    GraphDump {
        /// The graph description (opaque to this crate; produced by `ludion-render-graph`).
        graph: Value,
    },
    /// Result of `render.dump_timings` — per-pass GPU timings as JSON.
    TimingsDump {
        /// The timings document (opaque to this crate).
        timings: Value,
    },
    /// Result of `render.bounds` — the world AABB and screen rect of every renderable entity under
    /// the current camera (pure CPU; no GPU). Entities are in the same ascending-slot order `extract`
    /// walks, so the response is a deterministic function of the world.
    RenderBounds {
        /// The pixel resolution the screen rects are expressed in (the engine's render size).
        render_size: [u32; 2],
        /// One entry per renderable entity (sprite, billboard, mesh, or text), in extract order.
        entities: Vec<EntityBounds>,
    },

    /// Result of `render.gi_rebake` — the lever's setting after the command applied, and the
    /// staleness of the last frame this host produced. Every form of the command answers with both,
    /// so a client that only wants to *read* sends empty params.
    GiRebake {
        /// The rebake lever, after any `mode` in the request was applied.
        mode: crate::command::GiRebakeMode,
        /// Whether the last frame this host produced showed held global illumination. A requested
        /// rebake has not run yet when this reply is written, so it still describes the frame
        /// before it.
        stale: bool,
    },

    /// Result of `render.size` — the extent in force once the command applied. Every form of the
    /// command answers with it, so a client that only wants to *read* sends empty params, and one
    /// that set an extent learns the resulting pair rather than assuming its request was taken
    /// verbatim.
    RenderSize {
        /// The session's render width in pixels: what live stream frames are produced at, and what a
        /// dimensionless `render.screenshot` defaults to.
        width: u32,
        /// The session's render height in pixels. See `width`.
        height: u32,
    },

    /// Result of `render.view` — the view override in force once the command applied, or `null`
    /// when the stream is back on the scene's own `Camera`. Every form of the command answers with
    /// it, so a client that only wants to *read* sends empty params.
    RenderView {
        /// The pose the stream is flying at, or `None` for the scene camera.
        pose: Option<crate::command::ViewPose>,
        /// The camera the camera cull view tests against.
        cull_from: crate::command::CullFrom,
    },

    /// Result of `render.debug` — the debug-draw categories enabled once the command applied, and
    /// the categories the last completed tick emitted lines for. Every form of the command answers
    /// with both, so a client that only wants to *read* sends empty params.
    RenderDebug {
        /// The enabled categories, sorted.
        enabled: Vec<String>,
        /// The categories the last completed tick emitted lines for, sorted, whether or not they are
        /// enabled — so enabling a category nobody draws shows up as a missing entry here.
        drawn: Vec<String>,
        /// The SSR debug view in force. It does not say whether the view was drawn: a view on a
        /// frame without SSR draws the normal frame, and `render.dump_graph` shows whether
        /// `ssr_debug` is scheduled.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<crate::command::RenderDebugView>,
    },

    /// Result of `stream.subscribe`.
    Subscribed {
        /// The new subscription id; events on the channel carry it as their `sub`.
        sub: SubId,
    },

    /// Result of `asset.reload`.
    AssetsReloaded {
        /// The asset paths that were reloaded.
        paths: Vec<String>,
    },

    /// Result of `engine.describe` — the full machine-readable engine surface.
    EngineDescription {
        /// Component schemas from the registry plus the protocol schema (opaque to this crate).
        description: Value,
    },
    /// Result of `engine.snapshot` — the serialized engine state.
    Snapshot {
        /// The snapshot document; feed it back to `engine.restore` (opaque to this crate).
        snapshot: Value,
    },
    /// Result of `engine.restore` — the migration report (empty = a clean, lossless restore).
    ///
    /// Restore is a migration, not a strict round-trip: component-schema drift between the snapshot
    /// and the running build is reported here (dropped and field-migrated components) rather than
    /// failing the restore. See [`RestoreReport`](crate::restore::RestoreReport).
    Restored {
        /// What the migration had to change to load the snapshot (empty when nothing changed).
        report: crate::restore::RestoreReport,
    },

    /// Result of `profile.capture` — one performance report over the measured live frames.
    ///
    /// The same shape the offline `ludion profile` driver writes (see [`perf`](crate::perf)), so a
    /// budget file, a chart, or a trace exporter reads either source unchanged.
    ProfileReport {
        /// The measured window, reshaped as a report.
        // Boxed: a report dwarfs every other response body, and this enum is carried by value
        // through the whole reply path. `Box` is invisible on the wire and in the schema.
        report: Box<crate::perf::ProfileReport>,
    },
}

/// A descendant destroyed with its ancestor by `entity.destroy`: its old id and component state.
///
/// # Example
/// ```
/// use ludion_protocol::response::DestroyedEntity;
/// use ludion_core::ids::EntityId;
/// use std::collections::BTreeMap;
///
/// let d = DestroyedEntity { entity: EntityId::new(5, 0), components: BTreeMap::new() };
/// assert_eq!(d.entity.index(), 5);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DestroyedEntity {
    /// The destroyed entity's id; the `Parent` values in `components` name ids of this kind.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub entity: EntityId,
    /// Its components, keyed by registry name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, Value>,
}

/// One prefab instance as `prefab.overrides` reports it: what `scene.save` would write for it.
///
/// # Example
/// ```
/// use ludion_protocol::response::PrefabInstanceOverrides;
/// use ludion_core::ids::EntityId;
/// use std::collections::BTreeMap;
///
/// let o = PrefabInstanceOverrides {
///     root: EntityId::new(3, 0),
///     prefab: "lamp.prefab.yaml".into(),
///     name: Some("lamp".into()),
///     presentation: false,
///     placement_differs: false,
///     overrides: BTreeMap::from([("bulb".to_owned(), vec!["/Transform/translation".to_owned()])]),
///     removed: Vec::new(),
///     root_parts: vec!["lamp".to_owned()],
///     diagnostic: None,
/// };
/// assert!(serde_json::to_value(&o).unwrap().get("diagnostic").is_none());
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PrefabInstanceOverrides {
    /// The instance's root, the `PrefabInstance.root` its parts carry; may be a dead entity.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub root: EntityId,
    /// The prefab file, relative to the content root.
    pub prefab: String,
    /// The instance's name, `null` for an unnamed spawn (which a save leaves as plain entities).
    pub name: Option<String>,
    /// Whether the saved entry carries `presentation: true`.
    #[serde(default)]
    pub presentation: bool,
    /// Whether the saved `transform` differs, bit for bit, from the placement the instance was
    /// spawned or loaded at.
    pub placement_differs: bool,
    /// Part path to the JSON pointers of the overridden leaves, each `/<Component>[/<field>...]`.
    /// `/<part path>` followed by a pointer is exactly a `prefab.revert` pointer.
    pub overrides: BTreeMap<String, Vec<String>>,
    /// The part paths the instance dropped from its prefab.
    pub removed: Vec<String>,
    /// The prefab's root part paths, in part order: the parts with no `Parent` in the prefab file.
    /// Their `Transform` and `Parent` are the instance's placement, which `prefab.revert` leaves
    /// alone. Empty when `diagnostic` is set or the prefab cannot be read.
    pub root_parts: Vec<String>,
    /// The `L1234` or `L1230` a save would raise for this instance, in which case the fields above
    /// are empty or false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_proxy::WireErrorSchema>")]
    pub diagnostic: Option<ludion_core::diagnostics::Diagnostic>,
}

/// An entity's composed world transform, as `query.entities` reports it with `world: true`.
///
/// # Example
/// ```
/// use ludion_protocol::response::WorldTransform;
///
/// let w = WorldTransform {
///     translation: [1.0, 2.0, 3.0],
///     rotation: [0.0, 90.0, 0.0],
///     scale: [1.0, 1.0, 1.0],
///     sheared: false,
/// };
/// assert_eq!(w.rotation[1], 90.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorldTransform {
    /// World-space translation `[x, y, z]`.
    pub translation: [f32; 3],
    /// World-space rotation as XYZ Euler angles in degrees.
    pub rotation: [f32; 3],
    /// World-space scale `[x, y, z]`.
    pub scale: [f32; 3],
    /// A rotated child sits under a non-uniform accumulated scale, so the drawn transform has a shear
    /// this translation/rotation/scale cannot express.
    pub sheared: bool,
}

/// One entity matched by `query.entities`, with the requested component values.
///
/// # Example
/// ```
/// use ludion_protocol::response::EntityMatch;
/// use ludion_core::ids::EntityId;
/// use std::collections::BTreeMap;
///
/// let mut components = BTreeMap::new();
/// components.insert("Transform".to_string(), serde_json::json!({ "position": [0.0, 0.0, 0.0] }));
/// let m = EntityMatch { entity: EntityId::new(3, 1), components, world: None };
/// assert_eq!(m.entity.index(), 3);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EntityMatch {
    /// The matched entity.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub entity: EntityId,
    /// The requested component values, keyed by registry name (empty if none were requested).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, Value>,
    /// The composed world transform; present only when the query set `world` and the entity has a
    /// `Transform`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<WorldTransform>,
}

/// One renderable entity's bounds, as returned by `render.bounds`.
///
/// The `world_aabb` is always present — an entity's world-space extent is well-defined regardless of
/// the camera. The `screen_rect` is `null` when the entity's whole AABB is behind the camera (there
/// is no meaningful projection); otherwise it is the pixel rect bounding the AABB's projected
/// corners, which may fall partly or wholly outside the viewport — `on_screen` is the honest test for
/// "would a pixel of this land on the rendered image". An agent must consult both: a `null` rect
/// means *behind the camera*, a non-null rect with `on_screen: false` means *off to the side / out of
/// frame*.
///
/// # Example
/// ```
/// use ludion_protocol::response::{EntityBounds, WorldAabb, ScreenRect};
/// use ludion_core::ids::EntityId;
///
/// let b = EntityBounds {
///     entity: EntityId::new(2, 0),
///     world_aabb: WorldAabb { min: [3.0, 1.5, 0.0], max: [7.0, 3.5, 0.0] },
///     screen_rect: Some(ScreenRect { x: 130.0, y: 15.0, w: 40.0, h: 20.0 }),
///     on_screen: true,
///     hidden: false,
/// };
/// assert_eq!(b.world_aabb.max, [7.0, 3.5, 0.0]);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EntityBounds {
    /// The entity these bounds belong to.
    #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
    pub entity: EntityId,
    /// The entity's world-space axis-aligned bounding box.
    pub world_aabb: WorldAabb,
    /// The pixel rect bounding the AABB under the current camera, or `null` when the AABB is entirely
    /// behind the camera.
    pub screen_rect: Option<ScreenRect>,
    /// Whether `screen_rect` is present and intersects the `[0, render_size]` viewport.
    pub on_screen: bool,
    /// The entity, or an ancestor, has `Visibility { visible: false }`: it is not drawn; picking still
    /// finds it. Omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// A world-space axis-aligned bounding box: componentwise `min` ≤ `max`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorldAabb {
    /// The minimum corner `[x, y, z]`.
    pub min: [f32; 3],
    /// The maximum corner `[x, y, z]`.
    pub max: [f32; 3],
}

/// A screen-space rectangle in pixels, origin top-left, `x`/`y` at the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenRect {
    /// Left edge in pixels (may be negative when the entity extends off the left of the viewport).
    pub x: f32,
    /// Top edge in pixels (may be negative when the entity extends off the top of the viewport).
    pub y: f32,
    /// Width in pixels.
    pub w: f32,
    /// Height in pixels.
    pub h: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_is_just_its_tag() {
        assert_eq!(
            serde_json::to_value(ResponseBody::Ack).unwrap(),
            serde_json::json!({ "kind": "ack" }),
        );
    }

    #[test]
    fn stepped_omits_absent_hash_but_keeps_present_one() {
        assert_eq!(
            serde_json::to_value(ResponseBody::Stepped { hash: None }).unwrap(),
            serde_json::json!({ "kind": "stepped" }),
        );
        let h = WireHash::from_u64(0xdead_beef);
        assert_eq!(
            serde_json::to_value(ResponseBody::Stepped { hash: Some(h) }).unwrap(),
            serde_json::json!({ "kind": "stepped", "hash": "00000000deadbeef" }),
        );
    }

    #[test]
    fn bodies_round_trip() {
        let mut components = BTreeMap::new();
        components.insert("Transform".to_string(), serde_json::json!({ "x": 1.0 }));

        let bodies = [
            ResponseBody::Ack,
            ResponseBody::SceneLoaded { entity_count: 12 },
            ResponseBody::Spawned {
                entity: EntityId::new(1, 0),
            },
            ResponseBody::EntityDestroyed {
                components: components.clone(),
                descendants: vec![DestroyedEntity {
                    entity: EntityId::new(2, 0),
                    components: components.clone(),
                }],
            },
            ResponseBody::Reparented {
                previous_parent: Some(EntityId::new(1, 0)),
                previous_transform: None,
            },
            ResponseBody::PrefabSpawned {
                entities: vec![EntityId::new(3, 0), EntityId::new(4, 0)],
            },
            ResponseBody::PrefabCreated {
                path: "a.prefab.yaml".into(),
            },
            ResponseBody::PrefabApplied {
                path: "a.prefab.yaml".into(),
                parts: vec!["bulb".into()],
                removed: Vec::new(),
            },
            ResponseBody::PrefabReverted {
                entities: vec![EntityId::new(4, 0)],
                not_restored: vec!["lid".into()],
            },
            ResponseBody::PrefabUnpacked {
                entities: vec![EntityId::new(3, 0), EntityId::new(4, 0)],
            },
            ResponseBody::PrefabOverrides {
                instances: vec![PrefabInstanceOverrides {
                    root: EntityId::new(3, 0),
                    prefab: "a.prefab.yaml".into(),
                    name: Some("a".into()),
                    placement_differs: true,
                    overrides: BTreeMap::from([("bulb".into(), vec!["/Transform".into()])]),
                    removed: vec!["lid".into()],
                    root_parts: vec!["base".into()],
                    diagnostic: None,
                    presentation: false,
                }],
            },
            ResponseBody::ComponentSet {
                previous: Some(serde_json::json!({ "x": 0.0 })),
            },
            ResponseBody::ComponentSet { previous: None },
            ResponseBody::ComponentRemoved {
                previous: serde_json::json!({ "x": 0.0 }),
            },
            ResponseBody::SimHash {
                hash: WireHash::from_u64(7),
            },
            ResponseBody::Subscribed { sub: SubId::new(3) },
        ];
        for body in bodies {
            let value = serde_json::to_value(&body).unwrap();
            assert_eq!(serde_json::from_value::<ResponseBody>(value).unwrap(), body);
        }
    }

    #[test]
    fn profile_report_body_round_trips() {
        let body = ResponseBody::ProfileReport {
            report: Box::new(crate::perf::ProfileReport {
                scene: "scenes/plaza.yaml".into(),
                warmup_frames: 0,
                measured_frames: 60,
                quality: None,
                sim: crate::perf::SimProfile {
                    systems: Vec::new(),
                    sequential: Vec::new(),
                    frame_total_median_ms: 1.5,
                    frame_total_tail: None,
                    threads: Some(8),
                },
                gpu: crate::perf::GpuProfile {
                    available: false,
                    timestamps_supported: false,
                    frame_median_ms: None,
                    frame_tail: None,
                    passes: Vec::new(),
                },
                transport: None,
                cluster: None,
                local_shadows: None,
                sun_cascades: None,
                decals: None,
                load: None,
                probes: None,
                culling: None,
                skin: None,
                samples: None,
            }),
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["kind"], serde_json::json!("profile_report"));
        assert_eq!(value["report"]["measured_frames"], serde_json::json!(60));
        assert_eq!(serde_json::from_value::<ResponseBody>(value).unwrap(), body);
    }

    #[test]
    fn gi_rebake_answers_with_the_lever_and_the_staleness() {
        let body = ResponseBody::GiRebake {
            mode: crate::command::GiRebakeMode::Hold,
            stale: true,
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({ "kind": "gi_rebake", "mode": "hold", "stale": true }),
        );
        assert_eq!(
            serde_json::from_value::<ResponseBody>(serde_json::to_value(&body).unwrap()).unwrap(),
            body,
        );
        assert_eq!(
            serde_json::to_value(ResponseBody::GiRebake {
                mode: crate::command::GiRebakeMode::Auto,
                stale: false,
            })
            .unwrap(),
            serde_json::json!({ "kind": "gi_rebake", "mode": "auto", "stale": false }),
        );
    }

    #[test]
    fn render_size_answers_with_the_resulting_extent() {
        // Both axes are always present — a client that set an extent reads back what actually took
        // effect, and one that sent `{}` reads the same shape without a second method.
        let body = ResponseBody::RenderSize {
            width: 960,
            height: 540,
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({ "kind": "render_size", "width": 960, "height": 540 }),
        );
        assert_eq!(
            serde_json::from_value::<ResponseBody>(serde_json::to_value(&body).unwrap()).unwrap(),
            body,
        );
    }

    #[test]
    fn a_screenshot_carries_its_own_gi_staleness() {
        // The field is not optional: an agent decoding a screenshot always learns whether the light
        // it is looking at was rebuilt for this world, without a second round trip.
        let body = ResponseBody::Screenshot {
            width: 2,
            height: 1,
            format: "png".into(),
            data: "AA==".into(),
            stale: true,
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["kind"], serde_json::json!("screenshot"));
        assert_eq!(value["stale"], serde_json::json!(true));
        assert_eq!(serde_json::from_value::<ResponseBody>(value).unwrap(), body);
    }

    #[test]
    fn component_set_previous_is_explicit_null_for_a_fresh_insert() {
        // `previous` has no `skip_serializing_if` — the key is always present (`null` for a fresh
        // insert), so a client can distinguish "no previous value" from "field omitted".
        assert_eq!(
            serde_json::to_value(ResponseBody::ComponentSet { previous: None }).unwrap(),
            serde_json::json!({ "kind": "component_set", "previous": null }),
        );
    }
}
