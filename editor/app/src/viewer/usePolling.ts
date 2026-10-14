// A visibility-aware, non-overlapping polling hook.
//
// Several panels want to poll the engine at a modest rate (the inspector reads `query.entities` a
// few times a second). Two hazards, both handled here via the pure {@link shouldPoll} gate: stop
// polling when the tab/panel is hidden (`document.visibilityState`), and never launch a new poll
// while the previous one is still awaiting its reply (a slow engine must degrade to a lower poll
// rate, not a growing backlog). The callback is held in a ref so changing its identity every render
// doesn't restart the timer.

import { useEffect, useRef } from 'react';
import { shouldPoll } from './poll';

function documentVisible(): boolean {
  // `document` is absent under the node test environment; treat that as visible.
  return typeof document === 'undefined' || document.visibilityState === 'visible';
}

/**
 * Runs `poll` every `intervalMs` while `enabled`, skipping ticks when the document is hidden or a
 * previous `poll` is still in flight. Fires once immediately on enable and again whenever the
 * document becomes visible.
 */
export function usePolling(enabled: boolean, intervalMs: number, poll: () => Promise<void>): void {
  const pollRef = useRef(poll);
  pollRef.current = poll;
  const inFlightRef = useRef(false);

  useEffect(() => {
    if (!enabled) {
      return;
    }

    const tick = (): void => {
      if (!shouldPoll({ connected: enabled, visible: documentVisible(), inFlight: inFlightRef.current })) {
        return;
      }
      inFlightRef.current = true;
      void pollRef.current().finally(() => {
        inFlightRef.current = false;
      });
    };

    tick(); // poll immediately rather than waiting a full interval
    const timer = window.setInterval(tick, intervalMs);
    const onVisibility = (): void => {
      if (documentVisible()) {
        tick();
      }
    };
    document.addEventListener('visibilitychange', onVisibility);

    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [enabled, intervalMs]);
}
