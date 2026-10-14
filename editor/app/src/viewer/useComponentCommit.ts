// The one commit path for editing a component field over `component.set`, shared by every surface
// that edits one (the Level Designer's inspector form, the Art Director's chain).
//
// It owns the pending assembly base described in `pendingBase.ts`: the ref is the synchronous compose
// base (state updates haven't flushed when the next commit fires), the state mirror re-renders the
// displayed values, and `effective` is what widgets show and edits assemble from, so rapid sequential
// commits compose and a just-committed sibling shows its new value immediately instead of flashing
// stale for a poll cycle. The rules that decide things — handback, the stale-poll count, the
// identity-guarded rollback — are the pure functions this hook is built on.
//
// TWO CADENCES (`CommitCadence`). A gesture that is judged by eye — a slider drag — has to move the
// frame while it travels, which is dozens of `component.set`s for one edit. `App.handleMutation` turns
// every report into one undo entry, so a `'preview'` write does everything a `'commit'` does except
// report: same wire call, same base bookkeeping, no undo entry and no dirty. The gesture's end sends
// its exact final value at the `'commit'` cadence and reports once, which is how a drag of any length
// collapses to a single entry. The same shape the gizmo drag uses (`editor/gizmo/controller.ts`) and
// the voxel palette's `commit: boolean`.
//
// AND THAT ONE REPORT MUST SPAN THE WHOLE GESTURE. An undo entry's `before` is the response body's
// `previous` (`editor/mutations.ts`), which for the final write is the *last preview* — stepping back
// to it would land mid-drag. So the burst's FIRST reply is kept and reported in its place: the value
// the engine held before the gesture, read back off the engine's own answer rather than recomputed.

import { useCallback, useEffect, useRef, useState } from 'react';
import type { Command, ResponseBody } from '@ludion/protocol';
import type { EntityId } from '../App';
import type { ProtocolClient } from '../protocol/client';
import { componentSet } from '../protocol/commands';
import { toDiagnostic, type DiagnosticView } from './diagnostic';
import { applyFieldEdit, type FieldPath } from './inspect-fields';
import { pollHandback, shouldRollBack } from './pendingBase';

/** A completed mutation: the command that was sent and the body it resolved with. Reported so `App`
 *  can track dirty state and undo without the editing surface knowing about save/undo. */
export interface MutationReport {
  readonly command: Command;
  readonly body: ResponseBody;
}

/** Which cadence a write goes out at: `'preview'` moves the frame only, `'commit'` is the one write a
 *  gesture reports. A caller that names neither is committing — every non-gesture edit is one write. */
export type CommitCadence = 'preview' | 'commit';

export interface ComponentCommitOptions<T> {
  readonly client: ProtocolClient;
  /** The entity holding the component; `null` (no target resolved yet) makes every commit a no-op. */
  readonly entity: EntityId | null;
  /** The registry name of the component being edited. */
  readonly component: string;
  /** The last polled value. Its identity changing is what drives the handback check, so pass the
   *  polled value itself — never `effective`. */
  readonly value: T;
  /** The value to assemble the edit onto when no pending base is in play. Consulted on every commit,
   *  so it is also where a caller declines a path it does not edit: returning `undefined` sends
   *  nothing. */
  readonly initialBase: (path: FieldPath) => T | undefined;
  readonly onReport: (report: MutationReport) => void;
  readonly onError: (diagnostic: DiagnosticView) => void;
  /** Called after a successful send. `fromInitialBase` says the edit assembled onto `initialBase`
   *  rather than composing on an earlier send — which is how a caller recognises the first edit of a
   *  burst without duplicating the base bookkeeping. */
  readonly onApplied?: (fromInitialBase: boolean) => void;
}

export interface ComponentCommit<T> {
  /** The sent-but-not-yet-polled value if one exists, else the polled one. */
  readonly effective: T;
  readonly commit: (path: FieldPath, fieldValue: unknown, cadence?: CommitCadence) => Promise<void>;
  /** Hands the displayed value back to the poll: for a target change, or for another writer (a
   *  hold-to-compare) taking over the component — two writers over one base would fight. An open
   *  preview burst is deliberately NOT dropped: its origin is what a gesture's one undo entry steps
   *  back to, and a net-zero writer (the compare) leaves that value exactly where it was. */
  readonly resetBase: () => void;
}

