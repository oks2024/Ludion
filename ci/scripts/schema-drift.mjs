// Schema-drift gate — the one copy, run by scripts/check.mjs's `schema-drift` step (with --fast),
// the `schema-drift` job in .github/workflows/ci.yml and the private TeamCity configs. VCS-agnostic:
// it never calls git or lore, and it never writes a tracked file.
//
// Layers, any of which failing means a generated file drifted without being re-committed:
//   1. `cargo test -p ludion-protocol` — the committed protocol schema snapshot (and its schema_hash)
//      must match what the protocol code generates right now.
//   2. regenerate each TS file from its committed snapshot into a scratch directory.
//   3. byte-compare every regenerated file against the committed one.
// The pattern schema snapshot's Rust half (`cargo test -p ludion-engine --test
// pattern_schema_snapshot`) runs in the workspace test lane, not here: this job never builds the engine.
//
// `--fast` skips layers 1 and `npm ci`: check.mjs's test lane already runs the snapshot test and its
// npm-deps verdict covers the install.
//
// Run from the repository root. Requires cargo (rust-toolchain.toml pins the channel) and node 22+.

import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { run } from './gate-lib.mjs';

const checks = [
  { committed: 'editor/protocol/src/protocol.d.ts', generator: 'editor/protocol/scripts/generate.js', what: 'the protocol schema snapshot' },
  {
    committed: 'editor/app/src/editor/effects/patternSchema.ts',
    generator: 'editor/protocol/scripts/generate-pattern-schema.js',
    what: 'the pattern schema snapshot',
  },
  {
    committed: 'editor/app/src/protocol/schemaHash.ts',
    generator: 'editor/protocol/scripts/generate-schema-hash.js',
    what: 'the protocol schema snapshot',
  },
];
if (!existsSync(checks[0].committed)) {
  console.error(`not at the repository root: ${checks[0].committed} not found`);
  process.exit(1);
}

const fast = process.argv.includes('--fast');
if (!fast) {
  if ((await run('cargo', ['test', '-p', 'ludion-protocol'])) !== 0) process.exit(1);
  if ((await run('npm', ['ci'], { cwd: 'editor/protocol' })) !== 0) process.exit(1);
}

const tmp = mkdtempSync(path.join(os.tmpdir(), 'ludion-schema-drift-'));
let exitCode = 0;
try {
  let stale = 0;
  for (const { committed, generator, what } of checks) {
    const regenerated = path.join(tmp, path.basename(committed));
    if ((await run('node', [generator, '--out', regenerated])) !== 0) {
      exitCode = 1;
      break;
    }
    if (!readFileSync(committed).equals(readFileSync(regenerated))) {
      console.log(`SCHEMA DRIFT: ${committed} no longer matches ${what}.`);
      stale += 1;
    }
  }
  if (exitCode === 0 && stale > 0) {
    console.log('Regenerate (npm run generate, in editor/protocol) and commit the result.');
    exitCode = 1;
  } else if (exitCode === 0) {
    console.log('schema drift: none');
  }
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
process.exit(exitCode);
