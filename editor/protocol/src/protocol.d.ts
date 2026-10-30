/* eslint-disable */
/**
 * This file is generated from `crates/ludion-protocol/tests/protocol_schema.snapshot.json`
 * by `editor/protocol/scripts/generate.js`. Do not edit by hand -- run `npm run generate`
 * (from `editor/protocol/`) after the Rust-side schema snapshot changes.
 */

/**
 * Every capability the engine exposes over the protocol, as one exhaustive enum.
 *
 * The wire form is adjacently tagged: the dotted `method` name plus a `params` object (absent for
 * parameterless methods). Adding a variant here is the parity gate — a command with no dispatch
 * handler is a compile error in `ludion-engine`.
 *
 * # Example
 * ```
 * use ludion_protocol::Command;
 * use ludion_protocol::command::EntityDestroyParams;
 * use ludion_core::ids::EntityId;
 *
 * let cmd = Command::EntityDestroy(EntityDestroyParams { entity: EntityId::new(7, 0) });
 * assert_eq!(cmd.method(), "entity.destroy");
 * assert_eq!(
 *     serde_json::to_value(&cmd).unwrap(),
 *     serde_json::json!({
 *         "method": "entity.destroy",
 *         "params": { "entity": { "index": 7, "generation": 0 } },
 *     }),
 * );
 *
 * // A parameterless method serializes with no `params` key.
 * assert_eq!(
 *     serde_json::to_value(&Command::SimPause).unwrap(),
 *     serde_json::json!({ "method": "sim.pause" }),
 * );
 * ```
 */
export type Command =
  | {
      method: 'scene.load';
      params: SceneLoadParams;
      [k: string]: unknown;
    }
  | {
      method: 'scene.save';
      params: SceneSaveParams;
      [k: string]: unknown;
    }
  | {
      method: 'scene.check';
      params: SceneCheckParams;
      [k: string]: unknown;
    }
  | {
      method: 'entity.spawn';
      params: EntitySpawnParams;
      [k: string]: unknown;
    }
  | {
      method: 'entity.destroy';
      params: EntityDestroyParams;
      [k: string]: unknown;
    }
  | {
      method: 'entity.reparent';
      params: EntityReparentParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.spawn';
      params: PrefabSpawnParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.create';
      params: PrefabCreateParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.apply';
      params: PrefabApplyParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.revert';
      params: PrefabRevertParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.unpack';
      params: PrefabUnpackParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.overrides';
      params: PrefabOverridesParams;
      [k: string]: unknown;
    }
  | {
      method: 'component.set';
      params: ComponentSetParams;
      [k: string]: unknown;
    }
  | {
      method: 'component.get';
      params: ComponentGetParams;
      [k: string]: unknown;
    }
  | {
      method: 'component.remove';
      params: ComponentRemoveParams;
      [k: string]: unknown;
    }
  | {
      method: 'query.entities';
      params: QueryEntitiesParams;
      [k: string]: unknown;
    }
  | {
      method: 'sim.pause';
      [k: string]: unknown;
    }
  | {
      method: 'sim.resume';
      [k: string]: unknown;
    }
  | {
      method: 'sim.step';
      params: SimStepParams;
      [k: string]: unknown;
    }
  | {
      method: 'sim.set_speed';
      params: SimSetSpeedParams;
      [k: string]: unknown;
    }
  | {
      method: 'sim.hash';
      [k: string]: unknown;
    }
  | {
      method: 'input.inject';
      params: InputInjectParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.screenshot';
      params: RenderScreenshotParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.dump_graph';
      [k: string]: unknown;
    }
  | {
      method: 'render.dump_timings';
      [k: string]: unknown;
    }
  | {
      method: 'render.bounds';
      params: RenderBoundsParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.gi_rebake';
      params: RenderGiRebakeParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.size';
      params: RenderSizeParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.view';
      params: RenderViewParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.debug';
      params: RenderDebugParams;
      [k: string]: unknown;
    }
  | {
      method: 'stream.subscribe';
      params: StreamSubscribeParams;
      [k: string]: unknown;
    }
  | {
      method: 'stream.unsubscribe';
      params: StreamUnsubscribeParams;
      [k: string]: unknown;
    }
  | {
      method: 'asset.reload';
      params: AssetReloadParams;
      [k: string]: unknown;
    }
  | {
      method: 'engine.describe';
      [k: string]: unknown;
    }
  | {
      method: 'engine.snapshot';
      [k: string]: unknown;
    }
  | {
      method: 'engine.restore';
      params: EngineRestoreParams;
      [k: string]: unknown;
    }
  | {
      method: 'engine.shutdown';
      [k: string]: unknown;
    }
  | {
      method: 'profile.capture';
      params: ProfileCaptureParams;
      [k: string]: unknown;
    };
/**
 * Whether a host's global-illumination caches follow the world or hold the baseline they already
 * have. The wire form is the bare lowercase name (`"auto"` / `"hold"`).
 *
 * Host/session state, never world state: it is not hashed into a `WorldHash`, not captured by
 * `engine.snapshot`, and not recorded into a replay, so a held session simulates and records exactly
 * as an unheld one does.
 *
 * # Example
 * ```
 * use ludion_protocol::command::GiRebakeMode;
 * assert_eq!(serde_json::to_value(GiRebakeMode::Hold).unwrap(), serde_json::json!("hold"));
 * ```
 */
export type GiRebakeMode = 'auto' | 'hold';
/**
 * A debug view shown in place of the composite: the SSR trace at its own resolution, the probe map,
 * the local-light shadow atlas, or the lit frame tinted by sun cascade.
 * The spelling matches `ludion render --debug-view`.
 */
export type RenderDebugView =
  'ssr_confidence' | 'ssr_footprint' | 'probe_index' | 'local_shadow_atlas' | 'sun_cascades';
/**
 * The event envelope: one [`Event`] (its `event` name + `data`), plus `sub`, `seq`, and an optional
 * `frame`.
 *
 * # Example
 * ```
 * use ludion_protocol::{Event, EventMessage};
 * use ludion_protocol::event::{LogEvent, LogLevel};
 * use ludion_protocol::envelope::{Seq, SubId};
 *
 * let msg = EventMessage {
 *     event: Event::Log(LogEvent { level: LogLevel::Info, message: "loaded".into(), diagnostic: None }),
 *     sub: SubId::new(7),
 *     seq: Seq::new(981),
 *     frame: None,
 * };
 * let value = serde_json::to_value(&msg).unwrap();
 * assert_eq!(value["event"], serde_json::json!("log"));
 * assert_eq!(value["sub"], serde_json::json!(7));
 * assert_eq!(value["data"]["message"], serde_json::json!("loaded"));
 * ```
 */
export type EventMessage = {
  /**
   * The sim frame the event describes, when applicable.
   */
  frame?: number | null;
  /**
   * The per-subscription monotonic sequence number.
   */
  seq: number;
  /**
   * The subscription this event belongs to.
   */
  sub: number;
  [k: string]: unknown;
} & EventMessage1;
export type EventMessage1 =
  | {
      data: LogEvent;
      event: 'log';
      [k: string]: unknown;
    }
  | {
      data: SimEventsData;
      event: 'sim.events';
      [k: string]: unknown;
    }
  | {
      data: SceneChangedData;
      event: 'scene.changed';
      [k: string]: unknown;
    }
  | {
      data: AssetReloadedData;
      event: 'asset.reloaded';
      [k: string]: unknown;
    }
  | {
      data: ProfileWindowData;
      event: 'profile';
      [k: string]: unknown;
    };
/**
 * Mirrors [`ludion_core::diagnostics::Severity`]'s serde form: the lowercase strings `"error"`
 * and `"warning"`.
 */
export type SeveritySchema = 'error' | 'warning';
/**
 * The request envelope: a protocol version, a correlation id, and one flattened [`Command`].
 *
 * # Example
 * ```
 * use ludion_protocol::{Command, Request};
 * use ludion_protocol::command::SceneLoadParams;
 *
 * let request = Request::new(1, Command::SceneLoad(SceneLoadParams { path: "level.yaml".into() }));
 * let text = serde_json::to_string(&request).unwrap();
 * let back = Request::from_json(&text).unwrap();
 * assert_eq!(back.id.as_u64(), 1);
 * assert_eq!(back.command.method(), "scene.load");
 * ```
 */
export type Request = {
  /**
   * The client-chosen correlation id, echoed in the response.
   */
  id: number;
  /**
   * The protocol version; must equal [`PROTOCOL_VERSION`].
   */
  v: number;
  [k: string]: unknown;
} & Request1;
export type Request1 =
  | {
      method: 'scene.load';
      params: SceneLoadParams;
      [k: string]: unknown;
    }
  | {
      method: 'scene.save';
      params: SceneSaveParams;
      [k: string]: unknown;
    }
  | {
      method: 'scene.check';
      params: SceneCheckParams;
      [k: string]: unknown;
    }
  | {
      method: 'entity.spawn';
      params: EntitySpawnParams;
      [k: string]: unknown;
    }
  | {
      method: 'entity.destroy';
      params: EntityDestroyParams;
      [k: string]: unknown;
    }
  | {
      method: 'entity.reparent';
      params: EntityReparentParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.spawn';
      params: PrefabSpawnParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.create';
      params: PrefabCreateParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.apply';
      params: PrefabApplyParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.revert';
      params: PrefabRevertParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.unpack';
      params: PrefabUnpackParams;
      [k: string]: unknown;
    }
  | {
      method: 'prefab.overrides';
      params: PrefabOverridesParams;
      [k: string]: unknown;
    }
  | {
      method: 'component.set';
      params: ComponentSetParams;
      [k: string]: unknown;
    }
  | {
      method: 'component.get';
      params: ComponentGetParams;
      [k: string]: unknown;
    }
  | {
      method: 'component.remove';
      params: ComponentRemoveParams;
      [k: string]: unknown;
    }
  | {
      method: 'query.entities';
      params: QueryEntitiesParams;
      [k: string]: unknown;
    }
  | {
      method: 'sim.pause';
      [k: string]: unknown;
    }
  | {
      method: 'sim.resume';
      [k: string]: unknown;
    }
  | {
      method: 'sim.step';
      params: SimStepParams;
      [k: string]: unknown;
    }
  | {
      method: 'sim.set_speed';
      params: SimSetSpeedParams;
      [k: string]: unknown;
    }
  | {
      method: 'sim.hash';
      [k: string]: unknown;
    }
  | {
      method: 'input.inject';
      params: InputInjectParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.screenshot';
      params: RenderScreenshotParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.dump_graph';
      [k: string]: unknown;
    }
  | {
      method: 'render.dump_timings';
      [k: string]: unknown;
    }
  | {
      method: 'render.bounds';
      params: RenderBoundsParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.gi_rebake';
      params: RenderGiRebakeParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.size';
      params: RenderSizeParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.view';
      params: RenderViewParams;
      [k: string]: unknown;
    }
  | {
      method: 'render.debug';
      params: RenderDebugParams;
      [k: string]: unknown;
    }
  | {
      method: 'stream.subscribe';
      params: StreamSubscribeParams;
      [k: string]: unknown;
    }
  | {
      method: 'stream.unsubscribe';
      params: StreamUnsubscribeParams;
      [k: string]: unknown;
    }
  | {
      method: 'asset.reload';
      params: AssetReloadParams;
      [k: string]: unknown;
    }
  | {
      method: 'engine.describe';
      [k: string]: unknown;
    }
  | {
      method: 'engine.snapshot';
      [k: string]: unknown;
    }
  | {
      method: 'engine.restore';
      params: EngineRestoreParams;
      [k: string]: unknown;
    }
  | {
      method: 'engine.shutdown';
      [k: string]: unknown;
    }
  | {
      method: 'profile.capture';
      params: ProfileCaptureParams;
      [k: string]: unknown;
    };
