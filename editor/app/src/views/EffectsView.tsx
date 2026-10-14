// The Effects view (`Plan/particles-engine-and-editor.md` decision 6): the tab where a human tunes
// one `ParticleEmitter` live — drag a slider, watch the plume — with the persona that speaks
// particles sitting in the same window.
//
// LAYOUT — the Voxel Artist's, minus everything a stage with one component does not need: the
// emitter editor on the LEFT, the stage viewport in the CENTRE, the terminal column on the RIGHT.
// No asset browser (there is no file under this loop), no split (there is no second editor), no
// gizmo, no selection.
//
// THE SESSION IS NOT THE GAME'S. This view never touches App's `ProtocolClient`; it owns one dialed
// at the host-spawned effects stage (`shell/host.ts`'s second slot) once that session reports
// `listening`. The game server may not even be running. The terminal drawer is handed the stage's
// url for the same reason: the persona's `ludion-mcp` must dial the STAGE, never the game.
//
// WHAT THIS FILE OWNS, AND WHY IT IS ONLY THAT. The stage lifecycle and the three columns live
// here; every piece that needs a *dialed* client — the subject, its schema, the poll, the time
// strip's state and the one commit path — lives in `editor/effects/EmitterEditor.tsx`, which this
// view mounts only once the session answers. The reason is `useComponentCommit`'s contract: it
// takes a connected client, not a client-or-null, and the honest way to satisfy that is to have no
// component holding the hook while there is no session — not a placeholder client whose calls
// reject. Until then the column shows why it is waiting and the viewport's own state card says
// where the stage is (a cold start builds the engine first).
//
// THE CAMERA IS THE VIEW CAMERA (`Plan/editor-stage-camera.md` W3, decision 1: Effects included).
// `effects-stage.yaml`'s own `Camera` is what `Game` mode shows — unmoved, so the frame the persona
// was told about is still reachable one `G` away — but the viewport now flies and orbits like every
// other stage.
//
// The classes this view and `EmitterPanel` introduce are prefixed `fx-`; the shell primitives
// (`main-row`, `col`, `col-side`, `col-center`, `col-term`) are reused as-is.

import { useCallback, useEffect, useMemo, useState } from 'react';
import { ProtocolClient } from '../protocol/client';
import { isTauri } from '../shell/tauri';
import { onStageStatus, stageRestart, stageStart, stageStatus, type StageStatus } from '../shell/host';
import { mountStage } from '../shell/stageMount';
import { addStageScenes } from '../terminal/pty';
import { TerminalDrawer } from '../terminal/TerminalDrawer';
import type { FrameStatus } from '../viewer/useFrameStream';
import { DiagnosticBanner } from '../viewer/DiagnosticBanner';
import { EmitterEditor } from '../editor/effects/EmitterEditor';
import { StageViewport } from '../editor/voxel/StageViewport';
import { useViewCamera } from '../editor/camera/useViewCamera';
import { STAGE_FOV_Y_DEG, type Viewport as ViewportSize } from '../editor/voxel/orbit';
import {
  effectsStageSlot,
  FALLBACK_STAGE_URL,
  STAGE_SCENE,
  STAGE_SLOT,
  stageScenePath,
} from '../editor/effects/stage';

const STOPPED: StageStatus = {
  slot: STAGE_SLOT,
  phase: 'stopped',
  url: null,
  scene: null,
  message: null,
  code: null,
  listener: null,
};

export interface EffectsViewProps {
  readonly projectDir: string;
  /** The current project's name, validated through `project_info`; `null` before it resolves. */
  readonly projectName: string | null;
  readonly onOpenProjectSwitcher: () => void;
  readonly viewId: string;
  readonly onFrameStatus: (status: FrameStatus) => void;
  /** The one-line session readout for the status bar. */
  readonly onStatusNote: (note: string | null) => void;
}

