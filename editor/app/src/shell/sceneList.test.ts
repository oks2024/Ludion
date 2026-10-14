import { describe, expect, it } from 'vitest';
import { classifySceneText, generatorOf, isStageScene, listScenes, pickScene, sameScenePath, saveGate } from './sceneList';
import type { DirEntry } from './host';

describe('generatorOf / saveGate', () => {
  it('names the generator of a generated scene', () => {
    expect(generatorOf('format: 1\ngenerated_from: src/camp.map\nentities:')).toBe('src/camp.map');
    expect(generatorOf('format: 1\ngenerated_from: "src/camp.map"\n')).toBe('src/camp.map');
  });

  it('has no generator for a hand-authored scene', () => {
    expect(generatorOf('format: 1\nname: x\nentities:')).toBeNull();
    expect(generatorOf('format: 1\nentities:\n  - n:\n    generated_from: x')).toBeNull();
  });

  it('disables Save on a generated scene and names the generator', () => {
    const gate = saveGate('scenes/camp.yaml', { kind: 'generated', script: 'src/camp.map' });
    expect(gate.canSave).toBe(false);
    expect(gate.title).toContain('src/camp.map');
  });

  it('keeps Save enabled for a hand-authored scene, disabled without a path', () => {
    expect(saveGate('scenes/main.yaml', { kind: 'none' })).toEqual({ canSave: true, title: undefined });
    expect(saveGate('  ', { kind: 'none' }).canSave).toBe(false);
  });

  it('disables Save while the generator read is pending', () => {
    const gate = saveGate('scenes/camp.yaml', { kind: 'unknown' });
    expect(gate.canSave).toBe(false);
    expect(gate.title).toContain('checking');
  });

  it('keeps Save disabled when the generator read failed, naming the error', () => {
    const gate = saveGate('scenes/camp.yaml', { kind: 'error', message: 'permission denied' });
    expect(gate.canSave).toBe(false);
    expect(gate.title).toContain('permission denied');
  });
});

describe('classifySceneText', () => {
  it('is a plain scene with no kind line', () => {
    expect(classifySceneText('format: 1\nname: x\nentities:')).toEqual({ scene: true, generated: false });
  });

  it('is not a scene when kind appears at column 0 on line 2', () => {
    expect(classifySceneText('format: 1\nkind: voxel\nsize: [4, 4, 4]')).toEqual({ scene: false });
  });

  it('is generated when generated_from appears at column 0', () => {
    expect(classifySceneText('format: 1\ngenerated_from: src/camp.map\nentities:')).toEqual({
      scene: true,
      generated: true,
    });
  });

  it('stays a scene when kind is indented (inside a component, not a document header)', () => {
    expect(classifySceneText('format: 1\nentities:\n  - components:\n      kind: foo')).toEqual({
      scene: true,
      generated: false,
    });
  });

  it('stays a scene when kind appears beyond the first 40 lines', () => {
    const lines = ['format: 1', ...Array.from({ length: 40 }, (_, i) => `# line ${i}`), 'kind: voxel'];
    expect(classifySceneText(lines.join('\n'))).toEqual({ scene: true, generated: false });
  });

  it('handles CRLF line endings', () => {
    expect(classifySceneText('format: 1\r\nkind: voxel\r\nsize: [4, 4, 4]')).toEqual({ scene: false });
  });
});

describe('isStageScene', () => {
  it('matches a hyphenated -stage basename, with or without a further suffix', () => {
    expect(isStageScene('model-stage.yaml')).toBe(true);
    expect(isStageScene('sub/model-stage-night.yaml')).toBe(true);
    expect(isStageScene('effects-stage.yaml')).toBe(true);
  });

  it('requires the hyphen, so a bare or fused "stage" name is not one', () => {
    expect(isStageScene('stage.yaml')).toBe(false);
    expect(isStageScene('backstage.yaml')).toBe(false);
  });
});

