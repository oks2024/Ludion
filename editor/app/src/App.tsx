import { useCallback, useEffect, useMemo, useReducer, useRef, useState, type FormEvent } from 'react';
import type { Command, ServerHello } from '@ludion/protocol';
import { ProtocolClient } from './protocol/client';
import { schemaMismatch } from './protocol/schemaCheck';
import { SCHEMA_HASH } from './protocol/schemaHash';
import { createDialGate, dial } from './viewer/dial';
import { openSubscription } from './protocol/subscription';
import { Viewport, type PlaceResolver } from './viewer/Viewport';
import { NO_FRAMES, setFrameStatus } from './viewer/frameStatus';
import { Inspector } from './viewer/Inspector';
import type { MutationReport } from './viewer/useComponentCommit';
import { DiagnosticBanner } from './viewer/DiagnosticBanner';
import { logEventToDiagnostic, toDiagnostic, type DiagnosticView } from './viewer/diagnostic';
import { loadEntityRefs } from './editor/entityRefs';
import { UndoHistory, executeRedo, executeUndo, type UndoEntry } from './editor/undo';
import { mutationToEntries, mutationToEntry } from './editor/mutations';
import { PalettePanel } from './editor/PalettePanel';
import { HierarchyPanel } from './editor/HierarchyPanel';
import { PrefabBrowser } from './editor/PrefabBrowser';
import { placePrefab } from './editor/prefab-place';
import { duplicateEntity, destroyEntity, spawnArchetype, type EntityOpResult } from './editor/entity-ops';
import { isTextEditingTarget } from './viewer/keymap';
import { CONNECTED, DEFAULT_BACKOFF, reconnectStatus, reduce, type ReconnectEvent, type ReconnectPhase } from './viewer/reconnect';
import { Titlebar } from './shell/Titlebar';
import { ViewSwitcher } from './shell/ViewSwitcher';
import { SessionStrip } from './shell/SessionStrip';
import { StatusBar } from './shell/StatusBar';
import { TerminalDrawer } from './terminal/TerminalDrawer';
import { GameDesignerView } from './views/GameDesignerView';
import { VoxelArtistView } from './views/VoxelArtistView';
import { ModelsView } from './views/ModelsView';
import { GenerationView } from './views/GenerationView';
import { EffectsView } from './views/EffectsView';
import { ArtDirectorView } from './views/ArtDirectorView';
import { LighterView } from './views/LighterView';
import { PerformanceView } from './perf/PerformanceView';
import { isTauri } from './shell/tauri';
import { PROJECT_DIR_KEY, pickProjectDir, resolveProjectDir, toStoredProjectDir } from './shell/projectDir';
import { RECENT_PROJECTS_KEY, parseRecents, pushRecent } from './shell/recents';
import { switchProject } from './shell/projectSwitch';
import { GAME_SLOT, onStageStatus, stageStart, stageStatus, stageStop, type StagePhase, type StageStatus } from './shell/host';
import { autoAttachUrl, dropsSession, gameCard, gameOpts, type ConnectionState, type GameAction } from './shell/gameSession';
import { sceneChangeOutcome } from './shell/sceneChange';
import { StageSlotCard } from './editor/voxel/StageSlotCard';
import { join } from './editor/voxel/stage';
import { editorPaths, projectInfo, writeEditorContext, type ProjectInfo } from './terminal/pty';
import {
  buildEditorContext,
  type ContextArtDirector,
  type ContextCamera,
  type ContextLighter,
  type ContextSelection,
  type ContextVoxel,
} from './terminal/context';
import { docParts } from './shell/docpath';

/** The editor's play/edit toggle (Plan/phase-6-editing.md design decision #6). Edit mode pausing the
 *  sim is an editor convention enforced here, not by the engine -- a running world stays editable
 *  over the wire, it's just not what the editor's edit mode does. */
export type EditorMode = 'play' | 'edit';

/** An entity handle, matching the wire's `{ index, generation }` shape. Named here (rather than
 *  imported from the generated protocol types, which mint a fresh interface per response arm) since
 *  `App` is where selection is lifted to and every consumer just needs the two fields. */
export interface EntityId {
  readonly index: number;
  readonly generation: number;
}

const DEFAULT_URL = 'ws://127.0.0.1:9411';

/** The game slot's status before its first `stage_status` answer arrives. */
const GAME_STOPPED: StageStatus = {
  slot: GAME_SLOT,
  phase: 'stopped',
  url: null,
  scene: null,
  message: null,
  code: null,
  listener: null,
};

/** True when two ids name the same handle (index *and* generation) — used to repair selection after
 *  an undo/redo respawn remaps an id. */
function sameEntity(a: EntityId, b: EntityId): boolean {
  return a.index === b.index && a.generation === b.generation;
}

/** The parent of a `/`-or-`\`-separated relative path, or `''` when it names no directory (a
 *  `default_scene` with no subfolder) — `join`'s empty-segment fold then leaves the content root at
 *  the project directory itself. */
function dirname(relPath: string): string {
  const normalized = relPath.replace(/\\/g, '/');
  const idx = normalized.lastIndexOf('/');
  return idx === -1 ? '' : normalized.slice(0, idx);
}

/** The connect card. Per `Plan/editor-voxel-artist.md` this is a **per-view precondition**,
 *  not the app's root gate: the shell (titlebar, view switcher, terminal drawer, status bar) always
 *  renders, and a view that needs the game session shows this in its viewport slot. Views that own
 *  their own engine — the Voxel Artist's host-spawned stage — never show it at all.
 *
 *  The game card (`gameCard`) leads: the game runs itself, and Run/Attach/Replace is the path a
 *  human actually wants. **Attach to URL** below it is for a server started elsewhere (a plain
 *  `ludion serve`, another machine) — never how the open project's own game is meant to be reached.
 *  Outside Tauri there is no host to spawn a game with, so `game` is omitted and only the form shows. */
