import { describe, expect, it } from 'vitest';
import type { DirEntry } from '../../shell/host';
import {
  ancestorDirs,
  buildTree,
  dirPaths,
  displayName,
  extBadge,
  filterTree,
  findNode,
  matchingExt,
  mergeTrees,
} from './tree';

const dir = (relPath: string): DirEntry => ({
  relPath,
  name: relPath.split('/').at(-1) as string,
  isDir: true,
});
const file = (relPath: string): DirEntry => ({
  relPath,
  name: relPath.split('/').at(-1) as string,
  isDir: false,
});

// A `listEditorDir` listing in the exact shape the host emits: flat, depth-first, directories before
// their contents and before sibling files, then case-insensitive by name. `enemies/beasts` is a
// nested category.
const LISTING: readonly DirEntry[] = [
  dir('buildings'),
  file('buildings/house-a.voxel.yaml'),
  file('buildings/house-b.voxel.yaml'),
  dir('enemies'),
  dir('enemies/beasts'),
  file('enemies/beasts/boar.voxel.yaml'),
  file('enemies/beasts/wolf.voxel.yaml'),
  file('enemies/monster-grunt.voxel.yaml'),
  file('adventurer.voxel.yaml'),
];

describe('buildTree', () => {
  it('nests the listing without reordering or regrouping it', () => {
    const tree = buildTree(LISTING);
    expect(tree.map((n) => n.relPath)).toEqual(['buildings', 'enemies', 'adventurer.voxel.yaml']);
    expect(tree[0].children.map((n) => n.name)).toEqual([
      'house-a.voxel.yaml',
      'house-b.voxel.yaml',
    ]);
    // Categories are folders: `enemies` keeps its sub-category AND its own loose file, in order.
    expect(tree[1].children.map((n) => n.relPath)).toEqual([
      'enemies/beasts',
      'enemies/monster-grunt.voxel.yaml',
    ]);
    expect(tree[1].children[0].children.map((n) => n.name)).toEqual([
      'boar.voxel.yaml',
      'wolf.voxel.yaml',
    ]);
  });

  it('marks files as leaves and directories as containers', () => {
    const tree = buildTree(LISTING);
    expect(tree[2]).toEqual({
      relPath: 'adventurer.voxel.yaml',
      name: 'adventurer.voxel.yaml',
      isDir: false,
      children: [],
    });
  });

  it('never drops an entry whose parent is missing from the listing', () => {
    const tree = buildTree([file('orphan/boar.voxel.yaml')]);
    expect(tree.map((n) => n.relPath)).toEqual(['orphan/boar.voxel.yaml']);
  });

  it('an empty listing is an empty tree', () => {
    expect(buildTree([])).toEqual([]);
  });
});

describe('filterTree', () => {
  const tree = buildTree(LISTING);

  it('keeps the whole parent chain of a deep match', () => {
    const filtered = filterTree(tree, 'wolf');
    expect(filtered.map((n) => n.relPath)).toEqual(['enemies']);
    expect(filtered[0].children.map((n) => n.relPath)).toEqual(['enemies/beasts']);
    expect(filtered[0].children[0].children.map((n) => n.name)).toEqual(['wolf.voxel.yaml']);
  });

  it('is case-insensitive and matches substrings', () => {
    expect(filterTree(tree, 'HOUSE')[0].children).toHaveLength(2);
  });

  it('a directory whose own name matches keeps its whole subtree', () => {
    const filtered = filterTree(tree, 'beasts');
    expect(filtered[0].children[0].children.map((n) => n.name)).toEqual([
      'boar.voxel.yaml',
      'wolf.voxel.yaml',
    ]);
  });

  it('drops directories with no matching descendant', () => {
    expect(filterTree(tree, 'boar').map((n) => n.relPath)).toEqual(['enemies']);
  });

  it('an empty or whitespace query returns the tree untouched', () => {
    expect(filterTree(tree, '')).toBe(tree);
    expect(filterTree(tree, '   ')).toBe(tree);
  });

  it('a match on nothing is an empty tree, not the whole tree', () => {
    expect(filterTree(tree, 'zzz')).toEqual([]);
  });
});

describe('navigation helpers', () => {
  const tree = buildTree(LISTING);

  it('findNode searches every depth and returns null when absent', () => {
    expect(findNode(tree, 'enemies/beasts/boar.voxel.yaml')?.name).toBe('boar.voxel.yaml');
    expect(findNode(tree, 'enemies/beasts')?.isDir).toBe(true);
    expect(findNode(tree, 'nope')).toBeNull();
  });

  it('dirPaths lists every directory, depth-first', () => {
    expect(dirPaths(tree)).toEqual(['buildings', 'enemies', 'enemies/beasts']);
  });

  it('ancestorDirs walks a path outermost first', () => {
    expect(ancestorDirs('enemies/beasts/boar.voxel.yaml')).toEqual(['enemies', 'enemies/beasts']);
    expect(ancestorDirs('adventurer.voxel.yaml')).toEqual([]);
  });
});

