// The Models view (`Plan/mesh-modeling.md`, renamed under `Plan/editor-stage-camera.md`
// decision 8): browse the project's `kind: sculpt` models, put one on the model stage, and judge the
// surface the mesher actually produced.
//
// IT IS THE VOXEL ARTIST'S STAGE. A sculpt is the second `Mesh.shape` model kind, so this view
// starts the same host slot (`model`) on the same committed scene (`model-stage.yaml`) and swaps the
// subject by `component.set` of `Mesh.shape.sculpt` exactly as the Voxel Artist sets
// `Mesh.shape.voxel`. Nothing about the stage is sculpt-shaped; only the library is.
//
// THREE LIBRARIES, ONE SUBJECT. Beside the sculpts it lists the project's `kind: mesh` documents —
// imported and generated geometry, the other `Mesh.shape` asset kind — and its skinned `kind: assembly`
// documents (`mesh:`), a skinned mesh with its bound clips (`Mesh.shape.assembly`). That library lists
// voxel assemblies too, which are not staged here. The stage holds exactly one subject, so the three
// browsers share one selection.
//
// A CHARACTER IS PREVIEWED THROUGH THE VOXEL ARTIST'S CLIP STRIP. Its clips are read from the
// character file and played by the same `useClipPlayback` / `ClipStrip` — `component.set` of
// `Animation`, `sim.pause` / `step` — so the preview needs no wire method of its own
// (`Plan/animation-clips.md` decision 18).
//
// LAYOUT — the Voxel Artist's, minus every editor a sculpt does not have yet: the two asset browsers
// over the document panel on the LEFT, the stage viewport in the CENTRE, the terminal column on the
// RIGHT. No slice painter, no palette editor — a sculpt is fields, not cells.
//
// THE SESSION IS NOT THE GAME'S. This view never touches App's `ProtocolClient`; it owns one dialed
// at the host-spawned stage session's url once that session reports `listening`. The game server may
// not even be running.
//
// The view NEVER calls `asset.reload`: the host's watcher does the reloading, so a
// save by the persona — or by the document panel, once it edits — is what updates the viewport, and
// a rejected file blanks the subject until the next good save.
//
// SCULPTS CARRY NO DECLARED SCALE. Model space is world units at `Transform.scale: 1`
// (`ludion docs sculpt-format`), so the stage scene's placement is the only scale there is — and
// selecting a sculpt restores exactly that, the committed subject's own scale. The invariant is
// unchanged; it is now written rather than assumed, because a mesh's authored scale precedes it.
//
// A MESH'S PLACEMENT IS AUTHORED, AND IT IS LIFTED WHOLE. `ludion import` writes the mesh document
// and the scene node that places it, and the material lives on that node (the generated idol's
// `metallic: 1.0`). Selecting a mesh therefore reads the nearest scene that references it and writes
// that node's whole `Mesh` — plus its `Transform.scale`, because the stage's subject stands at the
// voxel library's placeholder scale and an unstamped mesh would arrive a quarter size.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ResponseBody } from '@ludion/protocol';
import { ProtocolClient } from '../protocol/client';
import { componentSet } from '../protocol/commands';
import { isTauri } from '../shell/tauri';
import { listEditorDir, onStageStatus, stageRestart, stageStart, stageStatus, type StageStatus } from '../shell/host';
import { mountStage } from '../shell/stageMount';
import { addStageScenes, readEditorFile } from '../terminal/pty';
import { TerminalDrawer } from '../terminal/TerminalDrawer';
import type { FrameStatus } from '../viewer/useFrameStream';
import { DiagnosticBanner } from '../viewer/DiagnosticBanner';
import type { EntityId } from '../editor/undo';
import { parseHierarchy } from '../editor/hierarchy';
import { AssetBrowser } from '../editor/voxel/AssetBrowser';
import { StageViewport } from '../editor/voxel/StageViewport';
import { useViewCamera } from '../editor/camera/useViewCamera';
import {
  presetPose,
  PRESETS,
  STAGE_FOV_Y_DEG,
  type CameraPose,
  type PresetId,
  type Viewport as ViewportSize,
} from '../editor/voxel/orbit';
import { stageSceneName, stageScenePath, stageSlot, type Rig } from '../editor/voxel/stage';
import {
  SCULPT_EXT,
  sculptAssetRef,
  sculptDisplayName,
  sculptFilePath,
  sculptsRoot,
} from '../editor/sculpt/stage';
import { SculptInspector } from '../editor/sculpt/SculptInspector';
import { useSculptEdit } from '../editor/sculpt/useSculptEdit';
import {
  MESH_EXT,
  meshAssetRef,
  meshDisplayName,
  meshFilePath,
  meshesRoot,
} from '../editor/mesh/stage';
import {
  characterAssetRef,
  characterDisplayName,
  isSkinned,
  parseCharacterDoc,
  type CharacterDoc,
} from '../editor/mesh/character';
import { candidatePlacements, liftMesh, type MeshLift } from '../editor/mesh/lift';
import { ClipStrip } from '../editor/voxel/ClipStrip';
import { useClipPlayback } from '../editor/voxel/useClipPlayback';
import { ASSEMBLY_EXT, MODEL_POLL_MS } from '../editor/voxel/stage';
import type { ClipBinding } from '../editor/voxel/assembly';
import { usePolling } from '../viewer/usePolling';
import { vLen, vSub, type Vec3 } from '../editor/gizmo/vecmath';

