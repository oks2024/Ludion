// Hold-to-compare: press a row's toggle and the frame shows what it looks like without that
// contribution; release and the exact prior value comes back.
//
// WHY THE ENGINE'S OWN A/B IS NOT ENOUGH. Every look verdict in the post-process arc was reached by
// comparison — `render --sweep` renders a contact sheet, `image-stats` scores defects. Both are
// offline instruments over a still frame. What a human directing a look live needs is the same
// comparison against the moving scene they are judging, which means writing the neutral value and
// then taking it back.
//
// RESTORE IS FROM THE CAPTURE, NEVER FROM A RECOMPUTATION. The Art Director plan's decision 7 turns
// on this: the release writes back the object captured at the press, so the round trip is net-zero
// *by construction* and a compare is therefore neither a mutation nor an undo entry. Recomputing the
// prior value — re-fetching it, or re-deriving it from the neutral patch — would make correctness a
// property of two code paths agreeing, and the session these tabs live in is SHARED with the Level
// Designer, where a `scene.save` from the other tab would persist whatever a comparison had left
// behind. So: one captured object in, the same values out.
//
// AND THE CAPTURE IS ISOLATED IN BOTH DIRECTIONS. It is cloned on the way in, so a caller that keeps
// editing the value it passed cannot rewrite history, and cloned on the way out, so a caller that
// mutates the restore patch cannot corrupt a session it might still hold.
//
// THE ABSENT COMPONENT IS A REAL CASE. The global-look singletons are opt-in and mostly unauthored:
// most scenes carry no `PostProcess`, and two of its shipped defaults are live — `exposure` at `1.3`
// and `dither` on — so comparing them is exactly as meaningful there. Pressing materializes a
// component carrying only the neutralized fields, and releasing removes it again: `component.remove`
// is the honest inverse of a fresh insert, the same rule `undo.ts` encodes for a `set` with no
// `before`.
//
// THE NEUTRAL IS THE CALLER'S. This module knows nothing about which component it is holding or what
// "off" means for it — the Art Director derives its neutral from the effect chain, the Lighter holds
// one `GiVolume` field at a time — so both arrive here as a plain patch and a component name. An
// empty patch means the caller had nothing to neutralize, and the press is refused rather than
// writing the value back to itself.
//
// Pure: no React, no wire — the reducer returns the commands, the caller sends them (see
// compare.test.ts).

import type { Command } from '@ludion/protocol';
import { componentSet } from '../protocol/commands';
import type { EntityId } from './undo';

/** A JSON value as the compared components carry them: scalars, a closed enum's string, and the
 *  fixed triples. */
type JsonValue = number | boolean | string | null | readonly JsonValue[];

/** Deep-copies a component value so a session's capture shares no object with anything the caller
 *  holds. These components are scalars and fixed triples, so this is total over what can arrive. */
function cloneJson(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(cloneJson);
  }
  if (typeof value === 'object' && value !== null) {
    const out: Record<string, unknown> = {};
    for (const [key, element] of Object.entries(value as Record<string, unknown>)) {
      out[key] = cloneJson(element);
    }
    return out;
  }
  return value as JsonValue;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** A comparison in progress: the component held, the entity it is held on, and the value to put back.
 *  `captured` is `null` when the entity carried no such component at the press — the release removes
 *  the component rather than restoring a value. `label` is the caller's own name for the row being
 *  held, carried so a view can tell which of several toggles owns the session. */
export interface CompareSession {
  readonly component: string;
  readonly label: string;
  readonly entity: EntityId;
  readonly captured: Readonly<Record<string, unknown>> | null;
}

/**
 * One wire write a comparison needs, with the two flags that make the caller dumb.
 *
 * `undoable` and `dirties` are `false` on *both* ends and always will be: a press and its release
 * cancel exactly, so recording either would put a no-op on the undo stack and mark a shared session
 * dirty for a change that did not happen.
 */
export interface ComparePatch {
  readonly command: Command;
  readonly undoable: false;
  readonly dirties: false;
}

/** The press: the session to hold and the write that neutralizes the row. */
export interface CompareStart {
  readonly session: CompareSession;
  readonly patch: ComparePatch;
}

function setPatch(component: string, entity: EntityId, value: Record<string, unknown>): ComparePatch {
  return {
    command: componentSet(entity, component, value),
    undoable: false,
    dirties: false,
  };
}

function removePatch(component: string, entity: EntityId): ComparePatch {
  return {
    command: { method: 'component.remove', params: { entity, component } },
    undoable: false,
    dirties: false,
  };
}

/**
 * Presses hold-to-compare on `component`.
 *
 * `current` is the entity's live value of it, or `null` when it carries none; `neutral` is the
 * partial value that switches the compared contribution off. Returns the session to keep until the
 * release and the write that applies it — the neutral's fields over a copy of everything else,
 * because `component.set` takes the whole component and never a patch.
 *
 * `null` when `neutral` is empty, i.e. the caller has nothing to neutralize (the Art Director's
 * tonemap: every operator is a curve, so there is no "without it" to show), so a view can offer the
 * toggle only where it means something.
 */
export function beginCompare(
  component: string,
  entity: EntityId,
  current: Readonly<Record<string, unknown>> | null,
  neutral: Readonly<Record<string, unknown>>,
  label = '',
): CompareStart | null {
  if (Object.keys(neutral).length === 0) {
    return null;
  }
  const captured = isRecord(current) ? (cloneJson(current) as Record<string, unknown>) : null;
  const value: Record<string, unknown> = { ...(cloneJson(captured ?? {}) as Record<string, unknown>) };
  for (const [name, off] of Object.entries(neutral)) {
    value[name] = cloneJson(off);
  }
  return { session: { component, label, entity, captured }, patch: setPatch(component, entity, value) };
}

/**
 * Releases the comparison: the write that puts the captured state back, byte for byte.
 *
 * Reads nothing but the session — no field table, no defaults, no re-fetch — which is what makes the
 * round trip net-zero rather than merely intended to be. A capture of `null` (the component was
 * absent at the press) releases as a `component.remove`, undoing the materialization the press did.
 */
export function endCompare(session: CompareSession): ComparePatch {
  if (session.captured === null) {
    return removePatch(session.component, session.entity);
  }
  return setPatch(session.component, session.entity, cloneJson(session.captured) as Record<string, unknown>);
}

/** The value a release restores, as data rather than as a command — for a caller that wants to show
 *  it, or to assert the round trip. `null` means the release removes the component. */
export function restoredValue(session: CompareSession): Record<string, unknown> | null {
  return session.captured === null ? null : (cloneJson(session.captured) as Record<string, unknown>);
}