describe('listScenes', () => {
  const entry = (relPath: string, isDir = false): DirEntry => ({ relPath, name: relPath, isDir });

  it('skips directories, drops a rejected read, and keeps listing order', async () => {
    const entries: DirEntry[] = [entry('sub', true), entry('a.yaml'), entry('bad.yaml'), entry('b.yaml')];
    const texts: Record<string, string> = { 'a.yaml': 'format: 1\n', 'b.yaml': 'format: 1\n' };
    const result = await listScenes(entries, (relPath) => {
      const text = texts[relPath];
      return text !== undefined ? Promise.resolve(text) : Promise.reject(new Error('missing'));
    });
    expect(result.map((e) => e.relPath)).toEqual(['a.yaml', 'b.yaml']);
  });

  it('drops non-scene documents and marks stage/generated ones', async () => {
    const entries: DirEntry[] = [entry('main.yaml'), entry('voxel.yaml'), entry('model-stage.yaml'), entry('camp.yaml')];
    const texts: Record<string, string> = {
      'main.yaml': 'format: 1\nentities:',
      'voxel.yaml': 'format: 1\nkind: voxel',
      'model-stage.yaml': 'format: 1\nentities:',
      'camp.yaml': 'format: 1\ngenerated_from: src/camp.map',
    };
    const result = await listScenes(entries, (relPath) => Promise.resolve(texts[relPath] ?? ''));
    expect(result).toEqual([
      { relPath: 'main.yaml', marks: { stage: false, generated: false } },
      { relPath: 'model-stage.yaml', marks: { stage: true, generated: false } },
      { relPath: 'camp.yaml', marks: { stage: false, generated: true } },
    ]);
  });
});

describe('sameScenePath', () => {
  it('folds backslashes and strips a leading ./', () => {
    expect(sameScenePath('main.yaml', './main.yaml')).toBe(true);
    expect(sameScenePath('sub\\a.yaml', 'sub/a.yaml')).toBe(true);
  });

  it('is false for genuinely different paths', () => {
    expect(sameScenePath('main.yaml', 'other.yaml')).toBe(false);
  });

  it('never matches an empty side, itself included', () => {
    expect(sameScenePath('', '')).toBe(false);
    expect(sameScenePath('', 'main.yaml')).toBe(false);
    expect(sameScenePath('main.yaml', '')).toBe(false);
  });
});

describe('pickScene', () => {
  it('loads directly when not dirty, without confirming', async () => {
    const confirm = () => Promise.resolve(false);
    let loadedPath: string | null = null;
    const load = (path: string) => {
      loadedPath = path;
      return Promise.resolve(undefined);
    };
    const outcome = await pickScene('main.yaml', { dirty: false, savePath: 'other.yaml', confirm, load });
    expect(outcome).toBe('loaded');
    expect(loadedPath).toBe('main.yaml');
  });

  it('cancels without loading when dirty and the confirmation is declined', async () => {
    let loadCalled = false;
    const outcome = await pickScene('main.yaml', {
      dirty: true,
      savePath: 'other.yaml',
      confirm: () => Promise.resolve(false),
      load: () => {
        loadCalled = true;
        return Promise.resolve(undefined);
      },
    });
    expect(outcome).toBe('cancelled');
    expect(loadCalled).toBe(false);
  });

  it('names savePath in the confirmation message, then loads when accepted', async () => {
    let message: string | null = null;
    const outcome = await pickScene('main.yaml', {
      dirty: true,
      savePath: 'other.yaml',
      confirm: (m) => {
        message = m;
        return Promise.resolve(true);
      },
      load: () => Promise.resolve(undefined),
    });
    expect(message).toBe('Discard unsaved changes to other.yaml?');
    expect(outcome).toBe('loaded');
  });

  it('uses a path-free message when savePath is empty', async () => {
    let message: string | null = null;
    await pickScene('main.yaml', {
      dirty: true,
      savePath: '',
      confirm: (m) => {
        message = m;
        return Promise.resolve(true);
      },
      load: () => Promise.resolve(undefined),
    });
    expect(message).toBe('Discard unsaved changes?');
  });
});
