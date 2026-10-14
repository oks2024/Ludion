// The Voxel Artist view (Editor slice #3, `Plan/editor-voxel-artist.md`): browse the
// project's voxel models, look at one live under real game lighting, and hand the value judgment to
// the persona sitting in the same window.
//
// LAYOUT — the plan's sketch, both halves inheriting an existing precedent: asset browser over the
// model panel on the LEFT (the Level Designer's column), the shared `VerticalSplit` in the CENTRE
// (the Game Designer's viewport-over-panel with its maximize toggle) with the stage viewport over
// the slice painter, the terminal column on the RIGHT. No gizmo, no hierarchy, no selection, no
// Play/Edit — the stage has one subject and no sim.
//
// THE SESSION IS NOT THE GAME'S. This view never touches App's `ProtocolClient`; it owns one dialed
// at the host-spawned stage session's url once that session reports `listening`. The game server may
// not even be running. Everything the view sends goes to the stage:
//
//   - selecting a model    → `component.set` of `Mesh` on `subject` (whole-value replace, so the
//                            current value is read first and only `shape` is swapped), plus the
//                            model's DECLARED scale stamped into its `Transform` — placement-time
//                            application, which is what that field is for: the doc is authoritative
//                            as intent, the scene as fact.
//   - orbit / pan / dolly  → throttled `component.set` of the camera's `Transform`.
//   - the rig toggle       → `scene.load`. Safe here and nowhere else, because nothing in the stage
//                            is ever edited (contract 6). It costs the camera pose and the model, so
//                            both are re-applied after the load and the toggle changes only light.
//
// The view NEVER calls `asset.reload`: the host's watcher does the reloading
// (contract 3). Saving a model — by the persona, or by the palette editor and slice painter — is
// what updates the viewport, and a rejected file blanks the subject until the next good save
// (accepted, documented behaviour) with the readout's diagnostics as the explanation.
//
// EDITING is owned by `useVoxelEdit`: it holds the working document, writes it under
// `save.ts`'s policy, and runs the settled save/watch reducer against the persona editing the same
// file. The view keeps only what is shared between the two editors — the paint glyph, which the
// swatch strip selects and the painter paints with — and re-runs `check-scene` whenever the session
// says the DISK changed.
//
// ASSEMBLIES ARE VIEWED, NOT EDITED, AND THAT IS STRUCTURAL. Selecting a `.assembly.yaml` hands
// `useVoxelEdit` a `null` path, so the editing session holds no document and there is no write path
// to reach: the palette editor is not rendered (the panel is given no write callbacks), and the
// slice grid draws the selected PART read-only under the assembly's palette. What the panels follow
// instead is `useWatchedFile` — one poll per file, the assembly and the open part — while the stage
// follows the host's watcher as it always has.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ResponseBody } from '@ludion/protocol';
import { ProtocolClient } from '../protocol/client';
import { componentSet } from '../protocol/commands';
import { isTauri } from '../shell/tauri';
import {
  onStageStatus,
  runCheckScene,
  stageRestart,
  stageStart,
  stageStatus,
  type StageStatus,
} from '../shell/host';
import { mountStage } from '../shell/stageMount';
import { addStageScenes, readEditorFile } from '../terminal/pty';
import { TerminalDrawer } from '../terminal/TerminalDrawer';
import type { FrameStatus } from '../viewer/useFrameStream';
import { DiagnosticBanner } from '../viewer/DiagnosticBanner';
import type { EntityId } from '../editor/undo';
import { parseHierarchy } from '../editor/hierarchy';
import { AssetBrowser } from '../editor/voxel/AssetBrowser';
import { ClipStrip } from '../editor/voxel/ClipStrip';
import { ModelPanel } from '../editor/voxel/ModelPanel';
import { PartsPanel } from '../editor/voxel/PartsPanel';
import { SlicePainterPanel } from '../editor/voxel/SlicePainterPanel';
import { StageViewport } from '../editor/voxel/StageViewport';
import { useViewCamera } from '../editor/camera/useViewCamera';
import { EMPTY_GLYPH, parseVoxelDoc, type VoxelDoc } from '../editor/voxel/doc';
import { setPaletteColor, setPaletteMaterial, type MaterialField } from '../editor/voxel/edit';
import { useClipPlayback } from '../editor/voxel/useClipPlayback';
import { useVoxelEdit } from '../editor/voxel/useVoxelEdit';
import { useWatchedFile } from '../editor/voxel/useWatchedFile';
import {
  parseAssemblyDoc,
  parsePartDoc,
  partAsVoxelDoc,
  partFilePath,
  partRestBox,
  partWorldBox,
  type AssemblyDoc,
  type ClipBinding,
} from '../editor/voxel/assembly';
import { parseCheckScene, type ModelReadout } from '../editor/voxel/readout';
import { secondsOf } from '../editor/voxel/anim';
import {
  boxSphere,
  framing,
  mergeSpheres,
  modelSphere,
  orbitFromDirection,
  orbitPose,
  presetPose,
  PRESETS,
  STAGE_FOV_Y_DEG,
  turntableStep,
  type CameraPose,
  type FramingMode,
  type PresetId,
  type Sphere,
  type Viewport as ViewportSize,
} from '../editor/voxel/orbit';
import {
  ASSEMBLY_STAGE_SCALE,
  emitDelay,
  modelDisplayName,
  modelFilePath,
  modelKind,
  modelShape,
  MODEL_EXTS,
  READOUT_DEBOUNCE_MS,
  rigDef,
  RIGS,
  stageSceneName,
  stageScenePath,
  TURNTABLE_DEG_PER_TICK,
  TURNTABLE_TICK_MS,
  stageSlot,
  voxelsRoot,
  type Rig,
} from '../editor/voxel/stage';
import type { ContextVoxel, ContextVoxelAssembly } from '../terminal/context';
import { vAdd, vLen, vSub, type Vec3 } from '../editor/gizmo/vecmath';
import { VerticalSplit } from './VerticalSplit';

