#!/usr/bin/env node
// Size budget for `ludion-ecs`, the freeze-listed crate (engine-architecture.md §4): the ECS stays small
// enough to hold in an agent's context, so it carries a metric and a limit rather than a rough number.
//
//   code lines  ≤ 3000    per `crates/ludion-ecs/src/*.rs`, the lines before the first column-0
//                         `#[cfg(test)]` (every ECS file keeps its test module last), excluding blank
//                         lines and lines whose trimmed text starts with `//`
//
// Over the limit, the crate has outgrown "milestone-minimal": move the new capability to the crate that
// needs it, or take the growth to the user. Raising the limit is a decision for the user, not a fix.
//
// Usage:  node scripts/ecs-budget.mjs
//
// Exit 0 within budget, 1 over (or when no ECS source is found). Zero npm dependencies — Node built-ins only.

import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const LIMIT = 3000;

// Non-blank, non-comment lines of `text` before its first column-0 `#[cfg(test)]`.
export function countCodeLines(text) {
  const cut = text.search(/^#\[cfg\(test\)\]/m);
  const code = cut === -1 ? text : text.slice(0, cut);
  return code.split('\n').filter((l) => l.trim() !== '' && !l.trim().startsWith('//')).length;
}

function main() {
  const src = path.join(REPO_ROOT, 'crates', 'ludion-ecs', 'src');
  const files = readdirSync(src).filter((n) => n.endsWith('.rs'));
  if (files.length === 0) {
    console.error(`ecs-budget: no .rs files under ${src}`);
    process.exit(1);
  }
  const total = files.reduce((sum, n) => sum + countCodeLines(readFileSync(path.join(src, n), 'utf8').replace(/\r\n/g, '\n')), 0);
  if (total <= LIMIT) {
    console.log(`ecs-budget: ok (${total} of ${LIMIT} code lines across ${files.length} files)`);
    process.exit(0);
  }
  console.error(`ecs-budget: ludion-ecs is ${total} code lines, budget ${LIMIT} — the crate is freeze-listed and milestone-minimal.`);
  console.error("Move the new capability to the crate that needs it. Raising the limit is the user's decision.");
  process.exit(1);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
