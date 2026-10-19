// Target-dir guard for a persistent CI agent. Cargo never deletes a stale artifact: every change
// to a dependency, feature set, flag or toolchain builds a fresh hashed copy beside the old one, so
// an agent that builds every revision grows its target trees until the disk fills (one agent
// reached 250 GB on a 300 GB disk, and the sync interrupted by the full disk left truncated files
// that failed every later build). Deliberately absent from scripts/check.mjs and
// .github/workflows/ci.yml: a local loop and an ephemeral runner have no long-lived tree to police.
//
//   node ci/scripts/target-guard.mjs           every build: print each tree's size and the disk's
//                                              free space; past a limit, delete the largest trees
//                                              until back under it. Never fails the build.
//   node ci/scripts/target-guard.mjs --sweep   nightly: `cargo sweep` every tree down to the
//                                              artifacts used in the last LUDION_SWEEP_DAYS days.
//
// Limits (GB): LUDION_TARGET_GUARD_MAX_GB (all trees together, default 100) and
// LUDION_TARGET_GUARD_MIN_FREE_GB (default 40). Run from the repository root.

import { existsSync, lstatSync, readdirSync, rmSync, statfsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { capture, run } from './gate-lib.mjs';

const GB = 1024 ** 3;

// Every cargo target dir the CI configurations write to. `target/` is split by its top-level
// entries so a clean can drop the bloated profile (or the package-verify tree) and keep the rest.
const NESTED_ROOTS = ['target'];
const WHOLE_TREES = ['fuzz/target', 'editor/app/src-tauri/target'];

// The project directory and target dir each `cargo sweep` runs against.
const SWEEP_PROJECTS = [
  { project: '.', targetDir: 'target' },
  { project: '.', targetDir: 'target/package-verify' },
  { project: 'fuzz', targetDir: 'fuzz/target' },
  { project: 'editor/app/src-tauri', targetDir: 'editor/app/src-tauri/target' },
];

function dirBytes(dir) {
  let total = 0;
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return 0;
  }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) total += dirBytes(full);
    else if (entry.isFile()) {
      try {
        total += lstatSize(full);
      } catch {
        // A file removed mid-walk counts as nothing.
      }
    }
  }
  return total;
}

// lstat, not stat: a symlink in a target tree points somewhere this guard does not own.
const lstatSize = (file) => lstatSync(file).size;

/** Every tree this guard owns that exists under `root`, with its size in bytes. */
export function measure(root = '.') {
  const trees = [];
  for (const nested of NESTED_ROOTS) {
    const dir = path.join(root, nested);
    if (!existsSync(dir)) continue;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const rel = `${nested}/${entry.name}`;
      const full = path.join(root, rel);
      trees.push({ path: rel, bytes: entry.isDirectory() ? dirBytes(full) : lstatSize(full) });
    }
  }
  for (const rel of WHOLE_TREES) {
    const full = path.join(root, rel);
    if (existsSync(full)) trees.push({ path: rel, bytes: dirBytes(full) });
  }
  return trees;
}

/**
 * The trees to delete, largest first, until the total is at most `maxBytes` and the free space at
 * least `minFreeBytes` — or every tree, if even that is not enough.
 */
export function plan(trees, freeBytes, maxBytes, minFreeBytes) {
  let total = trees.reduce((sum, t) => sum + t.bytes, 0);
  let free = freeBytes;
  const doomed = [];
  for (const tree of [...trees].sort((a, b) => b.bytes - a.bytes)) {
    if (total <= maxBytes && free >= minFreeBytes) break;
    doomed.push(tree);
    total -= tree.bytes;
    free += tree.bytes;
  }
  return doomed;
}

const gb = (bytes) => `${(bytes / GB).toFixed(1)} GB`;

function freeBytes(root) {
  const fs = statfsSync(root);
  return fs.bavail * fs.bsize;
}

function report(trees, free) {
  const total = trees.reduce((sum, t) => sum + t.bytes, 0);
  console.log(`target guard: ${gb(total)} in target trees, ${gb(free)} free on the disk`);
  for (const tree of [...trees].sort((a, b) => b.bytes - a.bytes)) {
    if (tree.bytes >= 0.1 * GB) console.log(`  ${gb(tree.bytes).padStart(9)}  ${tree.path}`);
  }
}

function guard() {
  const maxBytes = Number(process.env.LUDION_TARGET_GUARD_MAX_GB ?? 100) * GB;
  const minFreeBytes = Number(process.env.LUDION_TARGET_GUARD_MIN_FREE_GB ?? 40) * GB;
  const trees = measure();
  const free = freeBytes('.');
  report(trees, free);
  const doomed = plan(trees, free, maxBytes, minFreeBytes);
  if (doomed.length === 0) return;
  console.log(
    `target guard: past the limit (${gb(maxBytes)} of trees, ${gb(minFreeBytes)} free) — deleting the largest trees`
  );
  for (const tree of doomed) {
    try {
      rmSync(tree.path, { recursive: true, force: true, maxRetries: 3 });
      console.log(`target guard: deleted ${tree.path} (${gb(tree.bytes)})`);
    } catch (err) {
      console.log(`target guard: could not delete ${tree.path}: ${err.message}`);
    }
  }
  console.log(`target guard: ${gb(freeBytes('.'))} free after cleaning`);
}

// Whether NTFS records last-access times. `cargo sweep` ages artifacts by access time; where the
// volume does not record it, every artifact looks as old as its creation and a sweep would delete
// warm ones. `fsutil` values: 0 user-managed enabled, 1 user-managed disabled, 2 system-managed
// enabled, 3 system-managed disabled.
async function windowsRecordsAccessTime() {
  const { output } = await capture('fsutil', ['behavior', 'query', 'disablelastaccess']);
  const value = Number(/DisableLastAccess\s*=\s*(\d+)/i.exec(output)?.[1]);
  return value === 0 || value === 2;
}

async function sweep() {
  const days = process.env.LUDION_SWEEP_DAYS ?? '7';
  report(measure(), freeBytes('.'));
  if ((await capture('cargo', ['sweep', '--version'])).code !== 0) {
    console.log('target sweep: installing cargo-sweep');
    if ((await run('cargo', ['install', 'cargo-sweep', '--locked'])) !== 0) return 1;
  }
  const byAge = process.platform !== 'win32' || (await windowsRecordsAccessTime());
  if (!byAge) {
    console.log(
      'target sweep: this volume does not record last-access times, so the age sweep is skipped ' +
        '(it would delete warm artifacts). Enable it, as an administrator: ' +
        '`fsutil behavior set disablelastaccess 0`. The size guard still bounds the trees.'
    );
  }
  let failed = 0;
  for (const { project, targetDir } of SWEEP_PROJECTS) {
    if (!existsSync(targetDir)) continue;
    const env = { CARGO_TARGET_DIR: path.resolve(targetDir) };
    const modes = [['--installed'], ...(byAge ? [['--time', days]] : [])];
    for (const mode of modes) {
      console.log(`target sweep: ${targetDir}: cargo sweep ${mode.join(' ')}`);
      if ((await run('cargo', ['sweep', ...mode, project], { env })) !== 0) failed += 1;
    }
  }
  report(measure(), freeBytes('.'));
  return failed === 0 ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.includes('--sweep')) process.exitCode = await sweep();
  else guard();
}