/** The pivot a preset or the turntable frames when nothing is selected — the stage's usual subject
 *  height. */
const DEFAULT_PIVOT: Vec3 = [0, 2, 0];

/** The stage session's url when it has not announced one yet — the address the host binds by
 *  default, and the one `editor/personas/voxel-artist.md` names. The drawer needs *some* url at
 *  spawn so the persona's `ludion-mcp` dials the stage rather than the game. */
const FALLBACK_STAGE_URL = 'ws://127.0.0.1:9412';

/** The last-selected model, so switching views (which unmounts this one) doesn't lose the model on
 *  the stage. Keyed by nothing else — one project dir at a time. */
const MODEL_KEY = 'ludion.editor.voxelModel';
// The rig is persisted for the same reason the model is: the stage SESSION outlives a view switch,
// so component state that reset on remount would disagree with the scene the stage actually holds.
const RIG_KEY = 'ludion.editor.voxelRig';

/** The two named entities the stage scenes carry. Names are load-bearing: `query.entities` is how
 *  the view finds them, and both rigs use the same two. The `camera` entity is no longer one of
 *  them — the view camera drives the stream through `render.view`, never `component.set`
 *  (`Plan/editor-stage-camera.md` decision 4). */
const SUBJECT = 'subject';
const REFERENCE = 'adventurer';

interface StageEntities {
  readonly subject: EntityId | null;
  readonly reference: EntityId | null;
}

const NO_ENTITIES: StageEntities = { subject: null, reference: null };

/** The subject's two components as the committed stage scene authors them. */
interface SubjectBase {
  readonly mesh: Record<string, unknown>;
  readonly transform: Record<string, unknown>;
}

const EMPTY_SUBJECT_BASE: SubjectBase = { mesh: {}, transform: {} };

/** The inert editing props a read-only slice grid is given. Module-level so the memoized panel is
 *  not re-rendered by a fresh identity each frame. */
const NO_OP = (): void => {};
const NO_CLIPS: readonly ClipBinding[] = [];
const NO_APPLY = (): void => {};

const STOPPED: StageStatus = {
  slot: 'model',
  phase: 'stopped',
  url: null,
  scene: null,
  message: null,
  code: null,
  listener: null,
};

interface VoxelArtistViewProps {
  readonly projectDir: string;
  /** The current project's name, validated through `project_info`; `null` before it resolves. */
  readonly projectName: string | null;
  readonly onOpenProjectSwitcher: () => void;
  readonly viewId: string;
  /** Reports the `voxel` block for `.ludion/editor-context.json` (App owns the debounced write). */
  readonly onVoxelContext: (context: ContextVoxel | null) => void;
  readonly onFrameStatus: (status: FrameStatus) => void;
  /** The one-line model readout for the status bar. */
  readonly onStatusNote: (note: string | null) => void;
}

/** The `value` of a `component.get` response, or `null`. Whole-value replace means every set starts
 *  from the value that is there — a `Mesh` set that dropped its `color` would silently retint the
 *  subject. */
function componentValue(body: ResponseBody): Record<string, unknown> | null {
  const b = body as { kind?: unknown; value?: unknown };
  if (b.kind !== 'component_value' || typeof b.value !== 'object' || b.value === null) {
    return null;
  }
  return b.value as Record<string, unknown>;
}

/** `[x, y, z]` out of an unknown component field, or `null`. */
function vec3(value: unknown): Vec3 | null {
  if (!Array.isArray(value) || value.length < 3) {
    return null;
  }
  const nums = value.slice(0, 3).map(Number);
  return nums.every((n) => Number.isFinite(n)) ? [nums[0], nums[1], nums[2]] : null;
}

