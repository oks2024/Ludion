import { describe, expect, it } from 'vitest';
import { autoAttachUrl, dropsSession, gameCard, gameOpts } from './gameSession';
import type { StageStatus } from './host';

function status(overrides: Partial<StageStatus> = {}): StageStatus {
  return {
    slot: 'game',
    phase: 'stopped',
    url: null,
    scene: null,
    message: null,
    code: null,
    listener: null,
    ...overrides,
  };
}

describe('gameOpts', () => {
  it('joins the project dir and default scene, and names the project as the game slot binary', () => {
    expect(gameOpts('/p/game', 'scenes/camp.yaml')).toEqual({
      scene: '/p/game/scenes/camp.yaml',
      project: '/p/game',
    });
  });

  it('tolerates a Windows-style project dir', () => {
    expect(gameOpts('C:\\p\\game', 'scenes/camp.yaml')).toEqual({
      scene: 'C:/p/game/scenes/camp.yaml',
      project: 'C:\\p\\game',
    });
  });
});

describe('gameCard', () => {
  it('offers nothing with no valid project', () => {
    const { slot, actions } = gameCard({ projectName: null, status: status(), error: null, connection: 'idle' });
    expect(slot.tone).toBe('idle');
    expect(actions).toEqual([]);
  });

  it('is an error with a run action on a stageStart rejection', () => {
    const { slot, actions } = gameCard({
      projectName: 'vigil',
      status: status(),
      error: 'no cargo on PATH',
      connection: 'idle',
    });
    expect(slot.tone).toBe('error');
    expect(slot.detail).toBe('no cargo on PATH');
    expect(actions).toEqual(['run']);
  });

  it('offers run while stopped', () => {
    const { slot, actions } = gameCard({
      projectName: 'vigil',
      status: status({ phase: 'stopped' }),
      error: null,
      connection: 'idle',
    });
    expect(slot.tone).toBe('idle');
    expect(actions).toEqual(['run']);
  });

  it('is busy with no actions while starting', () => {
    const { slot, actions } = gameCard({
      projectName: 'vigil',
      status: status({ phase: 'starting' }),
      error: null,
      connection: 'idle',
    });
    expect(slot.tone).toBe('busy');
    expect(slot.detail).toContain('vigil');
    expect(actions).toEqual([]);
  });

  it('is busy with no actions while listening and the auto-attach is still in flight', () => {
    const { slot, actions } = gameCard({
      projectName: 'vigil',
      status: status({ phase: 'listening', url: 'ws://127.0.0.1:9411' }),
      error: null,
      connection: 'connecting',
    });
    expect(slot.tone).toBe('busy');
    expect(slot.detail).toContain('ws://127.0.0.1:9411');
    expect(actions).toEqual([]);
  });

  it('offers attach once listening while idle or errored — the auto-attach is spent', () => {
    const idle = gameCard({
      projectName: 'vigil',
      status: status({ phase: 'listening', url: 'ws://127.0.0.1:9411' }),
      error: null,
      connection: 'idle',
    });
    expect(idle.slot.tone).toBe('idle');
    expect(idle.slot.title).toBe('The game is running');
    expect(idle.slot.detail).toContain('ws://127.0.0.1:9411');
    expect(idle.actions).toEqual(['attach']);

    const errored = gameCard({
      projectName: 'vigil',
      status: status({ phase: 'listening', url: 'ws://127.0.0.1:9411' }),
      error: null,
      connection: 'error',
    });
    expect(errored.actions).toEqual(['attach']);
  });

  it('shows the compile tail with a run action on failed', () => {
    const { slot, actions } = gameCard({
      projectName: 'vigil',
      status: status({ phase: 'failed', message: 'error[E0433]: broken' }),
      error: null,
      connection: 'idle',
    });
    expect(slot.tone).toBe('error');
    expect(slot.detail).toBe('error[E0433]: broken');
    expect(actions).toEqual(['run']);
  });

  it('is idle on a clean exit, error otherwise, both with a run action', () => {
    const clean = gameCard({
      projectName: 'vigil',
      status: status({ phase: 'exited', code: 0 }),
      error: null,
      connection: 'idle',
    });
    expect(clean.slot.tone).toBe('idle');
    expect(clean.actions).toEqual(['run']);

    const crashed = gameCard({
      projectName: 'vigil',
      status: status({ phase: 'exited', code: 101, message: 'panicked' }),
      error: null,
      connection: 'idle',
    });
    expect(crashed.slot.tone).toBe('error');
    expect(crashed.slot.title).toContain('101');
    expect(crashed.actions).toEqual(['run']);
  });

  it('offers attach and replace for a busy Ludion listener, never replace for a foreign one', () => {
    const ludion = gameCard({
      projectName: 'vigil',
      status: status({
        phase: 'busy',
        url: 'ws://127.0.0.1:9411',
        listener: { kind: 'ludion', engineVersion: '0.9.0' },
        message: '`127.0.0.1:9411` is already serving a Ludion engine (0.9.0).',
      }),
      error: null,
      connection: 'idle',
    });
    expect(ludion.slot.tone).toBe('idle');
    expect(ludion.slot.title).toContain('0.9.0');
    expect(ludion.actions).toEqual(['attach', 'replace']);

    const foreign = gameCard({
      projectName: 'vigil',
      status: status({
        phase: 'busy',
        url: 'ws://127.0.0.1:9411',
        listener: { kind: 'foreign' },
        message: '`127.0.0.1:9411` is held by something that is not a Ludion engine.',
      }),
      error: null,
      connection: 'idle',
    });
    expect(foreign.slot.tone).toBe('error');
    expect(foreign.actions).toEqual(['attach']);
  });
});