function ConnectPanel({
  url,
  state,
  error,
  onUrlChange,
  onSubmit,
  game,
  onRunGame,
  onAttachGame,
  onReplace,
}: {
  url: string;
  state: ConnectionState;
  error: string | null;
  onUrlChange: (url: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  game?: { readonly slot: ReturnType<typeof gameCard>['slot']; readonly actions: readonly GameAction[] };
  onRunGame: () => void;
  onAttachGame: () => void;
  onReplace: () => void;
}) {
  return (
    <div className="connect-screen connect-slot">
      <div className="connect-card">
        <div className="connect-brand">
          <div className="brand-mark" />
          <span className="brand-name">LUDION</span>
        </div>

        {game !== undefined && (
          <StageSlotCard slot={game.slot}>
            <div className="connect-game-actions">
              {game.actions.includes('run') && (
                <button type="button" className="connect-submit" onClick={onRunGame}>
                  Run game
                </button>
              )}
              {game.actions.includes('attach') && (
                <button type="button" className="icon-btn" onClick={onAttachGame}>
                  Attach
                </button>
              )}
              {game.actions.includes('replace') && (
                <button type="button" className="icon-btn" onClick={onReplace}>
                  Replace
                </button>
              )}
            </div>
          </StageSlotCard>
        )}

        <h1 className="connect-title">Attach to URL</h1>
        <p className="connect-sub">The game runs itself above; this is for a server started elsewhere.</p>

        <form className="connect-form" onSubmit={onSubmit}>
          <label className="connect-field-label" htmlFor="ws-url">
            Server URL
          </label>
          <input
            id="ws-url"
            type="text"
            aria-label="Server URL"
            value={url}
            onChange={(event) => onUrlChange(event.target.value)}
            disabled={state === 'connecting'}
            spellCheck={false}
          />
          <button type="submit" className="connect-submit" disabled={state === 'connecting'}>
            {state === 'connecting' ? 'Connecting…' : 'Connect'}
          </button>
        </form>

        <div className="connect-status" aria-live="polite">
          <span className={`state state-${state}`}>{state}</span>
          {error !== null && <p className="connect-error">{error}</p>}
        </div>
      </div>
    </div>
  );
}

// The editor shell (Meridian v2). One screen: titlebar, view switcher, session strip, a three-column
// main row (hierarchy+inspector · viewport+kit shelf · terminal drawer), and a status bar. Everything
// in the main row talks to the engine through the single `ProtocolClient`, held as React state so the
// child panels' effects (frame subscription, entity poll) start on connect and tear down on disconnect.
//
// The game session is a per-view precondition, not an app-wide gate: with no server connected the
// shell still renders and the views that need one show `ConnectPanel` in their viewport slot.
//
// Edit-mode state (`mode`, `selection`, `dirty`, `savePath`) lives here too, per the app's
// props-and-local-state convention -- no context, no store. `SessionStrip` owns the mode-toggle and
// save *commands*; `App` receives the *outcome* (mode changed, save succeeded, command error) and
// updates state accordingly. The entity count is reported up by the hierarchy; the live frame status
// is reported by each frame surface straight into `viewer/frameStatus`, which the status bar reads —
// it moves four times a second and nothing else in the app is a function of it.
export default function App() {
  const [url, setUrl] = useState(DEFAULT_URL);
  const [state, setState] = useState<ConnectionState>('idle');
  const [hello, setHello] = useState<ServerHello | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [client, setClient] = useState<ProtocolClient | null>(null);
  const clientRef = useRef<ProtocolClient | null>(null);

  // Auto-reconnect (Plan decision 6): `ludion dev` restarts the game on every hot
  // reload — the socket drops for a 1–3 s "blink" and returns on the same port — so a lost connection
  // is recoverable, not terminal. The pure machine in `viewer/reconnect.ts` holds the policy (backoff,
  // give-up); this component runs its effects (timers + connect attempts).
  // `reconnectRef` is the driver's source of truth; `reconnectPhase` mirrors it for the render.
  const reconnectRef = useRef<ReconnectPhase>(CONNECTED);
  const [reconnectPhase, setReconnectPhase] = useState<ReconnectPhase>(CONNECTED);
  const reconnectTimerRef = useRef<number | null>(null);
  // The engine's announced schema hash against the one the editor's types were generated from, checked
  // at every connect and reconnect (`promoteClient`). A mismatch warns and never refuses: the banner
  // stays until the session is torn down.
  const [schemaDrift, setSchemaDrift] = useState<DiagnosticView | null>(null);
  // Late-bound so `attemptReconnect`'s socket callbacks and the backoff timer can dispatch without a
  // definition cycle; assigned to the live `dispatchReconnect` on every render.
  const dispatchRef = useRef<(event: ReconnectEvent) => void>(() => {});

  // The active editor view (Meridian view switcher). Level Designer and Game Designer are enabled; the
  // switcher swaps which main-row layout renders and which persona the terminal drawer boots.
  const [activeView, setActiveView] = useState('level-designer');

  const [mode, setMode] = useState<EditorMode>('play');
  const [selection, setSelection] = useState<EntityId | null>(null);
  // The human's placed 3D pointer (a virtual "here" marker, world coords) — an editor concept, not a
  // scene entity. The viewport places/refines it; App persists it for the status bar + context bridge.
  const [pointer, setPointer] = useState<readonly [number, number, number] | null>(null);
  const [dirty, setDirty] = useState(false);
  const [savePath, setSavePath] = useState('');

  // The host's checkout root — the anchor a checkout-relative project dir resolves against. `null`
  // until `editor_paths` answers, and forever in the browser dev page, which has no host.
  const [repoRoot, setRepoRoot] = useState<string | null>(null);
  // The process's `--project` launch argument, if any (`scripts/editor.mjs`) — set alongside
  // `repoRoot` from the same `editor_paths` answer; a ref because only the one-shot boot probe
  // below reads it.
  const launchProjectRef = useRef<string | null>(null);
  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    editorPaths()
      .then((paths) => {
        if (!cancelled) {
          launchProjectRef.current = paths.launchProject;
          setRepoRoot(paths.repoRoot);
        }
      })
      .catch((e: unknown) => console.warn(`editor_paths failed: ${String(e)}`));
    return () => {
      cancelled = true;
    };
  }, []);

  // The game-project directory the terminal drawer spawns persona instances in, and the source of
  // the `.ludion/editor-context.json` path. Persisted in its checkout-relative form
  // (`shell/projectDir.ts`) so it survives reconnects, restarts AND a checkout move; every consumer
  // sees the resolved absolute path.
  const [storedProjectDir, setStoredProjectDir] = useState<string>(() => {
    try {
      return localStorage.getItem(PROJECT_DIR_KEY) ?? '';
    } catch {
      return '';
    }
  });
  const projectDir = useMemo(() => resolveProjectDir(storedProjectDir, repoRoot ?? ''), [storedProjectDir, repoRoot]);
  // The switcher's recents list (up to 8), persisted alongside the project setting itself.
  const [recents, setRecents] = useState<string[]>(() => {
    try {
      return parseRecents(localStorage.getItem(RECENT_PROJECTS_KEY));
    } catch {
      return [];
    }
  });
  // Whether the titlebar's project switcher popover is open.
  const [switcherOpen, setSwitcherOpen] = useState(false);
  // Where every project choice lands, once `switchProjectDir` below has stopped every stage session
  // bound to the OLD project: persist the new one and push it onto the recents list.
  const commit = useCallback(
    (dir: string) => {
      const stored = toStoredProjectDir(dir, repoRoot ?? '');
      setStoredProjectDir(stored);
      try {
        localStorage.setItem(PROJECT_DIR_KEY, stored);
      } catch {
        // Private-mode / storage-disabled: keep it in memory for this session.
      }
      setRecents((prev) => {
        const next = pushRecent(prev, stored);
        try {
          localStorage.setItem(RECENT_PROJECTS_KEY, JSON.stringify(next));
        } catch {
          // Private-mode / storage-disabled: recents stay in memory for this session.
        }
        return next;
      });
    },
    [repoRoot],
  );
  // Late-bound the same way `dispatchRef` is: `dropSession` (defined below, once `teardown` exists)
  // needs to be in scope here, above its own declaration.
  const dropSessionRef = useRef<() => void>(() => {});
  // The editor's ONE project switch point (`shell/projectSwitch.ts`, decision 4): every
  // host-spawned, project-bound process is stopped here before the new project is committed. The
  // browser dev page has no stage host at all, so its deps report everything already stopped.
  const switchProjectDir = useCallback(
    (dir: string): Promise<void> =>
      switchProject(dir, projectDir, {
        stageStatus: isTauri() ? stageStatus : async () => ({ phase: 'stopped' as StagePhase }),
        stageStop: isTauri() ? stageStop : async () => {},
        dropSession: () => dropSessionRef.current(),
        commit,
      }),
    [projectDir, commit],
  );
  // One boot-time pass, once the root is known: a `--project` launch argument wins over whatever is
  // stored, persisted the same way any project directory is (`pickProjectDir`); otherwise the
  // stored value stands as-is. Runs once, so a path the human types afterwards is never
  // second-guessed. No exists probe and no silent fallback for a bad stored path — `project_info`
  // below is the one source of truth for whether a project directory is valid. Routed through the
  // switch point too, so a launch project enters the recents like any other choice.
  const projectDirProbedRef = useRef(false);
  // Whether the boot probe above has settled — the game-start effect's gate: a `--project` launch
  // must not first start the STORED project's game before the probe has had a chance to switch onto
  // the launch project.
  const [bootProbed, setBootProbed] = useState(false);
  useEffect(() => {
    if (!isTauri() || repoRoot === null || projectDirProbedRef.current) {
      return;
    }
    projectDirProbedRef.current = true;
    const picked = pickProjectDir(storedProjectDir, launchProjectRef.current, repoRoot);
    if (picked !== storedProjectDir) {
      void switchProjectDir(resolveProjectDir(picked, repoRoot)).finally(() => setBootProbed(true));
    } else {
      // No launch argument and nothing stored: there is no project, so a bare launch lands on the picker.
      if (picked === '') setSwitcherOpen(true);
      setBootProbed(true);
    }
  }, [repoRoot, storedProjectDir, switchProjectDir]);

  // The current project, validated through `project_info` on every change — a switcher choice
  // included, so correcting a bad path re-validates rather than trusting the new string blindly.
  // `null` before the first answer (or in the browser dev page, which has no host). Kept as the
  // full `ProjectInfo` (not just its name): `dir`/`defaultScene` are what the game slot starts on.
  const [project, setProject] = useState<ProjectInfo | { error: string } | null>(null);
  useEffect(() => {
    if (!isTauri() || projectDir === '') {
      setProject(null);
      return;
    }
    let cancelled = false;
    projectInfo(projectDir)
      .then((info) => {
        if (!cancelled) setProject(info);
      })
      .catch((e: unknown) => {
        const message = String(e);
        console.warn(`project_info failed: ${message}`);
        if (!cancelled) setProject({ error: message });
      });
    return () => {
      cancelled = true;
    };
  }, [projectDir]);
  // The registry's component names, for the context file's selection snapshot; fetched once per
  // connection (reset with the client below).
  const contextComponentNamesRef = useRef<string[] | null>(null);

  // Status-bar readouts lifted from the child panels. The entity count is App state (it moves on a
  // scene change); the frame status is deliberately NOT — every frame surface reports one four times
  // a second, and holding it here re-rendered every mounted view for a number only the status bar
  // reads. `setFrameStatus` is the store's own setter, so it is stable by construction and the
  // reporting effects never re-fire.
  const [entityCount, setEntityCount] = useState(0);
  // A mode/save command failure surfaced by the session strip (the 42px strip has no room inline).
  const [sessionError, setSessionError] = useState<DiagnosticView | null>(null);
  // The Voxel Artist view's contributions to the shell: its `voxel` context block (the persona's
  // file promises it exists) and its status-bar line. Both are view-owned, App-held, because the
  // context file and the status bar are.
  const [voxelContext, setVoxelContext] = useState<ContextVoxel | null>(null);
  const [artContext, setArtContext] = useState<ContextArtDirector | null>(null);
  const [lighterContext, setLighterContext] = useState<ContextLighter | null>(null);
  // Which camera the main session's viewport is showing, and where the view camera stands while it
  // is showing — host render state the viewport reports up so the context file can carry it. It is
  // never scene data: nothing here reaches `scene.save`.
  const [cameraContext, setCameraContext] = useState<ContextCamera | null>(null);
  const [statusNote, setStatusNote] = useState<string | null>(null);

  // Undo/redo history (Plan/phase-6-editing.md decision #2): a client-side stack fed by the inverse
  // data each mutation's response carries. The `UndoHistory` mutates in place, so it lives in a ref;
  // `bumpHistory` forces the re-render that keeps the toolbar's canUndo/canRedo in sync after a
  // push/undo/redo/clear (the only mutations of the stack all go through this component).
  const historyRef = useRef(new UndoHistory());
  const [, bumpHistory] = useReducer((n: number) => n + 1, 0);
  const [undoError, setUndoError] = useState<DiagnosticView | null>(null);
  // The most recent warn/error the engine pushed on the `log` channel — chiefly a render failure
  // (e.g. a missing texture, L1101), which otherwise leaves the viewport silently stuck at "waiting
  // for frames…". Surfacing it as a banner makes the stall self-explaining. Latest-wins, dismissible.
  const [logDiagnostic, setLogDiagnostic] = useState<DiagnosticView | null>(null);
  // Set synchronously just before *our own* `scene.save`, so the resulting `scene.changed` event is
  // recognized as ours (clear dirty, keep the stack) rather than a foreign load/save (which invalidates
  // entity ids, so the stack and selection are dropped). See the `scene.changed` effect below.
  const expectOwnSaveRef = useRef(false);

  // Gates every dial: Disconnect, a project switch and the unmount invalidate it, so a handshake
  // that finishes afterwards is closed, never promoted.
  const dialGateRef = useRef(createDialGate());

  const clearReconnectTimer = useCallback(() => {
    if (reconnectTimerRef.current !== null) {
      window.clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
  }, []);

  const teardown = useCallback(() => {
    dialGateRef.current.invalidate();
    clearReconnectTimer();
    reconnectRef.current = CONNECTED;
    setReconnectPhase(CONNECTED);
    setSchemaDrift(null);
    clientRef.current?.close();
    clientRef.current = null;
    setClient(null);
    setHello(null);
    // A fresh connection starts from a clean slate: play mode, nothing selected, nothing dirty. The
    // save path is deliberately kept -- it's tied to a scene path on disk, not to the connection.
    setMode('play');
    setSelection(null);
    setPointer(null);
    setDirty(false);
    setUndoError(null);
    setLogDiagnostic(null);
    setSessionError(null);
    setFrameStatus(NO_FRAMES);
    setEntityCount(0);
  }, [clearReconnectTimer]);

  // The project's own game, host-spawned at the game slot (decision 5, `Plan/editor-open-project.md`).
  // `validProject` narrows out the `{ error }` arm so callers never start a game on a project
  // that failed to validate.
  const validProject = project !== null && !('error' in project) ? project : null;
  const [gameStatus, setGameStatus] = useState<StageStatus>(GAME_STOPPED);
  // A `stageStart` rejection (missing manifest, no cargo) — the host never reports these as a status
  // event, same as the stages' own `stageError`.
  const [gameError, setGameError] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) {
      setGameStatus(GAME_STOPPED);
      return;
    }
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onStageStatus(GAME_SLOT, (next) => {
      if (!cancelled) {
        setGameStatus(next);
      }
    }).then((off) => {
      if (cancelled) {
        off();
      } else {
        unlisten = off;
      }
    });
    void stageStatus(GAME_SLOT).then((current) => {
      if (!cancelled) {
        setGameStatus(current);
      }
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const startGame = useCallback(
    async (replace: boolean): Promise<void> => {
      if (!isTauri() || validProject === null) {
        return;
      }
      setGameError(null);
      try {
        setGameStatus(await stageStart(GAME_SLOT, gameOpts(validProject.dir, validProject.defaultScene), replace));
      } catch (e: unknown) {
        setGameError(String(e));
      }
    },
    [validProject],
  );

  // Runs the game once per `projectDir`, the moment `project_info` validates it — never a second
  // time for the same directory (a remount, or `gameStatus` settling, must not re-trigger it). The
  // `validProject.dir === projectDir` guard matters because `project` lags a switch by one async
  // answer: without it, the OLD project's default scene would start under the NEW directory the
  // instant `projectDir` changes.
  const gameStartedForRef = useRef<string | null>(null);
  useEffect(() => {
    if (!isTauri() || !bootProbed || validProject === null || validProject.dir !== projectDir) {
      return;
    }
    if (gameStartedForRef.current === projectDir) {
      return;
    }
    gameStartedForRef.current = projectDir;
    void startGame(false);
  }, [validProject, projectDir, startGame, bootProbed]);

  // The switch point's `dropSession` hook (wired through `dropSessionRef` above): a real switch
  // drops the main session unconditionally, whatever it is attached to (decision 4).
  const dropSession = useCallback((): void => {
    teardown();
    setState('idle');
  }, [teardown]);
  useEffect(() => {
    dropSessionRef.current = dropSession;
  }, [dropSession]);

  // The undo stack is meaningless across a connection boundary: a new engine (or a reconnect) hands
  // out its own entity ids, so drop the history whenever the client changes or goes away. Runs on
  // connect too, where it clears an already-empty stack — harmless.
  useEffect(() => {
    historyRef.current.clear();
    bumpHistory();
    expectOwnSaveRef.current = false;
    contextComponentNamesRef.current = null;
    historyRef.current.setEntityRefs(null);
  }, [client]);

  // Context bridge (Plan/editor-views.md Slice #1, step 4): keep `<projectDir>/.ludion/editor-
  // context.json` current so a persona resolves "here/this" against the human's live selection. It
  // writes on selection identity / scene-path / voxel-block change (debounced, atomic host write),
  // enriching the selection with a one-shot component snapshot rather than a new poll. Inside the
  // Tauri host with a project dir set; the plain-browser dev page skips it silently.
  //
  // Deliberately NOT gated on a game session: the Voxel Artist view owns its own engine and may be
  // the only one running, and its persona's file promises the `voxel` block is there. Without a
  // client there is simply no selection to enrich.
  useEffect(() => {
    if (!isTauri() || projectDir === '') {
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          let selectionOut: ContextSelection | null = null;
          if (client !== null && selection !== null) {
            if (contextComponentNamesRef.current === null) {
              const desc = await client.call({ method: 'engine.describe' });
              contextComponentNamesRef.current =
                desc.kind === 'engine_description' && typeof desc.description === 'object'
                  ? Object.keys((desc.description as { components?: Record<string, unknown> }).components ?? {})
                  : [];
            }
            const body = await client.call({
              method: 'query.entities',
              params: { filter: [], include: contextComponentNamesRef.current, entities: [{ ...selection }] },
            });
            const match =
              body.kind === 'query_result'
                ? body.entities.find(
                    (e) => e.entity.index === selection.index && e.entity.generation === selection.generation,
                  )
                : undefined;
            selectionOut = { id: selection, components: match?.components ?? null };
          }
          if (cancelled) {
            return;
          }
          const context = buildEditorContext({
            view: activeView,
            scenePath: savePath,
            selection: selectionOut,
            pointer: pointer === null ? null : { position: pointer },
            voxel: voxelContext,
            artDirector: artContext,
            lighter: lighterContext,
            camera: cameraContext,
          });
          await writeEditorContext(projectDir, JSON.stringify(context, null, 2));
        } catch {
          // Best effort: a failed describe/query/write just leaves the last context file in place.
        }
      })();
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [
    client,
    selection,
    savePath,
    projectDir,
    pointer,
    activeView,
    voxelContext,
    artContext,
    lighterContext,
    cameraContext,
  ]);

  // The selected entity's component names, for the Game Designer view's Systems match rule. A one-shot
  // fetch on selection/client change (reusing the registry-names ref the context bridge fills), gated
  // to the Game Designer view so the Level Designer path is untouched. The systems panel matches a
  // system's declared queries against this set.
  const [selectionComponents, setSelectionComponents] = useState<readonly string[] | null>(null);
  useEffect(() => {
    if (client === null || selection === null || activeView !== 'game-designer') {
      setSelectionComponents(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        if (contextComponentNamesRef.current === null) {
          const desc = await client.call({ method: 'engine.describe' });
          contextComponentNamesRef.current =
            desc.kind === 'engine_description' && typeof desc.description === 'object'
              ? Object.keys((desc.description as { components?: Record<string, unknown> }).components ?? {})
              : [];
        }
        const body = await client.call({
          method: 'query.entities',
          params: { filter: [], include: contextComponentNamesRef.current, entities: [{ ...selection }] },
        });
        if (cancelled) {
          return;
        }
        const match =
          body.kind === 'query_result'
            ? body.entities.find(
                (e) => e.entity.index === selection.index && e.entity.generation === selection.generation,
              )
            : undefined;
        setSelectionComponents(match?.components ? Object.keys(match.components) : null);
      } catch {
        // Best effort: leave the last snapshot in place.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, selection, activeView]);

  // Tracks the served scene's dirty state via the `scene.changed` event channel (Plan/phase-6-editing):
  // any mutation clears once the engine confirms it landed, and a non-null `path` in the
  // event prefills the save-path input so a fresh session doesn't need the human to type it in.
  useEffect(() => {
    if (client === null) {
      return;
    }
    const offEvent = client.onEvent((event) => {
      if (event.event !== 'scene.changed') {
        return;
      }
      // Ignore events belonging to a subscription we've already torn down (or never owned).
      if (!sub.accepts(event.sub)) {
        return;
      }
      // Was this our own save? (Set synchronously before we issued `scene.save`.) A picker load never
      // arms the flag, so it takes this same foreign path by construction. See `sceneChangeOutcome`.
      const own = expectOwnSaveRef.current;
      expectOwnSaveRef.current = false;
      const outcome = sceneChangeOutcome(own, event.data.path);
      if (outcome.resetEditState) {
        historyRef.current.clear();
        bumpHistory();
        setSelection(null);
        // A foreign load/save may invalidate the world the marker referenced; drop it with the selection.
        setPointer(null);
        setUndoError(null);
      }
      setDirty(false);
      if (outcome.savePath !== null) {
        setSavePath(outcome.savePath);
      }
    });

    // A failed subscribe here just means dirty-tracking degrades to save-triggered clears only;
    // not worth a banner over.
    const sub = openSubscription(client, 'scene.changed');

    return () => {
      offEvent();
      sub.close();
    };
  }, [client]);

  // Subscribe to the engine's `log` channel and surface warn/error entries as a banner. Its whole
  // reason to exist: the live loop reports a render failure (a missing texture, a bad extract) as an
  // error-level log and then emits no frame, so without this the viewport just sits at "waiting for
  // frames…" with no explanation. A fresh client starts with no log banner; the sub tears down with
  // the connection. Errors surfacing the log itself aren't worth a banner (nothing actionable).
  useEffect(() => {
    if (client === null) {
      return;
    }
    setLogDiagnostic(null);
    const offEvent = client.onEvent((event) => {
      if (event.event !== 'log') {
        return;
      }
      if (!sub.accepts(event.sub)) {
        return;
      }
      const diagnostic = logEventToDiagnostic(event.data);
      if (diagnostic !== null) {
        setLogDiagnostic(diagnostic);
      }
    });

    // A failed subscribe just means render failures stay silent as before — degraded, not broken.
    const sub = openSubscription(client, 'log');

    return () => {
      offEvent();
      sub.close();
    };
  }, [client]);

  // Every successful mutation the inspector reports becomes an undo entry (when its shape is one we
  // record) and marks the scene dirty. `mutationToEntry` returns null for anything unrecognized; the
  // scene still changed, so dirty is set regardless.
  const handleMutation = useCallback((report: MutationReport): void => {
    const entry = mutationToEntry(report);
    if (entry !== null) {
      historyRef.current.push(entry);
      bumpHistory();
    }
    setDirty(true);
  }, []);

  // A destroy records its cascaded subtree (one `entity.destroy` whose response lists the
  // descendants) as ONE undoable group, so a single Ctrl+Z restores the entire subtree.
  // Records that don't map to an entry are dropped; the scene still changed, so dirty is set regardless.
  const handleMutationGroup = useCallback((records: readonly MutationReport[]): void => {
    const entries: UndoEntry[] = [];
    for (const record of records) {
      entries.push(...mutationToEntries(record));
    }
    if (entries.length > 0) {
      historyRef.current.pushGroup(entries);
      bumpHistory();
    }
    setDirty(true);
  }, []);

  // Shared tail for spawn and duplicate: record the (single) spawn as an undo entry, then either
  // surface the failure or select the fresh entity. Both ops return the same `EntityOpResult` shape.
  const finishSpawn = useCallback(
    (result: EntityOpResult): void => {
      for (const record of result.records) {
        handleMutation(record);
      }
      if (result.error !== undefined) {
        setUndoError(toDiagnostic(result.error));
        return;
      }
      if (result.newEntity !== undefined) {
        setSelection(result.newEntity);
        setUndoError(null);
      }
    },
    [handleMutation],
  );

  const handleSpawn = useCallback(
    async (command: Command): Promise<void> => {
      const active = clientRef.current;
      if (active === null) {
        return;
      }
      finishSpawn(await spawnArchetype(command, (cmd) => active.call(cmd)));
    },
    [finishSpawn],
  );

  const handleDuplicate = useCallback(async (): Promise<void> => {
    const active = clientRef.current;
    if (active === null || selection === null) {
      return;
    }
    finishSpawn(await duplicateEntity(selection, (cmd) => active.call(cmd)));
  }, [selection, finishSpawn]);

  // Destroy reports its whole cascade through the group path. On success clear the selection (only if
  // it's still the entity we just destroyed); on a partial failure the records that landed are still
  // recorded as a group, and the error surfaces in the banner.
  const handleDestroy = useCallback(async (): Promise<void> => {
    const active = clientRef.current;
    const victim = selection;
    if (active === null || victim === null) {
      return;
    }
    const result = await destroyEntity(victim, (cmd) => active.call(cmd));
    if (result.records.length > 0) {
      handleMutationGroup(result.records);
    }
    if (result.error !== undefined) {
      setUndoError(toDiagnostic(result.error));
    } else {
      setUndoError(null);
      setSelection((current) => (current !== null && sameEntity(current, victim) ? null : current));
    }
  }, [selection, handleMutationGroup]);

  // A gizmo drag reports its ONE undo entry here (decision #7) — deliberately NOT through
  // `handleMutation`: the drag's throttled preview `component.set`s bypass the undo stack entirely, and
  // only this collapsed value-at-mousedown → value-at-release pair is recorded.
  const handleEditEntry = useCallback((entry: UndoEntry): void => {
    historyRef.current.push(entry);
    bumpHistory();
    setDirty(true);
  }, []);

  // A hierarchy reparent reports its whole undo group (reparent + exact local Transform) as ONE step.
  const handleEditGroup = useCallback((entries: UndoEntry[]): void => {
    historyRef.current.pushGroup(entries);
    bumpHistory();
    setDirty(true);
  }, []);

  // The first preview set of a drag marks the scene dirty on its own — a later cancel restores the
  // values but the engine did record real sets, so keeping dirty is the honest state.
  const handleDirty = useCallback((): void => setDirty(true), []);

  // Undo/redo run the executor against the live client. These deliberately do NOT flow through
  // `handleMutation` -- the executor's own inverse/redo commands must not be re-pushed as new edits
  // (and the inspector never sees them anyway). On success: mark dirty, and if a respawn remapped an
  // id, follow the selection onto the new handle. On failure: surface the diagnostic. On null (empty
  // stack): nothing.
  const runStep = useCallback(
    async (step: typeof executeUndo): Promise<void> => {
      const active = clientRef.current;
      if (active === null) {
        return;
      }
      if (historyRef.current.entityRefs === null) {
        try {
          historyRef.current.setEntityRefs(await loadEntityRefs((cmd) => active.call(cmd)));
        } catch (err) {
          setUndoError(toDiagnostic(err));
          return;
        }
      }
      const result = await step(historyRef.current, (cmd) => active.call(cmd));
      bumpHistory();
      if (result === null) {
        return;
      }
      if (result.ok) {
        setDirty(true);
        setUndoError(null);
        // A group step can respawn several entities; follow the selection through each substitution.
        for (const { from, to } of result.remaps) {
          setSelection((current) => (current !== null && sameEntity(current, from) ? to : current));
        }
      } else {
        setUndoError(toDiagnostic(result.error));
      }
    },
    [],
  );

  const handleUndo = useCallback(() => runStep(executeUndo), [runStep]);
  const handleRedo = useCallback(() => runStep(executeRedo), [runStep]);

  // Place-by-drag: a prefab row dropped on the viewport spawns an instance at the AABB-or-ground point
  // under the cursor (`prefab.spawn`), recorded as one undo group. A drop that misses (off the canvas,
  // the sky) is a no-op; the hierarchy poll picks the instance up by itself.
  const placeResolverRef = useRef<PlaceResolver | null>(null);
  const handlePlacePrefab = useCallback(
    async (relPath: string, clientX: number, clientY: number): Promise<void> => {
      const active = clientRef.current;
      const resolve = placeResolverRef.current;
      if (mode !== 'edit' || active === null || resolve === null) {
        return;
      }
      try {
        const at = await resolve(clientX, clientY);
        if (at === null) {
          return;
        }
        const { entries, first } = await placePrefab(relPath, at, (cmd) => active.call(cmd));
        handleEditGroup(entries);
        setSelection(first);
        setUndoError(null);
      } catch (err) {
        setUndoError(toDiagnostic(err));
      }
    },
    [mode, handleEditGroup],
  );

  // Armed synchronously before our own `scene.save` so the resulting `scene.changed` is recognized as
  // ours. Returns a cancel fn the save path calls if the command is rejected (no event will arrive).
  const handleSaveStarted = useCallback((): (() => void) => {
    expectOwnSaveRef.current = true;
    return () => {
      expectOwnSaveRef.current = false;
    };
  }, []);

  // Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z, edit mode only, and never when focus is in a text field (native
  // text-editing undo must keep working in the inspector's drafts).
  useEffect(() => {
    if (mode !== 'edit' || client === null) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent): void => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey) {
        return;
      }
      if (isTextEditingTarget(event.target)) {
        return;
      }
      const key = event.key.toLowerCase();
      if (key === 'z' && !event.shiftKey) {
        event.preventDefault();
        void handleUndo();
      } else if (key === 'y' || (key === 'z' && event.shiftKey)) {
        event.preventDefault();
        void handleRedo();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [mode, client, handleUndo, handleRedo]);

  // Installs a freshly-connected client as the live one. Swapping the `client` STATE is what makes
  // re-subscription complete by construction: every subscription lives in an effect keyed on `[client]`
  // (frames in `useFrameStream`, scene.changed + log here, the ~3 Hz overlay/inspector polls, the
  // undo-history clear), so promoting a new client tears the old subs down and opens fresh ones — new
  // sub ids, event `seq` back at 0, undo stack cleared — with no explicit re-subscribe list to keep in
  // step. Selection, edit mode, and the save path deliberately survive (the dev restore may keep entity
  // ids valid; the polls repair or clear a stale selection box on their own). The client's `onClose`
  // dispatches `lost` only while it is still the active client, so a superseded socket closing is inert.
  const promoteClient = useCallback((next: ProtocolClient, nextHello: ServerHello) => {
    clientRef.current = next;
    next.onClose(() => {
      if (clientRef.current === next) {
        dispatchRef.current({ type: 'lost' });
      }
    });
    setClient(next);
    setHello(nextHello);
    setSchemaDrift(schemaMismatch(SCHEMA_HASH, nextHello.schema_hash));
  }, []);

  // One reconnect attempt (a `connect` effect): open a fresh socket, run the handshake, and classify
  // the outcome for the machine: a failure is an ordinary retry; success promotes the client. Kept
  // off `clientRef`/`reconnectRef` reads directly so the machine stays the single driver.
  const attemptReconnect = useCallback(async () => {
    const next = new ProtocolClient();
    const outcome = await dial(next, url, dialGateRef.current);
    if (outcome.kind === 'superseded') {
      return;
    }
    if (outcome.kind === 'failed') {
      dispatchRef.current({ type: 'failed' });
      return;
    }
    clientRef.current?.close(); // drop the dead client we're replacing (no-op if already closed)
    promoteClient(next, outcome.hello);
    dispatchRef.current({ type: 'succeeded' });
  }, [url, promoteClient]);

  // The driver: fold an event through the pure machine, mirror the phase for the render, and run each
  // effect (arm the backoff timer / start a connect attempt). All timer + socket I/O lives here; the
  // transition logic is the injected-clock-testable `reduce`.
  const dispatchReconnect = useCallback(
    (event: ReconnectEvent) => {
      const { phase, effects } = reduce(reconnectRef.current, event, DEFAULT_BACKOFF, Date.now());
      reconnectRef.current = phase;
      setReconnectPhase(phase);
      for (const effect of effects) {
        if (effect.type === 'schedule') {
          clearReconnectTimer();
          reconnectTimerRef.current = window.setTimeout(() => dispatchRef.current({ type: 'timer' }), effect.delayMs);
        } else {
          void attemptReconnect();
        }
      }
    },
    [attemptReconnect, clearReconnectTimer],
  );
  // Keep the late-bound dispatch pointing at the live closure so async timer/socket callbacks use it.
  useEffect(() => {
    dispatchRef.current = dispatchReconnect;
  }, [dispatchReconnect]);

  const handleRetry = useCallback(() => dispatchRef.current({ type: 'retry' }), []);

  // Split from the form handler (`Plan/editor-open-project.md` decision 5) so the game card's Attach
  // button and the auto-attach effect below can dial a url that never passed through the form's
  // submit event.
  const connectTo = useCallback(
    (target: string) => {
      setUrl(target);
      setError(null);
      setState('connecting');
      clearReconnectTimer();
      reconnectRef.current = CONNECTED;
      setReconnectPhase(CONNECTED);

      const next = new ProtocolClient();
      void dial(next, target, dialGateRef.current).then((outcome) => {
        if (outcome.kind === 'superseded') {
          return;
        }
        if (outcome.kind === 'failed') {
          const err = outcome.error;
          setState('error');
          setError(err instanceof Error ? err.message : String(err));
          return;
        }
        promoteClient(next, outcome.hello);
        setState('connected');
      });
    },
    [clearReconnectTimer, promoteClient],
  );

  const handleConnect = useCallback(
    (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      connectTo(url);
    },
    [url, connectTo],
  );

  const handleDisconnect = useCallback(() => {
    teardown();
    setState('idle');
  }, [teardown]);

  // Clear any armed backoff timer if the whole app unmounts mid-reconnect.
  useEffect(
    () => () => {
      dialGateRef.current.invalidate();
      clearReconnectTimer();
    },
    [clearReconnectTimer],
  );

  // The game session's other half of `beforeStop`: once it is listening, attach to it as the main
  // session (unless one is already up or mid-connect); once it dies at the url we are attached to,
  // drop the session so the card — not a stuck "reconnecting…" banner — shows why.
  //
  // `lastAutoAttachedRef` is the exact `StageStatus` object last attached for: `state` settling to
  // `error` (a failed attach) or `idle` (a human Disconnect) both re-run this effect, and without the
  // ref an attach would retry forever on the same listening status — or undo the very Disconnect the
  // human just asked for. Set before `connectTo`, so a second run before it resolves is a no-op too.
  const lastAutoAttachedRef = useRef<StageStatus | null>(null);
  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    const attachUrl = autoAttachUrl(gameStatus, state, lastAutoAttachedRef.current);
    if (attachUrl !== null) {
      lastAutoAttachedRef.current = gameStatus;
      connectTo(attachUrl);
      return;
    }
    if (dropsSession(gameStatus, state === 'connected' ? url : null)) {
      teardown();
      setState('idle');
    }
  }, [gameStatus, state, url, connectTo, teardown]);

  // The tool layout stays mounted through a reconnect: the (now-dead) client is kept in state so the
  // last frame stays painted and selection/mode survive, while `connection` drives the banner + the
  // viewport's "reconnecting…" overlay so the human sees the blink rather than a silently dead viewport.
  const connection = reconnectStatus(reconnectPhase);

  // The one gate that used to wrap the whole app; now it only decides which main row renders.
  // Narrowed into an object because a boolean would not narrow `client`/`hello` for the JSX below.
  const session = state === 'connected' && client !== null && hello !== null ? { client, hello } : null;
  const sceneName = docParts(savePath).scene;
  const projectName = project !== null && 'name' in project ? project.name : null;
  const openProjectSwitcher = useCallback(() => setSwitcherOpen(true), []);
  // The content root the booted scene resolves paths against — the scene's own directory
  // (`builder.rs`'s `load_scene_file`), not a hardcoded `<project>/scenes`. `validProject.dir ===
  // projectDir` guards the same lag `gameStartedForRef` guards against: a switch's `project_info`
  // answer for the OLD directory must not compute a root under the NEW one. `''` (never rendered by
  // `ScenePicker`) until a validated project answers.
  const sceneRoot =
    validProject !== null && validProject.dir === projectDir
      ? join(validProject.dir, dirname(validProject.defaultScene))
      : '';
  // The connect panel's run-game card. Outside Tauri there is no host to spawn a game with, so the
  // panel gets no `game` prop at all rather than a card with no working actions.
  const gameCardResult = gameCard({ projectName, status: gameStatus, error: gameError, connection: state });

  return (
    <div className="app">
      <Titlebar
        scenePath={savePath}
        project={project}
        projectDir={projectDir}
        repoRoot={repoRoot ?? ''}
        recents={recents}
        open={switcherOpen}
        onOpenChange={setSwitcherOpen}
        onSwitch={switchProjectDir}
      />
      <ViewSwitcher active={activeView} onSelect={setActiveView} />
      {/* The session strip is the live session's own controls (mode, save, undo, disconnect), so
          it has nothing to show — and nothing to act on — without one. */}
      {session !== null && (
        <SessionStrip
          client={session.client}
          hello={session.hello}
          url={url}
          sceneRoot={sceneRoot}
          onDisconnect={handleDisconnect}
          mode={mode}
          onModeChange={setMode}
          dirty={dirty}
          savePath={savePath}
          onSaved={() => setDirty(false)}
          onSaveStarted={handleSaveStarted}
          canUndo={historyRef.current.canUndo}
          canRedo={historyRef.current.canRedo}
          onUndo={handleUndo}
          onRedo={handleRedo}
          onError={setSessionError}
        />
      )}
      {/* A dedicated strip (collapses when empty) so a visible banner never steals the main row's
          flex space. */}
      <div className="app-banner">
        <DiagnosticBanner diagnostic={schemaDrift} />
        {connection === 'reconnecting' && (
          <div className="reconnect-banner" role="status" aria-live="polite">
            <span className="reconnect-spinner" aria-hidden="true" />
            Reconnecting to the engine…
          </div>
        )}
        {connection === 'failed' && (
          <div className="reconnect-banner reconnect-failed" role="alert">
            <span>Lost the connection to the engine and gave up reconnecting.</span>
            <button type="button" className="reconnect-retry" onClick={handleRetry}>
              Retry
            </button>
          </div>
        )}
        <DiagnosticBanner diagnostic={sessionError} onDismiss={() => setSessionError(null)} />
        <DiagnosticBanner diagnostic={undoError} onDismiss={() => setUndoError(null)} />
        <DiagnosticBanner diagnostic={logDiagnostic} onDismiss={() => setLogDiagnostic(null)} />
      </div>

      {activeView === 'voxel-artist' ? (
        /* The Voxel Artist owns its own engine (the host-spawned model stage), so it renders
           whether or not a game session exists — and never reads `session`. */
        <VoxelArtistView
          projectDir={projectDir}
          projectName={projectName}
          onOpenProjectSwitcher={openProjectSwitcher}
          viewId={activeView}
          onVoxelContext={setVoxelContext}
          onFrameStatus={setFrameStatus}
          onStatusNote={setStatusNote}
        />
      ) : activeView === 'models' ? (
        /* The Models view stands on the very same host-spawned model stage, so it too renders
           whether or not a game session exists — and never reads `session`. */
        <ModelsView
          projectDir={projectDir}
          projectName={projectName}
          onOpenProjectSwitcher={openProjectSwitcher}
          viewId={activeView}
          onFrameStatus={setFrameStatus}
          onStatusNote={setStatusNote}
        />
      ) : activeView === 'generation' ? (
        /* The Generation view drives `ludion gen` as a subprocess (the host spawns it, exactly as
           it spawns a stage engine), so it has no engine session at all — no `session`, no stage,
           no frame stream. */
        <GenerationView projectDir={projectDir} onStatusNote={setStatusNote} />
      ) : activeView === 'effects-artist' ? (
        /* The Effects view owns its own engine too (the host-spawned effects stage), so it renders
           whether or not a game session exists — and never reads `session`. */
        <EffectsView
          projectDir={projectDir}
          projectName={projectName}
          onOpenProjectSwitcher={openProjectSwitcher}
          viewId={activeView}
          onFrameStatus={setFrameStatus}
          onStatusNote={setStatusNote}
        />
      ) : session === null ? (
        /* No game session: the shell still stands. The connect card takes the viewport slot and
           the terminal drawer keeps its column, so a persona can be spawned (and can start the
           very server this card is waiting for) before anything is connected. */
        <div className="main-row">
          <div className="col-center">
            <ConnectPanel
              url={url}
              state={state}
              error={error}
              onUrlChange={setUrl}
              onSubmit={handleConnect}
              game={isTauri() ? gameCardResult : undefined}
              onRunGame={() => void startGame(false)}
              onAttachGame={() => connectTo(gameStatus.url ?? DEFAULT_URL)}
              onReplace={() => void startGame(true)}
            />
          </div>
          <div className="col col-term">
            <TerminalDrawer
              url={url}
              viewId={activeView}
              scenePath={sceneName ?? ''}
              projectDir={projectDir}
              projectName={projectName}
              onOpenProjectSwitcher={openProjectSwitcher}
            />
          </div>
        </div>
      ) : activeView === 'game-designer' ? (
        <GameDesignerView
          client={session.client}
          mode={mode}
          selection={selection}
          onSelect={setSelection}
          onMutation={handleMutation}
          onEntityCount={setEntityCount}
          sceneName={sceneName}
          selectionComponents={selectionComponents}
          onEditEntry={handleEditEntry}
          onEditGroup={handleEditGroup}
          onDirty={handleDirty}
          connection={connection}
          onFrameStatus={setFrameStatus}
          onCamera={setCameraContext}
          pointer={pointer}
          onPointerChange={setPointer}
          url={url}
          viewId={activeView}
          projectDir={projectDir}
          projectName={projectName}
          onOpenProjectSwitcher={openProjectSwitcher}
        />
      ) : activeView === 'performance' ? (
        /* The Performance view is the `profile` channel, which arms the engine's profiler by being
           subscribed to, plus a small preview of the running game. No selection, no gizmo, no mutation
           path — so none of that is threaded through it; `mode` is, because the preview forwards input
           on the same play-mode gate every other frame surface uses. */
        <PerformanceView
          client={session.client}
          mode={mode}
          url={url}
          sceneName={sceneName}
          projectDir={projectDir}
          onFrameStatus={setFrameStatus}
        />
      ) : activeView === 'lighter' ? (
        <LighterView
          client={session.client}
          mode={mode}
          selection={selection}
          onSelect={setSelection}
          onMutation={handleMutation}
          sceneName={sceneName}
          onLighterContext={setLighterContext}
          onEditEntry={handleEditEntry}
          onDirty={handleDirty}
          connection={connection}
          onFrameStatus={setFrameStatus}
          onCamera={setCameraContext}
          pointer={pointer}
          onPointerChange={setPointer}
          url={url}
          viewId={activeView}
          projectDir={projectDir}
          projectName={projectName}
          onOpenProjectSwitcher={openProjectSwitcher}
        />
      ) : activeView === 'art-director' ? (
        <ArtDirectorView
          client={session.client}
          mode={mode}
          selection={selection}
          onSelect={setSelection}
          onMutation={handleMutation}
          sceneName={sceneName}
          onArtContext={setArtContext}
          onEditEntry={handleEditEntry}
          onDirty={handleDirty}
          connection={connection}
          onFrameStatus={setFrameStatus}
          onCamera={setCameraContext}
          pointer={pointer}
          onPointerChange={setPointer}
          url={url}
          viewId={activeView}
          projectDir={projectDir}
          projectName={projectName}
          onOpenProjectSwitcher={openProjectSwitcher}
        />
      ) : (
        <div className="main-row">
          {/* Left column: hierarchy over inspector. */}
          <div className="col col-side col-side-level">
            <HierarchyPanel
              client={session.client}
              mode={mode}
              selection={selection}
              onSelect={setSelection}
              onEditGroup={handleEditGroup}
              onEntityCount={setEntityCount}
              sceneName={sceneName}
            />
            <PrefabBrowser root={sceneRoot} mode={mode} onPlace={(rel, x, y) => void handlePlacePrefab(rel, x, y)} />
            <Inspector client={session.client} selection={selection} onMutation={handleMutation} mode={mode} />
          </div>

          {/* Center column: viewport (+ relocated editor controls) over the kit shelf. */}
          <div className="col-center">
            <Viewport
              client={session.client}
              mode={mode}
              selection={selection}
              onSelect={setSelection}
              onEditEntry={handleEditEntry}
              onDirty={handleDirty}
              connection={connection}
              scenePath={sceneName}
              onFrameStatus={setFrameStatus}
              onCamera={setCameraContext}
              pointer={pointer}
              onPointerChange={setPointer}
              placeResolverRef={placeResolverRef}
            />
            <PalettePanel
              mode={mode}
              selection={selection}
              onSpawn={(command) => void handleSpawn(command)}
              onDuplicate={() => void handleDuplicate()}
              onDestroy={() => void handleDestroy()}
            />
          </div>

          {/* Right column: the persona terminal drawer (peer-client `claude` instances). */}
          <div className="col col-term">
            <TerminalDrawer
              url={url}
              viewId="level-designer"
              scenePath={sceneName ?? ''}
              projectDir={projectDir}
              projectName={projectName}
              onOpenProjectSwitcher={openProjectSwitcher}
            />
          </div>
        </div>
      )}

      <StatusBar
        mode={mode}
        dirty={dirty}
        entityCount={entityCount}
        pointer={pointer}
        note={statusNote}
      />
    </div>
  );
}
