// The asset browser's tree: `listEditorDir`'s flat, depth-first `DirEntry[]` (`shell/host.ts`)
// nested into the shape the browser renders, plus the search filter that goes with it.
//
// CATEGORIES ARE FOLDERS (`Plan/editor-voxel-artist.md`): the directory structure IS the taxonomy.
// Nothing here groups, renames, splits or infers — the tree is the listing, nested. A derived usage
// index (model → use sites → category) was proposed and rejected in favour of plain file
// organization, so this module must never grow one.
//
// Pure: no React, no Tauri, no DOM. `DirEntry` is a type-only import, so nothing loads at runtime.

import type { DirEntry } from '../../shell/host';

/** One node of the browser tree. Files always have an empty `children`. */
export interface TreeNode {
  /** Path relative to the listed root, `/`-separated — the `DirEntry.relPath` verbatim. It is the
   *  node's identity: selection, expansion state, and `findNode` all key on it. */
  readonly relPath: string;
  /** The final path segment (`statue.voxel.yaml`, `enemies`). */
  readonly name: string;
  readonly isDir: boolean;
  /** Children in listing order (the host emits directories first, then case-insensitive by name). */
  readonly children: readonly TreeNode[];
}

/**
 * Nests a flat `listEditorDir` listing.
 *
 * Listing order is preserved exactly within every parent — the host already sorts directories first
 * then case-insensitively by name, and that is the order the browser renders. Nesting is by
 * `relPath` prefix, not by relying on the depth-first emission order, so an entry whose parent is
 * missing from the listing lands at the root rather than being dropped: a browser that silently
 * loses a model is worse than one showing it in the wrong place.
 */
export function buildTree(entries: readonly DirEntry[]): readonly TreeNode[] {
  const roots: TreeNode[] = [];
  const childrenOf = new Map<string, TreeNode[]>();

  for (const entry of entries) {
    const node: TreeNode = {
      relPath: entry.relPath,
      name: entry.name,
      isDir: entry.isDir,
      children: entry.isDir ? [] : EMPTY,
    };
    if (entry.isDir) {
      childrenOf.set(entry.relPath, node.children as TreeNode[]);
    }
    const cut = entry.relPath.lastIndexOf('/');
    const parent = cut < 0 ? undefined : childrenOf.get(entry.relPath.slice(0, cut));
    (parent ?? roots).push(node);
  }
  return roots;
}

const EMPTY: readonly TreeNode[] = [];

/** Directories-first, case-insensitive-by-name — the order `listEditorDir` already returns, and what
 *  a merge across several listings must reproduce rather than leaving to insertion order. */
function compareNodes(a: TreeNode, b: TreeNode): number {
  if (a.isDir !== b.isDir) {
    return a.isDir ? -1 : 1;
  }
  return a.name.localeCompare(b.name, undefined, { sensitivity: 'base' });
}

/** Merges two sibling lists, folding a directory present in both into one node whose children are
 *  merged recursively — every listing carries the full directory set regardless of its own
 *  extension, so a bare flatten would double every folder. A file path repeated across listings
 *  keeps its first occurrence rather than being duplicated (a listed extension is a suffix, not
 *  guaranteed disjoint from another). */
function mergeNodeLists(a: readonly TreeNode[], b: readonly TreeNode[]): TreeNode[] {
  const byPath = new Map<string, TreeNode>();
  for (const node of a) {
    byPath.set(node.relPath, node);
  }
  for (const node of b) {
    const existing = byPath.get(node.relPath);
    if (existing === undefined) {
      byPath.set(node.relPath, node);
    } else if (existing.isDir && node.isDir) {
      byPath.set(node.relPath, { ...existing, children: mergeNodeLists(existing.children, node.children) });
    }
  }
  return [...byPath.values()].sort(compareNodes);
}

/**
 * Merges several `buildTree` results — one per listed extension — into the one tree a
 * multi-extension browser renders. Nothing here groups or infers a category beyond what
 * `mergeNodeLists` already folds: a directory that only ever held files of unlisted kinds arrives
 * with empty `children`, which is exactly what `prune` is for.
 */
