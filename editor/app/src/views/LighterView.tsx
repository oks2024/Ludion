// The Lighter view: the tab where a human directs a scene's light — the baked global-illumination
// volume, the sun, and the placed lights — live, against the content they are judging.
//
// LAYOUT — the Art Director's, whose structure this clones: the lighting panel on the LEFT, the
// viewport in the CENTRE, the terminal column on the RIGHT. It shares App's session and its
// `ProtocolClient`: a `GiVolume` is global to the scene and is judged against real content, so there
// is no rig to isolate. There is no reference capture — that is the Art Director's imaging
// instrument, and every verdict here is reached by holding an A/B against the moving frame instead.
//
// THE CADENCES ARE DELIBERATE, AND THEY ARE NOT ALL THE SAME.
//   - The target query is UNFILTERED (the lowest-`EntityId` fallback needs every entity), which is
//     thousands on a district scene. It runs on mount, on a scene change, and on a materialization —
//     never on a timer. The sun and the lights list are FILTERED queries, cheap, and ride their own
//     equally low cadence: a scene change, or a `GiDynamic` toggle, which is the one gesture in this
//     tab that changes what they answer.
//   - `component.get` IS polled, at the inspector's ~3 Hz, up to five times a tick: the volume on
//     the target, `DirectionalLight` on the sun, `PointLight`, `SpotLight` and `GiDynamic` on App's
//     selection. Each answers `L3006` when the entity carries no such component, and for all five
//     that is the NORMAL case (most scenes author no `GiVolume`; most selections are not lights; most
//     lights are unmarked), so it reads as absent and never as a banner. `GiDynamic` is polled rather than read
//     off the lights list because the toggle that shows it also WRITES it: an undo of a mark has to
//     reach the control, and the list only re-queries when the scene does.
//
// THE REBAKE LEVER IS THE ONE CONTROL HERE THAT IS NOT WORLD STATE. `render.gi_rebake` sets a host's
// session lever (`auto`/`hold`) and can trigger one rebake; nothing about it is hashed, snapshotted,
// recorded or saved, so it goes through neither `onMutation` nor `onDirty` and no undo reaches it.
// Its current setting is read once per connection through the method's own query form (`{}`) rather
// than assumed — another client may have moved it. The STALE badge is NOT that reply: it is bit 0 of
// the frame header the viewport is already decoding, read through the frame-status store, so it
// describes the image on screen rather than what the lever was last set to.
//
// HOLD-TO-COMPARE IS NEITHER A MUTATION NOR AN UNDO ENTRY. Press writes the neutral, release writes
// back the object captured at the press — net-zero by construction — so neither end goes through
// `onMutation` or `onDirty`. The Level Designer shares this session, and a `scene.save` from that tab
// would otherwise persist whatever state a comparison was left in. A held compare therefore must
// never be able to stay held: it releases on pointer-up anywhere, on pointer-cancel, on window blur,
// on a target change, and on unmount.
//
// WHICH IS WHY A HELD COMPARE OWNS THE COMPONENT OUTRIGHT (`commitField`). The card's other live
// gesture is a slider drag, whose previews write `GiVolume` on their own cadence; the release
// restores the capture, so anything that landed under the hold would be silently reverted while
// leaving an undo entry for a value the engine no longer holds. The two exclude each other by
// refusing the later writer: while a comparison is held, field writes are dropped.
//
// THE TWO HOLDS COST DIFFERENT AMOUNTS, and that is stated rather than hidden. `sky_visibility: 0` is
// documented byte-identical to no volume and rides a sample-time uniform, so the GI A/B is free at
// both ends; the bounce hold crosses the bake's `0↔on` key, so it costs one bake per end. Neither is
// a `component.remove`: removing the volume would re-voxelize the scene on every flip.
//
// STYLING SEAM. Every class this view and `LighterPanel` introduce is prefixed `lt-`; the shared
// shell primitives (`main-row`, `col`, `col-side`, `col-center`, `col-term`, `panel-head`,
// `icon-btn`, `field*`) are reused as-is.