/** The stage session's url when it has not announced one yet — the address the host binds by
 *  default. The drawer needs *some* url at spawn so the persona's `ludion-mcp` dials the stage
 *  rather than the game. */
const FALLBACK_STAGE_URL = 'ws://127.0.0.1:9412';

/** The last selection, so switching views (which unmounts this one) doesn't lose the model on the
 *  stage. Stored as `<kind>:<relPath>`. The value keeps its pre-rename name (this was the Sculpt
 *  view) so an existing stored selection survives the view's rename to Models. */
const MODEL_KEY = 'ludion.editor.sculptModel';

/** What stands on the stage, and which library named it. The stage has exactly one subject, so the
 *  two browsers share this rather than each holding a selection that silently loses. */
interface StageModel {
  readonly kind: 'sculpt' | 'mesh' | 'character';
  /** Relative to that library's root — `sculptsRoot` or `meshesRoot` (a character's too). */
  readonly relPath: string;
}

/** The viewport's name for what is standing there. */
function stageLabel(model: StageModel | null): string {
  if (model === null) {
    return 'no model';
  }
  switch (model.kind) {
    case 'sculpt':
      return sculptDisplayName(model.relPath);
    case 'mesh':
      return meshDisplayName(model.relPath);
    case 'character':
      return characterDisplayName(model.relPath);
  }
}

function readStoredModel(): StageModel | null {
  let stored: string | null = null;
  try {
    stored = localStorage.getItem(MODEL_KEY);
  } catch {
    return null;
  }
  if (stored === null) {
    return null;
  }
  const cut = stored.indexOf(':');
  const kind = cut < 0 ? '' : stored.slice(0, cut);
  // A value written before the Meshes library existed is a bare sculpt path.
  return kind === 'sculpt' || kind === 'mesh' || kind === 'character'
    ? { kind, relPath: stored.slice(cut + 1) }
    : { kind: 'sculpt', relPath: stored };
}

/** The rig this view stands the subject under. The Voxel Artist owns the neutral/night toggle; a
 *  sculpt is judged for SURFACE, which is what the neutral key+fill was tuned to read. */
const RIG: Rig = 'neutral';

/** The one named entity this view reads. Names are load-bearing: `query.entities` is how it finds
 *  it. The stage's `camera` entity is no longer read or written here — the view camera drives the
 *  stream through `render.view`, never `component.set` (`Plan/editor-stage-camera.md` decision 4). */
const SUBJECT = 'subject';

interface StageEntities {
  readonly subject: EntityId | null;
}

const NO_ENTITIES: StageEntities = { subject: null };

const NO_CLIPS: readonly ClipBinding[] = [];

/** The stage subject's two components as the committed scene authors them. */
interface SubjectBase {
  readonly mesh: Record<string, unknown>;
  readonly transform: Record<string, unknown>;
}

const EMPTY_SUBJECT_BASE: SubjectBase = { mesh: {}, transform: {} };

