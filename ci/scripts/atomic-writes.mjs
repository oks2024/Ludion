#!/usr/bin/env node
// The in-place-write lint (audit 2026-10 review): no non-test Rust source in `ludion-cli`, `ludion`,
// `ludion-engine`, `ludion-testkit` or `ludion-render-graph` calls `fs::write`, `fs::copy` or
// `File::create` directly. Every writer that replaces a user-visible file goes through
// `ludion_core::atomic` (`write_atomic`, `copy_atomic`: a temp sibling, then a rename), so a crash
// or a full disk leaves the old file whole instead of a truncated one (r1290;
// `dev-docs/ludion-core.md`); the exceptions are allowlisted below.
//
// A hit is reported as `path:line: snippet`. Test code is exempt: `#[cfg(test)]` items,
// `#[cfg(all(test, …))]` items, and the out-of-line module files they declare. Comments and
// string/char literals are blanked first, so a doc-comment example or a quoted `fs::write(` is not a hit.
//
// Allowlist (`ALLOWLIST`, a file plus a needle found in the hit line, each with its reason): writes
// that are streaming logs or staging in a fresh per-run directory, where atomic replace does not
// apply. An entry that matches no hit is an error ("stale allowlist entry"), so the list cannot rot.
//
// A second lane covers the Node scripts under `scripts/`, `ci/scripts/`, `games/` and `examples/`
// (`*.mjs`, tests excluded; `node_modules`, `target` and the git-ignored editor scratch `.ludion` are
// skipped): a direct `writeFileSync`/`copyFileSync`/`appendFileSync`/`createWriteStream` is a hit unless the
// script routes the write through `scripts/atomic-write.mjs` (`writeFileAtomic`) or allowlists it.
//
// The walk reads the filesystem, never git: TeamCity checkouts are Lore workspaces with no `.git`.
// Mirrors the `atomic writes` step in `.github/workflows/ci.yml` and `atomic-writes` in `scripts/check.mjs`.
//
// Usage:  node ci/scripts/atomic-writes.mjs
//
// Exit 0 clean, 1 on any hit, stale entry, or when the walk saw no files.
// Zero npm dependencies — Node built-ins only.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
export const ROOTS = ['ludion-cli', 'ludion', 'ludion-engine', 'ludion-testkit', 'ludion-render-graph', 'ludion-gen', 'ludion-import', 'ludion-mesh-edit', 'ludion-shader-bindgen'].map(
  (c) => `crates/${c}/src`,
);

export const SCRIPT_ROOTS = ['scripts', 'ci/scripts', 'games', 'examples'];

