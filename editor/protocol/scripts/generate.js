#!/usr/bin/env node
'use strict';

// Regenerates `editor/protocol/src/protocol.d.ts` from the committed protocol schema snapshot
// (`crates/ludion-protocol/tests/protocol_schema.snapshot.json`).
//
// This is the TypeScript half of the schema-drift gate (engine-architecture.md §6, risk register
// #4): a Rust-side test asserts the snapshot matches the code (`cargo test -p ludion-protocol`);
// `ci/scripts/schema-drift.mjs` additionally re-runs this script with `--out <tmp>` and byte-compares
// the result with the committed file, so the generated TS types can never silently drift from the
// wire protocol. Two runs on unchanged input are byte-identical.
//
// The snapshot bundles eight independent JSON Schema documents — one per protocol section
// (`request`, `response`, `event`, `command`, `response_body`, `hello`, `server_hello`,
// `frame_header`) — each with its own local `$defs` (a ninth key, `protocol_version`, is a plain
// integer literal, not a schema). To emit one clean `.d.ts` with every named type declared exactly
// once, this script merges all sections into a single synthetic root schema (one shared `$defs`
// map, one property per section) before handing it to `json-schema-to-typescript` in one
// `compile()` call. Merging is safe because a type shared by more than one section (e.g. an entity
// id schema referenced from both `command` and `response_body`) is generated from the same Rust
// type every time, so its shape is byte-identical wherever it appears — `mergeDef` asserts that.

const fs = require('fs');
const path = require('path');
const { compile } = require('json-schema-to-typescript');

const REPO_ROOT = path.join(__dirname, '..', '..', '..');
const SNAPSHOT_PATH = path.join(
  REPO_ROOT,
  'crates',
  'ludion-protocol',
  'tests',
  'protocol_schema.snapshot.json',
);
const outArg = process.argv.indexOf('--out');
const OUT_PATH = outArg > 0 && process.argv[outArg + 1]
  ? path.resolve(process.argv[outArg + 1])
  : path.join(__dirname, '..', 'src', 'protocol.d.ts');

// Every section of the schema document except `protocol_version` is a fully self-contained JSON
// Schema with its own `title` and (optionally) `$defs`. Order here is alphabetical by section key,
// matching the schema document's own canonical (sorted) key order — arbitrary but stable, so the
// merged root schema's property order (and therefore the generated output) never depends on
// object-key enumeration order.
const SECTIONS = [
  'command',
  'event',
  'frame_header',
  'hello',
  'request',
  'response',
  'response_body',
  'server_hello',
];

// Inserts `def` under `name` in `defs`, unless an identical entry is already there.
function mergeDef(defs, name, def) {
  if (Object.prototype.hasOwnProperty.call(defs, name)) {
    if (JSON.stringify(defs[name]) !== JSON.stringify(def)) {
      throw new Error(
        `protocol schema sections disagree on the shape of "${name}" -- ` +
          'the merge assumption (a shared type is generated identically everywhere) does not hold',
      );
    }
    return;
  }
  defs[name] = def;
}

async function main() {
  const doc = JSON.parse(fs.readFileSync(SNAPSHOT_PATH, 'utf8'));

  const defs = {};
  const properties = {};
  const required = [];

  for (const key of SECTIONS) {
    const section = doc[key];
    if (!section || typeof section !== 'object') {
      throw new Error(`protocol schema snapshot is missing section "${key}"`);
    }
    const { $defs, $schema, title, ...rest } = section;
    if (!title) {
      throw new Error(`section "${key}" has no "title" to key its generated type on`);
    }
    mergeDef(defs, title, rest);
    for (const [name, def] of Object.entries($defs || {})) {
      mergeDef(defs, name, def);
    }
    properties[title] = { $ref: `#/$defs/${title}` };
    required.push(title);
  }
  required.sort();

  const merged = {
    $schema: 'https://json-schema.org/draft/2020-12/schema',
    title: 'ProtocolSchema',
    description:
      'Every wire type in the ludion protocol, generated from the schema snapshot. ' +
      'One property per protocol section (Command, Request, Response, ...); see each type\'s own ' +
      'members for the actual shape.',
    type: 'object',
    additionalProperties: false,
    properties,
    required,
    $defs: defs,
  };

  const ts = await compile(merged, 'ProtocolSchema', {
    bannerComment:
      '/* eslint-disable */\n' +
      '/**\n' +
      ' * This file is generated from `crates/ludion-protocol/tests/protocol_schema.snapshot.json`\n' +
      ' * by `editor/protocol/scripts/generate.js`. Do not edit by hand -- run `npm run generate`\n' +
      ' * (from `editor/protocol/`) after the Rust-side schema snapshot changes.\n' +
      ' */',
    style: { singleQuote: true },
    $refOptions: { resolve: { file: false, http: false } },
  });

  // Force LF regardless of the platform this ran on: the repo's `.gitattributes` normalizes line
  // endings on commit, but a byte-stable working tree is what makes `git diff --exit-code` in CI a
  // meaningful drift check rather than an EOL false positive.
  const lf = ts.replace(/\r\n/g, '\n');
  fs.mkdirSync(path.dirname(OUT_PATH), { recursive: true });
  fs.writeFileSync(OUT_PATH, lf);
  process.stdout.write(`wrote ${path.relative(REPO_ROOT, OUT_PATH)}\n`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
