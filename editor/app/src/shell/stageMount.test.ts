import { describe, expect, it, vi, type Mock } from 'vitest';
import { mountStage, type MountDeps } from './stageMount';
import type { StageStatus } from './host';

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void; reject: (reason: unknown) => void } {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const status = (phase: StageStatus['phase']): StageStatus => ({ slot: 'model', phase }) as StageStatus;

interface FakeDeps extends MountDeps {
  start: Mock<() => Promise<StageStatus>>;
  onStatus: Mock<(status: StageStatus) => void>;
  onStarted: Mock<(status: StageStatus) => void>;
  onError: Mock<(message: string) => void>;
}

function fakeDeps(current: Promise<StageStatus>, isCancelled: () => boolean): FakeDeps {
  return {
    status: () => current,
    start: vi.fn(async () => status('starting')),
    cancelled: isCancelled,
    onStatus: vi.fn(),
    onStarted: vi.fn(),
    onError: vi.fn(),
  };
}

describe('mountStage', () => {
  it('never starts the stage once cancelled while the status call was pending', async () => {
    const current = deferred<StageStatus>();
    let cancelled = false;
    const deps = fakeDeps(current.promise, () => cancelled);
    const mounted = mountStage(deps);
    cancelled = true;
    current.resolve(status('stopped'));
    await mounted;
    expect(deps.start).not.toHaveBeenCalled();
    expect(deps.onStatus).not.toHaveBeenCalled();
    expect(deps.onStarted).not.toHaveBeenCalled();
  });

  it('starts once and reports the started status when not cancelled', async () => {
    const deps = fakeDeps(Promise.resolve(status('stopped')), () => false);
    await mountStage(deps);
    expect(deps.onStatus).toHaveBeenCalledWith(status('stopped'));
    expect(deps.start).toHaveBeenCalledTimes(1);
    expect(deps.onStarted).toHaveBeenCalledWith(status('starting'));
    expect(deps.onError).not.toHaveBeenCalled();
  });

  it('drops the started status when cancelled while the start was pending', async () => {
    const started = deferred<StageStatus>();
    let cancelled = false;
    const deps = fakeDeps(Promise.resolve(status('stopped')), () => cancelled);
    deps.start.mockImplementation(() => {
      cancelled = true;
      return started.promise;
    });
    const mounted = mountStage(deps);
    started.resolve(status('starting'));
    await mounted;
    expect(deps.start).toHaveBeenCalledTimes(1);
    expect(deps.onStarted).not.toHaveBeenCalled();
  });

  it('reports a failure only while still mounted', async () => {
    const live = fakeDeps(Promise.reject(new Error('boom')), () => false);
    await mountStage(live);
    expect(live.onError).toHaveBeenCalledWith('Error: boom');

    const gone = fakeDeps(Promise.reject(new Error('boom')), () => true);
    await mountStage(gone);
    expect(gone.onError).not.toHaveBeenCalled();
  });
});
