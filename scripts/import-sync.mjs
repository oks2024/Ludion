#!/usr/bin/env node
// Derived-import sync gate.
//
// Every project whose `ludion.toml` carries an `[[import]]` table (`games/*`, `examples/*`) has a
// derived tree — glTF sources compiled by `ludion import` into an ignored directory the game and
// its tests read. This runs `ludion sync` on each so the tree is fresh before anything consumes
// it; an unchanged tree is a cheap skip (sources and the importer are fingerprinted).
//
// Run standalone (`node scripts/import-sync.mjs`) or via the `import-sync` step in
// scripts/check.mjs (before `test`); mirrored as the `derived imports sync` step in
// .github/workflows/ci.yml and the `import-sync` step in ci/teamcity/provision.mjs — keep the
// three in step.

import { readdirSync, readFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runLudion } from './ludion-bin.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SCAN_ROOTS = ['games', 'examples'];

// Project directories (relative to the repo root) whose manifest declares an `[[import]]` table.
function projects() {
  const out = [];
  for (const root of SCAN_ROOTS) {
    const abs = path.join(REPO_ROOT, root);
    if (!existsSync(abs)) continue;
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      const manifest = path.join(abs, entry.name, 'ludion.toml');
      if (entry.isDirectory() && existsSync(manifest) && /^\[\[import\]\]/m.test(readFileSync(manifest, 'utf8'))) {
        out.push(`${root}/${entry.name}`);
      }
    }
  }
  return out;
}

const failed = [];
for (const dir of projects()) {
  const result = runLudion(['sync', '--project', dir], { cwd: REPO_ROOT });
  process.stdout.write(`${dir}:\n${result.stdout ?? ''}`);
  if (result.stderr) process.stderr.write(result.stderr);
  if (result.status !== 0) failed.push(dir);
}
if (failed.length) {
  console.error(`\nimport-sync: FAILED for ${failed.join(', ')}.`);
  console.error('Run `ludion sync --project <dir>`; the diagnostics above name the source.');
  process.exit(1);
}
