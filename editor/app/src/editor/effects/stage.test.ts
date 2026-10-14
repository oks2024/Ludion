import { describe, expect, it } from 'vitest';
import type { StageStatus } from '../../shell/host';
import { EMITTER, EMITTER_POLL_MS, FALLBACK_STAGE_URL, PATTERN_KIND, PATTERN_PLAYER, STAGE_SCENE, STAGE_SLOT, SUBJECT, TARGET, TRANSFORM, contentRoot, effectsStageSlot, stageScenePath } from './stage';

const listening: StageStatus = {
  slot: 'effects',
  phase: 'listening',
  url: 'ws://127.0.0.1:9413',
  scene: '/repo/games/vigil/scenes/effects-stage.yaml',
  message: null,
  code: null,
  listener: null,
};

/** The default slot input: the desktop host, a project dir, nothing wrong. */
function input(overrides: Partial<Parameters<typeof effectsStageSlot>[0]> = {}) {
  return {
    isDesktop: true,
    projectDir: '/repo/games/vigil',
    status: listening,
    connected: true,
    error: null,
    ...overrides,
  };
}

describe('constants', () => {
  it('names the slot, scene, subject and component distinct from the voxel stage', () => {
    expect(STAGE_SLOT).toBe('effects');
    expect(STAGE_SCENE).toBe('effects-stage.yaml');
    expect(FALLBACK_STAGE_URL).toBe('ws://127.0.0.1:9413');
    expect(SUBJECT).toBe('subject');
    expect(EMITTER).toBe('ParticleEmitter');
    expect(EMITTER_POLL_MS).toBe(300);
  });

  it('names the pattern tab entity, components and document kind', () => {
    expect(TARGET).toBe('target');
    expect(PATTERN_PLAYER).toBe('PatternPlayer');
    expect(TRANSFORM).toBe('Transform');
    expect(PATTERN_KIND).toBe('pattern');
  });
});

describe('contentRoot', () => {
  it('is the stage scene directory — what every document key is relative to', () => {
    expect(contentRoot('/repo/games/vigil')).toBe('/repo/games/vigil/scenes');
    expect(contentRoot('D:\\Repos\\Ludion\\games\\vigil\\')).toBe('D:/Repos/Ludion/games/vigil/scenes');
  });
});

describe('stageScenePath', () => {
  it('builds the absolute path under <projectDir>/scenes', () => {
    expect(stageScenePath('/repo/games/vigil')).toBe('/repo/games/vigil/scenes/effects-stage.yaml');
  });

  it('normalizes a Windows project dir and a trailing separator, matching the voxel module', () => {
    expect(stageScenePath('D:\\Repos\\Ludion\\games\\vigil')).toBe('D:/Repos/Ludion/games/vigil/scenes/effects-stage.yaml');
    expect(stageScenePath('/repo/games/vigil/')).toBe('/repo/games/vigil/scenes/effects-stage.yaml');
  });
});

describe('effectsStageSlot', () => {
  it('yields the canvas only when the stage is listening AND dialed', () => {
    expect(effectsStageSlot(input())).toBeNull();
    const dialing = effectsStageSlot(input({ connected: false }));
    expect(dialing?.tone).toBe('busy');
    expect(dialing?.detail).toContain('ws://127.0.0.1:9413');
  });

  it('names the effects stage, not the model stage, in every phase title', () => {
    expect(effectsStageSlot(input({ isDesktop: false }))?.title).toBe('The effects stage needs the desktop editor');
    expect(effectsStageSlot(input({ error: 'boom' }))?.title).toBe('The effects stage could not be started');
    expect(effectsStageSlot(input({ status: { ...listening, phase: 'stopped', url: null } }))?.title).toBe(
      'The effects stage is not running',
    );
    expect(effectsStageSlot(input({ status: { ...listening, phase: 'starting', url: null } }))?.title).toBe(
      'Starting the effects stage…',
    );
    expect(effectsStageSlot(input({ status: { ...listening, phase: 'failed', url: null, message: null } }))?.title).toBe(
      'The effects stage failed to start',
    );
    expect(effectsStageSlot(input({ status: { ...listening, phase: 'exited', code: 1, message: null } }))?.title).toBe(
      'The effects stage exited (code 1)',
    );
    expect(effectsStageSlot(input({ connected: false }))?.title).toBe('Dialing the effects stage…');
  });

  it('still gates on the desktop host and a project dir ahead of the phase', () => {
    expect(effectsStageSlot(input({ projectDir: '' }))?.title).toContain('project directory');
    expect(effectsStageSlot(input({ isDesktop: false }))?.canRestart).toBe(false);
  });

  it('offers Add stage scenes on the host\'s missing-scene refusal, naming the effects stage', () => {
    const refusal =
      'no scene at `D:/p/scenes/effects-stage.yaml`. create it (or switch project from the title bar) and try again.';
    const slot = effectsStageSlot(input({ error: refusal }));
    expect(slot?.title).toBe('The effects stage scene is missing');
    expect(slot?.canAddScenes).toBe(true);
    expect(effectsStageSlot(input({ error: 'some other failure' }))?.canAddScenes).toBe(false);
  });
});
