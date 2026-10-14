import { describe, expect, it } from 'vitest';
import { buildEditorContext, statusSinkPath } from './context';

describe('buildEditorContext', () => {
  it('normalizes an empty scene path to null and passes selection through', () => {
    const ctx = buildEditorContext({
      view: 'level-designer',
      scenePath: '',
      selection: { id: { index: 3, generation: 1 }, components: { Transform: { position: [1, 0, 2] } } },
      pointer: null,
    });
    expect(ctx.scenePath).toBeNull();
    expect(ctx.view).toBe('level-designer');
    expect(ctx.selection?.id).toEqual({ index: 3, generation: 1 });
    expect(ctx.selection?.components).toEqual({ Transform: { position: [1, 0, 2] } });
  });

  it('keeps a real scene path and a null selection', () => {
    const ctx = buildEditorContext({
      view: 'level-designer',
      scenePath: 'district-plaza.yaml',
      selection: null,
      pointer: null,
    });
    expect(ctx.scenePath).toBe('district-plaza.yaml');
    expect(ctx.selection).toBeNull();
  });

  it('passes a null pointer through', () => {
    const ctx = buildEditorContext({
      view: 'level-designer',
      scenePath: 'district-plaza.yaml',
      selection: null,
      pointer: null,
    });
    expect(ctx.pointer).toBeNull();
  });

  it('passes a present pointer position through', () => {
    const ctx = buildEditorContext({
      view: 'level-designer',
      scenePath: 'district-plaza.yaml',
      selection: null,
      pointer: { position: [1.5, 0, -2.25] },
    });
    expect(ctx.pointer).toEqual({ position: [1.5, 0, -2.25] });
  });
});

describe('the voxel block', () => {
  // `editor/personas/voxel-artist.md` promises this block exists and names its three parts, so the
  // field names are a contract with a file the editor does not own.
  it('carries the model path, the rig, and the camera pose', () => {
    const ctx = buildEditorContext({
      view: 'voxel-artist',
      scenePath: 'model-stage-night',
      selection: null,
      voxel: {
        modelPath: 'scenes/voxels/statue.voxel.yaml',
        rig: 'night',
        cameraPose: { translation: [0, 2, -7], rotation: [0, 180, 0] },
      },
    });
    expect(ctx.voxel).toEqual({
      modelPath: 'scenes/voxels/statue.voxel.yaml',
      rig: 'night',
      cameraPose: { translation: [0, 2, -7], rotation: [0, 180, 0] },
    });
  });

  it('is null in every other view rather than absent — a persona can tell "no model" from stale', () => {
    const ctx = buildEditorContext({
      view: 'level-designer',
      scenePath: 'district-plaza.yaml',
      selection: null,
      pointer: null,
    });
    expect(ctx.voxel).toBeNull();
    expect('voxel' in ctx).toBe(true);
  });

  // Decision 6, `Plan/editor-voxel-assemblies.md`: the `assembly` sub-block is additive, so a plain
  // voxel model's serialized block must stay byte-identical to what it was before assemblies.
  it('omits the assembly field for a plain voxel model, so the written JSON is unchanged', () => {
    const ctx = buildEditorContext({
      view: 'voxel-artist',
      scenePath: 'model-stage',
      selection: null,
      voxel: {
        modelPath: 'scenes/voxels/statue.voxel.yaml',
        rig: 'neutral',
        cameraPose: null,
      },
    });
    expect(JSON.stringify(ctx.voxel)).toBe(
      '{"modelPath":"scenes/voxels/statue.voxel.yaml","rig":"neutral","cameraPose":null}',
    );
  });

  it('carries the assembly path, the open part, the clip and the paused tick/seconds', () => {
    const ctx = buildEditorContext({
      view: 'voxel-artist',
      scenePath: 'model-stage',
      selection: null,
      voxel: {
        modelPath: 'scenes/voxels/player.assembly.yaml',
        rig: 'neutral',
        cameraPose: null,
        assembly: {
          path: 'scenes/voxels/player.assembly.yaml',
          part: { name: 'lower-arm-l', path: 'scenes/voxels/parts/lower-arm-l.part.yaml' },
          clip: 'walk',
          tick: 27,
          seconds: 0.45,
          playing: false,
        },
      },
    });
    expect(ctx.voxel?.assembly).toEqual({
      path: 'scenes/voxels/player.assembly.yaml',
      part: { name: 'lower-arm-l', path: 'scenes/voxels/parts/lower-arm-l.part.yaml' },
      clip: 'walk',
      tick: 27,
      seconds: 0.45,
      playing: false,
    });
  });

  it('carries a null tick and seconds while the clip plays, with playing true', () => {
    const ctx = buildEditorContext({
      view: 'voxel-artist',
      scenePath: 'model-stage',
      selection: null,
      voxel: {
        modelPath: 'scenes/voxels/player.assembly.yaml',
        rig: 'neutral',
        cameraPose: null,
        assembly: {
          path: 'scenes/voxels/player.assembly.yaml',
          part: null,
          clip: 'walk',
          tick: null,
          seconds: null,
          playing: true,
        },
      },
    });
    expect(ctx.voxel?.assembly).toEqual({
      path: 'scenes/voxels/player.assembly.yaml',
      part: null,
      clip: 'walk',
      tick: null,
      seconds: null,
      playing: true,
    });
  });

  it('carries a null part and clip at rest', () => {
    const ctx = buildEditorContext({
      view: 'voxel-artist',
      scenePath: 'model-stage',
      selection: null,
      voxel: {
        modelPath: 'scenes/voxels/player.assembly.yaml',
        rig: 'neutral',
        cameraPose: null,
        assembly: {
          path: 'scenes/voxels/player.assembly.yaml',
          part: null,
          clip: null,
          tick: null,
          seconds: null,
          playing: true,
        },
      },
    });
    expect(ctx.voxel?.assembly?.clip).toBeNull();
    expect(ctx.voxel?.assembly?.part).toBeNull();
  });
});

