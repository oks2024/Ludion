#!/usr/bin/env node
// Vigil texture-set drift gate.
//
// A `kind: texture` recipe derives a map set next to it (`<name>/`): the PBR PNGs and a
// `generated.json` sidecar. The set is write-only, so the only thing keeping it honest is
// `ludion texture check`, which rebuilds the recipe and byte-compares every file (`L1286`) and runs
// the PBR validators. Every committed `*.texture.yaml` under games/vigil is checked, found at run
// time; finding none fails, since a gate that checks nothing proves nothing.
//
// Vigil is Lore-only, so this runs in scripts/check.mjs and TeamCity only, never in a GitHub workflow.

import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { REPO_ROOT, walkFiles } from './drift-lib.mjs';
import { runLudion } from './ludion-bin.mjs';

export function runGate({ root = REPO_ROOT, scanRoots = ['games/vigil'], ludion = runLudion } = {}) {
  const rel = (p) => path.relative(root, p).replaceAll('\\', '/');
  const errors = [];
  const recipes = walkFiles(root, scanRoots, (name) => name.endsWith('.texture.yaml'));
  if (recipes.length === 0) {
    errors.push(`vigil-texture-drift: found no texture recipes (*.texture.yaml) under ${scanRoots.map((r) => `${r}/`).join(' or ')} — a gate that checks nothing proves nothing`);
  }
  for (const recipe of recipes) {
    const run = ludion(['texture', 'check', recipe], { cwd: root });
    if (run.status !== 0) {
      errors.push(`${rel(recipe)}: \`texture check\` failed:\n${(run.stderr || run.stdout || '').trim()}`);
    }
  }
  return { errors, checked: recipes.map(rel) };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { errors, checked } = runGate();
  for (const r of checked) console.log(`checked ${r}`);
  if (errors.length > 0) {
    console.error('VIGIL TEXTURE DRIFT: a committed texture set no longer matches its recipe.\n');
    for (const d of errors) console.error(`  ${d}`);
    console.error('\nRebuild each with `ludion texture build <recipe.texture.yaml>` and commit the result.');
    process.exit(1);
  }
  console.log(`vigil-texture-drift: ${checked.length} texture recipe(s) checked, none drifted.`);
}