/** The open preview burst: the value the engine held before its first write, kept so the gesture's one
 *  report can carry it. `undefined` is the wire's "the component was absent", which undo inverts as a
 *  remove; `captured` stays false when that first write never answered. */
interface PreviewBurst {
  previous: Record<string, unknown> | undefined;
  captured: boolean;
}

export function useComponentCommit<T>({
  client,
  entity,
  component,
  value,
  initialBase,
  onReport,
  onError,
  onApplied,
}: ComponentCommitOptions<T>): ComponentCommit<T> {
  const pendingRef = useRef<T | null>(null);
  const [pending, setPending] = useState<T | null>(null);
  const inflight = useRef(0);
  const stalePolls = useRef(0);
  const burst = useRef<PreviewBurst | null>(null);
  const setPendingBase = useCallback((next: T | null): void => {
    pendingRef.current = next;
    setPending(next);
  }, []);
  const resetBase = useCallback((): void => setPendingBase(null), [setPendingBase]);

  // Runs on every poll refresh (a poll hands back fresh objects each cycle, so `value`'s identity
  // changes ~3 Hz even when its content doesn't): decide whether the poll has caught up.
  useEffect(() => {
    const outcome = pollHandback({
      polled: value,
      base: pendingRef.current,
      inflight: inflight.current,
      stalePolls: stalePolls.current,
    });
    stalePolls.current = outcome.stalePolls;
    if (outcome.handback) {
      setPendingBase(null);
    }
  }, [value, setPendingBase]);

  // Commit one field edit: reassemble the whole component value (the wire takes the full value) and
  // send it. On success report the exact command + body (the body's `previous` powers undo); on
  // failure show the diagnostic and let the rolled-back base / poll restore the displayed value.
  const commit = useCallback(
    async (path: FieldPath, fieldValue: unknown, cadence: CommitCadence = 'commit'): Promise<void> => {
      if (entity === null) {
        return;
      }
      const base = initialBase(path);
      if (base === undefined) {
        return; // a path this caller does not edit
      }
      const preview = cadence === 'preview';
      // Which write opens the burst is decided here and not when a reply arrives: replies can land out
      // of order, the issue order cannot. The commit that reports a burst also closes it.
      const opening = preview && burst.current === null;
      const gesture = opening ? { previous: undefined, captured: false } : burst.current;
      burst.current = preview ? gesture : null;
      const before = pendingRef.current;
      // What goes on the wire is shape-blind; `T` is only what the caller knows the component to be.
      const assembled = applyFieldEdit(before ?? base, path, fieldValue);
      setPendingBase(assembled as T); // set before the await, so an overlapping commit composes on top
      stalePolls.current = 0;
      const command = componentSet(entity, component, assembled as object);
      inflight.current += 1;
      try {
        const body = await client.call(command);
        if (opening && gesture !== null && burst.current === gesture) {
          gesture.previous = (body as { previous?: Record<string, unknown> }).previous;
          gesture.captured = true;
        }
        // Discriminant read past the generated union's index signature (see protocol/client.ts).
        if (!preview && (body as { kind?: string }).kind === 'component_set') {
          // A gesture's report carries the value the engine held before its FIRST preview, so undo
          // steps back past the whole drag instead of onto the preview before this one.
          const spanned = gesture !== null && gesture.captured;
          const reported = spanned ? ({ ...body, previous: gesture.previous } as ResponseBody) : body;
          onReport({ command, body: reported });
        }
        onApplied?.(before === null);
      } catch (err) {
        // Only update the base for successful sends: roll this assembly back so the rejected value
        // doesn't poison later commits — unless a newer commit already composed on top of it.
        if (shouldRollBack(pendingRef.current, assembled)) {
          setPendingBase(before);
        }
        onError(toDiagnostic(err));
      } finally {
        inflight.current -= 1;
      }
    },
    [client, entity, component, initialBase, onReport, onError, onApplied, setPendingBase],
  );

  return { effective: pending ?? value, commit, resetBase };
}