export const ALLOWLIST = [
  {
    file: 'crates/ludion-engine/src/engine.rs',
    needle: 'File::create(&temp)',
    reason: 'the streamed run recording temp, published by atomic::commit_durable',
  },
  {
    file: 'crates/ludion-cli/src/check_scene.rs',
    needle: 'File::create(record_path)',
    reason: 'replay recording sink, a streaming log (ledger trigger: the first L3010 truncated inside a record)',
  },
  {
    file: 'crates/ludion/src/embed.rs',
    needle: 'File::create(record_path)',
    reason: 'replay recording sink, a streaming log (ledger trigger: the first L3010 truncated inside a record)',
  },
  {
    file: 'crates/ludion-cli/src/dev.rs',
    needle: 'std::fs::copy(exe, &dest)',
    reason: 'a unique per-session copy of the game exe, never replaced in place',
  },
  {
    file: 'crates/ludion-engine/src/model_sheet.rs',
    needle: 'slot.join(file_name), text',
    reason: 'staging in a fresh per-render temp dir',
  },
  {
    file: 'crates/ludion-engine/src/model_sheet.rs',
    needle: 'copy(source, slot.join',
    reason: 'staging in a fresh per-render temp dir',
  },
  {
    file: 'crates/ludion-engine/src/model_sheet.rs',
    needle: 'source_dir.join(authored)',
    reason: 'staging in a fresh per-render temp dir',
  },
  {
    file: 'crates/ludion-engine/src/model_sheet.rs',
    needle: 'root.join(file_name), contents',
    reason: 'staging in a fresh per-render temp dir',
  },
  { file: 'scripts/atomic-write.mjs', needle: 'writeFileSync(temp', reason: "the helper's own temp write" },
  { file: 'scripts/check-lock.mjs', needle: 'writeFileSync(temp', reason: "the lock record's temp file, published by linkSync" },
  { file: 'scripts/check.mjs', needle: 'createWriteStream(logPath)', reason: 'per-step log under .build/, a streaming log' },
  { file: 'scripts/check.mjs', needle: 'writeFileSync(logPath', reason: 'per-step log under .build/' },
  { file: 'scripts/check.mjs', needle: 'copyFileSync(path.join(targetDir', reason: 'bin copy into the per-run bin dir' },
  { file: 'scripts/check.mjs', needle: 'appendFileSync(HISTORY', reason: 'append-only run history' },
  { file: 'scripts/commit.mjs', needle: 'writeFileSync(messageFile', reason: 'temp commit-message file' },
  { file: 'scripts/mirror-gap.mjs', needle: "values['paths-file']", reason: 'caller-named scratch output' },
  { file: 'scripts/physics-features.mjs', needle: "'src', 'lib.rs'", reason: 'fresh probe dir under target/' },
  { file: 'scripts/physics-features.mjs', needle: 'fs.copyFileSync(', reason: 'fresh probe dir under target/' },
  { file: 'scripts/physics-features.mjs', needle: 'fs.writeFileSync(', reason: 'fresh probe dir under target/' },
  { file: 'scripts/physics-pin-ritual.mjs', needle: 'path.join(BUILD_DIR, log)', reason: 'ritual log under .build/' },
  { file: 'games/vigil/art/town/lib/run.mjs', needle: 'fs.writeFileSync(log', reason: 'step log under target/town-lab' },
  { file: 'games/vigil/art/town/lib/run.mjs', needle: 'fs.writeFileSync(file, stage(rig))', reason: 'stage scene in the scratch kit' },
  { file: 'games/vigil/art/town/lib/export.mjs', needle: 'fs.createWriteStream(log)', reason: 'Blender build log, a streaming log' },
  { file: 'games/vigil/art/town/square/build-square.mjs', needle: '"map.txt"', reason: 'scratch report under target/town-lab' },
  { file: 'games/vigil/art/town/square/build-square.mjs', needle: '"stats.json"', reason: 'scratch report under target/town-lab' },
  { file: 'ci/scripts/denylist-canary.mjs', needle: "path.join(dir, 'Cargo.toml')", reason: 'fresh probe crate dir' },
  { file: 'ci/scripts/denylist-canary.mjs', needle: "path.join(dir, 'src', 'lib.rs')", reason: 'fresh probe crate dir' },
  { file: 'ci/scripts/denylist-canary.mjs', needle: "copyFileSync(path.join(probe.crate", reason: 'fresh probe crate dir' },
];

