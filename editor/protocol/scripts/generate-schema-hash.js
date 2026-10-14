#!/usr/bin/env node
'use strict';

// Regenerates `editor/app/src/protocol/schemaHash.ts` from the committed protocol schema snapshot
// (`crates/ludion-protocol/tests/protocol_schema.snapshot.json`).
//
// The value is `ludion_protocol::schema::schema_hash()`: XXH3-64 (seed 0) of the snapshot's exact
// bytes, 16 lowercase hex digits. The Rust test pins it (`EXPECTED_SCHEMA_HASH`); the editor's
// `schemaCheck.test.ts` compares the two. `ci/scripts/schema-drift.mjs` re-runs this script with
// `--out <tmp>` and byte-compares.

const fs = require('fs');
const path = require('path');
const { xxhash3 } = require('hash-wasm');

const REPO_ROOT = path.join(__dirname, '..', '..', '..');
const SNAPSHOT_PATH = path.join(REPO_ROOT, 'crates', 'ludion-protocol', 'tests', 'protocol_schema.snapshot.json');
const outArg = process.argv.indexOf('--out');
const OUT_PATH = outArg > 0 && process.argv[outArg + 1]
  ? path.resolve(process.argv[outArg + 1])
  : path.join(REPO_ROOT, 'editor', 'app', 'src', 'protocol', 'schemaHash.ts');

const BANNER = `// GENERATED from \`crates/ludion-protocol/tests/protocol_schema.snapshot.json\` by
// \`editor/protocol/scripts/generate-schema-hash.js\`. Do not edit by hand -- run \`npm run generate\`
// in \`editor/protocol\`. The \`schema-drift\` gate rejects a stale copy.

/** XXH3-64 of the protocol schema snapshot these editor types were generated from. */
`;

async function main() {
  // The raw bytes, never a decoded string: the Rust hash covers the file as committed.
  const hash = await xxhash3(fs.readFileSync(SNAPSHOT_PATH));
  fs.mkdirSync(path.dirname(OUT_PATH), { recursive: true });
  fs.writeFileSync(OUT_PATH, `${BANNER}export const SCHEMA_HASH = '${hash}';\n`);
  process.stdout.write(`wrote ${path.relative(REPO_ROOT, OUT_PATH)}\n`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