/**
 * The response envelope: the request's `id`, plus the [`WireResult`].
 *
 * # Example
 * ```
 * use ludion_protocol::{Response, ResponseBody};
 * use ludion_core::time::FrameIndex;
 *
 * let ok = Response::ok_at(42, ResponseBody::Ack, FrameIndex::new(1203), FrameIndex::new(1150));
 * assert_eq!(
 *     serde_json::to_value(&ok).unwrap(),
 *     serde_json::json!({ "id": 42, "ok": { "kind": "ack" }, "frame": 1203, "game_frame": 1150 }),
 * );
 * ```
 */
export type Response = Response1 & {
  /**
   * The correlation id of the request this answers.
   */
  id: number;
  [k: string]: unknown;
};
export type Response1 =
  | {
      /**
       * The sim frame the command applied at (every mutating success carries it).
       */
      frame?: number | null;
      /**
       * The gameplay frame at the same boundary: `frame` less every frame the game has held
       * through its `GameClock`. It is the frame a presentation `started` is stamped with, and
       * it equals `frame` in a session that never holds.
       */
      game_frame?: number | null;
      /**
       * The command's success payload.
       */
      ok:
        | {
            kind: 'ack';
            [k: string]: unknown;
          }
        | {
            /**
             * How many entities the loaded scene instantiated.
             */
            entity_count: number;
            kind: 'scene_loaded';
            [k: string]: unknown;
          }
        | {
            kind: 'scene_saved';
            /**
             * The path the canonical YAML was written to.
             */
            path: string;
            [k: string]: unknown;
          }
        | {
            /**
             * The diagnostics produced (empty when `valid`).
             */
            diagnostics?: WireErrorSchema[];
            kind: 'scene_checked';
            /**
             * Whether the scene validated with no errors.
             */
            valid: boolean;
            [k: string]: unknown;
          }
        | {
            entity: EntityIdSchema9;
            kind: 'spawned';
            [k: string]: unknown;
          }
        | {
            /**
             * The destroyed entity's components, keyed by registry name.
             */
            components?: {
              [k: string]: unknown;
            };
            /**
             * The cascaded descendants, parents before children; empty when the entity had none.
             */
            descendants?: DestroyedEntity[];
            kind: 'entity_destroyed';
            [k: string]: unknown;
          }
        | {
            kind: 'reparented';
            /**
             * The parent the entity had, or `null` for a root.
             */
            previous_parent?: EntityIdSchema2 | null;
            /**
             * The entity's `Transform` before the call, or `null` when it had none.
             */
            previous_transform?: {
              [k: string]: unknown;
            };
            [k: string]: unknown;
          }
        | {
            /**
             * The new entity ids, one per prefab part, in part order.
             */
            entities: EntityIdSchema2[];
            kind: 'prefab_spawned';
            [k: string]: unknown;
          }
        | {
            kind: 'prefab_created';
            /**
             * The prefab's normalized path, relative to the content root.
             */
            path: string;
            [k: string]: unknown;
          }
        | {
            kind: 'prefab_applied';
            /**
             * The part paths whose component overrides were written into the prefab.
             */
            parts: string[];
            /**
             * The prefab file written, relative to the content root.
             */
            path: string;
            /**
             * The part paths deleted from the prefab.
             */
            removed: string[];
            [k: string]: unknown;
          }
        | {
            /**
             * The parts with at least one component written or removed, in live order.
             */
            entities: EntityIdSchema2[];
            kind: 'prefab_reverted';
            /**
             * The prefab's part paths with no live part: removed from the instance, and not re-spawned.
             */
            not_restored: string[];
            [k: string]: unknown;
          }
        | {
            /**
             * The parts' ids, in live order.
             */
            entities: EntityIdSchema2[];
            kind: 'prefab_unpacked';
            [k: string]: unknown;
          }
        | {
            /**
             * What `scene.save` would write for each instance.
             */
            instances: PrefabInstanceOverrides[];
            kind: 'prefab_overrides';
            [k: string]: unknown;
          }
        | {
            kind: 'component_set';
            /**
             * The value the component held immediately before this set, or `None` for a fresh insert.
             */
            previous?: {
              [k: string]: unknown;
            };
            [k: string]: unknown;
          }
        | {
            kind: 'component_value';
            /**
             * The component's JSON value.
             */
            value: {
              [k: string]: unknown;
            };
            [k: string]: unknown;
          }
        | {
            kind: 'component_removed';
            /**
             * The value the component held immediately before removal.
             */
            previous: {
              [k: string]: unknown;
            };
            [k: string]: unknown;
          }
        | {
            /**
             * The matched entities and their requested component values.
             */
            entities: EntityMatch[];
            kind: 'query_result';
            [k: string]: unknown;
          }
        | {
            /**
             * The `WorldHash` of the resulting state, present iff the request set `hash: true`.
             */
            hash?: WireHash | null;
            kind: 'stepped';
            [k: string]: unknown;
          }
        | {
            /**
             * A 64-bit XXH3 digest as exactly 16 lowercase hex digits.
             */
            hash: string;
            kind: 'sim_hash';
            [k: string]: unknown;
          }
        | {
            /**
             * The image bytes as standard (RFC 4648) base64 of the PNG. The command path favours a
             * self-contained JSON payload over the binary frame channel; agents decode this directly.
             */
            data: string;
            /**
             * The image encoding — always `"png"` in M1.
             */
            format: string;
            /**
             * Image height in pixels.
             */
            height: number;
            kind: 'screenshot';
            /**
             * Whether this image's global illumination was *held* rather than rebuilt for the world it
             * shows (`render.gi_rebake` at `hold`). An agent that screenshots a stale world learns so
             * from the same reply, instead of reasoning about light that is one edit out of date.
             */
            stale: boolean;
            /**
             * Image width in pixels.
             */
            width: number;
            [k: string]: unknown;
          }
        | {
            /**
             * The graph description (opaque to this crate; produced by `ludion-render-graph`).
             */
            graph: {
              [k: string]: unknown;
            };
            kind: 'graph_dump';
            [k: string]: unknown;
          }
        | {
            kind: 'timings_dump';
            /**
             * The timings document (opaque to this crate).
             */
            timings: {
              [k: string]: unknown;
            };
            [k: string]: unknown;
          }
        | {
            /**
             * One entry per renderable entity (sprite, billboard, mesh, or text), in extract order.
             */
            entities: EntityBounds[];
            kind: 'render_bounds';
            /**
             * The pixel resolution the screen rects are expressed in (the engine's render size).
             *
             * @minItems 2
             * @maxItems 2
             */
            render_size: [number, number];
            [k: string]: unknown;
          }
        | {
            kind: 'gi_rebake';
            /**
             * Whether a host's global-illumination caches follow the world or hold the baseline they already
             * have. The wire form is the bare lowercase name (`"auto"` / `"hold"`).
             *
             * Host/session state, never world state: it is not hashed into a `WorldHash`, not captured by
             * `engine.snapshot`, and not recorded into a replay, so a held session simulates and records exactly
             * as an unheld one does.
             *
             * # Example
             * ```
             * use ludion_protocol::command::GiRebakeMode;
             * assert_eq!(serde_json::to_value(GiRebakeMode::Hold).unwrap(), serde_json::json!("hold"));
             * ```
             */
            mode: 'auto' | 'hold';
            /**
             * Whether the last frame this host produced showed held global illumination. A requested
             * rebake has not run yet when this reply is written, so it still describes the frame
             * before it.
             */
            stale: boolean;
            [k: string]: unknown;
          }
        | {
            /**
             * The session's render height in pixels. See `width`.
             */
            height: number;
            kind: 'render_size';
            /**
             * The session's render width in pixels: what live stream frames are produced at, and what a
             * dimensionless `render.screenshot` defaults to.
             */
            width: number;
            [k: string]: unknown;
          }
        | {
            kind: 'render_view';
            /**
             * The pose the stream is flying at, or `None` for the scene camera.
             */
            pose?: ViewPose | null;
            [k: string]: unknown;
          }
        | {
            /**
             * The categories the last completed tick emitted lines for, sorted, whether or not they are
             * enabled — so enabling a category nobody draws shows up as a missing entry here.
             */
            drawn: string[];
            /**
             * The enabled categories, sorted.
             */
            enabled: string[];
            kind: 'render_debug';
            /**
             * The SSR debug view in force. It does not say whether the view was drawn: a view on a
             * frame without SSR draws the normal frame, and `render.dump_graph` shows whether
             * `ssr_debug` is scheduled.
             */
            view?: RenderDebugView | null;
            [k: string]: unknown;
          }
        | {
            kind: 'subscribed';
            /**
             * The new subscription id; events on the channel carry it as their `sub`.
             */
            sub: number;
            [k: string]: unknown;
          }
        | {
            kind: 'assets_reloaded';
            /**
             * The asset paths that were reloaded.
             */
            paths: string[];
            [k: string]: unknown;
          }
        | {
            /**
             * Component schemas from the registry plus the protocol schema (opaque to this crate).
             */
            description: {
              [k: string]: unknown;
            };
            kind: 'engine_description';
            [k: string]: unknown;
          }
        | {
            kind: 'snapshot';
            /**
             * The snapshot document; feed it back to `engine.restore` (opaque to this crate).
             */
            snapshot: {
              [k: string]: unknown;
            };
            [k: string]: unknown;
          }
        | {
            kind: 'restored';
            report: RestoreReport;
            [k: string]: unknown;
          }
        | {
            kind: 'profile_report';
            report: ProfileReport;
            [k: string]: unknown;
          };
      [k: string]: unknown;
    }
  | {
      error: WireErrorSchema1;
      [k: string]: unknown;
    };
/**
 * A 64-bit XXH3 digest as exactly 16 lowercase hex digits.
 */
export type WireHash = string;
/**
 * One change an `engine.restore` migration made to a single component of a single entity.
 *
 * The two `Component…` drop variants map to diagnostic codes ([`code`](RestoreEntry::code)); the
 * informational [`ComponentMigrated`](RestoreEntry::ComponentMigrated) field-diff has none. Run
 * `ludion explain L3015` / `L3016` for the full explanation of each drop.
 */
export type RestoreEntry =
  | {
      /**
       * The dropped component's registry name (as stored in the snapshot).
       */
      component: string;
      entity: EntityIdSchema14;
      kind: 'component_unregistered';
      [k: string]: unknown;
    }
  | {
      /**
       * The dropped component's registry name.
       */
      component: string;
      entity: EntityIdSchema15;
      /**
       * The serde failure text explaining why the value would not deserialize.
       */
      error: string;
      kind: 'component_undeserializable';
      [k: string]: unknown;
    }
  | {
      /**
       * The migrated component's registry name.
       */
      component: string;
      entity: EntityIdSchema16;
      /**
       * Field paths newly filled from serde defaults (added since the snapshot), sorted; a nested
       * field is dotted (`inner.b`).
       */
      filled?: string[];
      kind: 'component_migrated';
      /**
       * Field paths present in the snapshot but no longer on the type (dropped), sorted; a nested
       * field is dotted.
       */
      removed?: string[];
      [k: string]: unknown;
    }
  | {
      /**
       * The running engine's version.
       */
      engine_version: string;
      kind: 'engine_version_differs';
      /**
       * The engine version stored in the snapshot.
       */
      snapshot_version: string;
      [k: string]: unknown;
    }
  | {
      /**
       * The unknown top-level field's name.
       */
      field: string;
      kind: 'snapshot_field_unknown';
      [k: string]: unknown;
    };