const STOPPED: StageStatus = {
  slot: 'model',
  phase: 'stopped',
  url: null,
  scene: null,
  message: null,
  code: null,
  listener: null,
};

/** The pivot every preset and the home pose frames — the stage subject's rough centre, in the
 *  absence of any real bounding-box data in this view (unlike the Voxel Artist, this view has no
 *  cheap size figure for a mesh). */
const HOME_TARGET: Vec3 = [0, 2, 0];
const HOME_DISTANCE = 7;

/** The committed FRONT pose of `model-stage.yaml` — `Home`'s destination. */
function homePose(): CameraPose {
  return presetPose('front', HOME_TARGET, HOME_DISTANCE);
}

export interface ModelsViewProps {
  readonly projectDir: string;
  /** The current project's name, validated through `project_info`; `null` before it resolves. */
  readonly projectName: string | null;
  readonly onOpenProjectSwitcher: () => void;
  readonly viewId: string;
  readonly onFrameStatus: (status: FrameStatus) => void;
  /** The one-line model readout for the status bar. */
  readonly onStatusNote: (note: string | null) => void;
}

/** The `value` of a `component.get` response, or `null`. `component.set` is a whole-value replace,
 *  so every write has to start from a complete component — one that dropped `color` would silently
 *  retint the subject. */
function componentValue(body: ResponseBody): Record<string, unknown> | null {
  const b = body as { kind?: unknown; value?: unknown };
  if (b.kind !== 'component_value' || typeof b.value !== 'object' || b.value === null) {
    return null;
  }
  return b.value as Record<string, unknown>;
}

