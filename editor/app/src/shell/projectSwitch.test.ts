import { describe, expect, it, vi } from 'vitest';
import { switchProject, type SwitchDeps } from './projectSwitch';
import type { StagePhase, StageSlot } from './host';

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

function fakeDeps(phases: Partial<Record<StageSlot, StagePhase>>): SwitchDeps & { calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    stageStatus: async (slot) => {
      calls.push(`status:${slot}`);
      return { phase: phases[slot] ?? 'stopped' };
    },
    stageStop: async (slot) => {
      calls.push(`stop:${slot}`);
    },
    dropSession: () => calls.push('dropSession'),
    commit: (dir) => calls.push(`commit:${dir}`),
  };
}

describe('switchProject', () => {
  it('stops the game first, then exactly the running stage slots, never a stopped/exited/failed one', async () => {
    const deps = fakeDeps({ game: 'listening', model: 'listening', effects: 'stopped' });
    await switchProject('games/other', 'games/vigil', deps);
    expect(deps.calls).toEqual([
      'dropSession',
      'status:game',
      'stop:game',
      'status:model',
      'stop:model',
      'status:effects',
      'commit:games/other',
    ]);
  });

  it('never stops a failed or exited slot either', async () => {
    const deps = fakeDeps({ model: 'failed', effects: 'exited' });
    await switchProject('games/other', 'games/vigil', deps);
    expect(deps.calls).toEqual(['dropSession', 'status:game', 'status:model', 'status:effects', 'commit:games/other']);
  });

  it('drops the session on every real switch, even when no slot is running', async () => {
    const deps = fakeDeps({ game: 'stopped', model: 'stopped', effects: 'busy' });
    await switchProject('games/other', 'games/vigil', deps);
    expect(deps.calls).toEqual(['dropSession', 'status:game', 'status:model', 'status:effects', 'commit:games/other']);
    expect(deps.calls.filter((c) => c.startsWith('stop:'))).toEqual([]);
  });

  it('a same-project switch keeps the session', async () => {
    const deps = fakeDeps({ game: 'listening', model: 'listening', effects: 'listening' });
    await switchProject('games/vigil', 'games/vigil', deps);
    expect(deps.calls).not.toContain('dropSession');
  });

  it('commits only after every stop has settled', async () => {
    const gate = deferred<void>();
    const order: string[] = [];
    const deps: SwitchDeps = {
      stageStatus: async () => ({ phase: 'listening' }),
      stageStop: async (slot) => {
        order.push(`stop-start:${slot}`);
        await gate.promise;
        order.push(`stop-done:${slot}`);
      },
      dropSession: () => {},
      commit: (dir) => order.push(`commit:${dir}`),
    };
    const done = switchProject('games/other', 'games/vigil', deps);
    // Nothing has committed yet: the stop for 'game' is still pending on the gate.
    await Promise.resolve();
    expect(order).not.toContain('commit:games/other');
    gate.resolve();
    await done;
    expect(order.indexOf('commit:games/other')).toBeGreaterThan(order.indexOf('stop-done:game'));
  });

  it('switching onto the same project stops nothing but still commits (refreshes recents)', async () => {
    const deps = fakeDeps({ game: 'listening', model: 'listening', effects: 'listening' });
    await switchProject('games/vigil', 'games/vigil', deps);
    expect(deps.calls).toEqual(['commit:games/vigil']);
  });

  it('a rejecting stageStop is warned and does not block the commit', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const calls: string[] = [];
    const deps: SwitchDeps = {
      stageStatus: async () => ({ phase: 'listening' }),
      stageStop: async () => {
        throw new Error('boom');
      },
      dropSession: () => {},
      commit: (dir) => calls.push(`commit:${dir}`),
    };
    await switchProject('games/other', 'games/vigil', deps);
    expect(calls).toEqual(['commit:games/other']);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