export function EffectsView(props: EffectsViewProps) {
  const { projectDir, onStatusNote } = props;

  const [status, setStatus] = useState<StageStatus>(STOPPED);
  const [stageError, setStageError] = useState<string | null>(null);
  const [client, setClient] = useState<ProtocolClient | null>(null);
  // The canvas's CSS size, as a value: `useViewCamera` takes it as a plain prop, not a ref.
  const [viewport, setViewport] = useState<ViewportSize>({ width: 16, height: 9 });

  // The view camera: plain left drag orbits (there is nothing to pick on a stage), and the
  // committed `effects-stage.yaml` FOV is the base Z/C narrows and widens from.
  const camera = useViewCamera({
    client,
    viewId: props.viewId,
    scenePath: STAGE_SCENE,
    viewport,
    plainLeftOrbits: true,
    baseFovYDeg: STAGE_FOV_Y_DEG,
  });

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
    void onStageStatus(STAGE_SLOT, (next) => {
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
      status: () => stageStatus(STAGE_SLOT),
      start: () => stageStart(STAGE_SLOT, { scene: stageScenePath(projectDir) }),
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

  const restartStage = useCallback(() => {
    if (!isTauri() || projectDir === '') {
      return;
    }
    setStageError(null);
    void stageRestart(STAGE_SLOT, { scene: stageScenePath(projectDir) })
      .then(setStatus)
      .catch((err: unknown) => setStageError(String(err)));
  }, [projectDir]);

  const addScenes = useCallback(() => {
    if (!isTauri() || projectDir === '') {
      return;
    }
    setStageError(null);
    void addStageScenes(projectDir)
      .then(() => restartStage())
      .catch((err: unknown) => setStageError(String(err)));
  }, [projectDir, restartStage]);

  // Dial the stage once it is listening: a fresh `ProtocolClient` per url, closed on teardown, held
  // in state so the frame stream's effects (keyed on `[client]`) — and the emitter editor — open and
  // close with it.
  const stageUrl = status.phase === 'listening' ? status.url : null;
  useEffect(() => {
    if (stageUrl === null) {
      setClient(null);
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

  // The status bar's line is the editor's while one is mounted; with no session there is no clock to
  // report, and a stale frame count must not outlive the session it was read from.
  useEffect(() => {
    if (client === null) {
      onStatusNote(null);
    }
  }, [client, onStatusNote]);
  useEffect(() => () => onStatusNote(null), [onStatusNote]);

  // Memoized because `StageViewport`'s resize observer keys on it: a fresh object per render tore the
  // observer down and rebuilt it several times a second.
  const slot = useMemo(
    () =>
      effectsStageSlot({
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
      {/* Left column: the emitter editor, or why there isn't one yet. */}
      <div className="col col-side">
        {client === null ? (
          <p className="fx-hint fx-waiting">
            Waiting for the effects stage — the viewport says where it is.
          </p>
        ) : (
          <EmitterEditor client={client} projectDir={projectDir} onStatusNote={onStatusNote} />
        )}
      </div>

      {/* Centre column: the stage's frame stream, under the view camera. */}
      <div className="col-center vx-center">
        <div className="vx-toolbar">
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
              drag to orbit · hold RMB to fly (WASD, wheel = speed) · Alt+drag to orbit/dolly/pan · G is game
            </span>
          </div>
        </div>
        <DiagnosticBanner diagnostic={camera.error} onDismiss={camera.dismissError} />

        <StageViewport
          client={client}
          slot={slot}
          onRestart={restartStage}
          onAddScenes={addScenes}
          handlers={camera.handlers}
          onViewportSize={setViewport}
          onFrameStatus={props.onFrameStatus}
          label={STAGE_SCENE}
        />
      </div>

      {/* Right column: the persona terminal drawer, dialing the STAGE session. */}
      <div className="col col-term">
        <TerminalDrawer
          url={stageUrl ?? FALLBACK_STAGE_URL}
          viewId={props.viewId}
          scenePath={STAGE_SCENE}
          projectDir={projectDir}
          projectName={props.projectName}
          onOpenProjectSwitcher={props.onOpenProjectSwitcher}
        />
      </div>
    </div>
  );
}
