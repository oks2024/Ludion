// Pure helpers for commit.mjs's shared-checkout safety: which staged entries a commit owns, which
// are foreign, and how many files `lore stage` reported. No I/O, so scripts/commit.test.mjs drives
// them with fixture strings.

import { isPrivate, overlaps } from './mirror-lib.mjs';

// `.`, `./` and the empty string all name the whole tree: `''`.
const norm = (p) => p.replaceAll('\\', '/').replace(/^\.(?:\/|$)/, '').replace(/\/+$/, '');

// The `Changes staged for commit:` block of `lore status`: `A path ` lines (a trailing space; a
// directory ends in `/`), up to the first line that is not one. A move is `V old -> new` and parses
// to `{ kind, path: new, from: old }` (` -> ` cannot occur in a Windows file name).
export function parseStagedSet(statusText) {
  const staged = [];
  let inBlock = false;
  for (const line of statusText.split(/\r?\n/)) {
    if (!inBlock) {
      inBlock = /^Changes staged for commit:\s*$/.test(line);
      continue;
    }
    const m = line.trimEnd().match(/^([A-Z]) (.+?)(?: -> (.+))?$/);
    if (!m) break;
    staged.push(m[3] === undefined ? { kind: m[1], path: m[2] } : { kind: m[1], path: m[3], from: m[2] });
  }
  return staged;
}

// A file is owned when it is a named path or lies under one; `.` names everything.
const fileOwned = (p, named) => {
  const a = norm(p);
  return named.some((q) => {
    const b = norm(q);
    return b === '' || a === b || a.startsWith(`${b}/`);
  });
};

// A staged directory entry is owned when it is a named path, lies under one, or is an ancestor of
// one (staging `n/q.txt` stages the new directory `n/` with it). A move is owned only when both of
// its ends are.
const endOwned = (p, named) => (p.endsWith('/') ? overlaps(p, named) : fileOwned(p, named));
const entryOwned = (e, named) => endOwned(e.path, named) && (e.from === undefined || endOwned(e.from, named));

// Staged entries this commit did not make. `named === null` is `--all`, which owns everything.
export function foreignStaged(staged, named) {
  return named === null ? [] : staged.filter((e) => !entryOwned(e, named));
}

// What to `lore unstage` to undo only this commit's staging: the named paths plus every staged
// directory entry that is an ancestor of one (unstaging the file alone leaves `A n/` behind).
export function ownUnstageTargets(staged, named) {
  if (named === null) return ['.'];
  const dirs = staged
    .filter((e) => e.path.endsWith('/'))
    .map((e) => norm(e.path))
    .filter((d) => named.some((q) => norm(q).startsWith(`${d}/`)));
  return [...new Set([...named, ...dirs])];
}

// How many files `lore stage` staged, or null for output this does not recognise — so the
// bulk-staging guard fails closed rather than reading an unknown shape as zero.
export function parseStagedCount(stageOutput) {
  if (/^No changes staged/m.test(stageOutput)) return 0;
  const m = stageOutput.match(/^Staging (?:\d+ directories \([^)]*\),\s+)?(\d+) files/m);
  return m ? Number(m[1]) : null;
}

// Git index paths a mirror commit did not stage itself: not named, not private.
export function foreignIndexPaths(indexPaths, named, privatePathList) {
  return named === null ? [] : indexPaths.filter((p) => !fileOwned(p, named) && !isPrivate(p, privatePathList));
}
