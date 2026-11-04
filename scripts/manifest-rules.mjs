#!/usr/bin/env node
// Manifest rules, checked mechanically. Two rules had been held by
// review alone and had drifted:
//
//   bare versions  a member crate's dependency inherits from the root `[workspace.dependencies]`
//                  (`foo.workspace = true`) or is a path dependency; a version written in the member
//                  (`foo = "1"`, `foo = { version = "1", .. }`) is a second place to bump. The
//                  detached `games/vigil` workspace has its own root, so it is skipped.
//   facade         a game's (and the `ludion new` template's) plain `[dependencies]` name only the
//                  `ludion` facade, never an engine crate; test harness crates are `[dev-dependencies]`.
//
// `scripts/lists-in-step.mjs` imports `checkManifests`, so `check.mjs` and CI run the same check, and
// `ci/scripts/gate-scripts.test.mjs` imports the two text functions. Zero npm dependencies — Node
// built-ins only.

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { workspaceDirs } from './clippy-denylist.mjs';

const TEMPLATE = 'crates/ludion-cli/templates/Cargo.toml.tpl';
const DETACHED = new Set(['games/vigil']);

// `[section]` headers of dependency tables: plain, target-specific, and the `[dependencies.foo]` form.
const DEP_SECTION = /^(?:target\.(?:'[^']*'|"[^"]*"|[^\]]*?)\.)?(dependencies|dev-dependencies|build-dependencies)(?:\.([\w-]+))?$/;

const stripComment = (line) => {
  let quote = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote) {
      if (c === quote) quote = null;
    } else if (c === '"' || c === "'") quote = c;
    else if (c === '#') return line.slice(0, i);
  }
  return line;
};

const depth = (text) => {
  let quote = null;
  let n = 0;
  for (const c of text) {
    if (quote) {
      if (c === quote) quote = null;
    } else if (c === '"' || c === "'") quote = c;
    else if (c === '{' || c === '[') n++;
    else if (c === '}' || c === ']') n--;
  }
  return n;
};

// Every dependency entry as `{ section, kind, name, value }`; `value` is the whole right-hand side
// (a multi-line inline table is accumulated until its braces balance). A `[dependencies.foo]` table
// is one entry whose value is the table's `key = value` lines joined.
function entries(manifestText) {
  const out = [];
  let section = null; // { header, kind, table }
  let pending = null; // { name, value } still open
  let tableEntry = null;
  const flushTable = () => {
    if (tableEntry) out.push(tableEntry);
    tableEntry = null;
  };
  for (const raw of manifestText.split('\n')) {
    const line = stripComment(raw).trim();
    if (pending) {
      pending.value += ` ${line}`;
      if (depth(pending.value) <= 0) {
        out.push({ ...pending, section: section.header, kind: section.kind });
        pending = null;
      }
      continue;
    }
    if (line === '') continue;
    if (/^\[\[.*\]\]$/.test(line)) {
      flushTable();
      section = null;
      continue;
    }
    const header = /^\[(.*)\]$/.exec(line);
    if (header) {
      flushTable();
      const m = DEP_SECTION.exec(header[1].trim());
      section = m ? { header: header[1].trim(), kind: m[1], table: m[2] ?? null } : null;
      if (section?.table) tableEntry = { section: section.header, kind: section.kind, name: section.table, value: '' };
      continue;
    }
    if (!section) continue;
    if (section.table) {
      tableEntry.value += ` ${line}`;
      continue;
    }
    const kv = /^([\w.-]+)\s*=\s*(.*)$/.exec(line);
    if (!kv) continue;
    const entry = { name: kv[1], value: kv[2] };
    if (depth(entry.value) > 0) pending = entry;
    else out.push({ ...entry, section: section.header, kind: section.kind });
  }
  flushTable();
  return out;
}

const inherits = (e) => e.name.endsWith('.workspace')
  || /\bworkspace\s*=\s*true\b/.test(e.value)
  || /\bpath\s*=/.test(e.value);

// Names every dependency entry that carries its own version instead of inheriting from the workspace.
export function bareVersions(manifestText) {
  return entries(manifestText).filter((e) => !inherits(e)).map((e) => e.name.replace(/\.workspace$/, ''));
}

// Names every `ludion-*` crate in a plain `[dependencies]` table (the template's `ludion = ..` facade
// line is the one allowed engine name).
export function nonFacadeDeps(manifestText) {
  return entries(manifestText)
    .filter((e) => e.kind === 'dependencies' && /^ludion-/.test(e.name))
    .map((e) => e.name);
}

const read = (p) => readFileSync(p, 'utf8');

// `{ errors, count }`: `dirs` are the repo-relative workspace members (`workspaceDirs(root)`).
export function checkManifests(root, dirs = workspaceDirs(root)) {
  const errors = [];
  let count = 0;
  for (const dir of dirs) {
    if (DETACHED.has(dir)) continue;
    const file = `${dir}/Cargo.toml`;
    count++;
    for (const name of bareVersions(read(path.join(root, file)))) {
      errors.push(`manifests: ${file} gives \`${name}\` its own version — add it to [workspace.dependencies] in the root Cargo.toml and write \`${name}.workspace = true\` here`);
    }
  }
  const games = path.join(root, 'games');
  const facadeFiles = [
    ...(existsSync(games) ? readdirSync(games) : [])
      .filter((n) => statSync(path.join(games, n)).isDirectory() && existsSync(path.join(games, n, 'Cargo.toml')))
      .map((n) => `games/${n}/Cargo.toml`),
    TEMPLATE,
  ];
  for (const file of facadeFiles) {
    if (!existsSync(path.join(root, file))) continue;
    count++;
    for (const name of nonFacadeDeps(read(path.join(root, file)))) {
      errors.push(`manifests: ${file} names \`${name}\` in [dependencies] — a game reaches the engine through the \`ludion\` facade; move a test-only use to [dev-dependencies]`);
    }
  }
  return { errors, count };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const { errors, count } = checkManifests(root);
  if (errors.length) {
    for (const e of errors) console.error(`  - ${e}`);
    process.exit(1);
  }
  console.log(`manifests: OK (${count} manifests)`);
}
