import { describe, expect, it } from 'vitest';
import { MESH_EXT, meshAssetRef, meshDisplayName, meshFilePath, meshesRoot } from './stage';

describe('paths', () => {
  it('lists meshes from the content root itself, not a subdirectory', () => {
    expect(meshesRoot('/repo/games/vigil')).toBe('/repo/games/vigil/scenes');
  });

  it('tolerates a Windows project dir and a trailing separator', () => {
    expect(meshesRoot('D:\\repo\\games\\vigil\\')).toBe('D:/repo/games/vigil/scenes');
  });

  it('builds the absolute path of a nested mesh document', () => {
    expect(meshFilePath('/repo/games/vigil', 'meshes/idol-generated/meshes/mesh-0.mesh.yaml')).toBe(
      '/repo/games/vigil/scenes/meshes/idol-generated/meshes/mesh-0.mesh.yaml',
    );
  });

  // The library root IS the stage session's content root, so the reference is the listing path — the
  // one place this library differs from the voxel and sculpt ones, which prepend their subdirectory.
  it('names a mesh the way a scene does', () => {
    expect(meshAssetRef('meshes/idol-generated/meshes/mesh-0.mesh.yaml')).toBe(
      'meshes/idol-generated/meshes/mesh-0.mesh.yaml',
    );
  });
});

describe('display', () => {
  it('strips the double extension', () => {
    expect(meshDisplayName('meshes/idol-generated/meshes/mesh-0.mesh.yaml')).toBe('mesh-0');
  });

  // The host's `ext` filter is a plain suffix match, so this is what keeps a `kind: mesh-edit` op
  // list out of a library of models.
  it('does not match a mesh-edit op list', () => {
    expect('idol-lit.mesh-edit.yaml'.endsWith(MESH_EXT)).toBe(false);
    expect('mesh.mesh.yaml'.endsWith(MESH_EXT)).toBe(true);
  });
});