/**
 * The success payload of a command response, tagged by `kind`.
 *
 * # Example
 * ```
 * use ludion_protocol::ResponseBody;
 * use ludion_core::ids::EntityId;
 *
 * let body = ResponseBody::Spawned { entity: EntityId::new(7, 0) };
 * assert_eq!(
 *     serde_json::to_value(&body).unwrap(),
 *     serde_json::json!({ "kind": "spawned", "entity": { "index": 7, "generation": 0 } }),
 * );
 *
 * // The empty acknowledgement is just its tag.
 * assert_eq!(serde_json::to_value(ResponseBody::Ack).unwrap(), serde_json::json!({ "kind": "ack" }));
 * ```
 */
export type ResponseBody =
  | {
      kind: 'ack';
      [k: string]: unknown;
    }
  | {
      /**
       * How many entities the loaded scene instantiated.
       */
      entity_count: number;
      kind: 'scene_loaded';
      [k: string]: unknown;
    }
  | {
      kind: 'scene_saved';
      /**
       * The path the canonical YAML was written to.
       */
      path: string;
      [k: string]: unknown;
    }
  | {
      /**
       * The diagnostics produced (empty when `valid`).
       */
      diagnostics?: WireErrorSchema[];
      kind: 'scene_checked';
      /**
       * Whether the scene validated with no errors.
       */
      valid: boolean;
      [k: string]: unknown;
    }
  | {
      entity: EntityIdSchema9;
      kind: 'spawned';
      [k: string]: unknown;
    }
  | {
      /**
       * The destroyed entity's components, keyed by registry name.
       */
      components?: {
        [k: string]: unknown;
      };
      /**
       * The cascaded descendants, parents before children; empty when the entity had none.
       */
      descendants?: DestroyedEntity[];
      kind: 'entity_destroyed';
      [k: string]: unknown;
    }
  | {
      kind: 'reparented';
      /**
       * The parent the entity had, or `null` for a root.
       */
      previous_parent?: EntityIdSchema2 | null;
      /**
       * The entity's `Transform` before the call, or `null` when it had none.
       */
      previous_transform?: {
        [k: string]: unknown;
      };
      [k: string]: unknown;
    }
  | {
      /**
       * The new entity ids, one per prefab part, in part order.
       */
      entities: EntityIdSchema2[];
      kind: 'prefab_spawned';
      [k: string]: unknown;
    }
  | {
      kind: 'prefab_created';
      /**
       * The prefab's normalized path, relative to the content root.
       */
      path: string;
      [k: string]: unknown;
    }
  | {
      kind: 'prefab_applied';
      /**
       * The part paths whose component overrides were written into the prefab.
       */
      parts: string[];
      /**
       * The prefab file written, relative to the content root.
       */
      path: string;
      /**
       * The part paths deleted from the prefab.
       */
      removed: string[];
      [k: string]: unknown;
    }
  | {
      /**
       * The parts with at least one component written or removed, in live order.
       */
      entities: EntityIdSchema2[];
      kind: 'prefab_reverted';
      /**
       * The prefab's part paths with no live part: removed from the instance, and not re-spawned.
       */
      not_restored: string[];
      [k: string]: unknown;
    }
  | {
      /**
       * The parts' ids, in live order.
       */
      entities: EntityIdSchema2[];
      kind: 'prefab_unpacked';
      [k: string]: unknown;
    }
  | {
      /**
       * What `scene.save` would write for each instance.
       */
      instances: PrefabInstanceOverrides[];
      kind: 'prefab_overrides';
      [k: string]: unknown;
    }
  | {
      kind: 'component_set';
      /**
       * The value the component held immediately before this set, or `None` for a fresh insert.
       */
      previous?: {
        [k: string]: unknown;
      };
      [k: string]: unknown;
    }
  | {
      kind: 'component_value';
      /**
       * The component's JSON value.
       */
      value: {
        [k: string]: unknown;
      };
      [k: string]: unknown;
    }
  | {
      kind: 'component_removed';
      /**
       * The value the component held immediately before removal.
       */
      previous: {
        [k: string]: unknown;
      };
      [k: string]: unknown;
    }
  | {
      /**
       * The matched entities and their requested component values.
       */
      entities: EntityMatch[];
      kind: 'query_result';
      [k: string]: unknown;
    }
  | {
      /**
       * The `WorldHash` of the resulting state, present iff the request set `hash: true`.
       */
      hash?: WireHash | null;
      kind: 'stepped';
      [k: string]: unknown;
    }
  | {
      /**
       * A 64-bit XXH3 digest as exactly 16 lowercase hex digits.
       */
      hash: string;
      kind: 'sim_hash';
      [k: string]: unknown;
    }
  | {
      /**
       * The image bytes as standard (RFC 4648) base64 of the PNG. The command path favours a
       * self-contained JSON payload over the binary frame channel; agents decode this directly.
       */
      data: string;
      /**
       * The image encoding — always `"png"` in M1.
       */
      format: string;
      /**
       * Image height in pixels.
       */
      height: number;
      kind: 'screenshot';
      /**
       * Whether this image's global illumination was *held* rather than rebuilt for the world it
       * shows (`render.gi_rebake` at `hold`). An agent that screenshots a stale world learns so
       * from the same reply, instead of reasoning about light that is one edit out of date.
       */
      stale: boolean;
      /**
       * Image width in pixels.
       */
      width: number;
      [k: string]: unknown;
    }
  | {
      /**
       * The graph description (opaque to this crate; produced by `ludion-render-graph`).
       */
      graph: {
        [k: string]: unknown;
      };
      kind: 'graph_dump';
      [k: string]: unknown;
    }
  | {
      kind: 'timings_dump';
      /**
       * The timings document (opaque to this crate).
       */
      timings: {
        [k: string]: unknown;
      };
      [k: string]: unknown;
    }
  | {
      /**
       * One entry per renderable entity (sprite, billboard, mesh, or text), in extract order.
       */
      entities: EntityBounds[];
      kind: 'render_bounds';
      /**
       * The pixel resolution the screen rects are expressed in (the engine's render size).
       *
       * @minItems 2
       * @maxItems 2
       */
      render_size: [number, number];
      [k: string]: unknown;
    }
  | {
      kind: 'gi_rebake';
      /**
       * Whether a host's global-illumination caches follow the world or hold the baseline they already
       * have. The wire form is the bare lowercase name (`"auto"` / `"hold"`).
       *
       * Host/session state, never world state: it is not hashed into a `WorldHash`, not captured by
       * `engine.snapshot`, and not recorded into a replay, so a held session simulates and records exactly
       * as an unheld one does.
       *
       * # Example
       * ```
       * use ludion_protocol::command::GiRebakeMode;
       * assert_eq!(serde_json::to_value(GiRebakeMode::Hold).unwrap(), serde_json::json!("hold"));
       * ```
       */
      mode: 'auto' | 'hold';
      /**
       * Whether the last frame this host produced showed held global illumination. A requested
       * rebake has not run yet when this reply is written, so it still describes the frame
       * before it.
       */
      stale: boolean;
      [k: string]: unknown;
    }
  | {
      /**
       * The session's render height in pixels. See `width`.
       */
      height: number;
      kind: 'render_size';
      /**
       * The session's render width in pixels: what live stream frames are produced at, and what a
       * dimensionless `render.screenshot` defaults to.
       */
      width: number;
      [k: string]: unknown;
    }
  | {
      kind: 'render_view';
      /**
       * The pose the stream is flying at, or `None` for the scene camera.
       */
      pose?: ViewPose | null;
      [k: string]: unknown;
    }
  | {
      /**
       * The categories the last completed tick emitted lines for, sorted, whether or not they are
       * enabled — so enabling a category nobody draws shows up as a missing entry here.
       */
      drawn: string[];
      /**
       * The enabled categories, sorted.
       */
      enabled: string[];
      kind: 'render_debug';
      /**
       * The SSR debug view in force. It does not say whether the view was drawn: a view on a
       * frame without SSR draws the normal frame, and `render.dump_graph` shows whether
       * `ssr_debug` is scheduled.
       */
      view?: RenderDebugView | null;
      [k: string]: unknown;
    }
  | {
      kind: 'subscribed';
      /**
       * The new subscription id; events on the channel carry it as their `sub`.
       */
      sub: number;
      [k: string]: unknown;
    }
  | {
      kind: 'assets_reloaded';
      /**
       * The asset paths that were reloaded.
       */
      paths: string[];
      [k: string]: unknown;
    }
  | {
      /**
       * Component schemas from the registry plus the protocol schema (opaque to this crate).
       */
      description: {
        [k: string]: unknown;
      };
      kind: 'engine_description';
      [k: string]: unknown;
    }
  | {
      kind: 'snapshot';
      /**
       * The snapshot document; feed it back to `engine.restore` (opaque to this crate).
       */
      snapshot: {
        [k: string]: unknown;
      };
      [k: string]: unknown;
    }
  | {
      kind: 'restored';
      report: RestoreReport;
      [k: string]: unknown;
    }
  | {
      kind: 'profile_report';
      report: ProfileReport;
      [k: string]: unknown;
    };

/**
 * Every wire type in the ludion protocol, generated from the schema snapshot. One property per protocol section (Command, Request, Response, ...); see each type's own members for the actual shape.
 */
export interface ProtocolSchema {
  Command: Command;
  EventMessage: EventMessage;
  FrameHeader: FrameHeader;
  Hello: Hello;
  Request: Request;
  Response: Response;
  ResponseBody: ResponseBody;
  ServerHello: ServerHello;
}
/**
 * Parameters for `scene.load`.
 */
export interface SceneLoadParams {
  /**
   * Path to the scene file, resolved by the engine relative to its project root.
   */
  path: string;
  [k: string]: unknown;
}
/**
 * Parameters for `scene.save`.
 */
export interface SceneSaveParams {
  /**
   * Destination path; the engine writes canonical YAML there.
   */
  path: string;
  [k: string]: unknown;
}
/**
 * Parameters for `scene.check`.
 */
export interface SceneCheckParams {
  /**
   * Path to the scene file to validate.
   */
  path: string;
  [k: string]: unknown;
}
/**
 * Parameters for `entity.spawn`.
 *
 * `components` maps registry component names to their JSON values — never a concrete Rust component
 * type (the boundary rule). The dispatcher validates each value against the named component's schema.
 *
 * # Example
 * ```
 * use ludion_protocol::command::EntitySpawnParams;
 * use std::collections::BTreeMap;
 *
 * let mut components = BTreeMap::new();
 * components.insert("Transform".to_string(), serde_json::json!({ "position": [0.0, 1.0, 0.0] }));
 * let params = EntitySpawnParams { name: Some("player".into()), components };
 * assert_eq!(params.components.len(), 1);
 * ```
 */
export interface EntitySpawnParams {
  /**
   * The entity's components, keyed by registry name; values are validated at dispatch.
   */
  components?: {
    [k: string]: unknown;
  };
  /**
   * Optional human-facing name for the entity (for logs / the inspector).
   */
  name?: string | null;
  [k: string]: unknown;
}
/**
 * Parameters for `entity.destroy`.
 */
export interface EntityDestroyParams {
  entity: EntityIdSchema;
  [k: string]: unknown;
}
/**
 * The entity to despawn.
 */
