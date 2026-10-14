import { describe, expect, it } from 'vitest';
import type { StageStatus } from '../../shell/host';
import {
  ASSEMBLY_STAGE_SCALE,
  emitDelay,
  isMissingStageScene,
  modelAssetRef,
  modelDisplayName,
  modelFilePath,
  modelKind,
  modelShape,
  MODEL_EXTS,
  rigDef,
  sendDue,
  stageSceneName,
  stageScenePath,
  stageSlot,
  voxelsRoot,
} from './stage';

const listening: StageStatus = {
  slot: 'model',
  phase: 'listening',
  url: 'ws://127.0.0.1:9412',
  scene: 'D:/repo/games/vigil/scenes/model-stage.yaml',
  message: null,
  code: null,
  listener: null,
};

/** The default slot input: the desktop host, a project dir, nothing wrong. */
function input(overrides: Partial<Parameters<typeof stageSlot>[0]> = {}) {
  return {
    isDesktop: true,
    projectDir: '/repo/games/vigil',
    status: listening,
    connected: true,
    error: null,
    ...overrides,
  };
}

describe('paths', () => {
  it('builds the stage scene path per rig', () => {
    expect(stageScenePath('/repo/games/vigil', 'neutral')).toBe('/repo/games/vigil/scenes/model-stage.yaml');
    expect(stageScenePath('/repo/games/vigil', 'night')).toBe('/repo/games/vigil/scenes/model-stage-night.yaml');
  });

  it('normalizes a Windows project dir and a trailing separator', () => {
    expect(stageScenePath('D:\\Repos\\Ludion\\games\\vigil', 'neutral')).toBe(
      'D:/Repos/Ludion/games/vigil/scenes/model-stage.yaml',
    );
    expect(voxelsRoot('/repo/games/vigil/')).toBe('/repo/games/vigil/scenes/voxels');
  });

  it('resolves a model to a file path and to the asset reference a scene would use', () => {
    expect(modelFilePath('/p', 'enemies/statue.voxel.yaml')).toBe('/p/scenes/voxels/enemies/statue.voxel.yaml');
    // Content root is the stage scene's own directory, so the wire path is `voxels/…` — byte
    // identical to how a real scene names the same model.
    expect(modelAssetRef('enemies/statue.voxel.yaml')).toBe('voxels/enemies/statue.voxel.yaml');
    expect(modelAssetRef('/statue.voxel.yaml')).toBe('voxels/statue.voxel.yaml');
  });

  it('names rigs and models for display, either listed kind', () => {
    expect(stageSceneName('night')).toBe('model-stage-night');
    expect(rigDef('night').file).toBe('model-stage-night.yaml');
    expect(modelDisplayName('enemies/statue.voxel.yaml')).toBe('statue');
    expect(modelDisplayName('house-a.voxel.yaml')).toBe('house-a');
    expect(modelDisplayName('player.assembly.yaml')).toBe('player');
  });
});

describe('modelKind / modelShape', () => {
  it('lists exactly the two kinds an assembly library browses', () => {
    expect(MODEL_EXTS).toEqual(['.voxel.yaml', '.assembly.yaml']);
  });

  it('stages an assembly at the 16-per-unit content convention, not at unit scale', () => {
    // The stage rig is built around a subject a few units tall; 1/16 is the convention a 32-voxel
    // rig is authored to. Not vigil's 0.125 — that is a placement, and a scene's Transform owns it.
    expect(ASSEMBLY_STAGE_SCALE).toBe(1 / 16);
  });

  it('reads the kind off the double extension, case insensitively', () => {
    expect(modelKind('player.assembly.yaml')).toBe('assembly');
    expect(modelKind('rigs/Player.Assembly.YAML')).toBe('assembly');
    expect(modelKind('enemies/statue.voxel.yaml')).toBe('voxel');
  });

  it('the kind is the shape key — what a scene itself writes', () => {
    expect(modelShape('enemies/statue.voxel.yaml')).toEqual({ voxel: 'voxels/enemies/statue.voxel.yaml' });
    expect(modelShape('player.assembly.yaml')).toEqual({ assembly: 'voxels/player.assembly.yaml' });
  });
});

describe('isMissingStageScene', () => {
  it('recognizes only the host\'s exact "no scene at " prefix', () => {
    expect(isMissingStageScene('no scene at `x.yaml`. …')).toBe(true);
    expect(isMissingStageScene('cannot start: no cargo on PATH')).toBe(false);
    expect(isMissingStageScene(null)).toBe(false);
  });
});