export function mergeTrees(trees: readonly (readonly TreeNode[])[]): readonly TreeNode[] {
  return trees.reduce<readonly TreeNode[]>((acc, tree) => mergeNodeLists(acc, tree), []);
}

/**
 * Filters the tree by a case-insensitive substring of a node's `name`.
 *
 * A directory survives when **any descendant matches** — a filtered tree with orphaned parents is
 * useless, and one whose parents are dropped cannot be rendered at all. A directory whose own name
 * matches keeps its **whole** subtree: clicking through to a category the search just named should
 * show the category, not a filtered slice of it.
 *
 * An empty or whitespace-only `query` returns `nodes` unchanged (same reference).
 */
export function filterTree(nodes: readonly TreeNode[], query: string): readonly TreeNode[] {
  const needle = query.trim().toLowerCase();
  if (needle === '') {
    return nodes;
  }
  const out: TreeNode[] = [];
  for (const node of nodes) {
    if (node.name.toLowerCase().includes(needle)) {
      out.push(node);
      continue;
    }
    if (!node.isDir) {
      continue;
    }
    const children = filterTree(node.children, query);
    if (children.length > 0) {
      out.push({ ...node, children });
    }
  }
  return out;
}

/** The node with this `relPath`, searched depth-first, or `null`. */
export function findNode(nodes: readonly TreeNode[], relPath: string): TreeNode | null {
  for (const node of nodes) {
    if (node.relPath === relPath) {
      return node;
    }
    const hit = findNode(node.children, relPath);
    if (hit !== null) {
      return hit;
    }
  }
  return null;
}

/** Every directory `relPath` in the tree, depth-first — the expansion set for "expand all", which
 *  is what a search result wants (a match three folders down must be visible without clicking). */
export function dirPaths(nodes: readonly TreeNode[]): string[] {
  const out: string[] = [];
  const walk = (level: readonly TreeNode[]): void => {
    for (const node of level) {
      if (node.isDir) {
        out.push(node.relPath);
        walk(node.children);
      }
    }
  };
  walk(nodes);
  return out;
}

/** A file node's label: its basename with the model kind's double extension stripped, case
 *  insensitively. `ext` is one extension or several — a browser merging several listed kinds strips
 *  whichever one the file actually carries. A name that does not carry any listed extension is shown
 *  verbatim rather than silently truncated. */
export function displayName(name: string, ext: string | readonly string[]): string {
  const base = name.split('/').at(-1) ?? name;
  const match = matchingExt(base, typeof ext === 'string' ? [ext] : ext);
  return match === null ? base : base.slice(0, base.length - match.length);
}

/** Which of `exts` this file's basename carries, case-insensitively — the longest matching suffix,
 *  so a hypothetical `.yaml` in the same list never shadows `.voxel.yaml`. `null` when none match. */
export function matchingExt(name: string, exts: readonly string[]): string | null {
  const base = name.split('/').at(-1) ?? name;
  const lower = base.toLowerCase();
  let best: string | null = null;
  for (const ext of exts) {
    if (lower.endsWith(ext.toLowerCase()) && (best === null || ext.length > best.length)) {
      best = ext;
    }
  }
  return best;
}

/** The kind badge's label for a listed extension: `.assembly.yaml` → `assembly`, `.voxel.yaml` →
 *  `voxel` — the double extension's own name, leading dot and trailing `.yaml` stripped. Shown only
 *  when a browser lists more than one kind, so a single-kind library never grows a redundant badge. */
export function extBadge(ext: string): string {
  return ext.replace(/^\./, '').replace(/\.yaml$/i, '');
}

/** The directory `relPath`s containing `relPath`, outermost first (`a/b/c.yaml` → `['a', 'a/b']`) —
 *  the set to expand when revealing a selection. Purely textual; it does not consult the tree. */
export function ancestorDirs(relPath: string): string[] {
  const segments = relPath.split('/');
  const out: string[] = [];
  for (let i = 1; i < segments.length; i += 1) {
    out.push(segments.slice(0, i).join('/'));
  }
  return out;
}
