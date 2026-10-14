import { describe, expect, it } from 'vitest';
import { SCULPT_EXT, sculptAssetRef, sculptDisplayName, sculptFilePath, sculptsRoot } from './stage';

describe('paths', () => {
  it('lists sculpts beside the voxels, under the content root', () => {
    expect(sculptsRoot('/repo/games/vigil')).toBe('/repo/games/vigil/scenes/sculpts');
  });

  it('tolerates a Windows project dir and a trailing separator', () => {
    expect(sculptsRoot('D:\\repo\\games\\vigil\\')).toBe('D:/repo/games/vigil/scenes/sculpts');
  });

  it('builds the absolute path of a nested model', () => {
    expect(sculptFilePath('/repo/games/vigil', 'idols/idol.sculpt.yaml')).toBe(
      '/repo/games/vigil/scenes/sculpts/idols/idol.sculpt.yaml',
    );
  });

  // The reference is content-root-relative, so it is byte-identical to what a committed scene
  // writes into `Mesh.shape.sculpt` — the stage is real game content, not a preview fixture.
  it('names a model the way a scene does', () => {
    expect(sculptAssetRef('idol.sculpt.yaml')).toBe('sculpts/idol.sculpt.yaml');
    expect(sculptAssetRef('idols/idol.sculpt.yaml')).toBe('sculpts/idols/idol.sculpt.yaml');
  });
});

describe('display', () => {
  it('strips the double extension', () => {
    expect(SCULPT_EXT).toBe('.sculpt.yaml');
    expect(sculptDisplayName('idols/idol.sculpt.yaml')).toBe('idol');
  });

  it('leaves a name that does not carry it alone', () => {
    expect(sculptDisplayName('notes.txt')).toBe('notes.txt');
  });
});
