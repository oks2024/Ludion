// The connect panel's run-game card, as pure data (`Plan/editor-open-project.md` decision 5):
// what `StageOpts` starts the game slot on, what the card shows for every `StageStatus` the game
// slot can report, and when the game's own listener is worth auto-attaching the main session to or
// dropping it from.
//
// Pure: no React, no Tauri, no DOM — same posture as `editor/voxel/stage.ts`'s `stageSlot`, whose
// card shape this reuses (`StageSlot` here names that card, never `host.ts`'s slot-kind union).

import { join, type StageSlot as StageCard } from '../editor/voxel/stage';
import type { StageOpts, StageStatus } from './host';

/** The main session's connection state, as `App` tracks it — owned here (not `App.tsx`) since
 *  `autoAttachUrl` is a function of it. */
export type ConnectionState = 'idle' | 'connecting' | 'connected' | 'error';

/** What the run-game card offers, alongside its `StageSlot` card. */
export type GameAction = 'run' | 'attach' | 'replace';

/** The game slot's `StageOpts`, from a validated project: `defaultScene` is already `scenes/…`-
 *  relative (`project::ProjectInfo`), so `join` alone makes it absolute. */
export function gameOpts(projectDir: string, defaultScene: string): StageOpts {
  return { scene: join(projectDir, defaultScene), project: projectDir };
}

/** The run-game card for the connect panel's game slot, and which of Run/Attach/Replace it offers.
 *
 *  `error` is a `stageStart` rejection (the host never reports these as a status event, same as
 *  `stageSlot`'s own `error` input). `projectName === null` means there is no valid project to run
 *  yet, checked first so a stale status from the previous project never shows through. `connection`
 *  is the main session's own state: while `listening`, it decides whether the auto-attach is still
 *  in flight (`connecting`/`connected`, the busy "Attaching…" card) or spent (`idle`/`error` — a
 *  failed attach or a human Disconnect), which needs its own Attach action rather than a permanent
 *  spinner with no way back in. */
export function gameCard(input: {
  readonly projectName: string | null;
  readonly status: StageStatus;
  readonly error: string | null;
  readonly connection: ConnectionState;
}): { slot: StageCard; actions: readonly GameAction[] } {
  const { projectName, status, error, connection } = input;
  if (projectName === null) {
    return {
      slot: {
        tone: 'idle',
        title: 'Open a project to run its game',
        detail: 'The title bar’s project switcher chooses which project’s game this card runs.',
        canRestart: false,
        canAddScenes: false,
      },
      actions: [],
    };
  }
  if (error !== null) {
    return {
      slot: { tone: 'error', title: 'The game could not be started', detail: error, canRestart: false, canAddScenes: false },
      actions: ['run'],
    };
  }
  switch (status.phase) {
    case 'stopped':
      return {
        slot: {
          tone: 'idle',
          title: 'The game is not running',
          detail: `Run it to start playing and editing ${projectName} live.`,
          canRestart: false,
          canAddScenes: false,
        },
        actions: ['run'],
      };
    case 'starting':
      return {
        slot: {
          tone: 'busy',
          title: `Starting ${projectName}…`,
          detail: `The host is building and launching ${projectName}’s own binary. A cold build compiles it first, so the first start can take several minutes.`,
          canRestart: false,
          canAddScenes: false,
        },
        actions: [],
      };
    case 'listening':
      if (connection === 'connecting' || connection === 'connected') {
        return {
          slot: {
            tone: 'busy',
            title: 'Attaching to the game…',
            detail: `Opening the wire connection to ${status.url ?? 'the game session'}.`,
            canRestart: false,
            canAddScenes: false,
          },
          actions: [],
        };
      }
      return {
        slot: {
          tone: 'idle',
          title: 'The game is running',
          detail: `Attach connects the editor to ${status.url ?? 'the game session'}.`,
          canRestart: false,
          canAddScenes: false,
        },
        actions: ['attach'],
      };
    case 'failed':
      return {
        slot: {
          tone: 'error',
          title: 'The game failed to build',
          detail: status.message ?? 'The game exited before it announced a url, and printed nothing to explain it.',
          canRestart: false,
          canAddScenes: false,
        },
        actions: ['run'],
      };
    case 'exited':
      return status.code === 0
        ? {
            slot: {
              tone: 'idle',
              title: 'The game exited',
              detail: 'It shut down cleanly. Run it again any time.',
              canRestart: false,
              canAddScenes: false,
            },
            actions: ['run'],
          }
        : {
            slot: {
              tone: 'error',
              title: status.code === null ? 'The game exited' : `The game exited (code ${status.code})`,
              detail: status.message ?? 'The game is gone, and printed nothing to explain it.',
              canRestart: false,
              canAddScenes: false,
            },
            actions: ['run'],
          };
    case 'busy':
      if (status.listener?.kind === 'ludion') {
        return {
          slot: {
            tone: 'idle',
            title: `Already serving a Ludion engine (${status.listener.engineVersion})`,
            detail: `Attach to use it as-is, or Replace to ask it to shut down first and start ${projectName}’s own game.`,
            canRestart: false,
            canAddScenes: false,
          },
          actions: ['attach', 'replace'],
        };
      }
      return {
        slot: {
          tone: 'error',
          title: 'The game’s port is held by something else',
          detail: status.message ?? 'Something that is not a Ludion engine is already listening there.',
          canRestart: false,
          canAddScenes: false,
        },
        actions: ['attach'],
      };
  }
}

/** The url worth auto-attaching the main session to: the game just started listening, the main
 *  session is not already connected or mid-connect (either would make a second attach pointless or
 *  disruptive), and `status` is not `lastAttached` — the exact `StageStatus` object last auto-
 *  attached for, by reference. Without that last check a failed attach (which settles `state` back
 *  to `error`) or a human Disconnect (which settles it to `idle`) would both re-trigger an attach to
 *  the SAME listening status on every subsequent render, since neither `error` nor `idle` is
 *  `connected`/`connecting`. A caller re-arms eligibility only by passing a genuinely new status
 *  object (the next `listening` event). `null` otherwise. */
export function autoAttachUrl(status: StageStatus, connection: ConnectionState, lastAttached: StageStatus | null): string | null {
  if (status.phase !== 'listening' || connection === 'connected' || connection === 'connecting' || status === lastAttached) {
    return null;
  }
  return status.url;
}

/** Whether the game just dying should drop the main session: it exited or failed, and its url is the
 *  one the session is attached to. Reconnecting to a dead game is pointless, and dropping the
 *  session is what lets the card show the failure's tail instead of a stuck "reconnecting…" banner. */
export function dropsSession(status: StageStatus, sessionUrl: string | null): boolean {
  return (status.phase === 'exited' || status.phase === 'failed') && status.url !== null && status.url === sessionUrl;
}
