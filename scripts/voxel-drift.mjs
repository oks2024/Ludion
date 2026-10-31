#!/usr/bin/env node
// Voxel-generation drift gate.
//
// Discovery is source-driven: every committed `*.voxel.gen.mjs` under `games/` and `examples/` is run
// with no argument (it prints its `kind: voxel-gen` JSON) and the output piped through
// `ludion voxel from-json --check` against the `<name>.voxel.yaml` beside it — the commit-output lean
// the map-authoring gate enforces (Plan/voxel-authoring-tools.md decision 6). A script with no model
// fails. A script whose JSON carries no `generated_from` and which no model claims is a documented
// one-shot scaffold and is skipped — but a run where every generator is skipped fails, since that
// byte-checked nothing (the usual cause is a model that lost its `generated_from`).
//
// The gate then pairs both ways with the models' top-level `generated_from:` key: a model naming a
// missing file, a file that is not a `*.voxel.gen.mjs`, or a script from anywhere but beside it is a
// moved or orphaned copy. A model with no such key is hand-authored (or was scaffolded once and
// sculpted since) and is ignored unless its script emits `generated_from`, which makes the lost key a
// drift.
//
// Cheap by construction: a file scan of `games/` and `examples/` only; node/cargo are invoked only
// once a generator is actually discovered.
//
// Run standalone (`node scripts/voxel-drift.mjs`) or via the `voxel-drift` step in
// scripts/check.mjs; mirrored as the `voxel-drift` step in .github/workflows/ci.yml — keep the
// three in step.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { REPO_ROOT, SCAN_ROOTS, emptyScan, walkFiles } from './drift-lib.mjs';
import { runLudion } from './ludion-bin.mjs';

// A generated model's JSON intermediate is the whole grid as text; the default 1 MB pipe buffer is
// short of a 64^3 model, so give the capture room the format itself can reach.
const MAX_BUFFER = 64 * 1024 * 1024;

// The value of a top-level `generated_from:` key, or null. Unlike a canonical scene, a voxel model
// carries comments and its `size:` sequence sits ABOVE this key — so scan the whole header rather
// than stopping at the first indented line, and rely on column 0 for the disambiguation (every
// nested value in the format is indented, every comment starts with `#`).
function generatedFrom(text) {
  for (const line of text.split('\n')) {
    const m = /^generated_from:\s*(.+?)\s*$/.exec(line);
    if (m) return m[1].replace(/^['"]|['"]$/g, '');
    if (line.startsWith('slices:')) break; // the grid begins; no top-level key can follow it
  }
  return null;
}

// `a.voxel.gen.mjs` owns `a.voxel.yaml`.
const modelOf = (source) => `${source.slice(0, -'.gen.mjs'.length)}.yaml`;

export function runGate({ root = REPO_ROOT, scanRoots = SCAN_ROOTS, ludion = runLudion } = {}) {
  const rel = (p) => path.relative(root, p).replaceAll('\\', '/');
  const errors = [];
  const sources = walkFiles(root, scanRoots, (name) => name.endsWith('.voxel.gen.mjs'));
  if (sources.length === 0) {
    errors.push(emptyScan('voxel-drift', 'voxel generators (*.voxel.gen.mjs)', scanRoots));
  }
  const claimOf = (model) => {
    const named = existsSync(model) ? generatedFrom(readFileSync(model, 'utf8')) : null;
    return named ? path.resolve(path.dirname(model), named) : null;
  };

  let checked = 0;
  let scaffolds = 0;
  for (const source of sources) {
    const model = modelOf(source);
    const generate = spawnSync(process.execPath, [source], {
      cwd: root, encoding: 'utf8', maxBuffer: MAX_BUFFER,
    });
    if (generate.status !== 0) {
      errors.push(`${rel(source)}: running it failed:\n${generate.stderr ?? ''}`);
      continue;
    }
    let emitsProvenance = false;
    try {
      emitsProvenance = Boolean(JSON.parse(generate.stdout).generated_from);
    } catch {
      errors.push(`${rel(source)}: printed no \`kind: voxel-gen\` JSON on stdout`);
      continue;
    }
    if (!emitsProvenance && claimOf(model) !== path.resolve(source)) {
      if (existsSync(model)) {
        errors.push(
          `${rel(model)} exists but neither it nor ${rel(source)} carries \`generated_from\` — `
          + 'add provenance to the generator, or `generated_from:` to the model naming it',
        );
      } else {
        scaffolds += 1;
      }
      continue;
    }
    checked += 1;
    if (!existsSync(model)) {
      errors.push(`${rel(source)} has no committed model ${rel(model)}`);
      continue;
    }
    const check = ludion(
      ['voxel', 'from-json', '--check', model],
      { cwd: root, input: generate.stdout, maxBuffer: MAX_BUFFER },
    );
    if (check.status !== 0) {
      errors.push(`${rel(model)}:\n${(check.stderr || check.stdout || '').trim()}`);
    }
  }

  if (scaffolds > 0 && checked === 0) {
    errors.push(
      `voxel-drift: every generator (${scaffolds}) was skipped as a scaffold, so no model was byte-checked — `
      + 'restore the lost generated_from on the models, or give the scripts provenance',
    );
  }

  for (const model of walkFiles(root, scanRoots, (name) => name.endsWith('.voxel.yaml'))) {
    const named = generatedFrom(readFileSync(model, 'utf8'));
    if (!named) continue;
    const resolved = path.resolve(path.dirname(model), named);
    if (!existsSync(resolved)) {
      errors.push(`${rel(model)}: names generator \`${named}\`, which does not exist beside it`);
    } else if (!resolved.endsWith('.voxel.gen.mjs')) {
      errors.push(`${rel(model)}: names \`${named}\`, which is not a *.voxel.gen.mjs generator`);
    } else if (modelOf(resolved) !== path.resolve(model)) {
      errors.push(
        `${rel(model)}: names \`${named}\`, which is not the model that script owns (it owns ${rel(modelOf(resolved))}): `
        + 'a moved or orphaned copy — delete it, or regenerate at the default path',
      );
    }
  }
  return { errors, checked, scaffolds };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { errors, checked, scaffolds } = runGate();
  if (errors.length > 0) {
    console.error('VOXEL DRIFT: committed generated models no longer match their generator.\n');
    for (const d of errors) console.error(`  ${d}`);
    console.error(
      '\nRegenerate each with `node <script> <model>.json` then `ludion voxel from-json --in '
      + '<model>.json --out <model>` and commit the result — never through a PowerShell pipe, which '
      + 're-encodes the bytes (`ludion explain L1135`). A generator that ignores the path argument '
      + 'predates `emit({ out })`; give it `out: process.argv[2]` first. The model file is '
      + 'write-only — the script is the source of truth.',
    );
    process.exit(1);
  }
  console.log(`voxel-drift: ${checked} generator(s) checked, ${scaffolds} scaffold(s) skipped, none drifted.`);
}