describe('autoAttachUrl', () => {
  it('is the game url once listening, unless the main session is connected or connecting', () => {
    const listening = status({ phase: 'listening', url: 'ws://127.0.0.1:9411' });
    expect(autoAttachUrl(listening, 'idle', null)).toBe('ws://127.0.0.1:9411');
    expect(autoAttachUrl(listening, 'error', null)).toBe('ws://127.0.0.1:9411');
    expect(autoAttachUrl(listening, 'connecting', null)).toBeNull();
    expect(autoAttachUrl(listening, 'connected', null)).toBeNull();
  });

  it('is null off the listening phase', () => {
    expect(autoAttachUrl(status({ phase: 'starting' }), 'idle', null)).toBeNull();
    expect(autoAttachUrl(status({ phase: 'busy' }), 'idle', null)).toBeNull();
  });

  it('never re-attaches to the same listening status once attached, even after a failed attempt', () => {
    // A failed attach settles `state` back to 'error' — without the `lastAttached` guard this would
    // retry forever on the exact same listening status.
    const listening = status({ phase: 'listening', url: 'ws://127.0.0.1:9411' });
    expect(autoAttachUrl(listening, 'error', listening)).toBeNull();
  });

  it('never re-attaches after a human Disconnect while the game is still listening', () => {
    const listening = status({ phase: 'listening', url: 'ws://127.0.0.1:9411' });
    expect(autoAttachUrl(listening, 'idle', listening)).toBeNull();
  });

  it('attaches again for a genuinely new listening status object', () => {
    const first = status({ phase: 'listening', url: 'ws://127.0.0.1:9411' });
    const second = status({ phase: 'listening', url: 'ws://127.0.0.1:9411' });
    expect(autoAttachUrl(second, 'idle', first)).toBe('ws://127.0.0.1:9411');
  });
});

describe('dropsSession', () => {
  it('drops when the game exited or failed at the session’s own url', () => {
    expect(dropsSession(status({ phase: 'exited', url: 'ws://127.0.0.1:9411' }), 'ws://127.0.0.1:9411')).toBe(true);
    expect(dropsSession(status({ phase: 'failed', url: 'ws://127.0.0.1:9411' }), 'ws://127.0.0.1:9411')).toBe(true);
  });

  it('never drops a different session, a running game, or when neither has a url', () => {
    expect(dropsSession(status({ phase: 'exited', url: 'ws://127.0.0.1:9411' }), 'ws://127.0.0.1:9412')).toBe(false);
    expect(dropsSession(status({ phase: 'listening', url: 'ws://127.0.0.1:9411' }), 'ws://127.0.0.1:9411')).toBe(false);
    expect(dropsSession(status({ phase: 'exited', url: null }), null)).toBe(false);
  });
});
