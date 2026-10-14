// The Art Director view (Editor slice #4, `Plan/editor-art-director.md`): the tab where a human
// directs the engine's look by eye, live, against the content they are judging.
//
// LAYOUT — the plan's sketch, every piece an existing precedent: the chain panel on the LEFT (the
// Level Designer's column), the viewport in the CENTRE over an optional pinned reference capture (the
// Game Designer's `VerticalSplit`), the terminal column on the RIGHT. It shares App's session and its
// `ProtocolClient`, unlike the Voxel Artist: post-process is global to the scene and is judged against
// real content, so there is no rig to isolate.
//
// THE CADENCES ARE DELIBERATE, AND THEY ARE NOT ALL THE SAME.
//   - `query.entities` with the target `include` returns EVERY entity (the lowest-`EntityId` fallback
//     needs them all), which is thousands on a district scene. It runs once on mount and again on a
//     scene change or a materialization — never on a timer.
//   - `component.get` on the resolved target IS polled, at the inspector's ~3 Hz: it is one entity, one
//     component, and it is what the whole panel reads. An entity that carries no `PostProcess` answers
//     `L3006` rather than null, and that is the NORMAL case (most scenes author none), so it reads as
//     `{}` and never as a banner.
//   - `render.dump_graph` runs on mount and after an edit to a pass-gating effect, because only those
//     three change the graph. Its answer is held against the chain's own model and any disagreement is
//     shown: a model that quietly diverges from the engine is worse than no model.
//   - The sun check is a FILTERED query (cheap) on the same low cadence as target resolution.
//
// HOLD-TO-COMPARE IS NEITHER A MUTATION NOR AN UNDO ENTRY (the plan's decision 7). Press writes the
// effect's neutral, release writes back the captured object — net-zero by construction — so neither end
// goes through `onMutation` or `onDirty`. That matters because the Level Designer shares this session,
// and a `scene.save` from that tab would otherwise persist whatever state a comparison was left in. A
// held compare therefore must never be able to stay held: it releases on pointer-up anywhere, on
// pointer-leave, on window blur, on a target change, and on unmount.
//
// WHICH IS WHY A HELD COMPARE OWNS THE COMPONENT OUTRIGHT (`commitField`). The panel's other live
// gesture is a slider drag, whose previews write `PostProcess` on their own cadence. The release
// restores the object captured at the press, so anything that landed under the hold is restored away —
// silently reverting the drag, and leaving an undo entry for a value the engine no longer holds. The
// two exclude each other by refusing the later writer rather than by ordering them: while a comparison
// is held, field writes are dropped, and the gesture that was refused reads the live value again the
// moment the release lands.
//
// STYLING SEAM. Every class this view and `ChainPanel` introduce is prefixed `ad-`; the shared shell
// primitives (`main-row`, `col`, `col-side`, `col-center`, `col-term`, `panel-head`, `icon-btn`,
// `field*`) are reused as-is. The `ad-` classes, and what each is meant to look like:
//   ad-center        the centre column's wrapper — a flex column, toolbar over the viewport/split
//   ad-toolbar       the thin strip above the viewport (reference capture + hint), like `.vx-toolbar`
//   ad-toolbar-note  muted hint text inside that strip
//   ad-split         the centre `VerticalSplit` container, filling the column (like `.vx-split`)
//   ad-reference     the pinned reference-capture pane — dark, flex column, fills the bottom pane
//   ad-reference-head that pane's header row: label left, buttons right, `.panel-head`'s weight
//   ad-reference-img the captured PNG: contained and centred, never cropped or stretched
//   ad-panel         the chain panel filling the left column (flex column, min-height 0)
//   ad-target        the line under the panel head: which entity holds the look and whether it is
//                    authored — quiet, one line, wrapping to two at a narrow column
//   ad-target-rule   the placement rule's half of that line, muted
//   ad-scroll        the scrolling row list under the panel head
//   ad-hint          muted placeholder text (no target yet, no schema yet)
//   ad-warn          a warning block: schema drift, or a chain/frame-graph disagreement
//   ad-row           one effect row (header plus, when open, its fields)
//   ad-row-open      modifier on the expanded row
//   ad-row-head      the row's single line: toggle, passes marker, A/B button
//   ad-row-toggle    the caret+label+chip button — transparent, full width, left-aligned
//   ad-caret         the ▸/▾ glyph
//   ad-label         the effect's name
//   ad-chip          the state chip, with tones ad-chip-live (accent), ad-chip-off (muted) and
//                    ad-chip-blocked (warning — the authored-but-dead "no sun" case)
//   ad-passes        the small marker on a row whose gate adds or removes frame-graph passes
//   ad-compare       the hold-to-compare button (momentary), ad-compare-held while it is down
//   ad-fields        the nested field list under an open row (indented)
//   ad-field         one field line, wrapping the shared `.field` layout
//   ad-field-default modifier: dimmed — the scene does not author this field
//   ad-grade-axis    modifier on a grade band row's channel tag — a word (master/temp/tint) where the
//                    stacked-vector block puts a single letter, so the column is wider
//   ad-default-tag   the "shipped default" tag on such a field
//   ad-other         modifier on the row carrying schema fields the chain does not recognise

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ContextCamera } from '../terminal/context';
import type { ResponseBody } from '@ludion/protocol';
import type { EditorMode, EntityId } from '../App';
import { ProtocolError, type ProtocolClient } from '../protocol/client';
import type { UndoEntry } from '../editor/undo';
import { Viewport } from '../viewer/Viewport';
import type { FrameStatus } from '../viewer/useFrameStream';
import { toDiagnostic, type DiagnosticView } from '../viewer/diagnostic';
import { usePolling } from '../viewer/usePolling';
import { useComponentCommit, type CommitCadence, type MutationReport } from '../viewer/useComponentCommit';
import type { FieldPath } from '../viewer/inspect-fields';
import { TerminalDrawer } from '../terminal/TerminalDrawer';
import type { ContextArtDirector } from '../terminal/context';
import {
  bucketSchemaFields,
  deriveLiveEffects,
  effectiveValue,
  effectiveValues,
  gateStateKey,
  neutralPatch,
  reconcileWithGraph,
  schemaFieldNames,
  type EffectId,
} from '../editor/art/chain';
import {
  parseTargetCandidates,
  pickTarget,
  POST_PROCESS,
  targetInclude,
  targetValue,
  type MaterializationTarget,
} from '../editor/art/target';
import { beginCompare, endCompare, type CompareSession } from '../editor/compare';
import { ChainPanel, type ChainRowId } from '../editor/art/ChainPanel';
import { VerticalSplit } from './VerticalSplit';

