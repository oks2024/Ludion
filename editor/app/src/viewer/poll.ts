// The guard that keeps a periodic poll well-behaved.
//
// The inspector polls `query.entities` (and, for a selection, `component.get`) on a fixed interval.
// Two rules must hold and are easy to get wrong in a `setInterval` callback: never poll while the
// tab/panel is hidden (wasted work the human can't see), and never start a poll while the previous
// one is still in flight (a slow engine must not pile up overlapping requests). Both collapse into
// one pure predicate so the scheduling logic is testable without timers or a socket.

/** The inputs a scheduler weighs before each tick: is there a client, is the surface visible, and
 *  is a request already outstanding. */
export interface PollGate {
  readonly connected: boolean;
  readonly visible: boolean;
  readonly inFlight: boolean;
}

/**
 * Whether a new poll should start now: only when connected, only when the surface is visible, and
 * only when no previous poll is still awaiting its reply.
 */
export function shouldPoll(gate: PollGate): boolean {
  return gate.connected && gate.visible && !gate.inFlight;
}