export function VoxelArtistView(props: VoxelArtistViewProps) {
  const { projectDir, onVoxelContext, onStatusNote } = props;

  // --- the stage session ------------------------------------------------------------------------
  const [status, setStatus] = useState<StageStatus>(STOPPED);
  const [stageError, setStageError] = useState<string | null>(null);
  const [client, setClient] = useState<ProtocolClient | null>(null);
  const clientRef = useRef<ProtocolClient | null>(null);
  clientRef.current = client;
  const [entities, setEntities] = useState<StageEntities>(NO_ENTITIES);
  const entitiesRef = useRef<StageEntities>(NO_ENTITIES);
  entitiesRef.current = entities;
  // The subject as the COMMITTED stage scene authors it. Every selection is written over this, never
  // over what the previous one left, so what stands on the stage is a function of the current
  // selection and the scene — never of the click history.
  const subjectBaseRef = useRef<SubjectBase>(EMPTY_SUBJECT_BASE);

  // --- the model --------------------------------------------------------------------------------
  const [selected, setSelected] = useState<string | null>(() => {
    try {
      return localStorage.getItem(MODEL_KEY);
    } catch {
      return null;
    }
  });
  const [readout, setReadout] = useState<ModelReadout | null>(null);
  const [checking, setChecking] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  /** The glyph the swatch strip selected and the painter paints with — shared between the two
   *  editors, so the view owns it. `.` erases. */
  const [paintGlyph, setPaintGlyph] = useState<string>(EMPTY_GLYPH);

  // --- the camera -------------------------------------------------------------------------------
  const [rig, setRig] = useState<Rig>(() => {
    try {
      return localStorage.getItem(RIG_KEY) === 'night' ? 'night' : 'neutral';
    } catch {
      return 'neutral';
    }
  });
  // Read by the mount effect, which must start the stage on the rig this view is showing without
  // taking `rig` as a dependency (switching it is a `scene.load`, never a restart).
  const rigRef = useRef<Rig>(rig);
  rigRef.current = rig;
  const [framingMode, setFramingMode] = useState<FramingMode>('fill-frame');
  const [turntable, setTurntable] = useState(false);
  // The canvas's CSS size, as a value: `useViewCamera` takes it as a plain prop, not a ref. The
  // framing/turntable math below reads the CURRENT camera pose through a ref instead (see
  // `cameraPoseRef`), so a resize alone still cannot re-fit the camera out from under the human.
  const [viewport, setViewport] = useState<ViewportSize>({ width: 16, height: 9 });
  const viewportRef = useRef<ViewportSize>(viewport);
  viewportRef.current = viewport;
  const [referenceSphere, setReferenceSphere] = useState<Sphere | null>(null);
  // When the `voxel` context block was last emitted (throttle floor, see the effect below).
  const lastEmitRef = useRef(0);

  const [maximized, setMaximized] = useState(false);

  const voxels = useMemo(() => (projectDir === '' ? '' : voxelsRoot(projectDir)), [projectDir]);
  const modelPath = useMemo(
    () => (projectDir === '' || selected === null ? null : modelFilePath(projectDir, selected)),
    [projectDir, selected],
  );
  const isAssembly = selected !== null && modelKind(selected) === 'assembly';

  // The editing session owns the file: the working text the palette editor and the painter draw, its
  // writes, its undo history, and the conflict policy against the persona. It also owns the mtime
  // poll this view used to run — one watcher per file, not two. An assembly is handed `null`: that,
  // and not a disabled control, is what makes the whole write path unreachable for one.
  const session = useVoxelEdit(isAssembly ? null : modelPath);
  const doc = session.doc;

  // --- the assembly and its open part -----------------------------------------------------------
  const assemblyFile = useWatchedFile(isAssembly ? modelPath : null);
  // `isAssembly` is re-checked here and not only at the watcher: the watcher's own reset lands one
  // commit later, and for that render the previous rig's text would still be in hand.
  const assembly = useMemo<AssemblyDoc | null>(
    () => (!isAssembly || assemblyFile.text === null ? null : parseAssemblyDoc(assemblyFile.text)),
    [isAssembly, assemblyFile.text],
  );
  const [partName, setPartName] = useState<string | null>(null);
  const part = assembly?.parts.find((entry) => entry.name === partName) ?? null;
  // Part paths resolve against the assembly file's own directory, and `selected` is relative to the
  // same root the resolved path has to be.
  const partPath = useMemo(
    () =>
      projectDir === '' || selected === null || part === null
        ? null
        : modelFilePath(projectDir, partFilePath(selected, part.file)),
    [projectDir, selected, part],
  );
  const partFile = useWatchedFile(partPath);
  const partDoc = useMemo(
    () => (partFile.text === null ? null : parsePartDoc(partFile.text)),
    [partFile.text],
  );

  // An assembly declares no `scale:` — it is a rig, not a placement — so the stage stands it at the
  // content-density convention, and the panel says which number it used.
  const declaredScale = isAssembly ? ASSEMBLY_STAGE_SCALE : (doc?.scale?.value ?? readout?.scale ?? 1);
  const modelSize = readout?.size ?? doc?.size ?? null;

  /** The assembly's palette in the model panel's own input shape. The swatch strip reads
   *  `doc.palette`, and an assembly has exactly one palette whatever part is open. */
  const assemblyPalette = useMemo<VoxelDoc | null>(
    () =>
      assembly === null
        ? null
        : {
            source: '',
            name: assembly.name,
            size: assembly.size,
            scale: null,
            palette: assembly.palette,
            layers: [],
          },
    [assembly],
  );

  /** The grid the slice panel draws: the model's own, or the open part's under the assembly's one
   *  palette — the only palette a part's glyphs ever resolve against. */
  const painterDoc = useMemo(() => {
    if (!isAssembly) {
      return doc;
    }
    return partDoc === null || assembly === null ? null : partAsVoxelDoc(partDoc, assembly.palette);
  }, [isAssembly, doc, partDoc, assembly]);

  // ---------------------------------------------------------------------------------------------
  // Stage lifecycle. `stageStart` is idempotent, so every mount just asks; readiness arrives as a
  // `listening` event (a cold start builds first, which is why this can never be a promise).
  // The rig is NOT a dependency: switching it is a `scene.load` on the running session, not a
  // restart.
  useEffect(() => {
    if (!isTauri() || projectDir === '') {
      setStatus(STOPPED);
      return;
    }
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onStageStatus('model', (next) => {
      if (!cancelled) {
        setStatus(next);
      }
    }).then((off) => {
      if (cancelled) {
        off();
      } else {
        unlisten = off;
      }
    });
    void mountStage({
      status: () => stageStatus('model'),
      start: () => stageStart('model', { scene: stageScenePath(projectDir, rigRef.current) }),
      cancelled: () => cancelled,
      onStatus: setStatus,
      onStarted: (started) => {
        setStatus(started);
        setStageError(null);
      },
      onError: setStageError,
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [projectDir]);

  const restart = useCallback(() => {
    if (!isTauri() || projectDir === '') {
      return;
    }
    setStageError(null);
    void stageRestart('model', { scene: stageScenePath(projectDir, rig) })
      .then(setStatus)
      .catch((err: unknown) => setStageError(String(err)));
  }, [projectDir, rig]);

  const addScenes = useCallback(() => {
    if (!isTauri() || projectDir === '') {
      return;
    }
    setStageError(null);
    void addStageScenes(projectDir)
      .then(() => restart())
      .catch((err: unknown) => setStageError(String(err)));
  }, [projectDir, restart]);

  // Dial the stage once it is listening. Same shape as App's own client lifecycle: a fresh
  // `ProtocolClient` per url, closed on teardown, held in state so the frame stream's effects
  // (keyed on `[client]`) open and close with it.
  const stageUrl = status.phase === 'listening' ? status.url : null;
  useEffect(() => {
    if (stageUrl === null) {
      setClient(null);
      setEntities(NO_ENTITIES);
      return;
    }
    let cancelled = false;
    const next = new ProtocolClient();
    void next
      .connect(stageUrl)
      .then(() => {
        if (cancelled) {
          next.close();
          return;
        }
        setClient(next);
        setStageError(null);
      })
      .catch((err: unknown) => {
        next.close();
        if (!cancelled) {
          setStageError(`could not dial the stage at ${stageUrl}: ${String(err)}`);
        }
      });
    return () => {
      cancelled = true;
      next.close();
      setClient(null);
    };
  }, [stageUrl]);

  // ---------------------------------------------------------------------------------------------
  // Reading the stage: the two named entities, and the scale reference's world sphere (its own
  // model's size × its own Transform — so `declared-scale` framing is honest whatever the stage
  // scene places there). No camera entity: `render.view` is host state, independent of the World,
  // so it needs no lookup and (per `switchRig` below) survives a `scene.load` untouched.
  const readStage = useCallback(
    async (c: ProtocolClient): Promise<StageEntities> => {
      const body = await c.call({ method: 'query.entities', params: { filter: [], include: ['Name'] } });
      const rows = parseHierarchy(body);
      const byName = (name: string): EntityId | null => rows.find((row) => row.name === name)?.entity ?? null;
      const found: StageEntities = { subject: byName(SUBJECT), reference: byName(REFERENCE) };

      // Read BEFORE the entities land: the effect that applies the selection keys on `subject`, and
      // a baseline read after it would write the first model over the component's defaults.
      if (found.subject !== null) {
        const [mesh, transform] = await Promise.all([
          c.call({ method: 'component.get', params: { entity: found.subject, component: 'Mesh' } }),
          c.call({ method: 'component.get', params: { entity: found.subject, component: 'Transform' } }),
        ]);
        subjectBaseRef.current = {
          mesh: componentValue(mesh) ?? {},
          transform: componentValue(transform) ?? {},
        };
      }

      setEntities(found);
      entitiesRef.current = found;

      if (found.reference !== null && projectDir !== '') {
        try {
          const [mesh, transform] = await Promise.all([
            c.call({ method: 'component.get', params: { entity: found.reference, component: 'Mesh' } }),
            c.call({ method: 'component.get', params: { entity: found.reference, component: 'Transform' } }),
          ]);
          const shape = (componentValue(mesh)?.shape ?? null) as { voxel?: unknown } | null;
          const value = componentValue(transform) ?? {};
          const authored = vec3(value.scale)?.[0] ?? 1;
          const translation = vec3(value.translation) ?? [0, 0, 0];
          if (typeof shape?.voxel === 'string') {
            // The reference's asset path is content-root-relative (`voxels/adventurer.voxel.yaml`),
            // and the content root is the stage scene's directory.
            const text = await readEditorFile(`${projectDir}/scenes/${shape.voxel}`);
            const doc = parseVoxelDoc(text);
            // The reference is stamped with its DECLARED scale, exactly like the subject — the whole
            // point of the figure is that it is the true size, and an authored Transform left over
            // from an earlier version of the model silently makes every size judgment wrong.
            const scale = doc.scale?.value ?? authored;
            if (scale !== authored) {
              await c.call(componentSet(found.reference, 'Transform', { ...value, scale: [scale, scale, scale] }));
            }
            setReferenceSphere(doc.size === null ? null : modelSphere(doc.size, scale, translation));
          }
        } catch {
          // No scale reference is a degraded `declared-scale` framing (it falls back to the subject
          // alone), never a failure of the view.
          setReferenceSphere(null);
        }
      }
      return found;
    },
    [projectDir],
  );

  useEffect(() => {
    if (client === null) {
      return;
    }
    void readStage(client).catch((err: unknown) => setStageError(String(err)));
  }, [client, readStage]);

  // `declared-scale` stands the subject beside the reference figure AT ITS DECLARED SCALE, and an
  // assembly declares none — the comparison would be against a number this view chose, which is no
  // comparison at all. So the mode is forced off, and its control is hidden rather than left as a
  // toggle that silently does nothing.
  const activeFraming: FramingMode = isAssembly ? 'fill-frame' : framingMode;

  // The subject's world sphere: its grid extent at its declared scale, placed at the origin (which
  // is where both stage scenes put `subject`).
  const subjectSphere = useMemo(
    () => (modelSize === null ? null : modelSphere(modelSize, declaredScale)),
    [modelSize, declaredScale],
  );

  // The open part's REST-POSE box in the same world space, so the camera can be aimed at one limb.
  // Rest pose and not the posed one: nothing on the wire reports where a clip has moved a part to,
  // and a box that silently meant "where this was at t = 0" would be worse than none.
  //
  // `entities.subject` is a dependency because the subject's translation is read off the baseline
  // ref, which a dial or a rig switch refills.
  const partSphere = useMemo<Sphere | null>(() => {
    if (part === null || assembly?.size == null || partDoc?.size == null || entities.subject === null) {
      return null;
    }
    const box = partWorldBox(partRestBox(part, partDoc.size), assembly.size, declaredScale);
    const at = vec3(subjectBaseRef.current.transform.translation) ?? [0, 0, 0];
    return boxSphere(vAdd(box.min, at), vAdd(box.max, at));
  }, [part, assembly, partDoc, declaredScale, entities.subject]);

  // ---------------------------------------------------------------------------------------------
  // The view camera. `selectionBounds` folds the framing mode and the reference figure in — the
  // hook itself knows nothing about either — so `F`/`Home` (no `homePose` given, so `Home` falls
  // back to the same framed pose `F` does) and the pivot an Alt-drag orbits around all agree with
  // what the toolbar's framing toggle is showing. An open part is always framed alone, never merged
  // with the reference: the mode is a judgment about the whole model.
  const selectionBounds = useCallback((): Sphere | null => {
    if (partSphere !== null) {
      return partSphere;
    }
    if (subjectSphere === null) {
      return null;
    }
    return activeFraming === 'declared-scale' && referenceSphere !== null
      ? mergeSpheres(subjectSphere, referenceSphere)
      : subjectSphere;
  }, [partSphere, subjectSphere, activeFraming, referenceSphere]);

  const camera = useViewCamera({
    client,
    viewId: props.viewId,
    scenePath: stageSceneName(rig),
    viewport,
    plainLeftOrbits: true,
    selectionBounds,
    baseFovYDeg: STAGE_FOV_Y_DEG,
  });
  // Pulled out so the callbacks below depend on the stable setters, not on `camera` itself — a fresh
  // object every render, since its `pose` updates every rAF tick while flying.
  const { pose: cameraPose, mode: cameraMode, goToPose, setMode: setCameraMode, toggleMode: toggleCameraMode } =
    camera;
  const cameraPoseRef = useRef<CameraPose>(cameraPose);
  cameraPoseRef.current = cameraPose;

  /** The pose that fits `sphere` under `mode`/`reference`, viewed from the camera's CURRENT world
   *  position — the same math `useViewCamera`'s own `frameSelection` uses, minus the mode switch: an
   *  automatic reframe (a new model, a new part, a framing toggle) must never override a human's
   *  persisted `Game` choice, only an explicit click does that (`handleSelect`, `applyPreset`). */
  const framedPoseFor = useCallback(
    (sphere: Sphere, mode: FramingMode, reference: Sphere | null): CameraPose => {
      const { target, distance } = framing(mode, sphere, reference, viewportRef.current);
      const state = orbitFromDirection(vSub(cameraPoseRef.current.translation, target), target, distance);
      return orbitPose(state);
    },
    [],
  );

  // Re-aim and re-fit whenever the subject, the open part, the framing mode, or the reference figure
  // changes — keeping the direction the human is currently looking from, the same continuity an
  // Alt-orbit gives. `fill-frame` reads SHAPE (every model the same apparent size); `declared-scale`
  // stands the model beside the reference at its true size — the size judgment a contact sheet
  // cannot give.
  useEffect(() => {
    const sphere = partSphere ?? subjectSphere;
    if (sphere === null) {
      return;
    }
    const mode = partSphere !== null ? 'fill-frame' : activeFraming;
    const reference = partSphere !== null ? null : referenceSphere;
    goToPose(framedPoseFor(sphere, mode, reference));
  }, [partSphere, subjectSphere, referenceSphere, activeFraming, framedPoseFor, goToPose]);

  const handleViewportSize = useCallback((size: ViewportSize): void => {
    setViewport(size);
  }, []);

  const applyPreset = useCallback(
    (id: PresetId): void => {
      const target = partSphere?.center ?? subjectSphere?.center ?? DEFAULT_PIVOT;
      // The preset keeps the camera's CURRENT distance from the pivot, so dollying in and then
      // picking a preset does not snap back out.
      const distance = vLen(vSub(cameraPoseRef.current.translation, target));
      goToPose(presetPose(id, target, distance));
      setCameraMode('view');
    },
    [partSphere, subjectSphere, goToPose, setCameraMode],
  );

  // The turntable: azimuth only, under the camera's current elevation and distance from the pivot.
  useEffect(() => {
    if (!turntable || client === null) {
      return;
    }
    const timer = window.setInterval(() => {
      const target = partSphere?.center ?? subjectSphere?.center ?? DEFAULT_PIVOT;
      const away = vSub(cameraPoseRef.current.translation, target);
      const state = orbitFromDirection(away, target, vLen(away));
      goToPose(orbitPose(turntableStep(state, TURNTABLE_DEG_PER_TICK)));
    }, TURNTABLE_TICK_MS);
    return () => window.clearInterval(timer);
  }, [turntable, client, partSphere, subjectSphere, goToPose]);

  /** Turning the turntable ON is a request to look at the spinning model; turning it off leaves the
   *  camera exactly where it stopped. */
  const toggleTurntable = useCallback((): void => {
    setTurntable((on) => {
      const next = !on;
      if (next) {
        setCameraMode('view');
      }
      return next;
    });
  }, [setCameraMode]);

  // ---------------------------------------------------------------------------------------------
  // The selected model: read it for the palette, check it for the readout, and put it on the stage.
  // A click is an instruction to look at something, so it also switches to the view camera — the
  // same reasoning `frameSelection` applies internally, applied here because the model's bounds
  // aren't known yet at click time (the readout arrives later, and reframes without switching mode —
  // see the effect above).
  const handleSelect = useCallback(
    (relPath: string): void => {
      setSelected(relPath);
      // A part name means nothing in another rig, and a stale one would frame the camera on a box
      // that is no longer on the stage.
      setPartName(null);
      setCameraMode('view');
      try {
        localStorage.setItem(MODEL_KEY, relPath);
      } catch {
        // Private-mode / storage-disabled: keep it for this session only.
      }
    },
    [setCameraMode],
  );

  // The engine's verdict, re-run whenever the DISK changed — a save of ours, or an edit by the
  // persona. Debounced, because the host does not (each call is a fresh `check-scene` subprocess).
  useEffect(() => {
    if (modelPath === null) {
      setReadout(null);
      setModelError(null);
      setChecking(false);
      return;
    }
    let cancelled = false;
    setChecking(true);
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const result = await runCheckScene(modelPath);
          if (!cancelled) {
            setReadout(parseCheckScene(result));
            setModelError(null);
          }
        } catch (err) {
          if (!cancelled) {
            setReadout(null);
            setModelError(String(err));
          }
        }
        if (!cancelled) {
          setChecking(false);
        }
      })();
    }, READOUT_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [modelPath, session.savedRevision]);

  // --- palette edits ---------------------------------------------------------------------------
  // Both go through the session, which re-derives the edit from the CURRENT document: a palette edit
  // moves offsets, so a precomputed result would be stale by the time it was applied. A drag streams
  // (`throttle` — watching the model change under torchlight IS the feature) and the release commits.
  // A new model opens on its FIRST palette entry, once, when its document arrives. A glyph does not
  // carry across models (palettes are per-model), and defaulting to the erase chip would make the
  // first drag on a fresh model destructive.
  const defaultedRef = useRef<string | null>(null);
  useEffect(() => {
    const first = doc?.palette[0]?.glyph;
    if (first !== undefined && defaultedRef.current !== selected) {
      defaultedRef.current = selected;
      setPaintGlyph(first);
    }
  }, [selected, doc]);

  const applyEdit = session.apply;
  const handleColor = useCallback(
    (glyph: string, rgb: readonly [number, number, number], commit: boolean): void => {
      applyEdit((current) => setPaletteColor(current, glyph, [rgb[0], rgb[1], rgb[2], null]), {
        write: commit ? 'commit' : 'throttle',
        coalesce: `color:${glyph}`,
      });
    },
    [applyEdit],
  );

  const handleMaterial = useCallback(
    (glyph: string, field: MaterialField, value: number, commit: boolean): void => {
      applyEdit((current) => setPaletteMaterial(current, glyph, field, value), {
        write: commit ? 'commit' : 'throttle',
        coalesce: `${field}:${glyph}`,
      });
    },
    [applyEdit],
  );

  // Put the selection on the stage. Whole-value replace on both components, over the committed
  // subject rather than over what the previous selection left. The scale stamp is the `scale` key's
  // whole purpose — placement-time application, never engine-applied.
  const applyModel = useCallback(
    async (c: ProtocolClient, subject: EntityId, relPath: string, scale: number): Promise<void> => {
      await c.call(componentSet(subject, 'Mesh', { ...subjectBaseRef.current.mesh, shape: modelShape(relPath) }));
      await c.call(
        componentSet(subject, 'Transform', { ...subjectBaseRef.current.transform, scale: [scale, scale, scale] }),
      );
    },
    [],
  );

  useEffect(() => {
    const subject = entities.subject;
    if (client === null || subject === null || selected === null) {
      return;
    }
    void applyModel(client, subject, selected, declaredScale).catch((err: unknown) =>
      setStageError(String(err)),
    );
  }, [client, entities.subject, selected, declaredScale, applyModel]);

  // ---------------------------------------------------------------------------------------------
  // The rig toggle: `scene.load` on the stage session. It is safe ONLY here — nothing in the stage
  // is ever edited — but it does replace the world, so the entity ids and the model are re-applied
  // afterwards. The view camera needs no re-send: `render.view` is host state, not the World's, so
  // it survives the reload untouched — only the stage's OWN scene changes the two rigs' lighting.
  const switchRig = useCallback(
    (next: Rig): void => {
      const remember = (value: Rig) => {
        setRig(value);
        try {
          localStorage.setItem(RIG_KEY, value);
        } catch {
          // Private-mode / quota: the rig just won't survive the next view switch.
        }
      };
      const c = clientRef.current;
      if (c === null || next === rig) {
        remember(next);
        return;
      }
      void (async () => {
        try {
          // The path is resolved against the content root — the directory of the scene the stage was
          // started with — so the basename is the whole reference, whatever the project dir looks
          // like on this machine.
          await c.call({ method: 'scene.load', params: { path: rigDef(next).file } });
          remember(next);
          // Fresh world, fresh entity ids; re-reading them is what re-applies the model (the effect
          // that keys on `entities.subject` does it).
          await readStage(c);
          setStageError(null);
        } catch (err) {
          setStageError(String(err));
        }
      })();
    },
    [rig, readStage],
  );

  // Drop the block when the view goes away, so a stale model path can't outlive the view that meant
  // it (the persona reads this file to resolve "this").
  useEffect(() => () => onVoxelContext(null), [onVoxelContext]);

  // The status bar's line: the engine's verdict verbatim, never paraphrased.
  useEffect(() => {
    if (selected === null) {
      onStatusNote(null);
      return;
    }
    onStatusNote(readout === null ? `${selected} · checking…` : `${selected} · ${readout.summary}`);
  }, [selected, readout, onStatusNote]);
  useEffect(() => () => onStatusNote(null), [onStatusNote]);

  // Memoized because `StageViewport`'s resize observer keys on it: a fresh object per render tore the
  // observer down and rebuilt it four times a second.
  const slot = useMemo(
    () =>
      stageSlot({
        isDesktop: isTauri(),
        projectDir,
        status,
        connected: client !== null,
        error: stageError,
      }),
    [projectDir, status, client, stageError],
  );

  const toggleMaximize = useCallback(() => setMaximized((m) => !m), []);

  // The open part's own library path — what the slice panel's header names, and the path the persona
  // would open.
  const partRelPath = selected !== null && part !== null ? partFilePath(selected, part.file) : null;

  // Clip bindings resolve against the assembly's own directory, inside the same library root the
  // selection is relative to. Memoized: the hook re-reads the clip when this identity changes.
  const readClipFile = useCallback(
    (relPath: string) => readEditorFile(modelFilePath(projectDir, relPath)),
    [projectDir],
  );
  const playback = useClipPlayback({
    client,
    subjectId: entities.subject,
    clips: assembly?.clips ?? NO_CLIPS,
    ownerRelPath: assembly === null ? null : selected,
    readFile: readClipFile,
  });

  // The paused half of the `assembly` context block, isolated from `playback.tick` itself: that tick
  // moves every sim frame while a clip plays, and folding it straight into the memo below would defeat
  // the emit throttle that follows (a fresh object every frame is a fresh reason to re-schedule it).
  const pausedTick = playback.playing ? null : playback.tick;

  // The `assembly` half of the voxel context block (decision 6, `Plan/editor-voxel-assemblies.md`) —
  // `undefined`, not `null`, when the stage holds a plain voxel model, so that model's own block stays
  // byte-identical to what it was before assemblies existed.
  const assemblyContext = useMemo<ContextVoxelAssembly | undefined>(() => {
    if (!isAssembly || selected === null) {
      return undefined;
    }
    return {
      path: `scenes/voxels/${selected}`,
      part:
        part === null || partRelPath === null
          ? null
          : { name: part.name, path: `scenes/voxels/${partRelPath}` },
      clip: playback.clip,
      tick: pausedTick,
      seconds: pausedTick === null ? null : secondsOf(pausedTick),
      playing: playback.playing,
    };
  }, [isAssembly, selected, part, partRelPath, playback.clip, playback.playing, pausedTick]);

  // ---------------------------------------------------------------------------------------------
  // The context bridge's `voxel` block. THROTTLED, not debounced: the pose changes continuously
  // under an orbit drag and permanently under the turntable, and a debounce would keep rescheduling
  // so the persona would never see one. `modelPath` is project-relative because the persona's cwd is
  // the project, so it opens with no further resolution.
  useEffect(() => {
    const timer = window.setTimeout(
      () => {
        lastEmitRef.current = performance.now();
        onVoxelContext({
          modelPath: selected === null ? null : `scenes/voxels/${selected}`,
          rig,
          cameraPose: { translation: cameraPose.translation, rotation: cameraPose.rotation },
          ...(assemblyContext === undefined ? {} : { assembly: assemblyContext }),
        });
      },
      emitDelay(lastEmitRef.current, performance.now()),
    );
    return () => window.clearTimeout(timer);
  }, [selected, rig, cameraPose, assemblyContext, onVoxelContext]);

  return (
    <div className="main-row">
      {/* Left column: the asset browser over the model panel, and the parts of an assembly under it. */}
      <div className="col col-side">
        <AssetBrowser
          root={voxels}
          ext={MODEL_EXTS}
          selected={selected}
          onSelect={handleSelect}
          reloadKey={session.savedRevision}
          // `parts/` and `anims/` hold nothing this library lists, so they are not categories here.
          prune
        />
        <ModelPanel
          relPath={selected}
          readout={readout}
          doc={isAssembly ? assemblyPalette : doc}
          checking={checking}
          error={modelError ?? session.loadError ?? (isAssembly ? assemblyFile.error : null)}
          paintGlyph={paintGlyph}
          onPaintGlyph={setPaintGlyph}
          editable={session.editable && !session.conflict}
          // Omitted for an assembly: no callbacks, no editor, no write path to reach.
          onColor={isAssembly ? undefined : handleColor}
          onMaterial={isAssembly ? undefined : handleMaterial}
          onJudgeUnderNight={rig === 'night' ? null : () => switchRig('night')}
          readOnlyNote={
            isAssembly
              ? 'Assemblies are read-only here — edit the file; the stage reloads on save.'
              : null
          }
          stagedScale={isAssembly ? declaredScale : null}
        />
        {isAssembly && assembly !== null && (
          <PartsPanel
            assembly={assembly}
            selected={partName}
            onSelect={setPartName}
            partSize={partDoc?.size ?? null}
            error={partFile.error}
          />
        )}
      </div>

      {/* Centre column: the shared vertical split, stage viewport over the slice painter. */}
      <div className="col-center vx-center">
        <div className="vx-toolbar">
          <div className="segmented vx-presets">
            {PRESETS.map((preset) => (
              <button key={preset.id} type="button" title={preset.label} onClick={() => applyPreset(preset.id)}>
                {preset.short}
              </button>
            ))}
          </div>
          <div className="vx-toggles">
            <span className="vx-toggle-label">rig</span>
            <div className="segmented">
              {RIGS.map((entry) => (
                <button
                  key={entry.id}
                  type="button"
                  className={entry.id === rig ? 'active' : undefined}
                  onClick={() => switchRig(entry.id)}
                >
                  {entry.label}
                </button>
              ))}
            </div>
            {!isAssembly && (
              <>
                <span className="vx-toggle-label">framing</span>
                <div className="segmented">
                  <button
                    type="button"
                    className={framingMode === 'fill-frame' ? 'active' : undefined}
                    title="Frame the subject alone — every model the same apparent size (shape reading)."
                    onClick={() => setFramingMode('fill-frame')}
                  >
                    fill frame
                  </button>
                  <button
                    type="button"
                    className={framingMode === 'declared-scale' ? 'active' : undefined}
                    title="Stand the model beside the adventurer at its declared scale (size reading)."
                    onClick={() => setFramingMode('declared-scale')}
                  >
                    declared scale
                  </button>
                </div>
              </>
            )}
            <button type="button" className={turntable ? 'icon-btn active' : 'icon-btn'} onClick={toggleTurntable}>
              turntable
            </button>
            <button
              type="button"
              className={cameraMode === 'view' ? 'icon-btn active' : 'icon-btn'}
              onClick={toggleCameraMode}
              title="G — toggle the in-game camera"
            >
              {cameraMode === 'view' ? 'View' : 'Game'}
            </button>
            {readout !== null && readout.scale !== null && (
              <span className="vx-toolbar-note">
                scale <span className="num">{readout.scale}</span>
              </span>
            )}
            {part !== null && (
              <span className="vx-toolbar-note">
                framed on <span className="num">{part.name}</span> — rest pose
              </span>
            )}
            <span className="vx-toolbar-note">
              drag to orbit · hold RMB to fly (WASD, wheel = speed) · Alt+drag to orbit/dolly/pan · F frames · Home
              resets
            </span>
          </div>
        </div>
        <DiagnosticBanner diagnostic={camera.error} onDismiss={camera.dismissError} />

        <VerticalSplit
          className="vx-split"
          maximized={maximized}
          onRestore={() => setMaximized(false)}
          hiddenLabel="Stage hidden"
          restoreLabel="Restore viewport"
          top={
            <>
              <StageViewport
                client={client}
                slot={slot}
                onRestart={restart}
                onAddScenes={addScenes}
                handlers={camera.handlers}
                onViewportSize={handleViewportSize}
                onFrameStatus={props.onFrameStatus}
                label={`${stageSceneName(rig)} · ${selected === null ? 'no model' : modelDisplayName(selected)}${
                  part === null ? '' : ` · ${part.name}`
                }`}
              />
              {assembly !== null && <ClipStrip {...playback} />}
            </>
          }
          bottom={
            isAssembly ? (
              <SlicePainterPanel
                head="Part slices"
                relPath={partRelPath}
                doc={painterDoc}
                glyph={paintGlyph}
                // No session, no write path: every editing prop here is inert by construction.
                editable={false}
                apply={NO_APPLY}
                canUndo={false}
                canRedo={false}
                onUndo={NO_OP}
                onRedo={NO_OP}
                conflict={false}
                onReload={NO_OP}
                onKeepMine={NO_OP}
                editError={null}
                maximized={maximized}
                onToggleMaximize={toggleMaximize}
                readOnlyNote="read-only — the part as authored, before mirroring"
                pivot={part?.pivot ?? null}
                emptyState={
                  part === null
                    ? {
                        title: 'No part selected',
                        detail:
                          'Pick a part in the hierarchy to open its grid here under the assembly’s palette, and to frame the camera on it.',
                      }
                    : null
                }
              />
            ) : (
              <SlicePainterPanel
                relPath={selected}
                doc={doc}
                glyph={paintGlyph}
                editable={session.editable}
                apply={session.apply}
                canUndo={session.canUndo}
                canRedo={session.canRedo}
                onUndo={session.undo}
                onRedo={session.redo}
                conflict={session.conflict}
                onReload={session.reload}
                onKeepMine={session.keepMine}
                editError={session.editError}
                maximized={maximized}
                onToggleMaximize={toggleMaximize}
              />
            )
          }
        />
      </div>

      {/* Right column: the persona terminal drawer, dialing the STAGE session. */}
      <div className="col col-term">
        <TerminalDrawer
          url={status.url ?? FALLBACK_STAGE_URL}
          viewId={props.viewId}
          scenePath={stageSceneName(rig)}
          projectDir={projectDir}
          projectName={props.projectName}
          onOpenProjectSwitcher={props.onOpenProjectSwitcher}
        />
      </div>
    </div>
  );
}
