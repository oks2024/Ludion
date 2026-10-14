// Shared pieces of the source-driven drift gates (atlas-drift, mesh-edit-drift, map-drift).
//
// A gate enumerates SOURCES, runs each source's `--check` verb, collects the output paths that verb
// printed, and pairs both ways with the outputs' `generated_from:` claims (`pairOutputs`).

import { closeSync, existsSync, openSync, readdirSync, readSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const SCAN_ROOTS = ['games', 'examples'];

const HEAD_BYTES = 8192;

// Sorted absolute paths of every file under `root/<scanRoot>` whose name passes `accept`.
export function walkFiles(root, scanRoots, accept) {
  const out = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(p);
      else if (accept(entry.name)) out.push(p);
    }
  };
  for (const scanRoot of scanRoots) {
    const abs = path.join(root, scanRoot);
    if (existsSync(abs)) walk(abs);
  }
  return out.sort();
}

function headText(file) {
  const buf = Buffer.alloc(HEAD_BYTES);
  const fd = openSync(file, 'r');
  let n;
  try {
    n = readSync(fd, buf, 0, HEAD_BYTES, 0);
  } finally {
    closeSync(fd);
  }
  return buf.toString('utf8', 0, n);
}

// A Node generator the map-drift gate runs with `--check` carries this line near its top.
export const DRIFT_GENERATOR_MARKER = '// ludion: drift-generator';

export function isDriftGenerator(file) {
  return headText(file)
    .split('\n')
    .some((line) => line.trim() === DRIFT_GENERATOR_MARKER);
}

// The message of a gate that enumerated no sources: an empty scan is a misconfigured gate, never a pass.
export const emptyScan = (gate, what, scanRoots) =>
  `${gate}: found no ${what} under ${scanRoots.map((r) => `${r}/`).join(' or ')} — a gate that checks nothing proves nothing; ` +
  'check SCAN_ROOTS and the file suffixes in scripts/drift-lib.mjs';

// The `generated_from:` value among a document's leading column-0 keys, or null. Only the first
// 8 KB are read — derived meshes are megabytes — and the scan stops at the first indented line.
export function headGeneratedFrom(file) {
  for (const line of headText(file).split('\n')) {
    const m = /^generated_from:\s*(.+?)\s*$/.exec(line);
    if (m) return m[1].replace(/^['"]|['"]$/g, '');
    if (/^\s/.test(line) && line.trim() !== '') break;
  }
  return null;
}

// The comma-separated paths after the last `up to date -> ` in a `--check` success line.
export function upToDatePaths(stdout) {
  const marker = 'up to date -> ';
  const at = stdout.lastIndexOf(marker);
  if (at < 0) return [];
  return stdout
    .slice(at + marker.length)
    .split('\n')[0]
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
}

// `verified`: Map<absSource, absOutput[]> of sources whose check passed. `claims`: [{ output,
// source }] (absolute; `source` is `generated_from` resolved against the output's directory).
// Claims whose source is not a verified key are the caller's business (missing or failed source).
export function pairOutputs({ root, verified, claims }) {
  const rel = (p) => path.relative(root, p).replaceAll('\\', '/');
  const errors = [];
  const byResolved = new Map([...verified].map(([src, outs]) => [path.resolve(src), outs.map((o) => path.resolve(o))]));
  for (const { output, source } of claims) {
    const outs = byResolved.get(path.resolve(source));
    if (outs && !outs.includes(path.resolve(output))) {
      errors.push(
        `${rel(output)} names ${rel(source)} under generated_from but is not the output that source writes ` +
          `(it writes ${outs.map(rel).join(', ')}): a moved or orphaned copy — delete it, or regenerate at the default path`,
      );
    }
  }
  for (const [src, outs] of byResolved) {
    for (const out of outs) {
      if (!out.endsWith('.yaml') || !existsSync(out)) continue;
      const named = headGeneratedFrom(out);
      if (named === null || path.resolve(path.dirname(out), named) !== src) {
        errors.push(`${rel(out)} is the output of ${rel(src)} but does not name it under generated_from — regenerate it with that source, or restore the \`generated_from:\` line`);
      }
    }
  }
  return errors;
}
