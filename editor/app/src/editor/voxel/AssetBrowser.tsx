// The model asset browser: every file of one or more model kinds under a listed root, nested by
// folder, searchable, one click to put a model on the stage.
//
// THE KIND IS A PROP. The Voxel Artist lists `.voxel.yaml` under `<projectDir>/scenes/voxels` and
// the Models view lists `.sculpt.yaml` under `<projectDir>/scenes/sculpts`; nothing else about the
// browsing differs, so the root and the extension arrive from the view rather than forking the
// component. `ext` also takes several extensions at once — an assembly library listing
// `.assembly.yaml` beside a model kind, say — and does one host listing per extension, merging the
// results client-side (`mergeTrees` in `voxel/tree.ts`); the host's own listing stays single-suffix.
//
// CATEGORIES ARE FOLDERS. The tree is the host's listing, nested (`voxel/tree.ts`) — nothing here
// groups, renames or infers a category, and no usage index is derived. Thin wiring: the listing is
// one host call per extension and every derivation is the pure module's.
//
// Props beyond the listing: `prune`, `head`/`counted` (the panel's wording) and `onFilePointerDown`
// (a file row's press, for a caller that drags rows out of the list).

import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { listEditorDir, type DirEntry } from '../../shell/host';
import {
  ancestorDirs,
  buildTree,
  displayName,
  extBadge,
  filterTree,
  matchingExt,
  mergeTrees,
  type TreeNode,
} from './tree';

interface AssetBrowserProps {
  /** Absolute path of the directory to list, or `''` when there is no project dir yet. */
  readonly root: string;
  /** The model kind's double extension (`.voxel.yaml`, `.sculpt.yaml`) — the listing filter, and
   *  what a row's label strips. Several extensions merge into one tree, one host listing each; a
   *  file row then also carries a kind badge, since with only one listed kind the badge would say
   *  nothing a human doesn't already know. */
  readonly ext: string | readonly string[];
  /** The selected model's path relative to `root`, or `null`. */
  readonly selected: string | null;
  readonly onSelect: (relPath: string) => void;
  /** Bumped by the parent to force a re-list (a model added on disk). */
  readonly reloadKey?: number;
  /** Whether to hide directories holding no match. The libraries rooted at a dedicated folder leave
   *  it off — there an empty category is still a category, which is why the host lists directories
   *  whatever `ext` says. The mesh library is rooted at the whole content root, where the folders
   *  holding no mesh are the rest of the game. */
  readonly prune?: boolean;
  /** The panel head, and the word its count uses. Defaults to the model library's, since that is
   *  what three of the four callers are browsing; the Generation view's recipes are not models. */
  readonly head?: string;
  readonly counted?: string;
  /** Fired on a file row's primary-button press, so a caller can start a drag out of the list (the
   *  row stays click-selectable). Directory rows never fire it. */
  readonly onFilePointerDown?: (relPath: string, event: React.PointerEvent) => void;
}

/** The tree with every directory holding no matching file anywhere beneath it removed. */
function withoutEmptyDirs(nodes: readonly TreeNode[]): TreeNode[] {
  const out: TreeNode[] = [];
  for (const node of nodes) {
    if (!node.isDir) {
      out.push(node);
      continue;
    }
    const children = withoutEmptyDirs(node.children);
    if (children.length > 0) {
      out.push({ ...node, children });
    }
  }
  return out;
}

