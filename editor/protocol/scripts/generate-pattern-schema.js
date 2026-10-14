#!/usr/bin/env node
'use strict';

// Regenerates `editor/app/src/editor/effects/patternSchema.ts` from the committed pattern schema
// snapshot (`crates/ludion-engine/tests/snapshots/pattern_schema.json`).
//
// Same two layers as protocol.d.ts: a Rust test asserts the snapshot matches `PatternDoc`
// (`cargo test -p ludion-engine --test pattern_schema_snapshot`); `ci/scripts/schema-drift.mjs`
// re-runs this script with `--out <tmp>` and byte-compares. The snapshot text is embedded verbatim
// (no parse/stringify round-trip, which could reorder integer-like keys). No npm dependency.

const fs = require('fs');
const path = require('path');

const REPO_ROOT = path.join(__dirname, '..', '..', '..');
const SNAPSHOT_PATH = path.join(
  REPO_ROOT,
  'crates',
  'ludion-engine',
  'tests',
  'snapshots',
  'pattern_schema.json',
);
const outArg = process.argv.indexOf('--out');
const OUT_PATH = outArg > 0 && process.argv[outArg + 1]
  ? path.resolve(process.argv[outArg + 1])
  : path.join(REPO_ROOT, 'editor', 'app', 'src', 'editor', 'effects', 'patternSchema.ts');

const BANNER = `// GENERATED from \`crates/ludion-engine/tests/snapshots/pattern_schema.json\` by
// \`editor/protocol/scripts/generate-pattern-schema.js\`. Do not edit by hand -- re-bless the Rust
// snapshot, then run \`npm run generate\` in \`editor/protocol\`. The \`schema-drift\` gate rejects a
// stale copy.
//
// The bundled \`kind: pattern\` JSON Schema, \`schema_for!(PatternDoc)\`: the document
// \`engine.describe\` reports under \`documents.pattern\`. The pattern tab prefers the live schema, but
// the effects stage is plain \`ludion serve\` -- no game crate registers the kind, so \`documents\` is
// empty -- and the tab still needs a schema to list, open, edit and save a file. The form's
// fallback and \`docFields.test.ts\`'s coverage assertion read this same constant.

/** \`schema_for!(PatternDoc)\`, draft 2020-12, exactly as schemars v1 emits it. */
`;

const text = fs.readFileSync(SNAPSHOT_PATH, 'utf8').replace(/\r\n/g, '\n').trim();
JSON.parse(text); // fail loudly on a malformed snapshot; the original text is what gets emitted
const body = text.split('\n').map((line, i) => (i === 0 ? line : `  ${line}`)).join('\n');
const ts = `${BANNER}export const PATTERN_SCHEMA: unknown =\n  ${body};\n`;

fs.mkdirSync(path.dirname(OUT_PATH), { recursive: true });
fs.writeFileSync(OUT_PATH, ts);
process.stdout.write(`wrote ${path.relative(REPO_ROOT, OUT_PATH)}\n`);
