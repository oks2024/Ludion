#!/usr/bin/env node
// Size budget for `ludion-ecs`, the freeze-listed crate (engine-architecture.md §4): the ECS stays small
// enough to hold in an agent's context, so it carries a metric and a limit rather than a rough number.
//
//   code lines  ≤ 3510   per `crates/ludion-ecs/src/*.rs`, the lines before the first column-0
//                         `#[cfg(test)]` (every ECS file keeps its test module last), excluding blank
//                         lines and lines whose trimmed text starts with `//`
//
// Over the limit, the crate has outgrown "milestone-minimal": move the new capability to the crate that
// needs it, or take the growth to the user. Raising the limit is a decision for the user, not a fix.
// Raised for the opt-in write journal (Serious Isaac request 0039), like J1 before it, and again to 3300
// for the spawn rank (Serious Isaac request 0043), and to 3310 for the scaled gameplay clock on
// SystemCtx (Neon Shroud request 0011; the arithmetic lives in ludion-core::time, SystemCtx forwards),
// and to 3405 for the sanctioned entity-ref collection, EntityRefCollection (Neon Shroud request 0003,
// L1392; user-approved 2026-11-21), and to 3500 for structured checkpoint columns (Neon Shroud
// request 0012; user-approved 2026-11-22), and to 3510 for the retired-spelling hook
// (Plan/character-assembly-merge.md decision 12; user-approved 2026-11-28).
// Never trade code quality or architecture to stay under the limit; ask the user to raise it.
//
// Usage:  node scripts/ecs-budget.mjs
//
// Exit 0 within budget, 1 over (or when no ECS source is found). Zero npm dependencies — Node built-ins only.

import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const LIMIT = 3510;

// Non-blank, non-comment lines of `text` before its first `#[cfg(test)]` that opens a `mod`.
export function countCodeLines(text) {
  const cut = text.search(/^#\[cfg\(test\)\]\s*\n\s*(?:pub(?:\([^)]*\))?\s+)?mod\s/m);
  const code = cut === -1 ? text : text.slice(0, cut);
  return code.split('\n').filter((l) => l.trim() !== '' && !l.trim().startsWith('//')).length;
}

export function rsFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? rsFiles(path.join(dir, e.name)) : e.name.endsWith('.rs') ? [path.join(dir, e.name)] : []);
}

function main() {
  const src = path.join(REPO_ROOT, 'crates', 'ludion-ecs', 'src');
  const files = rsFiles(src);
  if (files.length === 0) {
    console.error(`ecs-budget: no .rs files under ${src}`);
    process.exit(1);
  }
  const total = files.reduce((sum, n) => sum + countCodeLines(readFileSync(n, 'utf8').replace(/\r\n/g, '\n')), 0);
  if (total <= LIMIT) {
    console.log(`ecs-budget: ok (${total} of ${LIMIT} code lines across ${files.length} files)`);
    process.exit(0);
  }
  console.error(`ecs-budget: ludion-ecs is ${total} code lines, budget ${LIMIT} — the crate is freeze-listed and milestone-minimal.`);
  console.error("Move the new capability to the crate that needs it. Raising the limit is the user's decision.");
  process.exit(1);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
