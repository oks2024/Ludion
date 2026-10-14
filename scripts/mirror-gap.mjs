#!/usr/bin/env node
// mirror-gap — how far the git mirror lags Lore. Read-only: no git or Lore write (`lore status
// --scan` refreshes Lore's own dirty flags, nothing else).
//
// Reports, against git HEAD and the working tree:
//   missing   Lore-tracked public files absent from git
//   stale     Lore-clean files whose bytes differ from git HEAD (after git's own eol filters)
//   deleted   git-tracked files gone from disk and from Lore (deletions never mirrored)
//   private   git-tracked files under a private path (`.gitignore` minus `.loreignore`)
//   dirty     paths among missing/stale/deleted that Lore reports as changed — not catch-up
//             candidates until Lore commits them
//
// Usage:  node scripts/mirror-gap.mjs [--json] [--paths-file <file>]
//
//   --json              one JSON object: the five lists, their counts and `candidates`
//   --paths-file <file> also write the candidates (missing + stale + deleted minus dirty), one per
//                       line, for `node scripts/commit.mjs --mirror-only "<message>" --paths-file <file>`
//
// Plain output lists each category with its count, then the candidates. Zero npm dependencies.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { isPrivate, parseLoreDumpFiles, parseLoreStatus, privatePaths } from './mirror-lib.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const MAX_BUFFER = 1024 * 1024 * 1024;

let values;
try {
  ({ values } = parseArgs({
    options: {
      json: { type: 'boolean', default: false },
      'paths-file': { type: 'string' },
      help: { type: 'boolean', short: 'h', default: false },
    },
  }));
} catch (err) {
  console.error(`mirror-gap: ${err.message}`);
  process.exit(2);
}
if (values.help) {
  const help = [];
  for (const l of readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n')) {
    if (l.startsWith('#!')) continue;
    if (l.startsWith('//')) help.push(l.slice(3));
    else if (help.length) break;
  }
  console.log(help.join('\n'));
  process.exit(0);
}

// LUDION_COMMIT_LORE_SHIM names a `.mjs` that stands in for `lore`, the same seam commit.mjs has.
const LORE_SHIM = process.env.LUDION_COMMIT_LORE_SHIM;
const lore = (args) => (LORE_SHIM ? run(process.execPath, [LORE_SHIM, ...args]) : run('lore', args));

// `okCodes` are the exit codes that mean success: only `git check-ignore` uses 1 (no path ignored).
function run(cmd, args, input, okCodes = [0]) {
  const r = spawnSync(cmd, args, { cwd: REPO_ROOT, encoding: 'utf8', input, maxBuffer: MAX_BUFFER });
  if (r.error || !okCodes.includes(r.status)) {
    console.error(`mirror-gap: ${cmd} ${args.slice(0, 3).join(' ')} failed: ${r.error?.message ?? r.stderr}`);
    process.exit(2);
  }
  return r.stdout;
}

const readOptional = (rel) => (existsSync(path.join(REPO_ROOT, rel)) ? readFileSync(path.join(REPO_ROOT, rel), 'utf8') : '');
const priv = privatePaths(readOptional('.gitignore'), readOptional('.loreignore'));

const loreFiles = parseLoreDumpFiles(lore(['repository', 'dump', '-P']));
const loreSet = new Set(loreFiles);
const changes = parseLoreStatus(lore(['status', '--scan', '-P']));
const isDirty = (p) => changes.some((c) => c.path === p || (c.path.endsWith('/') && p.startsWith(c.path)));

const gitTree = new Map();
for (const rec of run('git', ['ls-tree', '-r', '-z', '--full-tree', 'HEAD']).split('\0')) {
  const tab = rec.indexOf('\t');
  if (tab >= 0) gitTree.set(rec.slice(tab + 1), rec.slice(0, tab).split(' ')[2]);
}

// Lore files git would refuse (build noise, ignore rules) are not part of the public set.
const candidates = loreFiles.filter((p) => !isPrivate(p, priv));
const ignored = new Set(run('git', ['check-ignore', '--no-index', '--stdin', '-z'], candidates.join('\0'), [0, 1]).split('\0').filter(Boolean));
const publicLore = candidates.filter((p) => !ignored.has(p));

const missing = publicLore.filter((p) => !gitTree.has(p));
const onBoth = publicLore.filter((p) => gitTree.has(p) && !isDirty(p) && existsSync(path.join(REPO_ROOT, p)));
const hashes = run('git', ['hash-object', '--stdin-paths'], onBoth.join('\n')).split('\n').filter(Boolean);
const stale = onBoth.filter((p, i) => hashes[i] !== gitTree.get(p));
const deleted = [...gitTree.keys()].filter((p) => !isPrivate(p, priv) && !loreSet.has(p) && !existsSync(path.join(REPO_ROOT, p)));
const privateTracked = [...gitTree.keys()].filter((p) => isPrivate(p, priv));

const all = [...missing, ...stale, ...deleted];
const dirty = all.filter(isDirty);
const clean = all.filter((p) => !isDirty(p)).sort();

if (values['paths-file'] !== undefined) writeFileSync(values['paths-file'], clean.map((p) => `${p}\n`).join(''));

if (values.json) {
  const lists = { missing, stale, deleted, private: privateTracked, dirty };
  const counts = Object.fromEntries(Object.entries(lists).map(([k, v]) => [k, v.length]));
  console.log(JSON.stringify({ counts, ...lists, candidates: clean }, null, 2));
} else {
  const show = (title, list) => {
    console.log(`${title}: ${list.length}`);
    for (const p of list) console.log(`  ${p}`);
  };
  show('missing (Lore-tracked, absent from git)', missing);
  show('stale (Lore-clean, differs from git HEAD)', stale);
  show('deleted (in git, gone from disk and Lore)', deleted);
  show('private (git-tracked under a private path)', privateTracked);
  show('Lore-dirty (not catch-up candidates)', dirty);
  console.log(`candidates: ${clean.length}`);
  for (const p of clean) console.log(p);
}