import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { ContextCamera } from '../terminal/context';
import type { Command, GiRebakeMode, ResponseBody } from '@ludion/protocol';
import type { EditorMode, EntityId } from '../App';
import { ProtocolError, type ProtocolClient } from '../protocol/client';
import { componentSet, entityRef } from '../protocol/commands';
import type { UndoEntry } from '../editor/undo';
import { Viewport } from '../viewer/Viewport';
import type { FrameStatus } from '../viewer/useFrameStream';
import { toDiagnostic, type DiagnosticView } from '../viewer/diagnostic';
import { usePolling } from '../viewer/usePolling';
import { getFrameStatus, subscribeFrameStatus } from '../viewer/frameStatus';
import { useComponentCommit, type CommitCadence, type MutationReport } from '../viewer/useComponentCommit';
import type { FieldPath } from '../viewer/inspect-fields';
import { TerminalDrawer } from '../terminal/TerminalDrawer';
import type { ContextLighter } from '../terminal/context';
import {
  parseTargetCandidates,
  pickTarget,
  targetInclude,
  targetValue,
  type MaterializationTarget,
} from '../editor/art/target';
import { beginCompare, endCompare, type CompareSession } from '../editor/compare';
import { LighterPanel, type GiHold } from '../editor/light/LighterPanel';
import {
  BOUNCE_AB,
  effectiveGi,
  GI_AB,
  GI_FIELD_NAMES,
  GI_VOLUME,
  type GiFieldName,
} from '../editor/light/giFields';
import {
  DIRECTIONAL_LIGHT,
  GI_DYNAMIC,
  lightInclude,
  parseLightEntries,
  pickSun,
  POINT_LIGHT,
  SPOT_LIGHT,
  type LightEntry,
} from '../editor/light/lights';

export interface LighterViewProps {
  readonly client: ProtocolClient;
  readonly mode: EditorMode;
  readonly selection: EntityId | null;
  readonly onSelect: (entity: EntityId | null) => void;
  readonly onMutation: (report: MutationReport) => void;
  readonly sceneName: string | null;
  /** Reports the `lighter` block for `.ludion/editor-context.json` (App owns the debounced write). */
  readonly onLighterContext: (context: ContextLighter | null) => void;
  // --- viewport plumbing (shared with the Level and Game Designer views) ---
  readonly onEditEntry: (entry: UndoEntry) => void;
  readonly onDirty: () => void;
  readonly connection: 'live' | 'reconnecting' | 'failed';
  readonly onFrameStatus: (status: FrameStatus) => void;
  /** The view camera, for `.ludion/editor-context.json` so a persona knows there is a "here". */
  readonly onCamera: (camera: ContextCamera | null) => void;
  readonly pointer: readonly [number, number, number] | null;
  readonly onPointerChange: (position: readonly [number, number, number] | null) => void;
  // --- terminal drawer ---
  readonly url: string;
  readonly viewId: string;
  readonly projectDir: string;
  /** The current project's name, validated through `project_info`; `null` before it resolves. */
  readonly projectName: string | null;
  readonly onOpenProjectSwitcher: () => void;
}

/** How often the live components refresh (~3 Hz) — the inspector's cadence, over three components on
 *  at most three entities. */
const POLL_INTERVAL_MS = 300;

/** `L3006` — the entity is alive and the component is registered, but it carries no value of it. For
 *  all three components this view polls that is an ordinary state, not a failure. */
const COMPONENT_ABSENT = 'L3006';

function isAbsent(err: unknown): boolean {
  return err instanceof ProtocolError && err.code === COMPONENT_ABSENT;
}

/** The value of a `component.get` response, or `null`. */
function componentValue(body: ResponseBody): Record<string, unknown> | null {
  const b = body as { kind?: unknown; value?: unknown };
  if (b.kind !== 'component_value' || typeof b.value !== 'object' || b.value === null) {
    return null;
  }
  return b.value as Record<string, unknown>;
}

function sameEntity(a: EntityId, b: EntityId): boolean {
  return a.index === b.index && a.generation === b.generation;
}

/** A polled component value tagged with the entity it was read from. The two light cards follow a
 *  selection and a scene, both of which move between polls, so an untagged value would render one
 *  entity's numbers in another's form — and an edit made in that window would write them there. */
