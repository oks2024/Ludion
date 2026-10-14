// The latest-wins gate in front of one in-flight mouse-move `input.inject` — the client's half of
// self-pacing to the server's reply rate. Modeled on `framePump.ts`'s decode gate (same busy-flag
// shedding, same at-most-one-parked bound), applied to outbound requests instead of inbound frames.
//
// WHY IT EXISTS. The viewport used to coalesce mouse moves to one `input.inject` per animation frame
// (~60/s) regardless of how fast the server could reply. Against a slow dev-mode game (~22 fps pass
// rate) that floods the connection: 60 requests/s in, ~22/s drained, and the excess backs up unbounded
// in the TCP send buffer with every reply older than the last (the editor's lag-probe overlay grows
// without bound). Pacing to the REPLY rate instead of the frame rate fixes it at the source — a move
// is never sent while a previous move is still in flight; a move that arrives meanwhile replaces
// whatever is waiting, because it is strictly the better answer to "where is the cursor now" and the
// sent-but-superseded position has no remaining value. Side benefit: armed `.lrp` recordings stop
// accumulating dozens of redundant `mouse_move` records per second.
//
// ORDERING. Absolute-position moves are last-write-wins in the sim, so dropping intermediate ones is
// invisible — but a discrete edge (a button or key going down) is not: if a click reaches the wire
// ahead of the aim position it was meant to follow, the shot fires from a stale spot. `flushMove` lets
// a caller about to send an edge push any parked move out first, so "aim, then click" reaches the wire
// in that order (the server's per-connection reply loop preserves request order once both are sent).
//
// Pure: no socket, no DOM, no timer — the caller supplies `send` and decides when an edge is imminent.

import type { InputInjectParams } from '@ludion/protocol';

/** The latest-wins gate in front of one in-flight mouse-move `input.inject`. */
export interface InjectCoalescer {
  /** Offers a move. Sent immediately if idle; otherwise parks it, replacing any previously parked
   *  move (latest-wins) — the in-flight request is left alone. */
  move(params: InputInjectParams): void;
  /** Sends the parked move now, if any (still counts as the in-flight request). Call before a
   *  discrete edge (button/key) so aim precedes the click on the wire. No-op if nothing is parked. */
  flushMove(): void;
}

/**
 * Creates an {@link InjectCoalescer}. One per connection; it holds no shared state.
 *
 * The caller's contract: `send`'s returned promise settling — resolve OR reject — is what releases
 * the gate. A rejection must release it too, or a transient failure would wedge the mouse forever.
 */
export function createInjectCoalescer(send: (params: InputInjectParams) => Promise<unknown>): InjectCoalescer {
  let inFlight = false;
  let parked: InputInjectParams | null = null;

  function dispatch(params: InputInjectParams): void {
    inFlight = true;
    void send(params).then(release, release);
  }

  // Runs when a dispatched request settles, whichever way. If a newer move arrived meanwhile, it goes
  // out next; otherwise the gate opens. A `flushMove` can leave two requests briefly in flight — the
  // earlier one's own `release` still fires and, finding nothing parked, clears the gate even though
  // the flushed one may still be outstanding. Harmless: wire order is fixed at `send()` call time, not
  // settle time, and the backlog stays O(1) either way — not worth a counter for a fire-and-forget path.
  function release(): void {
    const next = parked;
    parked = null;
    if (next !== null) {
      dispatch(next);
    } else {
      inFlight = false;
    }
  }

  return {
    move(params: InputInjectParams): void {
      if (inFlight) {
        parked = params;
        return;
      }
      dispatch(params);
    },
    flushMove(): void {
      if (parked !== null) {
        const next = parked;
        parked = null;
        dispatch(next);
      }
    },
  };
}
