// The Voxel Artist view's session plumbing, as pure data: where the stage scene and the model files
// live, what a `StageStatus` means on screen, and when a throttled camera write is due.
//
// The view owns a SECOND engine (`shell/host.ts`'s stage session), so none of this touches the game
// session's url, client, or scene. Two properties are worth stating because they shape every
// function here:
//
//   - **Readiness is an event.** `stageStart` returns `starting` and a cold build compiles the
//     engine first, which can take minutes. `stageSlot` therefore has to SAY that — a spinner with
//     no explanation reads as a hang, and this is the one screen where the honest answer is "it is
//     building".
//   - **The content root is the stage scene's directory** (`ludion-assets`' `resolve` is a plain
//     `root.join`), i.e. `<projectDir>/scenes`. So a model at `<projectDir>/scenes/voxels/x.voxel.
//     yaml` is referenced on the wire as exactly `voxels/x.voxel.yaml` — byte-identical to how a
//     real scene names it, which is the whole reason the stage is committed game content.
//
// Pure: no React, no Tauri, no DOM.

import type { StageStatus } from '../../shell/host';
import { displayName, matchingExt } from './tree';

// --- Rigs ------------------------------------------------------------------------------------- //

/** The two stage rigs. Neutral reads SHAPE (a hand-tuned approximation of the contact sheet's
 *  key+fill); night reads VALUE under the game's own `env-night.yaml` — the rig that retires the
 *  palette-calibration arithmetic. Switched with `scene.load`, safe only because nothing in the
 *  stage is ever edited. */
export type Rig = 'neutral' | 'night';

export interface RigDef {
  readonly id: Rig;
  readonly label: string;
  /** The scene file's basename, under `<projectDir>/scenes/`. */
  readonly file: string;
}

export const RIGS: readonly RigDef[] = [
  { id: 'neutral', label: 'neutral', file: 'model-stage.yaml' },
  { id: 'night', label: 'night', file: 'model-stage-night.yaml' },
];

export function rigDef(rig: Rig): RigDef {
  return RIGS.find((entry) => entry.id === rig) ?? RIGS[0];
}

// --- Paths ------------------------------------------------------------------------------------ //

/** Joins path segments with `/`, tolerating a Windows-style project dir and stray separators.
 *  The host accepts either separator; `/` keeps every string the editor shows (and writes into the
 *  context file) in one shape. */
export function join(...segments: readonly string[]): string {
  return segments
    .map((segment) => segment.replace(/\\/g, '/'))
    .map((segment, index) => (index === 0 ? segment.replace(/\/+$/, '') : segment.replace(/^\/+|\/+$/g, '')))
    .filter((segment) => segment !== '')
    .join('/');
}

/** The absolute path of a rig's stage scene. */
export function stageScenePath(projectDir: string, rig: Rig): string {
  return join(projectDir, 'scenes', rigDef(rig).file);
}

/** The stage scene's basename without its extension — the label the titlebar/drawer shows. */
export function stageSceneName(rig: Rig): string {
  return rigDef(rig).file.replace(/\.yaml$/, '');
}

/** The voxel format's double extension. */
export const VOXEL_EXT = '.voxel.yaml';

/** The assembly format's double extension. `.part.yaml` and `.anim.yaml` are deliberately absent:
 *  a part carries no palette and a clip no body, so neither means anything outside the assembly
 *  that binds it — they are reached through it. */
export const ASSEMBLY_EXT = '.assembly.yaml';

/** Everything the asset browser lists, as one module-level array. The browser re-lists whenever this
 *  changes identity, so a caller must pass THIS rather than an inline literal. */
export const MODEL_EXTS: readonly string[] = [VOXEL_EXT, ASSEMBLY_EXT];

/** Which of the two listed kinds a library path names. Anything that is not an assembly reads as a
 *  voxel model: only these two are listed, and what a file really is remains the engine's verdict
 *  (`readout.ts`), never this function's. */
export type ModelKind = 'voxel' | 'assembly';

export function modelKind(relPath: string): ModelKind {
  return matchingExt(relPath, MODEL_EXTS) === ASSEMBLY_EXT ? 'assembly' : 'voxel';
}

/** What the stage stands an assembly at. An assembly declares no `scale:`, and the stage rig — the
 *  ground plane, the key/fill distances, the shadow extent, the camera home — is built around a
 *  subject a few units tall, so an unscaled 32-voxel rig would be lit by a rig ten times too small
 *  for it. This is the project's content-density convention, 16 voxels per world unit
 *  (`Plan/roadmap-voxel-mesh-pbr.md` decision 11), NOT any game's placement: vigil stands its player
 *  at 0.125, and a scene's own `Transform` is what decides that. */