describe('the art director block', () => {
  // `editor/personas/art-director.md` promises this block exists and names its three parts, so the
  // field names are a contract with a file the editor does not own.
  it('carries the target entity, whether it is authored, and the open effect row', () => {
    const ctx = buildEditorContext({
      view: 'art-director',
      scenePath: 'district-plaza.yaml',
      selection: null,
      artDirector: { entity: { index: 3, generation: 1 }, authored: false, effect: 'fog' },
    });
    expect(ctx.artDirector).toEqual({
      entity: { index: 3, generation: 1 },
      authored: false,
      effect: 'fog',
    });
  });

  it('is null in every other view rather than absent', () => {
    const ctx = buildEditorContext({
      view: 'level-designer',
      scenePath: 'district-plaza.yaml',
      selection: null,
    });
    expect(ctx.artDirector).toBeNull();
    expect('artDirector' in ctx).toBe(true);
  });
});

describe('the lighter block', () => {
  it('carries the volume target, whether it is authored, and the sun', () => {
    const ctx = buildEditorContext({
      view: 'lighter',
      scenePath: 'gi-interior.yaml',
      selection: null,
      lighter: {
        entity: { index: 2, generation: 0 },
        authored: true,
        sun: { index: 5, generation: 0 },
      },
    });
    expect(ctx.lighter).toEqual({
      entity: { index: 2, generation: 0 },
      authored: true,
      sun: { index: 5, generation: 0 },
    });
  });

  it('carries a null target and a null sun rather than dropping the block', () => {
    const ctx = buildEditorContext({
      view: 'lighter',
      scenePath: 'empty.yaml',
      selection: null,
      lighter: { entity: null, authored: false, sun: null },
    });
    expect(ctx.lighter).toEqual({ entity: null, authored: false, sun: null });
  });

  it('is null in every other view rather than absent', () => {
    const ctx = buildEditorContext({
      view: 'level-designer',
      scenePath: 'district-plaza.yaml',
      selection: null,
    });
    expect(ctx.lighter).toBeNull();
    expect('lighter' in ctx).toBe(true);
  });
  it('carries the viewport camera, with the pose only while the view camera is flying', () => {
    const flown = buildEditorContext({
      view: 'level-designer',
      scenePath: 'district-plaza.yaml',
      selection: null,
      camera: { mode: 'view', pose: { translation: [3, 12, -4], rotation: [-20, 45, 0] } },
    });
    expect(flown.camera).toEqual({ mode: 'view', pose: { translation: [3, 12, -4], rotation: [-20, 45, 0] } });
    const game = buildEditorContext({
      view: 'level-designer',
      scenePath: 'district-plaza.yaml',
      selection: null,
      camera: { mode: 'game', pose: null },
    });
    expect(game.camera).toEqual({ mode: 'game', pose: null });
  });

  it('defaults the camera to null for a caller that reports none', () => {
    const ctx = buildEditorContext({ view: 'generation', scenePath: null, selection: null });
    expect(ctx.camera).toBeNull();
  });
});

describe('statusSinkPath', () => {
  it('places the sink under the project .ludion dir, keyed by term id', () => {
    expect(statusSinkPath('D:/games/vigil', 'level-designer-xyz')).toBe(
      'D:/games/vigil/.ludion/claude-status-level-designer-xyz.json',
    );
  });
});
