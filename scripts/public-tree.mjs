#!/usr/bin/env node
// Public-tree lane — proves the git-visible tree builds and tests green on its own.
//
// A public clone has no `games/vigil`, no `Plan-Private/`, no `.exarch/`; a local checkout has all of
// them, so a green `check.mjs` there proves nothing about the tree a stranger gets. This script
// exports exactly that tree to `target/public-tree/tree/`, `git init`s and stages it (so gates that
// call git see the export, not this checkout), and runs the hosted gates in it: the `no-gpu`
// workspace nextest profile (`--locked`), the editor frontend (oxlint, tsc, vitest), and ci.yml's
// node gates (privacy-canary, size-gate, claude-md-budget, ecs-budget, lists-in-step,
// diagnostic-prints, atomic-writes, schedule-comments, the gate-script, commit-wrapper, voxel-grid
// and physics-pin-ritual node tests; physics-features, mobile-check and the schema-drift test ride
// the full lane because they call cargo or need editor/protocol's npm install). The drift gates,
// schema-drift itself, clippy, doctests and packaging are not run here.
//
// The export is `ls-files -co` (tracked + untracked-not-ignored), so another instance's uncommitted
// files are exported too.
//
// The exported set is the same one scripts/size-gate.mjs and scripts/privacy-canary.mjs scan: `git
// ls-files -co --exclude-standard -z` minus `git check-ignore --no-index --stdin -z` (tracked-but-
// private paths, such as vigil while the mirror still tracks it) minus paths missing from disk.
// The export is incremental: a file is copied only when its size or mtime differs, and its mtime is
// carried over so cargo's fingerprints keep the workspace crates fresh between runs; a manifest
// (`target/public-tree/files.txt`) lets the next run delete what stopped being publishable, and
// nothing else in the export (its `node_modules/`) is touched. The lane builds in its own
// `target/public-tree/target`, non-incremental — sharing the root `target/` would overwrite the
// uplifted test binaries the real lanes use, and incremental dirs are the bulk of a full disk.
//
// Hosted CI has no such lane: it checks out the public repository, which already is this tree.
//
// Usage:  node scripts/public-tree.mjs [--export-only | --node-only]
//
//   --export-only   export the tree and stop (no cargo, no npm)
//   --node-only     export, git-init, and run only the cargo-free node gates (fast; no 25 GB build)
//
// Exit 0 clean, 1 when a sub-step failed, 2 on an infrastructure error. Zero npm dependencies — Node
// built-ins only. Runs unchanged on Windows and macOS.

import { spawnSync } from 'node:child_process';
import {
  existsSync, readFileSync, rmdirSync, statSync, unlinkSync, utimesSync,
} from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { writeFileAtomic } from './atomic-write.mjs';
import { spawnTool } from './kill-tree.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const LANE_DIR = path.join(REPO_ROOT, 'target', 'public-tree');

function git(root, args, input) {
  const r = spawnSync('git', args, { cwd: root, input, maxBuffer: 64 * 1024 * 1024 });
  if (r.error) throw new Error(`failed to spawn git: ${r.error.message}`);
  return r;
}

// Tracked plus untracked-not-ignored, minus tracked-but-ignored, minus missing from disk — the set
// a mirror commit could publish (same recipe as size-gate.mjs's publicFiles()).
export function publicFiles(root) {
  const ls = git(root, ['ls-files', '-co', '--exclude-standard', '-z']);
  if (ls.status !== 0) throw new Error(`git ls-files failed:\n${ls.stderr}`);
  const candidates = ls.stdout.toString('utf8').split('\0').filter(Boolean);
  const ci = git(root, ['check-ignore', '--no-index', '--stdin', '-z'], ls.stdout);
  if (ci.status !== 0 && ci.status !== 1) throw new Error(`git check-ignore failed:\n${ci.stderr}`);
  const ignored = new Set(ci.stdout.toString('utf8').split('\0').filter(Boolean));
  return candidates.filter((f) => {
    if (ignored.has(f)) return false;
    try { return statSync(path.join(root, f)).isFile(); } catch { return false; }
  });
}

// Incremental copy of the public set from `root` into `dest`; `manifest` lists what the previous
// export wrote. Returns { copied, unchanged, removed }.
export function exportTree(root, dest, manifest) {
  const fence = path.resolve(root, 'target') + path.sep;
  if (!path.resolve(dest).startsWith(fence)) throw new Error(`export root ${dest} is outside ${fence}`);
  const files = publicFiles(root);
  const wanted = new Set(files);
  let copied = 0;
  let unchanged = 0;
  for (const f of files) {
    const src = path.join(root, f);
    const dst = path.join(dest, f);
    const s = statSync(src);
    let same = false;
    try {
      const d = statSync(dst);
      same = d.size === s.size && Math.abs(d.mtimeMs - s.mtimeMs) < 2; // utimes Dates carry whole ms
    } catch { /* not exported yet */ }
    if (same) { unchanged++; continue; }
    writeFileAtomic(dst, readFileSync(src));
    utimesSync(dst, s.atime, s.mtime);
    copied++;
  }
  let removed = 0;
  if (existsSync(manifest)) {
    for (const f of readFileSync(manifest, 'utf8').split('\n').filter(Boolean)) {
      if (wanted.has(f)) continue;
      const dst = path.join(dest, f);
      if (!path.resolve(dst).startsWith(path.resolve(dest) + path.sep)) continue;
      try { unlinkSync(dst); removed++; } catch { continue; }
      for (let dir = path.dirname(dst); dir !== path.resolve(dest); dir = path.dirname(dir)) {
        try { rmdirSync(dir); } catch { break; } // stops at the first non-empty directory
      }
    }
  }
  writeFileAtomic(manifest, files.join('\n') + '\n');
  return { copied, unchanged, removed };
}