// Same length as `text`, newlines kept: comments (`//`, nested `/* */`) and the contents of string
// and char literals become spaces, so offsets and line numbers match the original.
export function strip(text) {
  const out = text.split('');
  const n = text.length;
  const blank = (from, to) => {
    for (let k = from; k < to; k++) if (out[k] !== '\n') out[k] = ' ';
  };
  const ident = (ch) => ch !== undefined && /[A-Za-z0-9_]/.test(ch);
  const raw = /b?r(#*)"/y;
  let i = 0;
  while (i < n) {
    const c = text[i];
    if (c === '/' && text[i + 1] === '/') {
      let j = text.indexOf('\n', i);
      if (j < 0) j = n;
      blank(i, j);
      i = j;
    } else if (c === '/' && text[i + 1] === '*') {
      let depth = 1;
      let j = i + 2;
      while (j < n && depth > 0) {
        if (text[j] === '/' && text[j + 1] === '*') {
          depth++;
          j += 2;
        } else if (text[j] === '*' && text[j + 1] === '/') {
          depth--;
          j += 2;
        } else j++;
      }
      blank(i, j);
      i = j;
    } else if ((c === 'r' || c === 'b') && !ident(text[i - 1]) && ((raw.lastIndex = i), raw.test(text))) {
      const open = raw.lastIndex;
      const hashes = text.slice(i, open).match(/#*"$/)[0].length - 1;
      const end = text.indexOf(`"${'#'.repeat(hashes)}`, open);
      blank(open, end < 0 ? n : end);
      i = end < 0 ? n : end + 1 + hashes;
    } else if (c === '"') {
      let j = i + 1;
      while (j < n && text[j] !== '"') j += text[j] === '\\' ? 2 : 1;
      blank(i + 1, Math.min(j, n));
      i = j + 1;
    } else if (c === "'") {
      if (text[i + 1] === '\\') {
        const j = text.indexOf("'", i + 3);
        const stop = j < 0 ? n : j;
        blank(i + 1, stop);
        i = stop + 1;
      } else if (text[i + 2] === "'" && text[i + 1] !== undefined) {
        blank(i + 1, i + 2);
        i += 3;
      } else i++;
    } else i++;
  }
  return out.join('');
}

// Index just past the bracket matching the one at `open`, in stripped text.
function closeOf(stripped, open) {
  const pairs = { '(': ')', '[': ']', '{': '}' };
  const stack = [];
  for (let k = open; k < stripped.length; k++) {
    const ch = stripped[k];
    if (pairs[ch]) stack.push(pairs[ch]);
    else if (stack.length && ch === stack.at(-1)) {
      stack.pop();
      if (!stack.length) return k + 1;
    }
  }
  return stripped.length;
}

const isTestCfg = (inner) => {
  const t = inner.trim();
  if (t === 'test') return true;
  return t.startsWith('all(') && !/\b(?:any|not)\s*\(/.test(t) && /\btest\b/.test(t);
};

const ITEM_HEADER =
  /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:const|async|unsafe|extern(?:\s*"[^"]*")?|default)\s+)*(?:fn|mod|impl|struct|enum|trait|union|static|const|type|use|macro_rules!)\b/;

// The `#[cfg(test)]`-style items of a file: byte ranges that are test code, and the out-of-line
// modules (`mod NAME;`) that make another file test-only, with their `#[path]` if any.
function testItems(original) {
  const stripped = strip(original);
  const ranges = [];
  const modules = [];
  const cfg = /#\[cfg\(/g;
  for (let m = cfg.exec(stripped); m; m = cfg.exec(stripped)) {
    const open = m.index + m[0].length - 1;
    const end = closeOf(stripped, open);
    if (!isTestCfg(stripped.slice(open + 1, end - 1))) continue;
    let pos = stripped.indexOf(']', end) + 1;
    let pathAttr = null;
    for (;;) {
      const rest = /^\s*#\[/.exec(stripped.slice(pos));
      if (!rest) break;
      const attrOpen = pos + rest[0].length - 1;
      const attrEnd = closeOf(stripped, attrOpen);
      const attr = original.slice(attrOpen, attrEnd);
      pathAttr = /^\[\s*path\s*=\s*"([^"]+)"/.exec(attr)?.[1] ?? pathAttr;
      pos = attrEnd;
    }
    const start = pos;
    // A struct field or enum variant is not an item: its scan must end at the `,` or `}` that closes
    // it, or it would exempt the next item's body. An item header may hold `,` (generics), so it scans on.
    const isItem = ITEM_HEADER.test(stripped.slice(pos, pos + 200));
    let depth = 0;
    let k = pos;
    for (; k < stripped.length; k++) {
      const ch = stripped[k];
      if (ch === '(' || ch === '[') depth++;
      else if (')]'.includes(ch)) depth--;
      else if (depth <= 0 && (ch === '{' || ch === ';')) break;
      else if (depth <= 0 && !isItem && (ch === '}' || ch === ',')) break;
    }
    if (stripped[k] === '{') {
      ranges.push([m.index, closeOf(stripped, k)]);
    } else {
      const named = /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*$/.exec(stripped.slice(start, k));
      if (named) modules.push({ name: named[1], pathAttr });
    }
  }
  return { stripped, ranges, modules };
}

const WRITE = /\b(?:fs::write|fs::copy|File::create(?:_new)?)\s*\(/g;

// Every in-place write outside test code in `text`: `{ line, snippet }`, 1-based line.
export function findWrites(text) {
  const { stripped, ranges } = testItems(text);
  const lines = text.split('\n');
  const hits = [];
  for (const m of stripped.matchAll(WRITE)) {
    if (ranges.some(([a, b]) => m.index >= a && m.index < b)) continue;
    const line = stripped.slice(0, m.index).split('\n').length;
    hits.push({ line, snippet: lines[line - 1].trim() });
  }
  return hits;
}

// Files the test-only `mod NAME;` declarations in `text` (read from `file`) point at.
function testOnlyFiles(file, text) {
  const dir = path.dirname(file);
  const base = path.basename(file);
  const stem = base.replace(/\.rs$/, '');
  const home = ['lib.rs', 'main.rs', 'mod.rs'].includes(base) ? dir : path.join(dir, stem);
  const found = [];
  for (const { name, pathAttr } of testItems(text).modules) {
    const candidates = pathAttr
      ? [path.resolve(dir, pathAttr)]
      : [path.join(home, `${name}.rs`), path.join(home, name, 'mod.rs')];
    found.push(...candidates.filter(existsSync));
  }
  return found;
}

const SCRIPT_WRITE = /\b(?:writeFileSync|writeFile|copyFileSync|appendFileSync|createWriteStream)\s*\(/;

// Every direct write in a script's `text`: `{ line, snippet }`, 1-based line. Line-based, with whole
// comment lines skipped — the Rust `strip()` would mis-lex JS.
export function findScriptWrites(text) {
  const hits = [];
  text.split('\n').forEach((raw, i) => {
    const line = raw.trim();
    if (line.startsWith('//') || line.startsWith('*')) return;
    if (SCRIPT_WRITE.test(line)) hits.push({ line: i + 1, snippet: line });
  });
  return hits;
}

// The gate's own allowlist names every write it exempts, so it cannot scan itself.
const SELF = fileURLToPath(import.meta.url);

function scriptFiles(dir) {
  const files = [];
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === '.ludion' || name === 'target') continue;
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) files.push(...scriptFiles(full));
    else if (name.endsWith('.mjs') && !name.endsWith('.test.mjs') && full !== SELF) files.push(full);
  }
  return files;
}

function rustFiles(dir) {
  const files = [];
  for (const name of readdirSync(dir)) {
    if (name === 'target') continue;
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) files.push(...rustFiles(full));
    else if (name.endsWith('.rs')) files.push(full);
  }
  return files;
}

