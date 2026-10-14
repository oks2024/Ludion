#!/usr/bin/env node
// Mesh-edit drift gate.
//
// A `kind: mesh-edit` document derives a whole directory: `mesh.mesh.yaml` plus one
// content-hash-named PNG per map its ops touched. That tree is write-only — regenerated from the
// edit document, the source mesh and the source's committed maps, never hand-edited — so the only
// thing keeping it honest is a byte-compare.
//
// Discovery is source-driven: every committed `*.mesh-edit.yaml` under `games/` and `examples/` is
// checked with `ludion mesh apply --check --json`, which re-derives in memory and byte-compares EVERY
// file of the tree itself (the document, each map, and any file in the directory the derive does not
// name); a missing tree fails there. The verb reports the `out_dir` it verified; the gate then pairs
// `<out_dir>/mesh.mesh.yaml` both ways with the derived meshes' `generated_from:` field: a mesh
// naming an edit from anywhere but that edit's own output directory is a moved or orphaned copy, and
// the verified mesh must name its edit. It fails loudly, listing the drifted edits, the moment one
// drifts.
//
// Cheap by construction: a file scan of `games/` and `examples/` only, not a whole-repo walk; a
// derived mesh is megabytes, so each candidate is read only as far as its 8 KB head.
//
// Run standalone (`node scripts/mesh-edit-drift.mjs`) or via the `mesh-edit-drift` step in
// scripts/check.mjs; mirrored as the `mesh-edit-drift` step in .github/workflows/ci.yml and
// ci/teamcity/provision.mjs — keep the three in step.

import { existsSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { REPO_ROOT, SCAN_ROOTS, emptyScan, headGeneratedFrom, pairOutputs, walkFiles } from './drift-lib.mjs';
import { runLudion } from './ludion-bin.mjs';

const parseJson = (text) => {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
};

export function runGate({ root = REPO_ROOT, scanRoots = SCAN_ROOTS, ludion = runLudion } = {}) {
  const rel = (p) => path.relative(root, p).replaceAll('\\', '/');
  const errors = [];
  const verified = new Map();
  const edits = walkFiles(root, scanRoots, (name) => name.endsWith('.mesh-edit.yaml'));

  if (edits.length === 0) {
    errors.push(emptyScan('mesh-edit-drift', 'mesh-edit documents (*.mesh-edit.yaml)', scanRoots));
  }
  for (const edit of edits) {
    const run = ludion(['mesh', 'apply', edit, '--check', '--json'], { cwd: root });
    const report = parseJson(run.stdout ?? '');
    if (run.status !== 0) {
      const drift = Array.isArray(report?.drift) && report.drift.length > 0 ? report.drift.join('\n') : null;
      errors.push(`${rel(edit)}: \`mesh apply --check\` failed:\n${drift ?? (run.stderr || run.stdout || '').trim()}`);
      continue;
    }
    if (typeof report?.out_dir !== 'string') {
      errors.push(`${rel(edit)}: \`mesh apply --check\` printed no verified path`);
      continue;
    }
    verified.set(edit, [path.join(path.resolve(root, report.out_dir), 'mesh.mesh.yaml')]);
  }

  const claims = [];
  for (const mesh of walkFiles(root, scanRoots, (name) => name.endsWith('.mesh.yaml'))) {
    const from = headGeneratedFrom(mesh);
    // A mesh derived from something else (a `kind: map` compile, say) is not this gate's business.
    if (!from || !from.endsWith('.mesh-edit.yaml')) continue;
    const source = path.resolve(path.dirname(mesh), from);
    if (!existsSync(source)) {
      errors.push(`${rel(mesh)}: names edit document \`${from}\`, which does not exist`);
      continue;
    }
    claims.push({ output: mesh, source });
  }
  errors.push(...pairOutputs({ root, verified, claims }));
  return { errors, checked: edits.length };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { errors, checked } = runGate();
  if (errors.length > 0) {
    console.error('MESH-EDIT DRIFT: committed derived meshes no longer match their edit document.\n');
    for (const d of errors) console.error(`  ${d}`);
    console.error('\nRegenerate each with `ludion mesh apply <edit.mesh-edit.yaml>` and commit the result.');
    process.exit(1);
  }
  console.log(`mesh-edit-drift: ${checked} mesh-edit document(s) checked, none drifted.`);
}