describe('stageSlot', () => {
  it('yields the canvas only when the stage is listening AND dialed', () => {
    expect(stageSlot(input())).toBeNull();
    const dialing = stageSlot(input({ connected: false }));
    expect(dialing?.tone).toBe('busy');
    expect(dialing?.canRestart).toBe(false);
    expect(dialing?.detail).toContain('ws://127.0.0.1:9412');
  });

  it('says a cold build compiles first while starting — the silence this view has to explain', () => {
    const slot = stageSlot(input({ status: { ...listening, phase: 'starting', url: null } }));
    expect(slot?.tone).toBe('busy');
    expect(slot?.title).toContain('Starting');
    expect(slot?.detail).toMatch(/compiles|minutes/);
    // Nothing to restart: something is already starting.
    expect(slot?.canRestart).toBe(false);
  });

  it('surfaces the child stderr tail on a failure, with a restart', () => {
    const slot = stageSlot(
      input({ status: { ...listening, phase: 'failed', url: null, message: 'error[L1114]: slice row 4 is 8 chars' } }),
    );
    expect(slot?.tone).toBe('error');
    expect(slot?.detail).toBe('error[L1114]: slice row 4 is 8 chars');
    expect(slot?.canRestart).toBe(true);
  });

  it('names the exit code when the stage dies after listening', () => {
    const slot = stageSlot(input({ status: { ...listening, phase: 'exited', code: 101, message: 'panicked' } }));
    expect(slot?.title).toContain('101');
    expect(slot?.canRestart).toBe(true);
  });

  it('offers a start when the stage is stopped', () => {
    const slot = stageSlot(input({ status: { ...listening, phase: 'stopped', url: null } }));
    expect(slot?.tone).toBe('idle');
    expect(slot?.canRestart).toBe(true);
  });

  it('reports a start rejection (missing stage scene, no cargo) — the host sends no event for it', () => {
    const slot = stageSlot(input({ error: 'cannot start: no such file model-stage.yaml' }));
    expect(slot?.tone).toBe('error');
    expect(slot?.detail).toContain('model-stage.yaml');
    expect(slot?.canAddScenes).toBe(false);
  });

  it('offers Add stage scenes on the host\'s exact missing-scene refusal, and never otherwise', () => {
    const refusal =
      'no scene at `D:/p/scenes/model-stage.yaml`. create it (or switch project from the title bar) and try again.';
    const missing = stageSlot(input({ error: refusal }));
    expect(missing?.tone).toBe('error');
    expect(missing?.canRestart).toBe(true);
    expect(missing?.canAddScenes).toBe(true);
    expect(missing?.detail).toContain(refusal);
    expect(missing?.detail).toContain('Add stage scenes');

    // Every other error, and every non-error phase, never offers it.
    expect(stageSlot(input({ error: 'cannot start: no cargo on PATH' }))?.canAddScenes).toBe(false);
    for (const phase of ['stopped', 'starting', 'failed', 'exited', 'listening', 'busy'] as const) {
      expect(stageSlot(input({ status: { ...listening, phase } }))?.canAddScenes ?? false).toBe(false);
    }
  });

  it('asks for the preconditions before any phase: the desktop host, then a project dir', () => {
    // Both are checked ahead of the phase, so a stale `listening` status cannot mask them.
    expect(stageSlot(input({ isDesktop: false }))?.title).toContain('desktop editor');
    expect(stageSlot(input({ projectDir: '' }))?.title).toContain('project directory');
    // Both model libraries hang off it — the stage is shared by the Voxel Artist and the Sculpt
    // view, so naming only one would send half the users looking in the wrong folder.
    expect(stageSlot(input({ projectDir: '' }))?.detail).toContain('scenes/voxels');
    expect(stageSlot(input({ projectDir: '' }))?.detail).toContain('scenes/sculpts');
    expect(stageSlot(input({ isDesktop: false }))?.canRestart).toBe(false);
  });
});

describe('sendDue', () => {
  it('always sends the first pose', () => {
    expect(sendDue(null, 0, 60)).toBe(true);
  });

  it('drops writes inside the interval and passes the one that reaches it', () => {
    expect(sendDue(1000, 1059, 60)).toBe(false);
    expect(sendDue(1000, 1060, 60)).toBe(true);
    expect(sendDue(1000, 5000, 60)).toBe(true);
  });
});

describe('emitDelay', () => {
  it('emits immediately when the floor has already elapsed', () => {
    expect(emitDelay(0, 10_000, 500)).toBe(0);
    expect(emitDelay(1000, 1500, 500)).toBe(0);
  });

  it('waits out the remainder, so rescheduling converges on lastEmit + min', () => {
    expect(emitDelay(1000, 1050, 500)).toBe(450);
    // Two more changes 50 ms apart: each schedules for the same absolute instant, 1500.
    expect(1100 + emitDelay(1000, 1100, 500)).toBe(1500);
    expect(1150 + emitDelay(1000, 1150, 500)).toBe(1500);
  });

  it('never stalls longer than the floor, whatever the clock says', () => {
    expect(emitDelay(2000, 1000, 500)).toBe(500);
    expect(emitDelay(0, -1, 500)).toBe(500);
  });
});
