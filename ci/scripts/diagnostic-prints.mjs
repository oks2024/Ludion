#!/usr/bin/env node
// The prefixed-print lint (audit 2026-10-03 diagnostics L7/L8): no Rust source under
// `crates/*/src`, `games/*/src`, their `examples/` or their `build.rs`, nor the editor's Tauri host
// (`editor/app/src-tauri`), prints a hand-written
// `error:` / `warning:` / `error[` / `warning[` line. A runtime
// failure is a coded `Diagnostic` printed through `Display` (which writes `error[L####]: ...` itself);
// a bad flag or argument goes through a usage helper. Hand-written prefixes had drifted into
// `warning: warning[L5001]` doubles and codeless failures an agent could not `ludion explain`.
//
// It matches print-family macros only (`eprintln!`, `println!`, `eprint!`, `print!`, `write!`,
// `writeln!`, `format!`), including the multi-line `eprintln!(\n "error: ..."` form, raw string
// literals (`r"…"`, `r#"…"#`) and a leading writer argument of any shape (`err`, `&mut err`,
// `std::io::stderr()`). A plain string literal in a test (`"error[E0308]..."`, an `assert!`) is not
// a hit.
//
// Allowlist, the two usage helpers: `crates/ludion-cli/src/diagnostic.rs` (`usage_error`) and
// `crates/ludion/src/usage.rs` (`usage_error`, `usage_warning`).
//
// Every hit is reported as `path:line: snippet`. Mirrors the `diagnostic prints` step in
// `.github/workflows/ci.yml` and `diagnostic-prints` in `scripts/check.mjs`.
//
// Usage:  node ci/scripts/diagnostic-prints.mjs
//
// Exit 0 clean, 1 on any hit (or when the walk saw no files). Zero npm dependencies — Node built-ins only.

import { readdirSync, readFileSync, existsSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
export const ALLOWLIST = ['crates/ludion-cli/src/diagnostic.rs', 'crates/ludion/src/usage.rs'];

const PREFIXED_PRINT = /\b(?:e?print(?:ln)?|write(?:ln)?|format)!\(\s*(?:[^,"()]+(?:\([^()]*\))?\s*,\s*)?r?#*"(?:error|warning)(?::|\[)/g;

// Every `error:` / `warning:` print hit in `text`, with its 1-based line (of the macro name).
export function findPrefixedPrints(text) {
  const hits = [];
  for (const match of text.matchAll(PREFIXED_PRINT)) {
    const line = text.slice(0, match.index).split('\n').length;
    hits.push({ line, snippet: match[0].replace(/\s+/g, ' ') });
  }
  return hits;
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

const subdirs = (dir) =>
  existsSync(dir) ? readdirSync(dir).filter((n) => statSync(path.join(dir, n)).isDirectory()) : [];

// Every Rust file the lint reads under `repoRoot`: the workspace members plus the editor's Tauri host.
export function scanFiles(repoRoot) {
  const members = [
    ...['crates', 'games'].flatMap((group) =>
      subdirs(path.join(repoRoot, group)).map((name) => path.join(repoRoot, group, name)),
    ),
    path.join(repoRoot, 'editor', 'app', 'src-tauri'),
  ];
  const dirs = members
    .flatMap((m) => ['src', 'examples'].map((d) => path.join(m, d)))
    .filter(existsSync);
  const buildScripts = members.map((m) => path.join(m, 'build.rs')).filter(existsSync);
  return [...dirs.flatMap(rustFiles), ...buildScripts];
}

function main() {
  let seen = 0;
  const failures = [];
  for (const file of scanFiles(REPO_ROOT)) {
    const rel = path.relative(REPO_ROOT, file).split(path.sep).join('/');
    if (ALLOWLIST.includes(rel)) continue;
    seen += 1;
    const text = readFileSync(file, 'utf8').replace(/\r\n/g, '\n');
    for (const { line, snippet } of findPrefixedPrints(text)) {
      failures.push(`${rel}:${line}: ${snippet} — print a coded Diagnostic through Display, or usage_error for a flag error`);
    }
  }
  if (seen === 0) failures.push('the walk saw no Rust files under crates/*, games/* or the editor host');
  for (const failure of failures) console.error(failure);
  process.exit(failures.length === 0 ? 0 : 1);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