export const ASSEMBLY_STAGE_SCALE = 0.0625;

/** The directory the asset browser lists (and the host watches). */
export function voxelsRoot(projectDir: string): string {
  return join(projectDir, 'scenes', 'voxels');
}

/** The absolute path of a model, from its `voxelsRoot`-relative path — what `runCheckScene` and
 *  `readEditorFile` take. */
export function modelFilePath(projectDir: string, relPath: string): string {
  return join(voxelsRoot(projectDir), relPath);
}

/** The model's asset reference as a scene names it: `voxels/<relPath>`, resolved against the stage
 *  scene's own directory. This is the value written into `Mesh.shape`. */
export function modelAssetRef(relPath: string): string {
  return join('voxels', relPath);
}

/** The `Mesh.shape` value that stages a library path. `shape` is externally tagged, so the kind IS
 *  the key — `{ voxel: … }` or `{ assembly: … }`, byte-identical to how a real scene names the same
 *  model. */
export function modelShape(relPath: string): Record<string, string> {
  const ref = modelAssetRef(relPath);
  return modelKind(relPath) === 'assembly' ? { assembly: ref } : { voxel: ref };
}

/** The model's display name — the basename with its format's double extension stripped. */
export function modelDisplayName(relPath: string): string {
  return displayName(relPath, MODEL_EXTS);
}

// --- Phase → UI --------------------------------------------------------------------------------//

export type StageTone = 'idle' | 'busy' | 'error';

/** The prefix `Stage::start_with` (`src-tauri/src/stage.rs`) puts on a missing-scene refusal —
 *  the one `stageSlot` error string that means "Add stage scenes" can fix it. */
const MISSING_STAGE_SCENE_PREFIX = 'no scene at ';

/** Whether a start rejection is the host's missing-scene refusal, as opposed to any other failure
 *  (no cargo, a busy port, …) that `stageSlot` also reports through `error`. */
export function isMissingStageScene(error: string | null): boolean {
  return error !== null && error.startsWith(MISSING_STAGE_SCENE_PREFIX);
}

/** What the viewport slot shows INSTEAD of the canvas. `null` from `stageSlot` means the stage is
 *  live and the canvas renders. */
export interface StageSlot {
  readonly tone: StageTone;
  readonly title: string;
  readonly detail: string;
  /** Whether to offer the restart control (pointless while something is already starting). */
  readonly canRestart: boolean;
  /** Whether to offer **Add stage scenes** — only on the missing-scene refusal. */
  readonly canAddScenes: boolean;
}

export interface StageSlotInput {
  /** False in the browser dev page, where the host commands do not exist. */
  readonly isDesktop: boolean;
  readonly projectDir: string;
  readonly status: StageStatus;
  /** Whether the view's own `ProtocolClient` has completed its handshake with `status.url`. */
  readonly connected: boolean;
  /** A rejection from `stageStart`/`stageRestart` itself (missing stage scene, no cargo), or a
   *  failed dial — the host never reports these as a status event. */
  readonly error: string | null;
  /** The human-readable stage name every status string below builds its title from (e.g. "the
   *  model stage is not running"). Defaults to `'model stage'`, reproducing this module's own
   *  copy unchanged. */
  readonly stageName?: string;
}

/**
 * The state card for the viewport slot, or `null` when the stage is listening AND dialed — the one
 * case where the canvas has something to paint.
 *
 * Every phase gets an honest answer; `starting` in particular says a cold build compiles first,
 * because that is minutes of silence the human would otherwise read as a hang.
 */