export interface ArtDirectorViewProps {
  readonly client: ProtocolClient;
  readonly mode: EditorMode;
  readonly selection: EntityId | null;
  readonly onSelect: (entity: EntityId | null) => void;
  readonly onMutation: (report: MutationReport) => void;
  readonly sceneName: string | null;
  /** Reports the `artDirector` block for `.ludion/editor-context.json` (App owns the debounced write). */
  readonly onArtContext: (context: ContextArtDirector | null) => void;
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

/** How often the live `PostProcess` refreshes (~3 Hz) — the inspector's cadence, over one component on
 *  one entity. */
const POLL_INTERVAL_MS = 300;

/** The registry name of the sun. `shafts` is live only with one in the scene, and the chain has no way
 *  to know that from `PostProcess`'s fields. */
const DIRECTIONAL_LIGHT = 'DirectionalLight';

/** `L3006` — the entity is alive and the component is registered, but it carries no value of it. For
 *  `PostProcess` that is the ordinary case, not a failure. */
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

/** The pass names of a `render.dump_graph` response, in graph order. */
function graphPassNames(body: ResponseBody): readonly string[] {
  const graph = (body as { kind?: unknown; graph?: unknown }).kind === 'graph_dump'
    ? (body as { graph?: { passes?: unknown } }).graph
    : undefined;
  const passes = graph?.passes;
  if (!Array.isArray(passes)) {
    return [];
  }
  return passes
    .map((pass) => (pass as { name?: unknown }).name)
    .filter((name): name is string => typeof name === 'string');
}

/** A `render.screenshot` response as an inline image source, or `null`. */
function screenshotSrc(body: ResponseBody): string | null {
  const b = body as { kind?: unknown; data?: unknown; format?: unknown };
  if (b.kind !== 'screenshot' || typeof b.data !== 'string') {
    return null;
  }
  return `data:image/${typeof b.format === 'string' ? b.format : 'png'};base64,${b.data}`;
}

function sameEntity(a: EntityId, b: EntityId): boolean {
  return a.index === b.index && a.generation === b.generation;
}

export function ArtDirectorView(props: ArtDirectorViewProps) {
  const { client, sceneName, onMutation, onArtContext } = props;

  const [schema, setSchema] = useState<unknown>(null);
  const [target, setTarget] = useState<MaterializationTarget | null>(null);
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [authored, setAuthored] = useState(false);
  const [hasSun, setHasSun] = useState(false);
  const [passes, setPasses] = useState<readonly string[]>([]);
  /** The gate state `passes` was dumped for; `null` invalidates it. While it differs from the live
   *  gate state the dump describes a world that no longer exists, and reconciling against it would
   *  report ignorance as disagreement. */
  const [graphKey, setGraphKey] = useState<string | null>(null);
  const [open, setOpen] = useState<ChainRowId | null>(null);
  const [holding, setHolding] = useState<EffectId | null>(null);
  const [reference, setReference] = useState<string | null>(null);
  const [maximized, setMaximized] = useState(false);
  const [error, setError] = useState<DiagnosticView | null>(null);
  /** Bumped to re-run target resolution: a materialization changes the answer to "is it authored". */
  const [resolveTick, setResolveTick] = useState(0);

  const onMutationRef = useRef(onMutation);
  onMutationRef.current = onMutation;
  const onReport = useCallback((report: MutationReport) => onMutationRef.current(report), []);

  /**
   * The base an edit assembles onto when no pending base is in play.
   *
   * The first edit on a scene with no `PostProcess` materializes the component carrying ONLY that
   * field (the plan's decision 8) — an empty `PostProcess` is byte-identical to no component, so a
   * partial value is honest and a full one would freeze today's defaults into the scene. Every later
   * edit assembles the whole live value, because the wire takes the full component and never a patch.
   */
  const initialBase = useCallback(
    (path: FieldPath): Record<string, unknown> | undefined => {
      const key = path[0];
      if (typeof key !== 'string') {
        return undefined;
      }
      return authored ? values : { [key]: effectiveValue(values, key) };
    },
    [authored, values],
  );

  /** A materialization is never echoed back unchanged — the engine answers with every field — so it is
   *  the pending base's two-stale-polls fallback that hands the panel back to the poll. */
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
   * Editing is allowed in BOTH modes, unlike the Inspector: judging a look against moving content is
   * the point of this tab.
   *
   * `live` is what the engine currently holds as far as this view knows: the sent-but-not-yet-polled
   * value if one exists, else the polled one.
   */
  const {
    effective: live,
    commit,
    resetBase,
  } = useComponentCommit<Record<string, unknown>>({
    client,
    entity: target?.entity ?? null,
    component: POST_PROCESS,
    value: values,
    initialBase,
    onReport,
    onError: setError,
    onApplied,
  });

  // The registry's schema for `PostProcess`, once. It is the only drift net between the chain's field
  // table and the Rust struct (the plan's decision 11), so the panel shows what it cannot place.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const body = await client.call({ method: 'engine.describe' });
        if (cancelled) {
          return;
        }
        const components =
          body.kind === 'engine_description' && typeof body.description === 'object'
            ? ((body.description as { components?: Record<string, unknown> }).components ?? {})
            : {};
        setSchema(components[POST_PROCESS] ?? null);
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

  // Target resolution, the sun check and the graph dump: mount, scene change, materialization. NOT a
  // poll — the unfiltered query carries every entity in the world.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const body = await client.call({
          method: 'query.entities',
          params: { filter: [], include: [...targetInclude(POST_PROCESS)] },
        });
        const candidates = parseTargetCandidates(body);
        const picked = pickTarget(candidates, POST_PROCESS);
        if (cancelled) {
          return;
        }
        setTarget(picked);
        if (picked !== null) {
          const chosen = candidates.find((candidate) => sameEntity(candidate.entity, picked.entity));
          setValues(chosen === undefined ? {} : { ...targetValue(chosen, POST_PROCESS) });
          setAuthored(picked.authored);
          resetBase();
        }
        const sun = await client.call({
          method: 'query.entities',
          params: { filter: [DIRECTIONAL_LIGHT], include: [] },
        });
        if (cancelled) {
          return;
        }
        setHasSun(sun.kind === 'query_result' && sun.entities.length > 0);
        // A new scene can schedule a different graph at an identical gate state (a sun appearing
        // changes the pass list without changing any `PostProcess` field), so the dump is invalidated
        // outright rather than left to the gate watcher to notice.
        setGraphKey(null);
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

  const poll = useCallback(async (): Promise<void> => {
    if (target === null) {
      return;
    }
    try {
      const body = await client.call({
        method: 'component.get',
        params: { entity: target.entity, component: POST_PROCESS },
      });
      const value = componentValue(body);
      setAuthored(value !== null);
      setValues(value ?? {});
    } catch (err) {
      if (isAbsent(err)) {
        // The scene authors no look: the frame runs the shipped defaults, which is what the chain
        // shows. Nothing to report.
        setAuthored(false);
        setValues({});
        return;
      }
      setError(toDiagnostic(err));
    }
  }, [client, target]);

  usePolling(target !== null, POLL_INTERVAL_MS, poll);

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
    (effect: EffectId): void => {
      if (target === null) {
        return;
      }
      release(); // a press while one is already held releases the first rather than stacking
      // The chain owns what "off" means for an effect; the reducer only holds the patch it is given.
      // An effect with nothing to neutralize yields an empty patch and is refused here as well as in
      // the panel, which disables its toggle.
      const start = beginCompare(POST_PROCESS, target.entity, authored ? live : null, neutralPatch(effect), effect);
      if (start === null) {
        return;
      }
      // The comparison writes straight to the engine, so hand the displayed value back to the poll:
      // two writers over one base would fight.
      resetBase();
      compareRef.current = start.session;
      setHolding(effect);
      void client.call(start.patch.command).catch((err: unknown) => {
        compareRef.current = null;
        setHolding(null);
        setError(toDiagnostic(err));
      });
    },
    [client, target, authored, live, release, resetBase],
  );

  /** The chain's one write path, at either cadence, refused while a comparison holds the component
   *  (the header's rule). The ref rather than `holding` because a press must exclude the very next
   *  event, not the next render. */
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
  // release is armed from every direction: the pointer coming up anywhere, the window losing focus, and
  // — via the effect below — the view or its target going away.
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

  // --- the reference capture ---------------------------------------------------------------------
  const capture = useCallback((): void => {
    void client
      .call({ method: 'render.screenshot', params: {} })
      .then((body) => {
        const src = screenshotSrc(body);
        if (src !== null) {
          setReference(src);
        }
      })
      .catch((err: unknown) => setError(toDiagnostic(err)));
  }, [client]);

  const clearReference = useCallback((): void => {
    setReference(null);
    setMaximized(false);
  }, []);

  // --- what the panel renders --------------------------------------------------------------------
  // The effective values keep any authored field the chain's table does not know (the "Other" bucket),
  // so nothing an entity actually carries becomes uneditable.
  const effective = useMemo(() => ({ ...live, ...effectiveValues(live) }), [live]);
  const effects = useMemo(() => deriveLiveEffects(live, { hasSun }), [live, hasSun]);

  const gateKey = useMemo(() => gateStateKey(effects), [effects]);

  // The dump is driven by the gate STATE, not by the call sites that change it. Tying it to "the
  // human committed a field edit" left every other writer invisible: a hold-to-compare on ambient
  // occlusion or depth of field drops real passes, and so does the drawer's persona setting the same
  // field over MCP — both moved the chain while the pass list stood still, and the panel reported its
  // own staleness as the engine disagreeing.
  const fetchingGraph = useRef<string | null>(null);
  useEffect(() => {
    if (graphKey === gateKey || fetchingGraph.current === gateKey) {
      return;
    }
    fetchingGraph.current = gateKey;
    void client
      .call({ method: 'render.dump_graph' })
      .then((body) => {
        setPasses(graphPassNames(body));
        setGraphKey(gateKey);
      })
      .catch((err: unknown) => {
        // Stamp the key anyway or the effect re-fires forever, and drop the pass list so the failure
        // suppresses reconciliation instead of feeding it a stale one.
        setPasses([]);
        setGraphKey(gateKey);
        setError(toDiagnostic(err));
      });
  }, [client, gateKey, graphKey]);

  const disagreements = useMemo(
    // Only a dump taken at the current gate state can disagree with it. Before the first one, and
    // through the round trip after any gate moves, an empty list is the honest answer.
    () => (passes.length === 0 || graphKey !== gateKey ? [] : reconcileWithGraph(effects, passes)),
    [effects, passes, graphKey, gateKey],
  );
  const buckets = useMemo(
    () => (schema === null ? null : bucketSchemaFields(schemaFieldNames(schema))),
    [schema],
  );
  const authoredFields = useMemo<ReadonlySet<string>>(() => new Set(Object.keys(live)), [live]);

  // --- what leaves the view ----------------------------------------------------------------------
  // The context bridge carries the placement rule's answer so the persona inherits it: a "just add some
  // fog" request lands where this tab would have put it.
  useEffect(() => {
    onArtContext(
      target === null
        ? null
        : { entity: target.entity, authored, effect: open },
    );
  }, [target, authored, open, onArtContext]);
  useEffect(() => () => onArtContext(null), [onArtContext]);

  const viewport = (
    <Viewport
      client={client}
      mode={props.mode}
      selection={props.selection}
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
  );

  return (
    <div className="main-row">
      {/* Left column: the chain. */}
      <div className="col col-side">
        <ChainPanel
          schema={schema}
          effects={effects}
          disagreements={disagreements}
          buckets={buckets}
          values={effective}
          authoredFields={authoredFields}
          target={target}
          authored={authored}
          open={open}
          onOpen={setOpen}
          holding={holding}
          onHoldStart={press}
          onHoldEnd={release}
          onCommit={commitField}
          error={error}
          onDismissError={() => setError(null)}
          onError={setError}
        />
      </div>

      {/* Centre column: the live frame, over a pinned reference capture when one exists. Every look
          verdict in the post-process arc was reached by comparison. */}
      <div className="col-center ad-center">
        <div className="ad-toolbar">
          <button type="button" className="icon-btn" onClick={capture}>
            Capture reference
          </button>
          {reference !== null && (
            <button type="button" className="icon-btn" onClick={clearReference}>
              Clear
            </button>
          )}
          <span className="ad-toolbar-note">hold a row&apos;s A/B to see the frame without that effect</span>
        </div>

        {reference === null ? (
          viewport
        ) : (
          <VerticalSplit
            className="ad-split"
            maximized={maximized}
            onRestore={() => setMaximized(false)}
            hiddenLabel="Viewport hidden"
            restoreLabel="Restore viewport"
            top={viewport}
            bottom={
              <div className="ad-reference">
                <div className="ad-reference-head">
                  Reference capture
                  <div className="head-actions">
                    <button type="button" className="icon-btn" onClick={() => setMaximized((m) => !m)}>
                      {maximized ? 'restore' : 'maximize'}
                    </button>
                    <button type="button" className="icon-btn" onClick={clearReference}>
                      clear
                    </button>
                  </div>
                </div>
                <img className="ad-reference-img" src={reference} alt="Reference capture" />
              </div>
            }
          />
        )}
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