export interface EntityIdSchema {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Parameters for `entity.reparent`.
 *
 * # Example
 * ```
 * use ludion_core::ids::EntityId;
 * use ludion_protocol::command::{EntityReparentParams, ReparentKeep};
 *
 * let params = EntityReparentParams {
 *     entity: EntityId::new(4, 0),
 *     parent: Some(EntityId::new(2, 0)),
 *     keep: ReparentKeep::World,
 * };
 * assert_eq!(serde_json::to_value(&params).unwrap()["keep"], "world");
 * ```
 */
export interface EntityReparentParams {
  entity: EntityIdSchema1;
  /**
   * Whether the entity keeps its world pose (`world`) or its local `Transform` (`local`).
   */
  keep: 'world' | 'local';
  /**
   * The new parent, or `null` to detach the entity into a root.
   */
  parent?: EntityIdSchema2 | null;
  [k: string]: unknown;
}
/**
 * The entity to move.
 */
export interface EntityIdSchema1 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema2 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Parameters for `prefab.spawn`.
 *
 * # Example
 * ```
 * use ludion_protocol::command::{Placement, PrefabSpawnParams};
 *
 * let params = PrefabSpawnParams {
 *     path: "prefabs/crate.prefab.yaml".into(),
 *     transform: Some(Placement { translation: [5.0, 0.0, 0.0], rotation: [0.0; 3], scale: [1.0; 3] }),
 *     name: Some("a".into()),
 * };
 * assert_eq!(serde_json::to_value(&params).unwrap()["name"], "a");
 * ```
 */
export interface PrefabSpawnParams {
  /**
   * An instance name: the parts become `<name>/<part>`; omitted, they keep their bare part names.
   */
  name?: string | null;
  /**
   * The prefab file, relative to the content root.
   */
  path: string;
  /**
   * A pose baked onto every root part of the prefab; omitted, the parts keep their authored pose.
   */
  transform?: Placement | null;
  [k: string]: unknown;
}
/**
 * A pose baked onto every root part of a spawned prefab. Field defaults equal a `Transform`'s.
 *
 * # Example
 * ```
 * use ludion_protocol::command::Placement;
 * let placement: Placement = serde_json::from_str(r#"{"translation": [5, 0, 0]}"#).unwrap();
 * assert_eq!(placement.scale, [1.0, 1.0, 1.0]);
 * ```
 */
export interface Placement {
  /**
   * Euler XYZ rotation in degrees, as `Transform.rotation`.
   *
   * @minItems 3
   * @maxItems 3
   */
  rotation?: [number, number, number];
  /**
   * Per-axis scale; defaults to `[1, 1, 1]`.
   *
   * @minItems 3
   * @maxItems 3
   */
  scale?: [number, number, number];
  /**
   * World-space translation.
   *
   * @minItems 3
   * @maxItems 3
   */
  translation?: [number, number, number];
  [k: string]: unknown;
}
/**
 * Parameters for `prefab.create`.
 *
 * # Example
 * ```
 * use ludion_core::ids::EntityId;
 * use ludion_protocol::command::PrefabCreateParams;
 *
 * let params = PrefabCreateParams {
 *     entities: vec![EntityId::new(1, 0), EntityId::new(2, 0)],
 *     path: "prefabs/lamp.prefab.yaml".into(),
 * };
 * assert_eq!(serde_json::to_value(&params).unwrap()["path"], "prefabs/lamp.prefab.yaml");
 * ```
 */
export interface PrefabCreateParams {
  /**
   * The prefab's parts: the first is the root, the rest follow in order. Each needs a unique
   * `Name` without `/`, and a reference between them must stay inside the set.
   */
  entities: EntityIdSchema2[];
  /**
   * The file to write, relative to the content root; overwritten if it exists.
   */
  path: string;
  [k: string]: unknown;
}
/**
 * Parameters for `prefab.apply`.
 *
 * # Example
 * ```
 * use ludion_core::ids::EntityId;
 * use ludion_protocol::command::PrefabApplyParams;
 *
 * let params = PrefabApplyParams { instance: EntityId::new(1, 0), parts: None };
 * assert!(serde_json::to_value(&params).unwrap().get("parts").is_none());
 * ```
 */
export interface PrefabApplyParams {
  instance: EntityIdSchema3;
  /**
   * Narrows the apply to these part paths; omitted, every edited part is written.
   */
  parts?: string[] | null;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema3 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Parameters for `prefab.revert`.
 *
 * # Example
 * ```
 * use ludion_core::ids::EntityId;
 * use ludion_protocol::command::PrefabRevertParams;
 *
 * let params = PrefabRevertParams {
 *     instance: EntityId::new(1, 0),
 *     pointer: Some("/bulb/Transform".into()),
 * };
 * assert_eq!(serde_json::to_value(&params).unwrap()["pointer"], "/bulb/Transform");
 * ```
 */
export interface PrefabRevertParams {
  instance: EntityIdSchema4;
  /**
   * Narrows the revert to `/<part path>`, `/<part path>/<Component>` or
   * `/<part path>/<Component>/<field>...`; omitted, the whole instance. A prefab root's
   * `Transform` and `Parent` are never reverted, and removed parts are not re-spawned.
   */
  pointer?: string | null;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema4 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Parameters for `prefab.unpack`.
 *
 * # Example
 * ```
 * use ludion_core::ids::EntityId;
 * use ludion_protocol::command::PrefabUnpackParams;
 *
 * let params = PrefabUnpackParams { instance: EntityId::new(1, 0) };
 * assert_eq!(serde_json::to_value(&params).unwrap()["instance"]["index"], 1);
 * ```
 */
export interface PrefabUnpackParams {
  instance: EntityIdSchema5;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema5 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Parameters for `prefab.overrides`.
 *
 * # Example
 * ```
 * use ludion_protocol::command::PrefabOverridesParams;
 *
 * let params = PrefabOverridesParams::default();
 * assert!(serde_json::to_value(&params).unwrap().get("instances").is_none());
 * ```
 */
export interface PrefabOverridesParams {
  /**
   * Entities naming the instances to read, each by any of its parts; omitted, every live
   * instance.
   */
  instances?: EntityIdSchema2[] | null;
  [k: string]: unknown;
}
/**
 * Parameters for `component.set`.
 */
export interface ComponentSetParams {
  /**
   * The registry name of the component to insert or overwrite.
   */
  component: string;
  entity: EntityIdSchema6;
  /**
   * The component's JSON value, validated against its schema at dispatch.
   */
  value: {
    [k: string]: unknown;
  };
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema6 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Parameters for `component.get`.
 */
export interface ComponentGetParams {
  /**
   * The registry name of the component to read.
   */
  component: string;
  entity: EntityIdSchema7;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema7 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Parameters for `component.remove`.
 */
export interface ComponentRemoveParams {
  /**
   * The registry name of the component to remove.
   */
  component: string;
  entity: EntityIdSchema8;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema8 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Parameters for `query.entities`.
 *
 * `filter` selects entities that carry **all** of the named components; `include` names the
 * components whose values are returned for each match (empty `include` returns just the ids).
 * `entities` restricts the matches to those ids.
 */
export interface QueryEntitiesParams {
  /**
   * Restrict the matches to these ids (absent = every live entity). An id that is not live with
   * the same generation is skipped silently, never an error; duplicates collapse, and matches
   * come back in the canonical order (ascending index), not the request order. `[]` matches
   * nothing. `filter` still applies on top.
   */
  entities?: EntityIdSchema2[] | null;
  /**
   * Registry names an entity must all carry to match (empty = every live entity).
   */
  filter?: string[];
  /**
   * Registry names whose values to return per matched entity (empty = ids only).
   */
  include?: string[];
  /**
   * Also return each matched entity's composed world `Transform` (`world` on the match); an entity
   * with no `Transform` has none.
   */
  world?: boolean;
  [k: string]: unknown;
}
/**
 * Parameters for `sim.step`.
 */
export interface SimStepParams {
  /**
   * Whether to compute and return the `WorldHash` of the resulting state.
   */
  hash?: boolean;
  /**
   * Number of ticks to run before pausing.
   */
  ticks: number;
  [k: string]: unknown;
}
/**
 * Parameters for `sim.set_speed`.
 *
 * The factor is validated at dispatch (`L4001` if it is not finite and greater than zero); the
 * protocol layer carries it as a plain `f64` and does not depend on `ludion-sim`'s `Speed`.
 */
export interface SimSetSpeedParams {
  /**
   * Wall-clock pacing factor: `1.0` real time, `2.0` double speed, `0.5` half speed.
   */
  speed: number;
  [k: string]: unknown;
}
/**
 * Parameters for `input.inject`.
 *
 * `event` is a `ludion-sim` input-event value (`{"kind": "key_down", "key": "W"}`) carried as an
 * opaque [`serde_json::Value`] — the protocol layer does not name the sim's `InputEvent` type.
 */
export interface InputInjectParams {
  /**
   * The input event, validated at dispatch.
   */
  event: {
    [k: string]: unknown;
  };
  /**
   * The frame at whose tick boundary the event applies; omit to apply at the next tick.
   */
  frame?: number | null;
  [k: string]: unknown;
}
/**
 * Parameters for `render.screenshot`.
 *
 * Omitted dimensions use the engine's configured render resolution.
 */
export interface RenderScreenshotParams {
  /**
   * Which camera to render through; omit for the scene's own (see [`CameraChoice`]).
   */
  camera?: 'scene' | 'view';
  /**
   * Optional output height in pixels.
   */
  height?: number | null;
  /**
   * Optional output width in pixels.
   */
  width?: number | null;
  [k: string]: unknown;
}
/**
 * Parameters for `render.bounds`.
 *
 * One optional field, so the wire form a client sent before it existed — a `render.bounds` envelope
 * with no `params` at all — still parses and still means the scene camera.
 */
export interface RenderBoundsParams {
  /**
   * Which camera to project the screen rects through; omit for the scene's own.
   */
  camera?: 'scene' | 'view';
  [k: string]: unknown;
}
/**
 * Parameters for `render.gi_rebake`.
 *
 * Both fields are optional and independent, which is what lets one verb set, trigger, and query:
 * an omitted `mode` leaves the lever where it is, so `{}` is a pure read of the reply's
 * `mode` + `stale`.
 *
 * # Example
 * ```
 * use ludion_protocol::Command;
 * use ludion_protocol::command::{GiRebakeMode, RenderGiRebakeParams};
 *
 * // Hold the baseline *and* refresh it once, in one call.
 * let cmd = Command::RenderGiRebake(RenderGiRebakeParams {
 *     mode: Some(GiRebakeMode::Hold),
 *     rebake: true,
 * });
 * assert_eq!(
 *     serde_json::to_value(&cmd).unwrap(),
 *     serde_json::json!({
 *         "method": "render.gi_rebake",
 *         "params": { "mode": "hold", "rebake": true },
 *     }),
 * );
 *
 * // The query form changes nothing.
 * assert_eq!(RenderGiRebakeParams::default(), serde_json::from_str("{}").unwrap());
 * ```
 */
export interface RenderGiRebakeParams {
  /**
   * The lever's new setting; omit to leave it exactly as it is.
   */
  mode?: GiRebakeMode | null;
  /**
   * Whether to run one rebake: the next frame rebuilds both caches against the current world and
   * makes that the new held baseline. A no-op at [`GiRebakeMode::Auto`], where nothing is held.
   */
  rebake?: boolean;
  [k: string]: unknown;
}
/**
 * Parameters for `render.size`.
 *
 * Both fields are optional, which is what lets one verb set and query: omitting both (`{}`) is a
 * pure read of the reply's extent. They are not independent, though — an extent is a *pair*, so a
 * request naming exactly one axis is refused (`L3025`) rather than half-applied. Zero, and anything
 * past 7680 x 4320, is refused the same way.
 *
 * **The extent is session state, shared by every attached client: the last writer wins.** That is
 * deliberate, not an oversight — `render.bounds` reports this same number as the pixel space its
 * screen rects are expressed in, so one extent per host is what keeps every viewer's picking and
 * gizmo math agreeing with the pixels it was sent.
 *
 * Two consequences worth knowing before reaching for it. A resize *inside* an open
 * `profile.capture` window folds two extents into one median — harmless, but the report then
 * describes no single resolution. And a game's mouse input arrives as render-target pixel
 * positions, so a session played at a resized extent is a different input trace: committed
 * golden-session recordings (`games/* /run.lrp`) are made at the host's default extent.
 *
 * # Example
 * ```
 * use ludion_protocol::Command;
 * use ludion_protocol::command::RenderSizeParams;
 *
 * // Halve the pixel count a laptop has to move, in one call.
 * let cmd = Command::RenderSize(RenderSizeParams {
 *     width: Some(960),
 *     height: Some(540),
 * });
 * assert_eq!(
 *     serde_json::to_value(&cmd).unwrap(),
 *     serde_json::json!({
 *         "method": "render.size",
 *         "params": { "width": 960, "height": 540 },
 *     }),
 * );
 *
 * // The query form changes nothing.
 * assert_eq!(RenderSizeParams::default(), serde_json::from_str("{}").unwrap());
 * ```
 */
export interface RenderSizeParams {
  /**
   * The new extent's height in pixels; omit (with `width`) to leave the extent as it is.
   */
  height?: number | null;
  /**
   * The new extent's width in pixels; omit (with `height`) to leave the extent as it is.
   */
  width?: number | null;
  [k: string]: unknown;
}
/**
 * Parameters for `render.view`.
 *
 * One verb that sets, clears and queries, the [`RenderSizeParams`] shape — but the three forms need
 * three states, so `pose` is a *double* option and each JSON form means something different:
 *
 * - `{}` — absent: a pure read. The reply carries the override in force.
 * - `{"pose": null}` — the override is cleared and the stream returns to the scene's `Camera`.
 * - `{"pose": {…}}` — the stream flies to that pose.
 *
 * The override is host render state, never world state: it is not hashed into a `WorldHash`, not
 * captured by `engine.snapshot`, and not recorded into a replay, so a session with a flown view
 * simulates and records exactly as one without it. It moves the *stream*; `render.screenshot` and
 * `render.bounds` choose per call through [`CameraChoice`], defaulting to the scene camera.
 *
 * # Example
 * ```
 * use ludion_protocol::Command;
 * use ludion_protocol::command::{RenderViewParams, ViewPose};
 *
 * // Fly the stream to a pose.
 * let cmd = Command::RenderView(RenderViewParams {
 *     pose: Some(Some(ViewPose {
 *         translation: [0.0, 2.0, 6.0],
 *         rotation: [-15.0, 0.0, 0.0],
 *         fov_y_deg: Some(50.0),
 *     })),
 * });
 * assert_eq!(
 *     serde_json::to_value(&cmd).unwrap(),
 *     serde_json::json!({
 *         "method": "render.view",
 *         "params": { "pose": {
 *             "translation": [0.0, 2.0, 6.0],
 *             "rotation": [-15.0, 0.0, 0.0],
 *             "fov_y_deg": 50.0,
 *         }},
 *     }),
 * );
 *
 * // An explicit `null` clears it; an absent `pose` only reads.
 * let cleared: RenderViewParams = serde_json::from_str(r#"{"pose": null}"#).unwrap();
 * assert_eq!(cleared.pose, Some(None));
 * assert_eq!(RenderViewParams::default().pose, None);
 * ```
 */
export interface RenderViewParams {
  /**
   * The pose to fly to, an explicit `null` to clear the override, or absent to read it.
   */
  pose?: ViewPose | null;
  [k: string]: unknown;
}
/**
 * A free camera pose for `render.view`: where the view camera stands, where it looks, and how wide
 * it sees.
 *
 * Geometry only — the clip planes, and the field of view when this pose omits one, come from the
 * scene's own `Camera`, so a flown view keeps the game's depth range rather than inventing one.
 * The override is always a *perspective* camera: an orthographic scene camera contributes its clip
 * planes and nothing else.
 *
 * # Example
 * ```
 * use ludion_protocol::command::ViewPose;
 *
 * let pose = ViewPose { translation: [0.0, 2.0, 6.0], rotation: [-15.0, 0.0, 0.0], fov_y_deg: None };
 * assert_eq!(
 *     serde_json::to_value(&pose).unwrap(),
 *     serde_json::json!({ "translation": [0.0, 2.0, 6.0], "rotation": [-15.0, 0.0, 0.0] }),
 * );
 * ```
 */
export interface ViewPose {
  /**
   * Vertical field of view in degrees; omit to keep the scene camera's own.
   */
  fov_y_deg?: number | null;
  /**
   * Orientation as Euler-XYZ **degrees**, the same convention a `Transform`'s rotation carries.
   *
   * @minItems 3
   * @maxItems 3
   */
  rotation: [number, number, number];
  /**
   * World-space position of the view camera.
   *
   * @minItems 3
   * @maxItems 3
   */
  translation: [number, number, number];
  [k: string]: unknown;
}
/**
 * Parameters for `render.debug`.
 *
 * One verb that reads and sets. `categories` absent (`{}`) is a pure read; a list replaces the set
 * of enabled debug-draw categories: `[]` turns everything off, and a list containing `"all"` turns
 * everything on. Any string is accepted — a category nothing draws is simply never in the reply's
 * `drawn`, which is how a client checks what a scene actually emits.
 *
 * `view` shows a debug view in place of the composite: a name sets it, an explicit `null`
 * clears it, and absent keeps what is in force. A view never turns its effect on — on a frame
 * without SSR, probes or a granted local-light shadow the normal frame is drawn.
 *
 * The state is host render state, never world state: not hashed, not captured by `engine.snapshot`,
 * not recorded into a replay. It applies to the stream and to `render.screenshot` alike, so a
 * screenshot taken with `motor` enabled shows the motor's lines.
 *
 * # Example
 * ```
 * use ludion_protocol::Command;
 * use ludion_protocol::command::{RenderDebugParams, RenderDebugView};
 *
 * let cmd = Command::RenderDebug(RenderDebugParams {
 *     categories: Some(vec!["path".to_string()]),
 *     view: None,
 * });
 * assert_eq!(
 *     serde_json::to_value(&cmd).unwrap(),
 *     serde_json::json!({ "method": "render.debug", "params": { "categories": ["path"] } }),
 * );
 *
 * // An absent `categories` only reads.
 * assert_eq!(RenderDebugParams::default().categories, None);
 *
 * // A `view` sets an SSR debug view, an explicit `null` clears it, an absent one keeps it.
 * let set: RenderDebugParams = serde_json::from_str(r#"{"view": "ssr_confidence"}"#).unwrap();
 * assert_eq!(set.view, Some(Some(RenderDebugView::SsrConfidence)));
 * let cleared: RenderDebugParams = serde_json::from_str(r#"{"view": null}"#).unwrap();
 * assert_eq!(cleared.view, Some(None));
 * ```
 */
export interface RenderDebugParams {
  /**
   * The categories to enable (`[]` for none, `"all"` for every one), or absent to read.
   */
  categories?: string[] | null;
  /**
   * The debug view to show, an explicit `null` to clear it, or absent to keep it.
   */
  view?: RenderDebugView | null;
  [k: string]: unknown;
}
/**
 * Parameters for `stream.subscribe`.
 *
 * # Example
 * ```
 * use ludion_protocol::command::StreamSubscribeParams;
 * use ludion_protocol::frame::FrameFormat;
 * use ludion_protocol::{Channel, SubscribeMode};
 *
 * // A viewer subscribes to the frame stream, degrading fps under load instead of buffering latency.
 * let params = StreamSubscribeParams {
 *     channel: Channel::Frames,
 *     mode: SubscribeMode::LatestWins { max_fps: Some(30.0) },
 *     format: FrameFormat::Qoi,
 * };
 * assert_eq!(
 *     serde_json::to_value(&params).unwrap(),
 *     serde_json::json!({
 *         "channel": "frames",
 *         "mode": { "mode": "latest_wins", "max_fps": 30.0 },
 *         "format": "QOI",
 *     }),
 * );
 * ```
 */
export interface StreamSubscribeParams {
  /**
   * Which channel to subscribe to.
   */
  channel: 'frames' | 'log' | 'sim.events' | 'scene.changed' | 'asset.reloaded' | 'profile';
  /**
   * Payload codec for `frames` subscriptions. Ignored on every other channel.
   */
  format?: 'QOI' | 'LZ4_RGBA' | 'RAW_RGBA';
  /**
   * Delivery mode (defaults to lossy `latest_wins`).
   */
  mode?: {
    /**
     * Optional maximum delivery rate in frames per second (`None` = uncapped).
     */
    max_fps?: number | null;
    mode: 'latest_wins';
    [k: string]: unknown;
  };
  [k: string]: unknown;
}
/**
 * Parameters for `stream.unsubscribe`.
 */
export interface StreamUnsubscribeParams {
  /**
   * The subscription to close (the `sub` id returned by `stream.subscribe`).
   */
  sub: number;
  [k: string]: unknown;
}
/**
 * Parameters for `asset.reload`.
 */
export interface AssetReloadParams {
  /**
   * A single asset path to reload; omit to reload every changed asset.
   */
  path?: string | null;
  [k: string]: unknown;
}
/**
 * Parameters for `engine.restore`.
 *
 * `snapshot` is an opaque snapshot document produced by `engine.snapshot`; the protocol carries it
 * as a [`serde_json::Value`] and the engine interprets it.
 */
export interface EngineRestoreParams {
  /**
   * The snapshot document to restore from.
   */
  snapshot: {
    [k: string]: unknown;
  };
  [k: string]: unknown;
}
/**
 * Parameters for `profile.capture`.
 *
 * The capture measures the next `frames` **live** frames — the ones the host is already producing —
 * and answers with a [`ProfileReport`] once that window has closed. The rolling
 * [`ProfileWindowData`] stream, if anyone is subscribed, keeps flowing underneath it.
 */
export interface ProfileCaptureParams {
  /**
   * How many live frames to measure. Must be at least 1 and at most
   * [`MAX_CAPTURE_FRAMES`] (`L4012`).
   */
  frames?: number;
  /**
   * Whether the reply carries the window's raw per-frame [`ProfileSamples`] as well as its
   * medians. Off by default: medians answer "what does a frame cost", and the samples are several
   * times the payload — ask for them when a timeline or a trace export is the point.
   */
  include_samples?: boolean;
  [k: string]: unknown;
}
/**
 * The payload of a `log` event.
 */
export interface LogEvent {
  /**
   * The whole diagnostic, present for an `L####` line; `message` is then its rustc-style
   * rendering.
   */
  diagnostic?: WireErrorSchema | null;
  /**
   * The record's severity.
   */
  level: 'trace' | 'debug' | 'info' | 'warn' | 'error';
  /**
   * The log message.
   */
  message: string;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::diagnostics::Diagnostic`]'s serde form (the wire error object): a
 * `code`, a `message`, an optional `severity` / `path` / `help`, and any number of `related`
 * sub-diagnostics.
 *
 * `severity` is absent on an ordinary error (the core type skips it when it is `Error`), so a
 * client that ignores the field reads every error exactly as it did before severity existed;
 * only a lint carries it.
 */
export interface WireErrorSchema {
  code: string;
  help?: string | null;
  message: string;
  path?: string | null;
  related?: WireErrorSchema[];
  severity?: SeveritySchema | null;
  [k: string]: unknown;
}
/**
 * The payload of a `sim.events` event: opaque, game-defined event values for this frame.
 */
export interface SimEventsData {
  /**
   * The gameplay events, as JSON values the protocol layer does not interpret.
   */
  events?: unknown[];
  [k: string]: unknown;
}
/**
 * The payload of a `scene.changed` event.
 */
export interface SceneChangedData {
  /**
   * The content-root-relative scene path of a `scene.load` or `scene.save`; absent for an
   * `engine.restore` or a game-driven scene switch.
   */
  path?: string | null;
  [k: string]: unknown;
}
/**
 * The payload of an `asset.reloaded` event.
 */
export interface AssetReloadedData {
  /**
   * The reloaded asset's new content hash (16 hex digits, matching the asset content hash form).
   */
  hash: string;
  /**
   * The reloaded asset's path.
   */
  path: string;
  [k: string]: unknown;
}
/**
 * One closed window of live frames: the medians, and the raw samples they were taken over.
 *
 * The payload of a `profile` subscription event — one per window, so a stream of them reads as
 * roughly 1 Hz at the default tick rate.
 */
export interface ProfileWindowData {
  /**
   * Whether this window answers a capture request rather than the rolling stream.
   */
  capture: boolean;
  /**
   * The frame the window's first sample ticked as.
   */
  first_frame: number;
  /**
   * Median wall time of one full sim tick.
   */
  frame_total_median_ms: number;
  /**
   * Frame samples in the window.
   */
  frames: number;
  gpu: LiveGpuProfile;
  /**
   * The frame the window's last sample ticked as.
   */
  last_frame: number;
  samples: ProfileSamples;
  /**
   * The live tick's sequential-span medians (`extract`, `submit`), in that fixed order — over the
   * frames that were rendered, so an unrendered stretch lowers no median (`0.0` when none was).
   */
  sequential: SystemTiming[];
  /**
   * Per-system median wall time, in schedule order.
   */
  systems: SystemTiming[];
  /**
   * How many `ludion_core::par` workers the measured tick ran on — see
   * [`SimProfile::threads`]. Required here: a live window always knows its host's worker count.
   */
  threads?: number;
  /**
   * Frame-delivery timings, filled by the server before the event goes out; absent when the
   * window's frames reached no subscriber.
   */
  transport?: TransportProfile | null;
  [k: string]: unknown;
}
/**
 * The GPU half: per-pass medians and how much of the window they cover.
 */
export interface LiveGpuProfile {
  /**
   * Per-pass median GPU time over the sampled frames, in scheduled order.
   */
  passes: PassTiming[];
  /**
   * How many of the window's frames carried a GPU sample. Below `frames` whenever the timing ring
   * skipped a frame or the host rendered fewer frames than it ticked — normal, not an error.
   */
  sampled_frames: number;
  /**
   * Whether the adapter supports `TIMESTAMP_QUERY`. When `false`, every `median_ms` is `null` —
   * the same degradation the offline [`GpuProfile`] reports. Also `false` before the first armed
   * render, when no timing ring exists yet to ask.
   */
  timestamps_supported: boolean;
  [k: string]: unknown;
}
/**
 * One frame-graph pass's median GPU time across the measured frames.
 */
export interface PassTiming {
  /**
   * Median GPU time in milliseconds, or `null` when the adapter lacks timestamp support.
   */
  median_ms?: number | null;
  /**
   * The pass name (e.g. `scene`, `bloom_threshold`, `composite`).
   */
  name: string;
  [k: string]: unknown;
}
/**
 * The window's raw per-frame samples, columnar (names once, parallel arrays).
 */
export interface ProfileSamples {
  /**
   * Per-frame render-scene extraction times (`null` for an unrendered frame).
   */
  extract_ms: (number | null)[];
  /**
   * Per-frame tick totals.
   */
  frame_total_ms: number[];
  /**
   * The frame each column entry belongs to, ascending.
   */
  frames: number[];
  /**
   * One row per entry in [`pass_names`](ProfileSamples::pass_names), each a per-frame array
   * (`null` where that frame carried no sample, or the adapter cannot time passes).
   */
  pass_ms: (number | null)[][];
  /**
   * GPU pass names, in scheduled order.
   */
  pass_names: string[];
  /**
   * Per-frame frame-submit times (`null` for an unrendered frame).
   */
  submit_ms: (number | null)[];
  /**
   * One row per entry in [`system_names`](ProfileSamples::system_names), each a per-frame array.
   */
  system_ms: number[][];
  /**
   * System names, in schedule order.
   */
  system_names: string[];
  [k: string]: unknown;
}
/**
 * One system's median wall time across the measured frames.
 */
export interface SystemTiming {
  /**
   * Median wall time, in milliseconds.
   */
  median_ms: number;
  /**
   * The system's registration name.
   */
  name: string;
  /**
   * The worst-frame statistics beside the median — what a median hides. `None` for a live
   * capture and for a report predating the field.
   */
  tail?: TimingTail | null;
  [k: string]: unknown;
}
/**
 * The tail of one timing's per-frame distribution: the worst frame and the 99th percentile.
 */
export interface TimingTail {
  /**
   * The produced-frame index (`engine.frame()` after the tick) the maximum occurred at; the
   * first one if several tie.
   */
  max_frame: number;
  /**
   * The largest single-frame wall time, in milliseconds.
   */
  max_ms: number;
  /**
   * The nearest-rank 99th percentile, in milliseconds: the `ceil(0.99·n)`-th smallest sample.
   */
  p99_ms: number;
  [k: string]: unknown;
}
/**
 * Frame-delivery timings — what happens to a rendered frame between the sim thread and the socket.
 *
 * Filled by the server, which owns both spans: `encode` is the frame's image codec run, `send` is
 * the write into the connection's sink (queueing for the wire, not the kernel's delivery of it).
 * There is deliberately no readback segment — the engine's pipelined stream ring folds readback
 * into its own `submit` span in [`SimProfile::sequential`], where it belongs.
 */
export interface TransportProfile {
  /**
   * Median encode time in milliseconds over the window's delivered frames.
   */
  encode_median_ms: number;
  /**
   * Per-frame encode times, aligned to [`ProfileSamples::frames`] (`null` for a frame that was
   * not delivered). Empty when only medians were recorded.
   */
  encode_ms?: (number | null)[];
  /**
   * How many of the window's frames were delivered, and so timed. Below the window's frame count
   * whenever a subscriber's latest-wins slot dropped a frame, or nobody was subscribed.
   */
  sampled_frames: number;
  /**
   * Median sink-write time in milliseconds over the window's delivered frames.
   */
  send_median_ms: number;
  /**
   * Per-frame sink-write times, aligned the same way.
   */
  send_ms?: (number | null)[];
  [k: string]: unknown;
}
/**
 * The fixed header of a binary frame message.
 *
 * # Example
 * ```
 * use ludion_protocol::frame::{FrameFormat, FrameHeader};
 * use ludion_protocol::envelope::SubId;
 * use ludion_core::time::FrameIndex;
 *
 * let header = FrameHeader {
 *     sub: SubId::new(7),
 *     frame: FrameIndex::new(1203),
 *     format: FrameFormat::Qoi,
 *     width: 1920,
 *     height: 1080,
 *     gi_stale: false,
 * };
 * let payload = b"...encoded pixels...";
 * let message = header.encode(payload);
 *
 * let (decoded, body) = FrameHeader::decode(&message).unwrap();
 * assert_eq!(decoded, header);
 * assert_eq!(body, payload);
 * ```
 */
export interface FrameHeader {
  /**
   * How the payload pixels are encoded.
   */
  format: 'QOI' | 'LZ4_RGBA' | 'RAW_RGBA';
  /**
   * The sim frame index this image was rendered from.
   */
  frame: number;
  /**
   * Whether this frame's global illumination was *held* — served from a voxelization or probe
   * bake built under inputs the frame no longer had, because the session's rebake lever is at
   * `hold` (`render.gi_rebake`). Travels with the image so a viewer, or an agent decoding a
   * frame, learns it is looking at stale light without asking a second question.
   */
  gi_stale?: boolean;
  /**
   * Image height in pixels.
   */
  height: number;
  /**
   * The subscription this frame belongs to.
   */
  sub: number;
  /**
   * Image width in pixels.
   */
  width: number;
  [k: string]: unknown;
}
/**
 * The client's opening message.
 *
 * # Example
 * ```
 * use ludion_protocol::handshake::Hello;
 * use ludion_protocol::PROTOCOL_VERSION;
 *
 * let hello = Hello {
 *     protocol_version: PROTOCOL_VERSION,
 *     client: "ludion-cli 0.0.0".into(),
 *     features: vec![],
 * };
 * assert_eq!(serde_json::to_value(&hello).unwrap()["protocol_version"], serde_json::json!(1));
 * ```
 */
export interface Hello {
  /**
   * A human-facing client identifier (name + version), for logs.
   */
  client: string;
  /**
   * Optional feature flags the client advertises; unknown ones are ignored by the server.
   */
  features?: string[];
  /**
   * The protocol version the client was built against.
   */
  protocol_version: number;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema9 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * A descendant destroyed with its ancestor by `entity.destroy`: its old id and component state.
 *
 * # Example
 * ```
 * use ludion_protocol::response::DestroyedEntity;
 * use ludion_core::ids::EntityId;
 * use std::collections::BTreeMap;
 *
 * let d = DestroyedEntity { entity: EntityId::new(5, 0), components: BTreeMap::new() };
 * assert_eq!(d.entity.index(), 5);
 * ```
 */
export interface DestroyedEntity {
  /**
   * Its components, keyed by registry name.
   */
  components?: {
    [k: string]: unknown;
  };
  entity: EntityIdSchema10;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema10 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * One prefab instance as `prefab.overrides` reports it: what `scene.save` would write for it.
 *
 * # Example
 * ```
 * use ludion_protocol::response::PrefabInstanceOverrides;
 * use ludion_core::ids::EntityId;
 * use std::collections::BTreeMap;
 *
 * let o = PrefabInstanceOverrides {
 *     root: EntityId::new(3, 0),
 *     prefab: "lamp.prefab.yaml".into(),
 *     name: Some("lamp".into()),
 *     placement_differs: false,
 *     overrides: BTreeMap::from([("bulb".to_owned(), vec!["/Transform/translation".to_owned()])]),
 *     removed: Vec::new(),
 *     root_parts: vec!["lamp".to_owned()],
 *     diagnostic: None,
 * };
 * assert!(serde_json::to_value(&o).unwrap().get("diagnostic").is_none());
 * ```
 */
export interface PrefabInstanceOverrides {
  /**
   * The `L1234` or `L1230` a save would raise for this instance, in which case the fields above
   * are empty or false.
   */
  diagnostic?: WireErrorSchema | null;
  /**
   * The instance's name, `null` for an unnamed spawn (which a save leaves as plain entities).
   */
  name?: string | null;
  /**
   * Part path to the JSON pointers of the overridden leaves, each `/<Component>[/<field>...]`.
   * `/<part path>` followed by a pointer is exactly a `prefab.revert` pointer.
   */
  overrides: {
    [k: string]: string[];
  };
  /**
   * Whether the saved `transform` differs, bit for bit, from the placement the instance was
   * spawned or loaded at.
   */
  placement_differs: boolean;
  /**
   * The prefab file, relative to the content root.
   */
  prefab: string;
  /**
   * The part paths the instance dropped from its prefab.
   */
  removed: string[];
  root: EntityIdSchema11;
  /**
   * The prefab's root part paths, in part order: the parts with no `Parent` in the prefab file.
   * Their `Transform` and `Parent` are the instance's placement, which `prefab.revert` leaves
   * alone. Empty when `diagnostic` is set or the prefab cannot be read.
   */
  root_parts: string[];
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema11 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * One entity matched by `query.entities`, with the requested component values.
 *
 * # Example
 * ```
 * use ludion_protocol::response::EntityMatch;
 * use ludion_core::ids::EntityId;
 * use std::collections::BTreeMap;
 *
 * let mut components = BTreeMap::new();
 * components.insert("Transform".to_string(), serde_json::json!({ "position": [0.0, 0.0, 0.0] }));
 * let m = EntityMatch { entity: EntityId::new(3, 1), components, world: None };
 * assert_eq!(m.entity.index(), 3);
 * ```
 */
export interface EntityMatch {
  /**
   * The requested component values, keyed by registry name (empty if none were requested).
   */
  components?: {
    [k: string]: unknown;
  };
  entity: EntityIdSchema12;
  /**
   * The composed world transform; present only when the query set `world` and the entity has a
   * `Transform`.
   */
  world?: WorldTransform | null;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema12 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * An entity's composed world transform, as `query.entities` reports it with `world: true`.
 *
 * # Example
 * ```
 * use ludion_protocol::response::WorldTransform;
 *
 * let w = WorldTransform {
 *     translation: [1.0, 2.0, 3.0],
 *     rotation: [0.0, 90.0, 0.0],
 *     scale: [1.0, 1.0, 1.0],
 *     sheared: false,
 * };
 * assert_eq!(w.rotation[1], 90.0);
 * ```
 */
export interface WorldTransform {
  /**
   * World-space rotation as XYZ Euler angles in degrees.
   *
   * @minItems 3
   * @maxItems 3
   */
  rotation: [number, number, number];
  /**
   * World-space scale `[x, y, z]`.
   *
   * @minItems 3
   * @maxItems 3
   */
  scale: [number, number, number];
  /**
   * A rotated child sits under a non-uniform accumulated scale, so the drawn transform has a shear
   * this translation/rotation/scale cannot express.
   */
  sheared: boolean;
  /**
   * World-space translation `[x, y, z]`.
   *
   * @minItems 3
   * @maxItems 3
   */
  translation: [number, number, number];
  [k: string]: unknown;
}
/**
 * One renderable entity's bounds, as returned by `render.bounds`.
 *
 * The `world_aabb` is always present — an entity's world-space extent is well-defined regardless of
 * the camera. The `screen_rect` is `null` when the entity's whole AABB is behind the camera (there
 * is no meaningful projection); otherwise it is the pixel rect bounding the AABB's projected
 * corners, which may fall partly or wholly outside the viewport — `on_screen` is the honest test for
 * "would a pixel of this land on the rendered image". An agent must consult both: a `null` rect
 * means *behind the camera*, a non-null rect with `on_screen: false` means *off to the side / out of
 * frame*.
 *
 * # Example
 * ```
 * use ludion_protocol::response::{EntityBounds, WorldAabb, ScreenRect};
 * use ludion_core::ids::EntityId;
 *
 * let b = EntityBounds {
 *     entity: EntityId::new(2, 0),
 *     world_aabb: WorldAabb { min: [3.0, 1.5, 0.0], max: [7.0, 3.5, 0.0] },
 *     screen_rect: Some(ScreenRect { x: 130.0, y: 15.0, w: 40.0, h: 20.0 }),
 *     on_screen: true,
 * };
 * assert_eq!(b.world_aabb.max, [7.0, 3.5, 0.0]);
 * ```
 */
export interface EntityBounds {
  entity: EntityIdSchema13;
  /**
   * Whether `screen_rect` is present and intersects the `[0, render_size]` viewport.
   */
  on_screen: boolean;
  /**
   * The pixel rect bounding the AABB under the current camera, or `null` when the AABB is entirely
   * behind the camera.
   */
  screen_rect?: ScreenRect | null;
  world_aabb: WorldAabb;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema13 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * A screen-space rectangle in pixels, origin top-left, `x`/`y` at the top-left corner.
 */
export interface ScreenRect {
  /**
   * Height in pixels.
   */
  h: number;
  /**
   * Width in pixels.
   */
  w: number;
  /**
   * Left edge in pixels (may be negative when the entity extends off the left of the viewport).
   */
  x: number;
  /**
   * Top edge in pixels (may be negative when the entity extends off the top of the viewport).
   */
  y: number;
  [k: string]: unknown;
}
/**
 * The entity's world-space axis-aligned bounding box.
 */
export interface WorldAabb {
  /**
   * The maximum corner `[x, y, z]`.
   *
   * @minItems 3
   * @maxItems 3
   */
  max: [number, number, number];
  /**
   * The minimum corner `[x, y, z]`.
   *
   * @minItems 3
   * @maxItems 3
   */
  min: [number, number, number];
  [k: string]: unknown;
}
/**
 * What the migration had to change to load the snapshot (empty when nothing changed).
 */
export interface RestoreReport {
  /**
   * One entry per divergence the migration found (empty = clean restore).
   */
  entries?: RestoreEntry[];
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema14 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema15 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
 */
export interface EntityIdSchema16 {
  generation: number;
  index: number;
  [k: string]: unknown;
}
/**
 * The measured window, reshaped as a report.
 */
export interface ProfileReport {
  /**
   * Clustered-forward light binning: what the frame's cluster tables cost to build and how
   * crowded they came out. Filled by the **offline** driver and absent for a live capture — the
   * mirror image of `transport`, and for the mirror-image reason: the live host builds its
   * cluster tables inside a frame a profiler must neither stall nor make pay twice, while the
   * offline driver owns its own loop and can time the build as its own span.
   */
  cluster?: ClusterProfile | null;
  gpu: GpuProfile;
  /**
   * What loading the scene cost, phase by phase, and how many assets each kind decoded or reused.
   * Filled by the **offline** driver and absent for a live capture: a load is a one-off event the
   * host has already run by the time a capture asks, and a disarmed host reads no clock for it.
   */
  load?: LoadProfile | null;
  /**
   * Local-light shadow planning: how many shadowed lights the frame requested, was granted and
   * skipped. Filled by the **offline** driver and absent for a live capture, for `cluster`'s
   * reason: the live host plans inside a frame a profiler must not make pay twice.
   */
  local_shadows?: LocalShadowProfile | null;
  /**
   * Frames measured; every median below is over this many samples.
   */
  measured_frames: number;
  /**
   * Reflection probes of the last resolve: loaded from the store versus captured. Absent for a
   * scene without probes. Filled offline and by a live capture.
   */
  probes?: ProbeProfile | null;
  /**
   * The render quality this measurement was taken at — the setting the host chose and the
   * parameter axes every GPU number below depends on. Filled by the host that owns the setting
   * (the offline driver directly, the live host when it answers a capture), so `None` means only
   * a report predating the field.
   */
  quality?: QualityProfile | null;
  /**
   * The measured frames' raw per-frame columns, present when the request asked for them.
   */
  samples?: ProfileSamples1 | null;
  /**
   * The scene that was profiled — the path as given on the command line offline, the loaded
   * scene's name for a live capture.
   */
  scene: string;
  sim: SimProfile;
  /**
   * Sun cascade planning: how many cascades the sun authored and the frame fitted, and each
   * one's depth range, texel size and caster count. Filled by the **offline** driver and absent
   * for a live capture, for `cluster`'s reason: the live host fits inside a frame a profiler
   * must not make pay twice.
   */
  sun_cascades?: SunCascadeProfile | null;
  /**
   * Frame-delivery timings, filled by the server for a live capture and absent otherwise (the
   * offline driver delivers no frames, so it has no transport to measure).
   */
  transport?: TransportProfile | null;
  /**
   * Warmup frames driven and discarded before measuring (JIT/cache settling). Always `0` for a
   * live capture: the frames it measures are the ones the host was already producing.
   */
  warmup_frames: number;
  [k: string]: unknown;
}
/**
 * Clustered-forward light binning — the CPU build's cost, and the occupancy of the tables it built.
 *
 * The occupancy fields are not measurements: they are the *shape* of the tables, reduced out of the
 * binning's own counting sort, so two runs of the same frame produce identical numbers and any
 * change in them is a change in the binning rather than noise. Only `build_median_ms` is a
 * wall-clock median like the rest of the report.
 *
 * They travel together because that pairing is the whole point: a `scene`-pass regression with the
 * occupancy unmoved means per-fragment shading got more expensive, while the same regression *with*
 * the mean or max list length climbing means the binning got looser. One `ludion profile` run tells
 * them apart, which a GPU capture previously had to.
 */
export interface ClusterProfile {
  /**
   * Median wall time of one cluster build, in milliseconds, over the measured frames.
   */
  build_median_ms: number;
  /**
   * Cells in the cluster grid.
   */
  cells: number;
  /**
   * Lights in the frame's global light array.
   */
  lights: number;
  /**
   * The longest single cell's list — the worst-case loop a fragment can pay.
   */
  max_list_len: number;
  /**
   * The mean list length **over occupied cells** — the loop length a lit fragment pays where the
   * loop runs at all. The all-cells mean is `total_refs / cells`, one division away, which is why
   * this one is the occupied mean rather than a second spelling of a derivable number.
   */
  mean_list_len: number;
  /**
   * Cells holding at least one light. The complement is free frame — cells a fragment can read
   * and pay nothing for.
   */
  occupied_cells: number;
  /**
   * Total index references across every cell. This is the number that grows when conservatism
   * gets looser, whether or not any single list gets longer.
   */
  total_refs: number;
  [k: string]: unknown;
}
/**
 * GPU-side timings: per-pass medians from the render pipeline.
 */
export interface GpuProfile {
  /**
   * Whether a GPU adapter was available at all. `false` (with empty `passes`) on a headless host
   * with no adapter — the sim section is still fully measured. Also `false` for a replay
   * profile, which measures the sim only.
   */
  available: boolean;
  /**
   * Per-pass median GPU time, in scheduled order.
   */
  passes: PassTiming[];
  /**
   * Whether the adapter supports `TIMESTAMP_QUERY`. When `false`, each pass's `median_ms` is
   * `null` (the shape is stable; the numbers are simply unavailable on that adapter).
   */
  timestamps_supported: boolean;
  [k: string]: unknown;
}
/**
 * What loading a scene cost — wall time per phase, and deterministic per-kind asset counters.
 *
 * The phases are **exclusive** self-times: a mesh's map decode counts in `texture_ms`, not
 * `mesh_ms`, so the phases never overlap. `total_ms` runs from the first phase's start to the end
 * of the measured load, and `total_ms` minus the sum of the phases is the unattributed share.
 * The counters are not measurements: two loads of the same scene produce identical numbers, so a
 * change in them is a change in what gets decoded rather than noise.
 */
export interface LoadProfile {
  /**
   * One entry per asset kind, always all of them, in a fixed order.
   */
  assets: AssetLoadCounters[];
  /**
   * Time to the first rendered frame; absent until the renderer reports it.
   */
  first_frame_ms?: number | null;
  /**
   * Time spent baking GI probes; absent until the renderer reports it.
   */
  gi_bake_ms?: number | null;
  /**
   * Time the renderer spent uploading the scene's assets to the GPU; absent until the renderer
   * reports it.
   */
  gpu_upload_ms?: number | null;
  /**
   * Time spent decoding mesh documents (their maps count in `texture_ms`).
   */
  mesh_ms: number;
  /**
   * Time spent decoding every other asset kind.
   */
  other_ms: number;
  /**
   * Time spent capturing and prefiltering reflection probes the store did not hold; absent when
   * none was captured.
   */
  probe_bake_ms?: number | null;
  /**
   * Time spent creating the renderer's device and pipelines; absent until the renderer reports it.
   */
  renderer_init_ms?: number | null;
  /**
   * Time spent instantiating the scene document into a world, outside any asset decode.
   */
  scene_ms: number;
  /**
   * Time spent decoding textures: loose sprite textures and mesh maps alike.
   */
  texture_ms: number;
  /**
   * First phase start to end of the measured load.
   */
  total_ms: number;
  /**
   * Time spent voxelizing the scene for GI; absent until the renderer reports it.
   */
  voxelize_ms?: number | null;
  [k: string]: unknown;
}
/**
 * How many assets of one kind a load decoded or reused.
 */
export interface AssetLoadCounters {
  /**
   * On-disk size of the files those decodes read.
   */
  bytes_read: number;
  /**
   * Cooked-store reads (KTX2), counted apart from `decoded`; only `texture` is ever non-zero.
   */
  cooked?: number;
  /**
   * Decode events: one per decoded file, so a map shared by two meshes decodes once.
   */
  decoded: number;
  /**
   * The asset kind's name (`texture`, `mesh`, `voxel`, `voxel_grid`, `sculpt`, `assembly`,
   * `character`, `atlas`).
   */
  kind: string;
  /**
   * Distinct keys whose first lookup in the window was a cache hit.
   */
  reused: number;
  [k: string]: unknown;
}
/**
 * Local-light shadow planning — which shadowed lights the frame wanted and which it got.
 *
 * Not measurements: the counts are the shape of the frame's plan, reduced from the same planner the
 * renderer runs, so two runs of the same frame produce identical numbers. `skipped > 0` means
 * opted-in lights the camera sees went unshadowed (the game's `skipped == 0` check). A quality that
 * masks shadows grants nothing: `granted` is 0, `skipped` equals `requested` and `atlas_edge` is 0,
 * which is honest for a frame that drew no local shadow — the report's `quality.shadows` names why.
 */
export interface LocalShadowProfile {
  /**
   * The atlas edge the plan used, in texels; 0 when the quality masks shadows.
   */
  atlas_edge: number;
  /**
   * Lights granted a set of atlas tiles.
   */
  granted: number;
  /**
   * Casting lights whose footprint touches the camera frustum.
   */
  requested: number;
  /**
   * `requested − granted`: candidates that did not fit the caster budget.
   */
  skipped: number;
  /**
   * Shadow views drawn: a spot takes one, a point light (or a spot wider than 60°) six.
   */
  views: number;
  [k: string]: unknown;
}
/**
 * The last reflection-probe resolve: probes loaded from the store versus captured.
 *
 * `hits` stays 0 on the Fresh paths (`ludion profile`, `--profile`), which never read the store, by
 * design: committed images and measurements never depend on a cache file existing.
 */
export interface ProbeProfile {
  /**
   * Probes captured and prefiltered.
   */
  captures: number;
  /**
   * Probes loaded from the probe store.
   */
  hits: number;
  [k: string]: unknown;
}
/**
 * The render quality a measurement was taken at: what the machine chose, which optional effects
 * that resolved to, and how hard each one worked.
 *
 * It travels with a report for the reason [`SimProfile::threads`] does — two measurements taken
 * under different machine settings are not comparable, and only the report can say so. Quality is a
 * per-machine host switch (`--quality`, `LUDION_QUALITY`), never scene content and never sim state,
 * so nothing here affects a `WorldHash`: a report at `low` and one at `full` describe the same
 * simulation costing different pixels.
 *
 * The `preset` label and the six toggles are the *choice*; the parameter axes below them are the
 * *build* — a shader's sample count or a texture descriptor's size, reported as the renderer
 * currently runs them. A false toggle leaves that effect's passes out of the frame graph entirely,
 * so its axis describes work that was not done: `shadows: false` beside a `shadow_map_size` of 1024
 * means no map of any size was drawn, and reading the axis without the toggle is the misreading
 * every toggle here exists to prevent.
 */
export interface QualityProfile {
  /**
   * Whether depth of field ran on a scene that authored it.
   */
  dof: boolean;
  /**
   * Taps in the depth-of-field spiral.
   */
  dof_taps: number;
  /**
   * Whether per-frame GI dynamic-light injection ran. Masked off, a marked light contributes its
   * direct lighting only and no injection pass is scheduled.
   */
  gi_dynamic: boolean;
  /**
   * Multiplier on the scene's authored GI probe spacing; `1.0` bakes at exactly what the scene
   * asked for.
   */
  gi_probe_scale: number;
  /**
   * Rays cast per probe by the GI bake.
   */
  gi_rays: number;
  /**
   * Edge length of a face of the sky's specular cube, in texels.
   */
  ibl_specular_size: number;
  /**
   * Edge length of the square local-light shadow atlas, in texels; the caster budget is a fixed
   * view count, so only resolution follows this.
   */
  local_shadow_atlas_size: number;
  /**
   * Edge length of the shadow map's PCF kernel, in taps (`3` is a 3×3 average).
   */
  pcf: number;
  /**
   * The name of the setting this machine chose: a preset name, or `custom` for a local override
   * file. Not derivable from the toggles — several presets currently resolve to the same ones.
   */
  preset: string;
  /**
   * Fraction of the output extent the frame was drawn at before being resampled up to it; `1.0`
   * is native.
   *
   * **Every GPU number in this report is a per-pixel cost**, and this is the only field that says
   * how many pixels they were paid over. A capture at `low` draws 56% of them, so a report naming
   * its sample counts and its shadow map but not its resolution would show a smaller frame as a
   * faster one — the same class of lie [`shadows`](QualityProfile::shadows) is here to prevent,
   * applied to every pass at once rather than to one.
   */
  render_scale: number;
  /**
   * Edge length of the square sun shadow map, in texels.
   */
  shadow_map_size: number;
  /**
   * Whether the sun's shadow map was drawn on a scene carrying a sun. Masked off, the `shadow`
   * pass is not scheduled, the light shafts that march the map go with it, and the sun still
   * lights the scene.
   *
   * Not derivable from anything else in this block, which is why it is here: `shafts: false` says
   * the same thing for a scene that never authored shafts, and an override file reports `custom`
   * for `preset` — so without this field a capture could report `shadow_map_size` for a frame
   * that drew no shadow map.
   */
  shadows: boolean;
  /**
   * Ray-march steps per light-shaft pixel.
   */
  shaft_steps: number;
  /**
   * Whether volumetric light shafts ran on a scene that authored them.
   */
  shafts: boolean;
  /**
   * Whether ambient occlusion ran on a scene that authored it.
   */
  ssao: boolean;
  /**
   * Hemisphere samples per ambient-occlusion pixel.
   */
  ssao_samples: number;
  /**
   * Whether this machine's quality allows screen-space reflections (a scene still opts in via
   * `PostProcess.ssr`).
   */
  ssr: boolean;
  /**
   * The SSR trace's extent as a fraction of the **output** extent, independent of `render_scale`.
   */
  ssr_scale: number;
  /**
   * The most cascades a sun fits on this machine (`min(authored, this)`); a sun without
   * `cascades` draws the anchored box.
   */
  sun_cascades: number;
  [k: string]: unknown;
}
/**
 * A window's raw per-frame samples, stored columnar: every name appears once and each measurement
 * is a parallel array indexed by the window's frame position.
 *
 * This is what a timeline is drawn from — a median cannot say *when* a spike happened.
 */
export interface ProfileSamples1 {
  /**
   * Per-frame render-scene extraction times (`null` for an unrendered frame).
   */
  extract_ms: (number | null)[];
  /**
   * Per-frame tick totals.
   */
  frame_total_ms: number[];
  /**
   * The frame each column entry belongs to, ascending.
   */
  frames: number[];
  /**
   * One row per entry in [`pass_names`](ProfileSamples::pass_names), each a per-frame array
   * (`null` where that frame carried no sample, or the adapter cannot time passes).
   */
  pass_ms: (number | null)[][];
  /**
   * GPU pass names, in scheduled order.
   */
  pass_names: string[];
  /**
   * Per-frame frame-submit times (`null` for an unrendered frame).
   */
  submit_ms: (number | null)[];
  /**
   * One row per entry in [`system_names`](ProfileSamples::system_names), each a per-frame array.
   */
  system_ms: number[][];
  /**
   * System names, in schedule order.
   */
  system_names: string[];
  [k: string]: unknown;
}
/**
 * Sim-side timings: per-system medians and the whole-frame median.
 */
export interface SimProfile {
  /**
   * Median wall time of one full sim frame (the observed schedule run plus per-frame overhead).
   */
  frame_total_median_ms: number;
  /**
   * Worst-frame statistics of the frame total. Filled by the offline driver and a replay
   * profile; `None` for a live capture and for a report predating the field.
   */
  frame_total_tail?: TimingTail | null;
  /**
   * The tick's **sequential** costs — the per-tick work that is per-*world*, not per-entity, and
   * so cannot be split across threads. One [`SystemTiming`] each, same median statistic as
   * `systems`, always present in a fixed order. Measured beside the schedule, never inside it: none
   * of them is part of `frame_total_median_ms`, so a parallelizable share is
   * `Σ systems / (frame_total + Σ sequential)` — the Amdahl split. The offline driver reports
   * three (`world_hash`, `extract`, `command_drain`); a live capture reports the two a streaming
   * tick actually pays (`extract`, `submit`).
   */
  sequential: SystemTiming[];
  /**
   * Per-system median wall time, in registration order. Empty when the profiled scene runs no
   * game systems (a scene loaded through the built-in registry has none — the CLI cannot link a
   * game's own systems, so its `systems` list is the built-in schedule's, which is empty).
   */
  systems: SystemTiming[];
  /**
   * How many `ludion_core::par` workers the measured sim ran on.
   *
   * Results never depend on this — the sim is bit-identical at any worker count — but every
   * duration above does, so a report is not comparable to another one taken at a different count.
   * It varies per machine by design (`LUDION_THREADS`, a host's `--threads`, else the machine's
   * capped available parallelism), which is exactly why it travels with the measurement instead of
   * being something a reader has to remember.
   *
   * `None` only in a report predating the field — historical committed JSON, whose worker count
   * lives in its filename if anywhere. Any live or offline capture fills it.
   */
  threads?: number | null;
  [k: string]: unknown;
}
/**
 * Sun cascade planning — what the sun asked for and what the frame fitted.
 *
 * Not measurements: the numbers are the shape of the frame's plan, reduced from the same fit and
 * per-cascade cull the renderer runs, so two runs of the same frame produce identical numbers.
 * `count < authored` means the quality capped the cascades; `count` 0 with `authored > 0` means
 * shadows were masked or the camera was degenerate, so neither the cascades nor the anchored box
 * drew — the report's `quality.shadows` names the first cause.
 */
export interface SunCascadeProfile {
  /**
   * The cascade count the sun authors; 0 for a sun without `cascades` or a sunless scene.
   */
  authored: number;
  /**
   * One entry per fitted cascade, nearest first.
   */
  cascades: SunCascadeSliceProfile[];
  /**
   * Cascades fitted this frame; 0 when none ran.
   */
  count: number;
  /**
   * Edge of one cascade's shadow tile in texels; 0 when none ran.
   */
  tile_edge: number;
  [k: string]: unknown;
}
/**
 * One fitted sun cascade.
 */
export interface SunCascadeSliceProfile {
  /**
   * Instances the cascade's box keeps for the shadow pass.
   */
  casters: number;
  /**
   * View depth, in metres, where the slice ends.
   */
  far: number;
  /**
   * View depth, in metres, where the slice starts.
   */
  near: number;
  /**
   * World metres per shadow texel.
   */
  texel: number;
  [k: string]: unknown;
}
/**
 * Mirrors [`ludion_core::diagnostics::Diagnostic`]'s serde form (the wire error object): a
 * `code`, a `message`, an optional `severity` / `path` / `help`, and any number of `related`
 * sub-diagnostics.
 *
 * `severity` is absent on an ordinary error (the core type skips it when it is `Error`), so a
 * client that ignores the field reads every error exactly as it did before severity existed;
 * only a lint carries it.
 */
export interface WireErrorSchema1 {
  code: string;
  help?: string | null;
  message: string;
  path?: string | null;
  related?: WireErrorSchema[];
  severity?: SeveritySchema | null;
  [k: string]: unknown;
}
/**
 * The server's reply to a [`Hello`].
 *
 * # Example
 * ```
 * use ludion_protocol::handshake::ServerHello;
 * use ludion_protocol::{schema, PROTOCOL_VERSION};
 *
 * let reply = ServerHello {
 *     engine_version: "0.0.0".into(),
 *     protocol_version: PROTOCOL_VERSION,
 *     schema_hash: schema::schema_hash(),
 * };
 * assert_eq!(reply.schema_hash.to_string().len(), 16);
 * ```
 */
export interface ServerHello {
  /**
   * The engine build's version string.
   */
  engine_version: string;
  /**
   * The protocol version the server speaks (must match the client's for the session to proceed).
   */
  protocol_version: number;
  /**
   * A 64-bit XXH3 digest as exactly 16 lowercase hex digits.
   */
  schema_hash: string;
  [k: string]: unknown;
}
