// The editor's ONE project switch point (decision 4): every host-spawned, project-bound process is
// stopped here before the new project is committed — the two stage slots, and the game slot, which
// runs the open project's own binary (`Plan/editor-open-project.md`). The next project's game
// starts once its own manifest validates; nothing here starts it. Pure logic, deps injected, so the
// sequencing is testable without a Tauri host.

import { sameProjectPath } from './projectDir';
import { GAME_SLOT, STAGE_SLOTS, type StagePhase, type StageSlot } from './host';

export interface SwitchDeps {
  readonly stageStatus: (slot: StageSlot) => Promise<{ phase: StagePhase }>;
  readonly stageStop: (slot: StageSlot) => Promise<void>;
  /** Drops the main session unconditionally, whatever it is attached to — our own game, a
   *  Busy→Attach server, or an Attach-to-URL session. Called once, before any slot is stopped, so
   *  the reconnect machine never spends its backoff budget on a session the switch is about to
   *  make stale either way. It never shuts a server down; only the session App holds. */
  readonly dropSession: () => void;
  /** Push the recent + persist + set state. Runs only after every stop settled. */
  readonly commit: (dir: string) => void;
}

const RUNNING: readonly StagePhase[] = ['starting', 'listening'];

/** The game slot first, then the stages; the session is already dropped by the time this runs, so
 *  the order only sets which port frees first. */
const SWITCH_SLOTS: readonly StageSlot[] = [GAME_SLOT, ...STAGE_SLOTS];

/** Switch the project the editor works in to `dir`. A switch onto the SAME project (by
 *  `sameProjectPath`) skips the drop and the stops entirely but still commits, so re-choosing the
 *  current project refreshes its place in the recents list. Otherwise `dropSession` runs once,
 *  unconditionally — the main session may be attached to a slot that never ran here at all (a
 *  Busy→Attach or an Attach-to-URL session), so nothing short of "always" keeps it from outliving
 *  the switch — then every slot still running is stopped, in order, before `commit` runs. A
 *  status/stop rejection is logged and does not block the switch, since a host process the editor
 *  can no longer reach is not a reason to refuse the new project. */
export async function switchProject(dir: string, current: string, deps: SwitchDeps): Promise<void> {
  if (!sameProjectPath(dir, current)) {
    deps.dropSession();
    for (const slot of SWITCH_SLOTS) {
      try {
        const { phase } = await deps.stageStatus(slot);
        if (RUNNING.includes(phase)) {
          await deps.stageStop(slot);
        }
      } catch (e) {
        console.warn(`project switch: stopping stage slot "${slot}" failed: ${String(e)}`);
      }
    }
  }
  deps.commit(dir);
}