interface Polled {
  readonly entity: EntityId;
  readonly value: Record<string, unknown>;
}

/** `polled`'s value if it was read from `entity`, else `null` — the card shows its hint for the one
 *  tick it takes the poll to catch up. */
function valueFor(polled: Polled | null, entity: EntityId | null): Record<string, unknown> | null {
  return polled !== null && entity !== null && sameEntity(polled.entity, entity) ? polled.value : null;
}

/** Tags a poll's answer, or drops it when the component is absent. */
function tag(entity: EntityId, value: Record<string, unknown> | null): Polled | null {
  return value === null ? null : { entity, value };
}

export function LighterView(props: LighterViewProps) {
  const { client, sceneName, onMutation, onLighterContext, selection } = props;

  const [schemas, setSchemas] = useState<Record<string, unknown> | null>(null);
  const [target, setTarget] = useState<MaterializationTarget | null>(null);
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [authored, setAuthored] = useState(false);
  const [sun, setSun] = useState<EntityId | null>(null);
  const [sunValue, setSunValue] = useState<Polled | null>(null);
  const [lights, setLights] = useState<readonly LightEntry[]>([]);
  const [pointLight, setPointLight] = useState<Polled | null>(null);
  const [spotLight, setSpotLight] = useState<Polled | null>(null);
  const [dynamic, setDynamic] = useState<Polled | null>(null);
  const [holding, setHolding] = useState<GiHold | null>(null);
  const [error, setError] = useState<DiagnosticView | null>(null);
  /** The session's rebake lever, `null` until the query form answers. */
  const [rebakeMode, setRebakeMode] = useState<GiRebakeMode | null>(null);
  /** Bumped to re-run target resolution: a materialization changes the answer to "is it authored". */
  const [resolveTick, setResolveTick] = useState(0);
  /** Bumped to re-run the two cheap light queries: a `GiDynamic` toggle changes a list row's chip.
   *  Separate from `resolveTick` because that one re-runs the UNFILTERED query over every entity. */
  const [lightsTick, setLightsTick] = useState(0);

  /** Bit 0 of the newest frame header, through the store the viewport already writes. */
  const giStale = useSyncExternalStore(subscribeFrameStatus, () => getFrameStatus().giStale);

  const onMutationRef = useRef(onMutation);
  onMutationRef.current = onMutation;
  const onReport = useCallback((report: MutationReport) => onMutationRef.current(report), []);

  /**
   * The base an edit assembles onto when no pending base is in play.
   *
   * The first edit on a scene with no `GiVolume` materializes the component carrying ONLY that field,
   * so the scene records the number a human chose and inherits the rest — writing the full default
   * set would freeze today's defaults into the file. Every later edit assembles the whole live value,
   * because the wire takes the full component and never a patch.
   */
  const initialBase = useCallback(
    (path: FieldPath): Record<string, unknown> | undefined => {
      const key = path[0];
      if (typeof key !== 'string') {
        return undefined;
      }
      if (!GI_FIELD_NAMES.includes(key as GiFieldName)) {
        return undefined; // a path this card does not edit
      }
      return authored ? values : { [key]: effectiveGi(values)[key as GiFieldName] };
    },
    [authored, values],
  );

  /** A materialization is never echoed back unchanged — the engine answers with every field — so it is
   *  the pending base's two-stale-polls fallback that hands the card back to the poll. */
  const onApplied = useCallback(
    (fromInitialBase: boolean): void => {
      if (fromInitialBase && !authored) {
        setAuthored(true);
        setResolveTick((tick) => tick + 1);
      }
    },
    [authored],
  );

  /**
   * The commit path, shared with the Level Designer's inspector (`useComponentCommit`), including the
   * pending assembly base that keeps a second edit inside one poll interval from reverting the first.
   *
   * Editing is allowed in BOTH modes, unlike the Inspector: judging light against moving content is
   * the point of this tab.
   */
  const {
    effective: live,
    commit,
    resetBase,
  } = useComponentCommit<Record<string, unknown>>({
    client,
    entity: target?.entity ?? null,
    component: GI_VOLUME,
    value: values,
    initialBase,
    onReport,
    onError: setError,
    onApplied,
  });

  // The registry's component schemas, once: the volume's drives the card's slider rows, and the two
  // light components' drive the inspector forms this panel embeds.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const body = await client.call({ method: 'engine.describe' });
        if (cancelled) {
          return;
        }
        setSchemas(
          body.kind === 'engine_description' && typeof body.description === 'object'
            ? ((body.description as { components?: Record<string, unknown> }).components ?? {})
            : {},
        );
      } catch (err) {
        if (!cancelled) {
          setError(toDiagnostic(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client]);

  // Target resolution: mount, scene change, materialization. NOT a poll — this query carries every
  // entity in the world.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const body = await client.call({
          method: 'query.entities',
          params: { filter: [], include: [...targetInclude(GI_VOLUME)] },
        });
        const candidates = parseTargetCandidates(body);
        const picked = pickTarget(candidates, GI_VOLUME);
        if (cancelled) {
          return;
        }
        setTarget(picked);
        if (picked !== null) {
          const chosen = candidates.find((candidate) => sameEntity(candidate.entity, picked.entity));
          setValues(chosen === undefined ? {} : { ...targetValue(chosen, GI_VOLUME) });
          setAuthored(picked.authored);
          resetBase();
        }
      } catch (err) {
        if (!cancelled) {
          setError(toDiagnostic(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, sceneName, resolveTick, resetBase]);

  // The sun and the lights list: two cheap FILTERED queries, on their own tick. Kept off
  // `resolveTick` so materializing a `GiVolume` does not re-ask questions whose answers it cannot
  // have changed, and so a `GiDynamic` toggle can refresh the list's chips without dragging the
  // unfiltered query along.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const suns = await client.call({
          method: 'query.entities',
          params: { filter: [DIRECTIONAL_LIGHT], include: [DIRECTIONAL_LIGHT, 'Name'] },
        });
        if (cancelled) {
          return;
        }
        setSun(pickSun(parseLightEntries(suns)));
        const placed = await client.call({
          method: 'query.entities',
          params: { filter: [POINT_LIGHT], include: [...lightInclude()] },
        });
        const cones = await client.call({
          method: 'query.entities',
          params: { filter: [SPOT_LIGHT], include: [...lightInclude()] },
        });
        if (cancelled) {
          return;
        }
        setLights(parseLightEntries(placed, cones));
      } catch (err) {
        if (!cancelled) {
          setError(toDiagnostic(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, sceneName, lightsTick]);

  // The session's rebake lever, once per connection: the query form (`{}`) changes nothing and reads
  // back both the mode and whether the last frame was held. Read rather than assumed — the lever is
  // the host's, and another client may have moved it.
  useEffect(() => {
    let cancelled = false;
    void client
      .call({ method: 'render.gi_rebake', params: {} })
      .then((body) => {
        if (!cancelled && body.kind === 'gi_rebake') {
          setRebakeMode(body.mode);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(toDiagnostic(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [client]);

  /** One polled component. Absence is a state, not a failure, so `L3006` resolves to `null` and
   *  everything else surfaces. */
  const getComponent = useCallback(
    async (entity: EntityId, component: string): Promise<Record<string, unknown> | null> => {
      try {
        const body = await client.call({
          method: 'component.get',
          params: { entity: entityRef(entity), component },
        });
        return componentValue(body);
      } catch (err) {
        if (isAbsent(err)) {
          return null;
        }
        setError(toDiagnostic(err));
        return null;
      }
    },
    [client],
  );

  const poll = useCallback(async (): Promise<void> => {
    if (target !== null) {
      const value = await getComponent(target.entity, GI_VOLUME);
      setAuthored(value !== null);
      setValues(value ?? {});
    }
    if (sun !== null) {
      setSunValue(tag(sun, await getComponent(sun, DIRECTIONAL_LIGHT)));
    }
    if (selection !== null) {
      setPointLight(tag(selection, await getComponent(selection, POINT_LIGHT)));
      setSpotLight(tag(selection, await getComponent(selection, SPOT_LIGHT)));
      setDynamic(tag(selection, await getComponent(selection, GI_DYNAMIC)));
    }
  }, [getComponent, target, sun, selection]);

  usePolling(target !== null || sun !== null || selection !== null, POLL_INTERVAL_MS, poll);

  // --- hold-to-compare ---------------------------------------------------------------------------
  // Neither end is reported through `onMutation` or `onDirty`: the pair is net-zero by construction,
  // so recording it would put a no-op on the undo stack and dirty a session the Level Designer shares.
  const compareRef = useRef<CompareSession | null>(null);

  const release = useCallback((): void => {
    const session = compareRef.current;
    if (session === null) {
      return;
    }
    compareRef.current = null;
    setHolding(null);
    void client.call(endCompare(session).command).catch((err: unknown) => setError(toDiagnostic(err)));
  }, [client]);

  const press = useCallback(
    (hold: GiHold): void => {
      if (target === null) {
        return;
      }
      release(); // a press while one is already held releases the first rather than stacking
      const start = beginCompare(
        GI_VOLUME,
        target.entity,
        authored ? live : null,
        hold === 'gi' ? GI_AB : BOUNCE_AB,
        hold,
      );
      if (start === null) {
        return;
      }
      // The comparison writes straight to the engine, so hand the displayed value back to the poll:
      // two writers over one base would fight.
      resetBase();
      compareRef.current = start.session;
      setHolding(hold);
      void client.call(start.patch.command).catch((err: unknown) => {
        compareRef.current = null;
        setHolding(null);
        setError(toDiagnostic(err));
      });
    },
    [client, target, authored, live, release, resetBase],
  );

  /** The card's one write path, at either cadence, refused while a comparison holds the component.
   *  The ref rather than `holding` because a press must exclude the very next event, not the next
   *  render. */
  const commitField = useCallback(
    (path: FieldPath, value: unknown, cadence?: CommitCadence): void => {
      if (compareRef.current !== null) {
        return;
      }
      void commit(path, value, cadence);
    },
    [commit],
  );

  // A held input that never releases has bitten this editor before (the play-mode mouse path), so the
  // release is armed from every direction: the pointer coming up anywhere, the window losing focus,
  // and — via the effect below — the view or its target going away.
  useEffect(() => {
    if (holding === null) {
      return;
    }
    const off = (): void => release();
    window.addEventListener('pointerup', off);
    window.addEventListener('pointercancel', off);
    window.addEventListener('blur', off);
    return () => {
      window.removeEventListener('pointerup', off);
      window.removeEventListener('pointercancel', off);
      window.removeEventListener('blur', off);
    };
  }, [holding, release]);

  const targetKey = target === null ? null : `${target.entity.index}:${target.entity.generation}`;
  useEffect(() => () => release(), [targetKey, release]);

  /**
   * "Enable GI": an ordinary undoable, dirtying mutation — not a comparison, and not a field edit.
   *
   * `component.set` with an empty value validates against the schema and serde fills every field, so
   * the scene gets the shipped volume. The reply carries no `previous` (the component was absent),
   * which `editor/mutations.ts` already reads as a fresh insert whose inverse is a `component.remove`
   * — so one Ctrl+Z takes the volume back out with no special case here.
   */
  const enableGi = useCallback((): void => {
    if (target === null || authored) {
      return;
    }
    const command = componentSet(target.entity, GI_VOLUME, {});
    resetBase();
    void client
      .call(command)
      .then((body) => {
        onMutationRef.current({ command, body });
        setAuthored(true);
        setResolveTick((tick) => tick + 1);
      })
      .catch((err: unknown) => setError(toDiagnostic(err)));
  }, [client, target, authored, resetBase]);

  /**
   * The `GiDynamic` toggle: one write per press, no preview cadence.
   *
   * An ordinary undoable, dirtying mutation, exactly like "Enable GI" — a marker is world state and
   * it is saved with the scene. `editor/mutations.ts` already inverts both directions: the insert
   * carries no `previous`, so its inverse is a `component.remove`, and the remove's `previous` is the
   * empty value that puts the marker back.
   */
  const toggleDynamic = useCallback(
    (entity: EntityId, next: boolean): void => {
      const command: Command = next
        ? componentSet(entity, GI_DYNAMIC, {})
        : { method: 'component.remove', params: { entity: entityRef(entity), component: GI_DYNAMIC } };
      void client
        .call(command)
        .then((body) => {
          onMutationRef.current({ command, body });
          // The write succeeded, so the control can lead the poll by one tick; the list's chips need
          // the query re-run, since presence is what it read.
          setDynamic(next ? { entity, value: {} } : null);
          setLightsTick((tick) => tick + 1);
        })
        .catch((err: unknown) => setError(toDiagnostic(err)));
    },
    [client],
  );

  /** The lever's two writers. Neither is a mutation: `render.gi_rebake` moves host session state, so
   *  it is not reported through `onMutation` and never dirties the scene. */
  const setLever = useCallback(
    (mode: GiRebakeMode): void => {
      void client
        .call({ method: 'render.gi_rebake', params: { mode } })
        .then((body) => {
          if (body.kind === 'gi_rebake') {
            setRebakeMode(body.mode);
          }
        })
        .catch((err: unknown) => setError(toDiagnostic(err)));
    },
    [client],
  );

  const rebakeNow = useCallback((): void => {
    void client
      .call({ method: 'render.gi_rebake', params: { rebake: true } })
      .catch((err: unknown) => setError(toDiagnostic(err)));
  }, [client]);

  // --- what leaves the view ----------------------------------------------------------------------
  // The context bridge carries the placement rule's answer so the persona inherits it: a "raise the
  // bounce" request lands on the entity this tab would have written.
  useEffect(() => {
    onLighterContext({ entity: target === null ? null : target.entity, authored, sun });
  }, [target, authored, sun, onLighterContext]);
  useEffect(() => () => onLighterContext(null), [onLighterContext]);

  /** What the card reads as the live volume: `null` while unauthored, so the state chip, the two A/B
   *  availability rules and the dimming all see absence rather than an empty object. */
  const gi = authored ? live : null;

  return (
    <div className="main-row">
      {/* Left column: the lighting panel. */}
      <div className="col col-side">
        <LighterPanel
          client={client}
          schemas={schemas}
          target={target}
          authored={authored}
          gi={gi}
          onEnableGi={enableGi}
          onCommit={commitField}
          rebakeMode={rebakeMode}
          onRebakeMode={setLever}
          onRebakeNow={rebakeNow}
          giStale={giStale}
          holding={holding}
          onHoldStart={press}
          onHoldEnd={release}
          sun={sun}
          sunValue={valueFor(sunValue, sun)}
          lights={lights}
          selection={selection}
          onSelect={props.onSelect}
          pointLight={valueFor(pointLight, selection)}
          spotLight={valueFor(spotLight, selection)}
          selectionDynamic={valueFor(dynamic, selection) !== null}
          onToggleDynamic={toggleDynamic}
          onReport={onReport}
          error={error}
          onDismissError={() => setError(null)}
          onError={setError}
        />
      </div>

      {/* Centre column: the live frame. Selection, pointer and gizmo are all live — clicking a lit
          surface selects what is under it, and the lights list reaches the ones with no mesh. */}
      <div className="col-center lt-center">
        <div className="lt-toolbar">
          <span className="lt-toolbar-note">
            hold a row&apos;s A/B to see the frame without that contribution
          </span>
        </div>
        <Viewport
          client={client}
          mode={props.mode}
          selection={selection}
          onSelect={props.onSelect}
          onEditEntry={props.onEditEntry}
          onDirty={props.onDirty}
          connection={props.connection}
          scenePath={sceneName}
          onFrameStatus={props.onFrameStatus}
          onCamera={props.onCamera}
          pointer={props.pointer}
          onPointerChange={props.onPointerChange}
        />
      </div>

      {/* Right column: the persona terminal drawer (unchanged). */}
      <div className="col col-term">
        <TerminalDrawer
          url={props.url}
          viewId={props.viewId}
          scenePath={sceneName ?? ''}
          projectDir={props.projectDir}
          projectName={props.projectName}
          onOpenProjectSwitcher={props.onOpenProjectSwitcher}
        />
      </div>
    </div>
  );
}
