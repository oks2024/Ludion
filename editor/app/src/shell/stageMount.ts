import type { StageStatus } from './host';

/** What a view's mount effect hands in; the effect keeps its own listener wiring. */
export interface MountDeps {
  status: () => Promise<StageStatus>;
  start: () => Promise<StageStatus>;
  /** True once the effect was cleaned up — checked after every await. */
  cancelled: () => boolean;
  onStatus: (status: StageStatus) => void;
  onStarted: (status: StageStatus) => void;
  onError: (message: string) => void;
}

/**
 * The first-entry sequence of a stage view: read the slot's status, then start it. A view that
 * unmounted while the status call was pending must not start the stage — StrictMode's throwaway
 * first mount would otherwise spawn a session nobody is watching.
 */
export async function mountStage(deps: MountDeps): Promise<void> {
  try {
    const current = await deps.status();
    if (deps.cancelled()) {
      return;
    }
    deps.onStatus(current);
    const started = await deps.start();
    if (!deps.cancelled()) {
      deps.onStarted(started);
    }
  } catch (err) {
    if (!deps.cancelled()) {
      deps.onError(String(err));
    }
  }
}