export function ModelsView(props: ModelsViewProps) {
  const { projectDir, onStatusNote } = props;

  // --- the stage session ------------------------------------------------------------------------
  const [status, setStatus] = useState<StageStatus>(STOPPED);
  const [stageError, setStageError] = useState<string | null>(null);
  const [client, setClient] = useState<ProtocolClient | null>(null);
  const clientRef = useRef<ProtocolClient | null>(null);
  clientRef.current = client;
  const [entities, setEntities] = useState<StageEntities>(NO_ENTITIES);
  const entitiesRef = useRef<StageEntities>(NO_ENTITIES);
  entitiesRef.current = entities;
  // The subject as the committed stage scene authors it. EVERY model is written over this, never
  // over what the previous selection left: what stands on the stage is a function of the current
  // selection and the committed scene, never of the click history. Without it a mesh's authored
  // `emissive` would follow the next sculpt onto the stage and quietly falsify the judgment the
  // stage exists for.
  const subjectBaseRef = useRef<SubjectBase>(EMPTY_SUBJECT_BASE);

  // --- the model --------------------------------------------------------------------------------
  const [model, setModel] = useState<StageModel | null>(readStoredModel);
  // What the lift could not do plainly: a multi-node import, or a mesh no scene or prefab places.
  const [meshNotice, setMeshNotice] = useState<string | null>(null);
  const selectedSculpt = model?.kind === 'sculpt' ? model.relPath : null;
  const selectedMesh = model?.kind === 'mesh' ? model.relPath : null;
  const selectedCharacter = model?.kind === 'character' ? model.relPath : null;
  // The staged character's file as last read; re-read on the model poll so an edited overlay shows.
  // Tagged with its path, so a selection is never judged by the previous file's document.
  const [characterRead, setCharacterRead] = useState<{ readonly path: string; readonly doc: CharacterDoc } | null>(
    null,
  );
  const characterSourceRef = useRef<string | null>(null);

  // --- the camera -------------------------------------------------------------------------------
  // The canvas's CSS size, as a value: `useViewCamera` takes it as a plain prop, not a ref.
  const [viewport, setViewport] = useState<ViewportSize>({ width: 16, height: 9 });

  const camera = useViewCamera({
    client,
    viewId: props.viewId,
    scenePath: stageSceneName(RIG),
    viewport,
    plainLeftOrbits: true,
    baseFovYDeg: STAGE_FOV_Y_DEG,
    homePose,
  });
  // Pulled out so the callbacks below depend on the two stable setters, not on `camera` itself —
  // which is a fresh object every render (its `pose` updates every rAF tick while flying).
  const { goToPose, setMode: setCameraMode } = camera;
  const cameraPoseRef = useRef<CameraPose>(camera.pose);
  cameraPoseRef.current = camera.pose;

  const characterPath = useMemo(
    () => (projectDir === '' || selectedCharacter === null ? null : meshFilePath(projectDir, selectedCharacter)),
    [projectDir, selectedCharacter],
  );
  const characterDoc = characterRead !== null && characterRead.path === characterPath ? characterRead.doc : null;
  // `null` until the selected assembly has been read.
  const stagedSkinned = characterDoc === null ? null : isSkinned(characterDoc);

  const sculpts = useMemo(() => (projectDir === '' ? '' : sculptsRoot(projectDir)), [projectDir]);
  const meshes = useMemo(() => (projectDir === '' ? '' : meshesRoot(projectDir)), [projectDir]);
  const modelPath = useMemo(
    () => (projectDir === '' || selectedSculpt === null ? null : sculptFilePath(projectDir, selectedSculpt)),
    [projectDir, selectedSculpt],
  );

  // The editing session owns the file: the working document the inspector draws, its throttled
  // writes, and the conflict policy against the persona editing the same file. Each write is what
  // updates the viewport — the host's watcher turns it into the stage's `asset.reload`.
  const session = useSculptEdit(modelPath);

  // ---------------------------------------------------------------------------------------------
  // Stage lifecycle. `stageStart` is idempotent, so every mount just asks; readiness arrives as a
  // `listening` event (a cold start builds first, which is why this can never be a promise).
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
      start: () => stageStart('model', { scene: stageScenePath(projectDir, RIG) }),
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
    void stageRestart('model', { scene: stageScenePath(projectDir, RIG) })
      .then(setStatus)
      .catch((err: unknown) => setStageError(String(err)));
  }, [projectDir]);

  const addScenes = useCallback(() => {
    if (!isTauri() || projectDir === '') {
      return;
    }
    setStageError(null);
    void addStageScenes(projectDir)
      .then(() => restart())
      .catch((err: unknown) => setStageError(String(err)));
  }, [projectDir, restart]);

  // Dial the stage once it is listening: a fresh `ProtocolClient` per url, closed on teardown, held
  // in state so the frame stream's effects (keyed on `[client]`) open and close with it.
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
  // Reading the stage: the one named entity, and the subject's baseline. `render.view` is host
  // state, not the World's, so there is no camera entity to look up any more.
  //
  // THE BASELINE IS THE COMMITTED SCENE, AND IT IS READ BEFORE ANY SELECTION IS APPLIED. The base is
  // captured before `entities` is published, because the selection effect keys on `entities.subject`
  // — publishing first would let a model be written over whatever the previous one left. A fresh
  // dial (a restart among them) re-enters this effect with a new client and re-reads it, so a base
  // can never outlive the session it was read from.
  useEffect(() => {
    if (client === null) {
      setEntities(NO_ENTITIES);
      entitiesRef.current = NO_ENTITIES;
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const body = await client.call({
          method: 'query.entities',
          params: { filter: [], include: ['Name'] },
        });
        const rows = parseHierarchy(body);
        const byName = (name: string): EntityId | null => rows.find((row) => row.name === name)?.entity ?? null;
        const found: StageEntities = { subject: byName(SUBJECT) };
        if (cancelled) {
          return;
        }
        if (found.subject !== null) {
          const [mesh, transform] = await Promise.all([
            client.call({ method: 'component.get', params: { entity: found.subject, component: 'Mesh' } }),
            client.call({
              method: 'component.get',
              params: { entity: found.subject, component: 'Transform' },
            }),
          ]);
          subjectBaseRef.current = {
            mesh: componentValue(mesh) ?? {},
            transform: componentValue(transform) ?? {},
          };
        }
        if (cancelled) {
          return;
        }
        setEntities(found);
        entitiesRef.current = found;
      } catch (err) {
        if (!cancelled) {
          setStageError(String(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client]);

  const handleViewportSize = useCallback((size: ViewportSize): void => {
    setViewport(size);
  }, []);

  const applyPreset = useCallback(
    (id: PresetId): void => {
      // The pivot every preset frames from: a fixed point, since this view has no bounding-box data
      // for what is on stage. The distance is the camera's CURRENT distance from it, so dollying in
      // and then picking a preset keeps the zoom.
      const distance = vLen(vSub(cameraPoseRef.current.translation, HOME_TARGET));
      goToPose(presetPose(id, HOME_TARGET, distance));
      setCameraMode('view');
    },
    [goToPose, setCameraMode],
  );

  // ---------------------------------------------------------------------------------------------
  // The selection. A click is an instruction to look at something, so it also switches to the view
  // camera — the same reasoning `useViewCamera`'s own `frameSelection` applies (decision 1's stages
  // have no bounds to frame from here, so the switch happens at the click instead).
  const select = useCallback(
    (next: StageModel): void => {
      setModel(next);
      setCameraMode('view');
      try {
        localStorage.setItem(MODEL_KEY, `${next.kind}:${next.relPath}`);
      } catch {
        // Private-mode / storage-disabled: keep it for this session only.
      }
    },
    [setCameraMode],
  );

  const handleSelectSculpt = useCallback(
    (relPath: string): void => select({ kind: 'sculpt', relPath }),
    [select],
  );
  const handleSelectMesh = useCallback((relPath: string): void => select({ kind: 'mesh', relPath }), [select]);
  const handleSelectCharacter = useCallback(
    (relPath: string): void => select({ kind: 'character', relPath }),
    [select],
  );

  /** Stamps `scale` on the subject, whole-value replace over what is there. */
  const stampScale = useCallback(async (c: ProtocolClient, subject: EntityId, scale: unknown): Promise<void> => {
    const transform = await c.call({
      method: 'component.get',
      params: { entity: subject, component: 'Transform' },
    });
    await c.call(componentSet(subject, 'Transform', { ...(componentValue(transform) ?? {}), scale }));
  }, []);

  /** The committed subject's scale — a `Transform` authoring none is already at the component's own
   *  default. */
  const baseScale = useCallback((): unknown => subjectBaseRef.current.transform.scale ?? [1, 1, 1], []);

  const applySculpt = useCallback(
    async (c: ProtocolClient, subject: EntityId, relPath: string): Promise<void> => {
      await c.call(
        componentSet(subject, 'Mesh', { ...subjectBaseRef.current.mesh, shape: { sculpt: sculptAssetRef(relPath) } }),
      );
      await stampScale(c, subject, baseScale());
    },
    [stampScale, baseScale],
  );

  /** Puts a character on the stage at rest: `Mesh.shape.assembly` over the committed subject's
   *  `Mesh`, at unit scale — the skin's own size is authored in the glb. */
  const applyCharacter = useCallback(
    async (c: ProtocolClient, subject: EntityId, relPath: string): Promise<void> => {
      await c.call(
        componentSet(subject, 'Mesh', { ...subjectBaseRef.current.mesh, shape: { assembly: characterAssetRef(relPath) } }),
      );
      await stampScale(c, subject, [1, 1, 1]);
    },
    [stampScale],
  );

  /** Puts the committed subject back, so an unstaged selection never leaves the previous model standing. */
  const applyBase = useCallback(
    async (c: ProtocolClient, subject: EntityId): Promise<void> => {
      await c.call(componentSet(subject, 'Mesh', subjectBaseRef.current.mesh));
      await stampScale(c, subject, baseScale());
    },
    [stampScale, baseScale],
  );

  /** The scene node that places `relPath`, with the scene it came from — read lazily, nearest
   *  ancestor directory first, stopping at the first match so the project's own scenes stay unread.
   *
   *  The listing is re-walked per selection rather than shared with the browser, which would mean
   *  threading its state back out through `AssetBrowser`. A content root is tens of directories and
   *  this runs on a click; the walk is cheaper than the coupling. */
  const liftPlacement = useCallback(
    async (relPath: string): Promise<{ readonly scene: string; readonly lift: MeshLift } | null> => {
      const entries = await listEditorDir(meshes, '.yaml');
      for (const scene of candidatePlacements(relPath, entries)) {
        const lift = liftMesh(await readEditorFile(meshFilePath(projectDir, scene)), scene, relPath);
        if (lift !== null) {
          return { scene, lift };
        }
      }
      return null;
    },
    [projectDir, meshes],
  );

  /** Puts a mesh on the stage under its own authored material and scale, and answers what could not
   *  be said plainly. Whole-value replace on both components, the Voxel Artist's idiom. */
  const applyMesh = useCallback(
    async (c: ProtocolClient, subject: EntityId, relPath: string): Promise<string | null> => {
      const placed = await liftPlacement(relPath);
      // `shape` last: the lifted one is named relative to its own scene's directory, not the
      // stage's content root.
      await c.call(
        componentSet(subject, 'Mesh', {
          ...subjectBaseRef.current.mesh,
          ...(placed?.lift.mesh ?? {}),
          shape: { asset: meshAssetRef(relPath) },
        }),
      );
      // No placement means no authored scale either, and a mesh document's model space is world
      // units — the same contract a sculpt has, and what the importer's own scene writes.
      await stampScale(c, subject, placed?.lift.scale ?? [1, 1, 1]);
      if (placed === null) {
        return 'No scene or prefab places this mesh, so it is standing under the stage’s committed material at unit scale.';
      }
      return placed.lift.meshNodes > 1
        ? `${placed.scene} places ${placed.lift.meshNodes} mesh nodes — showing the first.`
        : null;
    },
    [liftPlacement, stampScale],
  );

  useEffect(() => {
    const subject = entities.subject;
    if (client === null || subject === null || model === null) {
      return;
    }
    let cancelled = false;
    void (async () => {
      if (model.kind === 'sculpt') {
        setMeshNotice(null);
        await applySculpt(client, subject, model.relPath);
        return;
      }
      if (model.kind === 'character') {
        setMeshNotice(null);
        if (stagedSkinned === true) {
          await applyCharacter(client, subject, model.relPath);
        } else if (stagedSkinned === false) {
          await applyBase(client, subject);
        }
        return;
      }
      const notice = await applyMesh(client, subject, model.relPath);
      if (!cancelled) {
        setMeshNotice(notice);
      }
    })().catch((err: unknown) => {
      if (!cancelled) {
        setStageError(String(err));
      }
    });
    return () => {
      cancelled = true;
    };
  }, [client, entities.subject, model, stagedSkinned, applySculpt, applyMesh, applyCharacter, applyBase]);

  // The status bar's line: what is on the stage, and nothing the engine did not say.
  const unstaged = stagedSkinned === false;
  const assetRef =
    model === null || unstaged
      ? null
      : model.kind === 'sculpt'
        ? sculptAssetRef(model.relPath)
        : model.kind === 'character'
          ? characterAssetRef(model.relPath)
          : meshAssetRef(model.relPath);
  useEffect(() => {
    onStatusNote(assetRef);
  }, [assetRef, onStatusNote]);
  useEffect(() => () => onStatusNote(null), [onStatusNote]);

  // --- the character's clips -------------------------------------------------------------------
  useEffect(() => {
    characterSourceRef.current = null;
    setCharacterRead(null);
  }, [characterPath]);
  const readCharacter = useCallback(async (): Promise<void> => {
    if (characterPath === null) {
      return;
    }
    try {
      const text = await readEditorFile(characterPath);
      if (text !== characterSourceRef.current) {
        characterSourceRef.current = text;
        setCharacterRead({ path: characterPath, doc: parseCharacterDoc(text) });
      }
    } catch {
      // A file caught mid-save reads as missing for an instant; the next poll sees the new bytes.
    }
  }, [characterPath]);
  usePolling(characterPath !== null, MODEL_POLL_MS, readCharacter);

  // Clip files resolve against the character file's own directory, inside the content root.
  const readClipFile = useCallback(
    (relPath: string) => readEditorFile(meshFilePath(projectDir, relPath)),
    [projectDir],
  );
  const playback = useClipPlayback({
    client,
    subjectId: entities.subject,
    clips: stagedSkinned === true ? (characterDoc?.clips ?? NO_CLIPS) : NO_CLIPS,
    ownerRelPath: selectedCharacter,
    readFile: readClipFile,
  });

  // Memoized because `StageViewport`'s resize observer keys on it: a fresh object per render tore the
  // observer down and rebuilt it several times a second.
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

  return (
    <div className="main-row">
      {/* Left column: the two libraries over the document panel. */}
      <div className="col col-side">
        <AssetBrowser
          root={sculpts}
          ext={SCULPT_EXT}
          selected={selectedSculpt}
          onSelect={handleSelectSculpt}
          reloadKey={session.savedRevision}
          head="Sculpts"
          counted="sculpts"
        />

        {/* The mesh library is rooted at the content root itself, so it is pruned: the folders with
            no mesh in them are the rest of the game. */}
        <AssetBrowser
          root={meshes}
          ext={MESH_EXT}
          selected={selectedMesh}
          onSelect={handleSelectMesh}
          prune
          head="Meshes"
          counted="meshes"
        />
        <AssetBrowser
          root={meshes}
          ext={ASSEMBLY_EXT}
          selected={selectedCharacter}
          onSelect={handleSelectCharacter}
          prune
          head="Assemblies"
          counted="assemblies"
        />
        {unstaged && <div className="insp-hint">a voxel assembly — open it in the Voxel Artist</div>}
        {selectedCharacter !== null && stageError !== null && <div className="vx-error">{stageError}</div>}
        {selectedMesh !== null && meshNotice !== null && <div className="insp-hint">{meshNotice}</div>}
        {selectedMesh !== null && stageError !== null && <div className="vx-error">{stageError}</div>}

        {/* The document panel: what is on the stage, where its file is, and a slider per number in
            it. Every drag writes the file; the host's watcher does the reload. */}
        <section className="inspector vx-model">
          <div className="panel-head">Sculpt</div>
          <div className="insp-scroll">
            {selectedSculpt === null ? (
              <div className="insp-hint">Select a sculpt to put it on the stage.</div>
            ) : (
              <div className="vx-model-body">
                <div className="vx-path" title={modelPath ?? selectedSculpt}>
                  {sculptAssetRef(selectedSculpt)}
                </div>

                {session.conflict && (
                  <div className="code-banner" role="alert">
                    <span className="code-banner-text">
                      This model changed on disk while you have unsaved edits. Writes are held until you
                      choose.
                    </span>
                    <button type="button" className="icon-btn" onClick={session.reload}>
                      Reload
                    </button>
                    <button type="button" className="icon-btn" onClick={session.keepMine}>
                      Keep mine
                    </button>
                  </div>
                )}

                {session.loadError !== null && <div className="vx-error">{session.loadError}</div>}
                {session.writeError !== null && <div className="vx-error">{session.writeError}</div>}
                {stageError !== null && <div className="vx-error">{stageError}</div>}

                {session.doc !== null && (
                  <SculptInspector
                    doc={session.doc}
                    onChange={session.setScalarAt}
                    disabled={!session.editable || session.conflict}
                  />
                )}
              </div>
            )}
          </div>
        </section>
      </div>

      {/* Centre column: the stage's frame stream under the view camera. */}
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
            <button
              type="button"
              className={camera.mode === 'view' ? 'icon-btn active' : 'icon-btn'}
              onClick={camera.toggleMode}
              title="G — toggle the in-game camera"
            >
              {camera.mode === 'view' ? 'View' : 'Game'}
            </button>
            <span className="vx-toolbar-note">
              drag to orbit · hold RMB to fly (WASD, wheel = speed) · Alt+drag to orbit/dolly/pan · Home resets
            </span>
          </div>
        </div>
        <DiagnosticBanner diagnostic={camera.error} onDismiss={camera.dismissError} />

        <StageViewport
          client={client}
          slot={slot}
          onRestart={restart}
          onAddScenes={addScenes}
          handlers={camera.handlers}
          onViewportSize={handleViewportSize}
          onFrameStatus={props.onFrameStatus}
          label={`${stageSceneName(RIG)} · ${stageLabel(unstaged ? null : model)}`}
        />
        {stagedSkinned === true && <ClipStrip {...playback} />}
      </div>

      {/* Right column: the persona terminal drawer, dialing the STAGE session. */}
      <div className="col col-term">
        <TerminalDrawer
          url={status.url ?? FALLBACK_STAGE_URL}
          viewId={props.viewId}
          scenePath={stageSceneName(RIG)}
          projectDir={projectDir}
          projectName={props.projectName}
          onOpenProjectSwitcher={props.onOpenProjectSwitcher}
        />
      </div>
    </div>
  );
}