export function stageSlot(input: StageSlotInput): StageSlot | null {
  const { status } = input;
  const stageName = input.stageName ?? 'model stage';
  if (!input.isDesktop) {
    return {
      tone: 'idle',
      title: `The ${stageName} needs the desktop editor`,
      detail:
        'The stage is a second `ludion serve` the Tauri host owns. Open the editor in the desktop shell (not the browser dev page) to use this view.',
      canRestart: false,
      canAddScenes: false,
    };
  }
  if (input.projectDir === '') {
    return {
      tone: 'idle',
      title: 'Set a project directory',
      detail:
        'The stage scene and the model libraries are read from it (`<projectDir>/scenes/model-stage.yaml`, with `<projectDir>/scenes/voxels` and `<projectDir>/scenes/sculpts`). Open a project from the title bar.',
      canRestart: false,
      canAddScenes: false,
    };
  }
  if (input.error !== null) {
    if (isMissingStageScene(input.error)) {
      return {
        tone: 'error',
        title: `The ${stageName} scene is missing`,
        detail: `${input.error} Add stage scenes copies the engine's template stage scenes into this project; existing files are never overwritten.`,
        canRestart: true,
        canAddScenes: true,
      };
    }
    return {
      tone: 'error',
      title: `The ${stageName} could not be started`,
      detail: input.error,
      canRestart: true,
      canAddScenes: false,
    };
  }
  switch (status.phase) {
    case 'stopped':
      return {
        tone: 'idle',
        title: `The ${stageName} is not running`,
        detail: 'Start it to put a model on screen. Nothing in the stage is ever edited, so it is always safe to restart.',
        canRestart: true,
        canAddScenes: false,
      };
    case 'starting':
      return {
        tone: 'busy',
        title: `Starting the ${stageName}…`,
        detail:
          'The host is building and launching a second `ludion serve`. A cold build compiles the engine first, so the first start of a session can take several minutes; the viewport appears the moment it announces its url.',
        canRestart: false,
        canAddScenes: false,
      };
    case 'failed':
      return {
        tone: 'error',
        title: `The ${stageName} failed to start`,
        detail: status.message ?? 'The stage engine exited before it announced a url, and printed nothing to explain it.',
        canRestart: true,
        canAddScenes: false,
      };
    case 'exited':
      return {
        tone: 'error',
        title: status.code === null ? `The ${stageName} exited` : `The ${stageName} exited (code ${status.code})`,
        detail: status.message ?? 'The stage engine is gone. Restarting it costs a rebuild at most — it holds no unsaved state.',
        canRestart: true,
        canAddScenes: false,
      };
    case 'listening':
      return input.connected
        ? null
        : {
            tone: 'busy',
            title: `Dialing the ${stageName}…`,
            detail: `Opening the wire connection to ${status.url ?? 'the stage session'}.`,
            canRestart: false,
            canAddScenes: false,
          };
    case 'busy':
      // A stage's port is held by another editor's stage or an engine no editor left behind; the
      // host reclaims only an orphan its own record names. Rendered like `failed`, with the
      // host's message naming the remedy.
      return {
        tone: 'error',
        title: `The ${stageName} is busy`,
        detail: status.message ?? `${status.url ?? 'its port'} is already in use.`,
        canRestart: true,
        canAddScenes: false,
      };
  }
}

// --- Throttles -------------------------------------------------------------------------------- //

/** How often an interactive camera pose is written to the stage (~16 Hz). The same latest-wins
 *  posture as the gizmo drag: intermediate poses are dropped, and the gesture's final pose is always
 *  sent unthrottled, so the camera can never come to rest somewhere the human did not leave it. */
export const CAMERA_SEND_MS = 60;

/** Whether a throttled write is due. `null` (nothing sent yet) is always due. */
export function sendDue(lastSentMs: number | null, now: number, intervalMs = CAMERA_SEND_MS): boolean {
  return lastSentMs === null || now - lastSentMs >= intervalMs;
}

/** The floor between two `voxel` context-block emissions.
 *
 *  A plain debounce cannot be used here: an orbit drag — and a running turntable especially —
 *  changes the camera pose continuously, so a debounce would keep rescheduling and the persona would
 *  never see a pose at all. This is a trailing throttle instead. */
export const CONTEXT_MIN_MS = 500;

/** How long to wait before the next emission: zero once the floor has already elapsed, otherwise the
 *  remainder. Rescheduling on every change converges on `lastEmitMs + minMs`. Clamped to `[0,
 *  minMs]`, so no clock reading can stall an emission for longer than the floor itself. */
export function emitDelay(lastEmitMs: number, now: number, minMs = CONTEXT_MIN_MS): number {
  return Math.min(minMs, Math.max(0, minMs - (now - lastEmitMs)));
}

/** Turntable cadence: 20 Hz at 0.9° a tick — one revolution in 20 s, slow enough to read a
 *  silhouette and fast enough not to feel broken. */
export const TURNTABLE_TICK_MS = 50;
export const TURNTABLE_DEG_PER_TICK = 0.9;

/** How long the readout waits for the typing/saving to stop before shelling out to `check-scene`.
 *  The host does not debounce (each call is a fresh subprocess), so this is the only guard. */
export const READOUT_DEBOUNCE_MS = 250;

/** How often the selected model's mtime is polled, so an edit by the persona refreshes the readout
 *  and the palette without a click. Matches the code panel's own change poll. */
export const MODEL_POLL_MS = 1000;
