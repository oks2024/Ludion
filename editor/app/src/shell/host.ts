// Typed wrappers over the Tauri host commands that back the Voxel Artist view
// (`src-tauri/src/files.rs` + `src-tauri/src/stage.rs`): recursive directory listing, the
// stage-session lifecycle, and the `check-scene` subprocess. Same posture as `terminal/pty.ts` —
// everything Tauri-specific funnels through one module so components never touch `invoke`/`listen`.
//
// A stage session is an EXTRA `ludion serve`, owned by the host and separate from the game session
// the rest of the editor attaches to. There are three, one per slot — the model stage the Voxel
// Artist serves, the effects stage the Effects view serves, and the game slot, which is the open
// project's own binary, host-spawned the same way (decision 5, `Plan/editor-open-project.md`) — all
// driven by one host implementation and told apart by the `slot` every command takes and every
// status carries. Each is lazy (nothing runs until its `stageStart`), all three survive view
// switches and run at once, and the host tears them down on app exit.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** One entry of a `listEditorDir` listing (`files::DirEntry`). */
export interface DirEntry {
  /** Path relative to the listed root, always `/`-separated. */
  readonly relPath: string;
  /** The final path segment. */
  readonly name: string;
  /** Directories are always listed, whatever the extension filter says. */
  readonly isDir: boolean;
}

/** List `path` recursively, depth-first, directories before their contents and before sibling
 *  files. `ext` is a filename *suffix* (`.voxel.yaml`), matched case-insensitively; omit it to keep
 *  every file. Rejects with a readable message when the path is missing or is not a directory. */
export function listEditorDir(path: string, ext?: string): Promise<DirEntry[]> {
  return invoke('list_editor_dir', { path, ext });
}

/** Which stage session a command addresses (`stage::StageSlot`). One host per slot, all three live. */
export type StageSlot = 'model' | 'effects' | 'game';

/** The stage slots the project switcher stops on its own — the game slot joins them there too, but
 *  through its own `beforeStop` hook (`projectSwitch.ts`), since dropping the main session has to
 *  happen before it stops. */
export const STAGE_SLOTS: readonly StageSlot[] = ['model', 'effects'];

/** The game slot, named once so callers never spell the string literal. */
export const GAME_SLOT: StageSlot = 'game';

/** The stage session's lifecycle state (`stage::StagePhase`). `busy` is game-only: the slot's port
 *  was already held by something else, so nothing was spawned. */
export type StagePhase = 'stopped' | 'starting' | 'listening' | 'failed' | 'exited' | 'busy';

/** What holds a busy port (`stage::Listener`), present only on a `busy` status. */
export type Listener = { readonly kind: 'ludion'; readonly engineVersion: string } | { readonly kind: 'foreign' };

/** A snapshot of one stage session (`stage::StageStatus`) — the return of every stage command and
 *  the payload of `stage://status`. */
export interface StageStatus {
  /** Which session this describes: the event stream is shared, so every listener filters on it. */
  readonly slot: StageSlot;
  readonly phase: StagePhase;
  /** `ws://host:port`, present from `listening` onward, and on `busy` (the address that was probed). */
  readonly url: string | null;
  /** The stage scene this session was started on. */
  readonly scene: string | null;
  /** A readable explanation — the child's stderr tail on `failed`, on a non-zero `exited`, or the
   *  busy-port message on `busy`. */
  readonly message: string | null;
  /** The child's exit code, once it exited. */
  readonly code: number | null;
  /** What holds the slot's port, present only on `busy`. */
  readonly listener: Listener | null;
}

/** Start/restart parameters (`stage::StageOpts`). */
export interface StageOpts {
  /** Absolute path to the stage scene, e.g. `<projectDir>/scenes/model-stage.yaml`. */
  readonly scene: string;
  /** `host:port` to bind; defaults host-side to the slot's port (model `127.0.0.1:9412`, effects
   *  `127.0.0.1:9413`, game `127.0.0.1:9411`). */
  readonly addr?: string;
  /** Absolute path to the project directory. Required for the game slot (its binary and cwd are the
   *  open project's own); unused for the stages. */
  readonly project?: string;
}

/** Start `slot`'s stage session if it is not already up, and return the status *at that moment*.
 *
 *  Idempotent — an existing `starting`/`listening` session is returned untouched, so the view can
 *  call this on every mount. It returns immediately: a cold build compiles first, so
 *  readiness arrives later as a `listening` status event, never as this promise resolving. Rejects
 *  only when the session could not be started at all (missing stage scene, no cargo, or the game
 *  slot with no `project`).
 *
 *  `replace` matters only for the game slot: a busy port serving a Ludion engine is reclaimed first
 *  when `true`; a foreign listener is never reclaimed regardless, and every other slot ignores it. */
export function stageStart(slot: StageSlot, opts: StageOpts, replace = false): Promise<StageStatus> {
  return invoke('stage_start', { slot, opts, replace });
}

/** `slot`'s current status. Use it to re-sync after a remount instead of replaying events. */
export function stageStatus(slot: StageSlot): Promise<StageStatus> {
  return invoke('stage_status', { slot });
}

/** Stop `slot`'s running session and start a fresh one on `opts`. Blocking host-side (bounded by
 *  the graceful-teardown budget); the new session's readiness still arrives as an event. */
export function stageRestart(slot: StageSlot, opts: StageOpts): Promise<StageStatus> {
  return invoke('stage_restart', { slot, opts });
}

/** Stop `slot`'s session and wait for it to actually go down (graceful `engine.shutdown`, kill
 *  fallback); a no-op for a stopped slot. Blocking host-side, off the UI thread. */
export function stageStop(slot: StageSlot): Promise<void> {
  return invoke('stage_stop', { slot });
}

/** Subscribe to `slot`'s lifecycle transitions. Every transition is a full status snapshot; the
 *  host emits one event stream for every slot, so foreign payloads are dropped here. */
export function onStageStatus(
  slot: StageSlot,
  handler: (status: StageStatus) => void,
): Promise<UnlistenFn> {
  return listen<StageStatus>('stage://status', (event) => {
    if (event.payload.slot === slot) {
      handler(event.payload);
    }
  });
}

/** The raw result of a `ludion check-scene` run (`stage::CheckSceneResult`). */
export interface CheckSceneResult {
  readonly stdout: string;
  readonly stderr: string;
  /** `null` only if the process was killed by a signal. */
  readonly exitCode: number | null;
}

/** Run `ludion check-scene <path> --json` and return its output verbatim.
 *
 *  This is the model readout's ONLY data source: one checker, the engine's. Never reimplement
 *  voxel size/fill/connectivity parsing in TS — read the summary object on stdout. Callers debounce; the host does
 *  not (each call is a fresh subprocess). */
export function runCheckScene(path: string): Promise<CheckSceneResult> {
  return invoke('run_check_scene', { path });
}