export function AssetBrowser({
  root,
  ext,
  selected,
  onSelect,
  reloadKey = 0,
  prune = false,
  head = 'Models',
  counted = 'models',
  onFilePointerDown,
}: AssetBrowserProps) {
  // Normalized once so every reader downstream (the fetch, the badge, `displayName`) sees the same
  // list regardless of which shape the caller passed.
  const extList = useMemo(() => (typeof ext === 'string' ? [ext] : ext), [ext]);
  const extKey = extList.join('\u0000');
  const showKindBadge = extList.length > 1;

  // One listing per extension — the host's own filter is single-suffix, so several kinds are several
  // calls, merged client-side below.
  const [listings, setListings] = useState<readonly (readonly DirEntry[])[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());

  useEffect(() => {
    if (root === '') {
      setListings([]);
      setError(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const listed = await Promise.all(extList.map((one) => listEditorDir(root, one)));
        if (!cancelled) {
          setListings(listed);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          setListings([]);
          setError(String(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // `extKey` stands in for `extList`: a caller that passes an inline array literal must not re-list
    // on every render just because the reference changed.
  }, [root, extKey, reloadKey]);

  const tree = useMemo(() => {
    const trees = listings.map((one) => buildTree(one));
    const merged = trees.length <= 1 ? (trees[0] ?? []) : mergeTrees(trees);
    return prune ? withoutEmptyDirs(merged) : merged;
  }, [listings, prune]);
  const shown = useMemo(() => filterTree(tree, query), [tree, query]);

  // Directories are open by default (the library is shallow and a hidden model is a lost model);
  // `collapsed` is the human's exceptions. A search overrides them so a match three folders down is
  // visible without clicking, and revealing the selection expands its ancestors the same way.
  const searching = query.trim() !== '';
  const expandedAncestors = useMemo(
    () => (selected === null ? new Set<string>() : new Set(ancestorDirs(selected))),
    [selected],
  );
  const isOpen = useCallback(
    (relPath: string): boolean => searching || expandedAncestors.has(relPath) || !collapsed.has(relPath),
    [searching, expandedAncestors, collapsed],
  );
  const toggle = useCallback((relPath: string): void => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(relPath)) {
        next.delete(relPath);
      } else {
        next.add(relPath);
      }
      return next;
    });
  }, []);

  const modelCount = useMemo(
    () => listings.reduce((sum, one) => sum + one.filter((entry) => !entry.isDir).length, 0),
    [listings],
  );

  const rows: ReactNode[] = [];
  const walk = (nodes: readonly TreeNode[], depth: number): void => {
    for (const node of nodes) {
      const pad = 8 + depth * 14;
      if (node.isDir) {
        const open = isOpen(node.relPath);
        rows.push(
          <li key={node.relPath}>
            <button
              type="button"
              className="tree-row vx-dir"
              style={{ paddingLeft: pad }}
              onClick={() => toggle(node.relPath)}
              aria-expanded={open}
            >
              <span className="caret">{open ? '▾' : '▸'}</span>
              <span className="name">{node.name}</span>
            </button>
          </li>,
        );
        if (open) {
          walk(node.children, depth + 1);
        }
        continue;
      }
      const kind = showKindBadge ? matchingExt(node.name, extList) : null;
      rows.push(
        <li key={node.relPath}>
          <button
            type="button"
            className={node.relPath === selected ? 'tree-row selected' : 'tree-row'}
            style={{ paddingLeft: pad }}
            title={node.relPath}
            onClick={() => onSelect(node.relPath)}
            onPointerDown={onFilePointerDown === undefined ? undefined : (e) => onFilePointerDown(node.relPath, e)}
          >
            <span className="node-dot" />
            <span className="name">{displayName(node.name, extList)}</span>
            {kind !== null && <span className="sys-chip">{extBadge(kind)}</span>}
          </button>
        </li>,
      );
    }
  };
  walk(shown, 0);

  return (
    <section className="hierarchy vx-browser">
      <div className="panel-head">
        {head}
        <span className="count">
          <span className="num">{modelCount}</span> {counted}
        </span>
      </div>

      <div className="vx-search">
        <input
          type="search"
          value={query}
          placeholder="filter"
          aria-label={`Filter ${counted}`}
          spellCheck={false}
          onChange={(event) => setQuery(event.target.value)}
        />
      </div>

      {error !== null && <div className="vx-error">{error}</div>}

      <div className="tree-scroll">
        <ul className="tree">{rows}</ul>
        {rows.length === 0 && error === null && (
          <div className="tree-empty">{root === '' ? 'no project directory' : `no ${counted}`}</div>
        )}
      </div>
    </section>
  );
}
