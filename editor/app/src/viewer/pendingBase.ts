// The stale-assembly guard: who owns a component's displayed value while an edit is in flight.
//
// A component is edited by writing its *whole* value back with `component.set` (the wire takes the
// full component, never a patch), while a ~3 Hz poll refreshes the displayed value. That value is
// therefore up to a poll interval stale, and a second field committed inside that window, assembled
// from it, would silently revert the first (tab-through editing of a Transform vector hits this
// routinely). So an editing surface keeps the most recently *sent* assembled value as a pending base,
// and both assembly and display read through it.
//
// Two decisions keep that base honest, and they are the whole of this module: when ownership hands
// back to the poll (or the base wedges stale forever), and whether a failed send may unwind its
// assembly (or a rejected value poisons every later commit). They are pure so they are testable
// without React — `vite.config.ts` collects `src/**/*.test.ts` under `environment: 'node'`. The refs,
// the state mirror and the `component.set` round trip live in `useComponentCommit.ts`.
import { stringifyWire } from '../protocol/wireJson';

/** Consecutive refreshes that neither match the base nor race a send before the base yields anyway.
 *  Two, because two cycles guarantee at least one poll that began after the set applied. */
export const STALE_POLL_LIMIT = 2;

/** One poll refresh weighed against the live pending base. */
export interface PollRefresh {
  /** The freshly polled component value. */
  readonly polled: unknown;
  /** The pending base, or `null` when the poll already owns the displayed value. */
  readonly base: unknown;
  /** How many `component.set`s are still awaiting their reply. */
  readonly inflight: number;
  /** The count carried out of the previous refresh. */
  readonly stalePolls: number;
}

/** What the refresh decided: whether to drop the base, and the count to carry into the next one. */
export interface PollOutcome {
  readonly handback: boolean;
  readonly stalePolls: number;
}

/**
 * Whether the poll has caught up with the pending base.
 *
 * It has when the polled value deep-equals the base — `stringifyWire` is fine here: small canonical
 * wire data, and key order is preserved because the assembled value spreads the polled one. Failing
 * that, ownership yields after {@link STALE_POLL_LIMIT} consecutive refreshes with nothing in flight,
 * which is the fallback for the value the engine legitimately never echoes back byte-for-byte (f64→f32
 * rounding, a materialization the engine answers with every field, another client writing the same
 * component). A refresh that races a send is not evidence of anything, so it resets the count rather
 * than advancing it.
 */
export function pollHandback(refresh: PollRefresh): PollOutcome {
  const { polled, base, inflight, stalePolls } = refresh;
  if (base === null) {
    return { handback: false, stalePolls };
  }
  if (stringifyWire(polled) === stringifyWire(base)) {
    return { handback: true, stalePolls: 0 };
  }
  if (inflight > 0) {
    return { handback: false, stalePolls: 0 };
  }
  const next = stalePolls + 1;
  return next >= STALE_POLL_LIMIT
    ? { handback: true, stalePolls: 0 }
    : { handback: false, stalePolls: next };
}

/**
 * Whether a failed send may roll its assembly back off the base.
 *
 * Only when the base is still the exact object that send assembled: a newer commit composed on top
 * owns the base now, and restoring the older value under it would revert an edit that succeeded. The
 * comparison is identity, deliberately — two commits can assemble deep-equal values, and the question
 * is which one is standing, not what it contains.
 */
export function shouldRollBack(base: unknown, assembled: unknown): boolean {
  return base === assembled;
}
