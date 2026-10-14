#!/usr/bin/env node
// Baked-pair drift gate (sprite atlases and fonts).
//
// Discovery is source-driven: every committed `*.atlas-source.yaml` / `*.font-source.yaml` under
// `games/` and `examples/` is checked with `ludion generate-atlas --check <source>`, which
// regenerates the pair in memory and byte-compares BOTH the derived document and its `.png` against
// the committed files — a source whose output is missing fails there. The verb prints the paths it
// verified; the gate then pairs them both ways with the baked documents' `generated_from:` field
// (the provenance marker `Plan/sprite-engine.md` Wave 1 shares with `generate-map`): a document
// naming a source from anywhere but that source's own output path is a moved or orphaned copy, and a
// verified document that no longer names its source has lost its provenance. It fails loudly,
// listing the drifted sources, the moment one drifts.
//
// Cheap by construction: a file scan of `games/` and `examples/` only, not a whole-repo walk; each
// baked document is read only as far as its 8 KB head.
//
// Run standalone (`node scripts/atlas-drift.mjs`) or via the `atlas-drift` step in
// scripts/check.mjs; mirrored as the `atlas-drift` step in .github/workflows/ci.yml and
// ci/teamcity/provision.mjs — keep the three in step.

import { existsSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { REPO_ROOT, SCAN_ROOTS, emptyScan, headGeneratedFrom, pairOutputs, upToDatePaths, walkFiles } from './drift-lib.mjs';
import { runLudion } from './ludion-bin.mjs';

const isSource = (name) => name.endsWith('.atlas-source.yaml') || name.endsWith('.font-source.yaml');
const isBaked = (name) => name.endsWith('.atlas.yaml') || name.endsWith('.font.yaml');

export function runGate({ root = REPO_ROOT, scanRoots = SCAN_ROOTS, ludion = runLudion } = {}) {
  const rel = (p) => path.relative(root, p).replaceAll('\\', '/');
  const errors = [];
  const verified = new Map();
  const sources = walkFiles(root, scanRoots, isSource);

  if (sources.length === 0) {
    errors.push(emptyScan('atlas-drift', 'atlas or font sources (*.atlas-source.yaml, *.font-source.yaml)', scanRoots));
  }
  for (const source of sources) {
    const run = ludion(['generate-atlas', '--check', source], { cwd: root });
    if (run.status !== 0) {
      errors.push(`${rel(source)}: \`generate-atlas --check\` failed:\n${(run.stderr || run.stdout || '').trim()}`);
      continue;
    }
    const paths = upToDatePaths(run.stdout ?? '');
    if (paths.length === 0) {
      errors.push(`${rel(source)}: \`generate-atlas --check\` printed no verified path`);
      continue;
    }
    verified.set(source, paths.map((p) => path.resolve(root, p)));
  }

  const claims = [];
  for (const baked of walkFiles(root, scanRoots, isBaked)) {
    const from = headGeneratedFrom(baked);
    if (!from) continue;
    const source = path.resolve(path.dirname(baked), from);
    if (!existsSync(source)) {
      errors.push(`${rel(baked)}: names source \`${from}\`, which does not exist beside it`);
      continue;
    }
    claims.push({ output: baked, source });
  }
  errors.push(...pairOutputs({ root, verified, claims }));
  return { errors, checked: sources.length };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { errors, checked } = runGate();
  if (errors.length > 0) {
    console.error('ATLAS DRIFT: committed generated atlases no longer match their source.\n');
    for (const d of errors) console.error(`  ${d}`);
    console.error('\nRegenerate each with `ludion generate-atlas <source>` and commit the result.');
    process.exit(1);
  }
  console.log(`atlas-drift: ${checked} atlas/font source(s) checked, none drifted.`);
}