// A git repository over the export (no commit needed): `ls-files` and friends then answer for the
// export. Re-staged on every run so deletions and edits are seen.
export function initGit(dest) {
  if (!existsSync(path.join(dest, '.git'))) {
    const init = git(dest, ['init', '-q']);
    if (init.status !== 0) throw new Error(`git init failed:
${init.stderr}`);
  }
  const add = git(dest, ['add', '-A', '--force']);
  if (add.status !== 0) throw new Error(`git add failed:
${add.stderr}`);
}

const NODE_TESTS = [
  ['scripts/voxel/grid.test.mjs'],
  ['scripts/commit.test.mjs', 'scripts/mirror-lib.test.mjs', 'scripts/public-tree.test.mjs'],
  ['scripts/physics-pin-ritual.test.mjs'],
  ['ci/scripts/gate-scripts.test.mjs'],
];

const NODE_GATES = [
  ['privacy-canary', 'scripts/privacy-canary.mjs'],
  ['size-gate', 'scripts/size-gate.mjs'],
  ['claude-md-budget', 'scripts/claude-md-budget.mjs'],
  ['ecs-budget', 'scripts/ecs-budget.mjs'],
  ['lists-in-step', 'scripts/lists-in-step.mjs'],
  ['diagnostic-prints', 'ci/scripts/diagnostic-prints.mjs'],
  ['atomic-writes', 'ci/scripts/atomic-writes.mjs'],
  ['schedule-comments', 'ci/scripts/schedule-comments.mjs'],
];

const CARGO_NODE_GATES = [
  ['physics-features', 'scripts/physics-features.mjs'],
  ['mobile-check', 'scripts/mobile-check.mjs'],
];

function run(label, cmd, args, cwd, env) {
  console.log(`\npublic-tree: ${label}: ${cmd} ${args.join(' ')}`);
  return new Promise((resolve) => {
    const child = spawnTool(cmd, args, { cwd, env: { ...process.env, ...env }, stdio: 'inherit' });
    child.on('error', (err) => { console.error(`public-tree: ${label}: ${err.message}`); resolve(false); });
    child.on('exit', (code) => resolve(code === 0));
  });
}

async function main() {
  let values;
  try {
    ({ values } = parseArgs({ options: { 'export-only': { type: 'boolean', default: false }, 'node-only': { type: 'boolean', default: false } } }));
  } catch (err) {
    console.error(`public-tree: ${err.message}`);
    process.exit(2);
  }
  const tree = path.join(LANE_DIR, 'tree');
  let stats;
  try {
    stats = exportTree(REPO_ROOT, tree, path.join(LANE_DIR, 'files.txt'));
  } catch (err) {
    console.error(`public-tree: ${err.message}`);
    process.exit(2);
  }
  console.log(`public-tree: exported to ${tree}: ${stats.copied} copied, ${stats.unchanged} unchanged, ${stats.removed} removed`);
  if (values['export-only']) return;

  try {
    initGit(tree);
  } catch (err) {
    console.error(`public-tree: ${err.message}`);
    process.exit(2);
  }
  const app = path.join(tree, 'editor', 'app');
  const results = [];
  const step = async (label, cmd, args, cwd, env) => results.push([label, await run(label, cmd, args, cwd, env)]);
  const cargoEnv = { CARGO_TARGET_DIR: path.join(LANE_DIR, 'target'), CARGO_INCREMENTAL: '0', LUDION_FORBID_GPU: '1' };
  const nodeGates = async () => {
    for (const [label, script] of NODE_GATES) await step(label, 'node', [script], tree);
    for (const files of NODE_TESTS) await step(`node --test ${files[0]}`, 'node', ['--test', ...files], tree);
  };
  if (values['node-only']) {
    await nodeGates();
    finish(results);
  }
  await step('nextest', 'cargo', ['nextest', 'run', '--workspace', '--profile', 'no-gpu', '--locked', '--no-fail-fast'], tree, {
    ...cargoEnv,
  });
  if (await run('npm ci', 'npm', ['ci'], app)) {
    await step('oxlint', 'npx', ['oxlint'], app);
    await step('tsc', 'npx', ['tsc', '-b'], app);
    await step('vitest', 'npx', ['vitest', 'run'], app);
  } else {
    results.push(['npm ci', false]);
  }
  await nodeGates();
  for (const [label, script] of CARGO_NODE_GATES) await step(label, 'node', [script], tree, cargoEnv);
  await step('node --test ci/scripts/schema-drift.test.mjs', 'node', ['--test', 'ci/scripts/schema-drift.test.mjs'], tree, cargoEnv);
  finish(results);
}

function finish(results) {
  console.log('');
  for (const [label, ok] of results) console.log(`public-tree: ${label} ${ok ? 'ok' : 'FAILED'}`);
  process.exit(results.every(([, ok]) => ok) ? 0 : 1);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