describe('display names', () => {
  it('strips the model kind the browser was given', () => {
    expect(displayName('boar.voxel.yaml', '.voxel.yaml')).toBe('boar');
    expect(displayName('idol.sculpt.yaml', '.sculpt.yaml')).toBe('idol');
  });

  // A file that does not carry the extension is shown verbatim rather than blindly truncated — the
  // browser filters by the same suffix, so this is only reachable through a listing that did not.
  it('leaves an unrelated name alone', () => {
    expect(displayName('boar.voxel.yaml', '.sculpt.yaml')).toBe('boar.voxel.yaml');
  });

  it('takes the basename of a nested path', () => {
    expect(displayName('enemies/beasts/boar.voxel.yaml', '.voxel.yaml')).toBe('boar');
  });

  it('strips whichever of several listed extensions the name carries', () => {
    expect(displayName('boar.voxel.yaml', ['.voxel.yaml', '.assembly.yaml'])).toBe('boar');
    expect(displayName('player.assembly.yaml', ['.voxel.yaml', '.assembly.yaml'])).toBe('player');
  });

  it('leaves a name matching none of several listed extensions alone', () => {
    expect(displayName('boar.part.yaml', ['.voxel.yaml', '.assembly.yaml'])).toBe('boar.part.yaml');
  });
});

describe('matchingExt', () => {
  it('finds the one extension a single-element list carries', () => {
    expect(matchingExt('boar.voxel.yaml', ['.voxel.yaml'])).toBe('.voxel.yaml');
  });

  it('finds which of several listed extensions a name carries', () => {
    expect(matchingExt('player.assembly.yaml', ['.voxel.yaml', '.assembly.yaml'])).toBe('.assembly.yaml');
  });

  it('is case-insensitive', () => {
    expect(matchingExt('BOAR.VOXEL.YAML', ['.voxel.yaml', '.assembly.yaml'])).toBe('.voxel.yaml');
  });

  it('prefers the longest match, so a broader suffix never shadows a more specific one', () => {
    expect(matchingExt('boar.voxel.yaml', ['.yaml', '.voxel.yaml'])).toBe('.voxel.yaml');
  });

  it('returns null when nothing matches', () => {
    expect(matchingExt('boar.part.yaml', ['.voxel.yaml', '.assembly.yaml'])).toBeNull();
  });
});

describe('extBadge', () => {
  it('strips the leading dot and the trailing .yaml', () => {
    expect(extBadge('.assembly.yaml')).toBe('assembly');
    expect(extBadge('.voxel.yaml')).toBe('voxel');
  });
});

describe('mergeTrees', () => {
  const voxelListing: readonly DirEntry[] = [
    dir('parts'),
    dir('enemies'),
    file('enemies/boar.voxel.yaml'),
    file('adventurer.voxel.yaml'),
  ];
  const assemblyListing: readonly DirEntry[] = [
    dir('parts'),
    dir('enemies'),
    file('enemies/boar.assembly.yaml'),
    file('player.assembly.yaml'),
  ];

  it('folds a directory the host lists under every extension into one node', () => {
    const merged = mergeTrees([buildTree(voxelListing), buildTree(assemblyListing)]);
    // Not two `enemies` rows: one node holding both extensions' files, directories first then
    // case-insensitive by name, exactly the single-listing order.
    expect(merged.map((n) => n.relPath)).toEqual(['enemies', 'parts', 'adventurer.voxel.yaml', 'player.assembly.yaml']);
    expect(merged[0].children.map((n) => n.name)).toEqual(['boar.assembly.yaml', 'boar.voxel.yaml']);
  });

  it('a directory listed by only one extension keeps only that extension’s files', () => {
    const partsOnlyVoxel: readonly DirEntry[] = [dir('parts'), file('parts/leg.part.yaml')];
    const merged = mergeTrees([buildTree(partsOnlyVoxel), buildTree(voxelListing)]);
    const parts = merged.find((n) => n.relPath === 'parts');
    expect(parts?.children.map((n) => n.name)).toEqual(['leg.part.yaml']);
  });

  it('merging zero trees is an empty tree, and merging one is that tree, reordered to the sort rule', () => {
    expect(mergeTrees([])).toEqual([]);
    expect(mergeTrees([buildTree(voxelListing)]).map((n) => n.relPath)).toEqual([
      'enemies',
      'parts',
      'adventurer.voxel.yaml',
    ]);
  });
});