// { errors, scanned, scriptsScanned } over the Rust `roots` and the `scriptRoots` (repo-relative) of `root`.
export function runGate({ root = REPO_ROOT, roots = ROOTS, scriptRoots = SCRIPT_ROOTS, allowlist = ALLOWLIST } = {}) {
  const errors = [];
  const files = roots.map((r) => path.join(root, r)).filter(existsSync).flatMap(rustFiles);
  const texts = new Map(files.map((f) => [f, readFileSync(f, 'utf8').replace(/\r\n/g, '\n')]));
  const exempt = new Set();
  for (const [file, text] of texts) for (const f of testOnlyFiles(file, text)) exempt.add(path.resolve(f));
  const used = new Set();
  let scanned = 0;
  for (const [file, text] of texts) {
    if (exempt.has(path.resolve(file))) continue;
    scanned += 1;
    const rel = path.relative(root, file).split(path.sep).join('/');
    for (const { line, snippet } of findWrites(text)) {
      const entry = allowlist.find((a) => a.file === rel && snippet.includes(a.needle));
      if (entry) used.add(entry);
      else {
        errors.push(
          `${rel}:${line}: ${snippet} — route it through ludion_core::atomic::{write_atomic, copy_atomic}, or allowlist it with a reason`,
        );
      }
    }
  }
  const scripts = scriptRoots.map((r) => path.join(root, r)).filter(existsSync).flatMap(scriptFiles);
  for (const file of scripts) {
    const rel = path.relative(root, file).split(path.sep).join('/');
    for (const { line, snippet } of findScriptWrites(readFileSync(file, 'utf8'))) {
      const entry = allowlist.find((a) => a.file === rel && snippet.includes(a.needle));
      if (entry) used.add(entry);
      else {
        errors.push(
          `${rel}:${line}: ${snippet} — route it through scripts/atomic-write.mjs writeFileAtomic, or allowlist it with a reason`,
        );
      }
    }
  }
  const vigilAbsent = !existsSync(path.join(root, 'games', 'vigil')); // the Lore-only game
  for (const entry of allowlist) {
    if (vigilAbsent && entry.file.startsWith('games/vigil/')) continue;
    if (!used.has(entry)) errors.push(`stale allowlist entry: ${entry.file} \`${entry.needle}\` matches no write — remove it`);
  }
  if (roots.length > 0 && scanned === 0) errors.push(`the walk saw no Rust files under ${roots.join(', ')}`);
  if (scriptRoots.length > 0 && scripts.length === 0) {
    errors.push(`the walk saw no script files under ${scriptRoots.join(', ')}`);
  }
  return { errors, scanned, scriptsScanned: scripts.length };
}

function main() {
  const { errors, scanned, scriptsScanned } = runGate();
  if (errors.length) {
    for (const e of errors) console.error(e);
    process.exit(1);
  }
  console.log(`atomic-writes: ${scanned} Rust files and ${scriptsScanned} scripts scanned, ${ALLOWLIST.length} allowlisted writes, clean`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
