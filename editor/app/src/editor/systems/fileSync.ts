// The save/watch concurrency state machine for the code panel's open file — pure, so the whole
// policy is unit-tested (see fileSync.test.ts) and the React component only wires events to it and
// runs the returned effect. Settled policy: the open file is polled for
// external mtime changes; a change while the panel is CLEAN silently reloads the disk content, a
// change while DIRTY raises a non-blocking conflict banner (Reload / Keep mine); saves are
// last-writer-wins (no merge UI) and adopt the write's returned mtime as the new baseline.
//
// The editor doc itself lives in CodeMirror, not here. This reducer tracks only the sync *status*
// (clean/dirty/conflict) and the acknowledged on-disk mtime, and emits a `setContent` effect when
// the component must replace the editor's doc from disk (a reload).

export type SyncPhase =
  /** Editor content matches the acknowledged disk baseline. */
  | 'clean'
  /** Editor has unsaved edits (no external change pending). */
  | 'dirty'
  /** Editor has unsaved edits AND an external change is waiting (banner shown). */
  | 'conflict';

export interface SyncState {
  readonly phase: SyncPhase;
  /** The disk mtime we consider current (own writes and acknowledged external changes). `null`
   *  before the first load — any external change then reloads (clean) without a false conflict. */
  readonly baselineMtime: number | null;
  /** The disk content waiting behind a conflict (what "Reload" would apply), else `null`. */
  readonly incoming: { readonly content: string; readonly mtime: number } | null;
}

export type SyncEvent =
  /** A file was (re)opened/loaded fresh into the editor at this on-disk mtime. Resets to clean. */
  | { readonly type: 'loaded'; readonly mtime: number }
  /** The user modified the editor doc (fired per keystroke; idempotent past the first). */
  | { readonly type: 'edit' }
  /** The change poll saw the file's mtime differ from the baseline, carrying the fresh disk text. */
  | { readonly type: 'externalChange'; readonly mtime: number; readonly content: string }
  /** A save succeeded; adopt the write's returned mtime as the new baseline. */
  | { readonly type: 'save'; readonly mtime: number }
  /** Banner "Reload": discard local edits, adopt the pending disk content. */
  | { readonly type: 'reload' }
  /** Banner "Keep mine": dismiss the banner, keep local edits, acknowledge the disk mtime so the
   *  same external change doesn't re-prompt (the next save overwrites it — last-writer-wins). */
  | { readonly type: 'keepMine' };

export type SyncEffect =
  | { readonly type: 'none' }
  /** Replace the editor's doc with disk content (a reload — the change is NOT a user edit). */
  | { readonly type: 'setContent'; readonly content: string; readonly mtime: number };

export const initialSyncState: SyncState = {
  phase: 'clean',
  baselineMtime: null,
  incoming: null,
};

const NONE: SyncEffect = { type: 'none' };

/** The pure transition. Returns the next state and the (at most one) side effect the caller runs. */
export function reduce(state: SyncState, event: SyncEvent): { state: SyncState; effect: SyncEffect } {
  switch (event.type) {
    case 'loaded':
      return { state: { phase: 'clean', baselineMtime: event.mtime, incoming: null }, effect: NONE };

    case 'edit':
      // First edit moves clean → dirty; already-dirty/conflict stay put (same reference → no churn).
      return state.phase === 'clean'
        ? { state: { ...state, phase: 'dirty' }, effect: NONE }
        : { state, effect: NONE };

    case 'externalChange': {
      // Our own write (or a redundant poll) — the mtime already matches the baseline. Ignore.
      if (state.baselineMtime !== null && event.mtime === state.baselineMtime) {
        return { state, effect: NONE };
      }
      if (state.phase === 'clean') {
        // Silent reload: adopt disk content + mtime, stay clean.
        return {
          state: { phase: 'clean', baselineMtime: event.mtime, incoming: null },
          effect: { type: 'setContent', content: event.content, mtime: event.mtime },
        };
      }
      // Dirty (or already conflicting) → conflict, holding the latest disk content for the banner.
      return {
        state: {
          phase: 'conflict',
          baselineMtime: state.baselineMtime,
          incoming: { content: event.content, mtime: event.mtime },
        },
        effect: NONE,
      };
    }

    case 'save':
      return { state: { phase: 'clean', baselineMtime: event.mtime, incoming: null }, effect: NONE };

    case 'reload':
      if (state.incoming === null) {
        return { state, effect: NONE };
      }
      return {
        state: { phase: 'clean', baselineMtime: state.incoming.mtime, incoming: null },
        effect: { type: 'setContent', content: state.incoming.content, mtime: state.incoming.mtime },
      };

    case 'keepMine':
      return {
        state: {
          phase: 'dirty',
          baselineMtime: state.incoming?.mtime ?? state.baselineMtime,
          incoming: null,
        },
        effect: NONE,
      };
  }
}
