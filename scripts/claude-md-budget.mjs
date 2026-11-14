#!/usr/bin/env node
// Size budget for the root CLAUDE.md.
//
// CLAUDE.md is loaded into every session and every subagent, so each byte is paid thousands of
// times. It has been trimmed by hand twice and regrew both times the same way: a wave closes and
// appends a narrative parenthetical to "Current state" (one line reached 5.3 KB). This gate is the
// back-pressure. Three limits:
//
//   whole file          ≤ 20 KB    the operational summary, rules included
//   ## Current state    ≤  5 KB    active plans grouped by what gates them, short tags of what
//                                  exists, no narrative — the section that regrows
//   any single line     ≤ 1200 ch  a line that long is a paragraph hiding in a bullet
//   a nested CLAUDE.md  ≤ 12 KB    games/<game>/CLAUDE.md, editor/CLAUDE.md, … — loaded on demand when
//                                  an agent reads a file in that subtree; same line limit. They regrow
//                                  the same way (vigil's reached 23 KB, 13 KB of it one paragraph).
//
// Over a limit, MOVE text to its owner — never cut a rule to hit the number:
//   what a wave shipped            → that plan's Status section in Plan/
//   anything owed (a sitting, the second pinned machine, a user decision)  → the owed ledger (CLAUDE.local.md names it)
//   a rare-event runbook           → a skill under .claude/skills/, or a dev-docs/ topic
//   a rule only one subtree needs  → that topic in docs/ or dev-docs/, with a one-line pointer
// Raising a limit is a decision for the user, not a fix.
//
// Nested files are every CLAUDE.md in `git ls-files -co --exclude-standard` besides the root one.
//
// Usage:  node scripts/claude-md-budget.mjs [--file <path>]     (--file checks one file as the root)
//
// Exit 0 within budget, 1 over, 2 on a usage error. Zero npm dependencies — Node built-ins only.

import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { parseArgs } from 'node:util';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const FILE_LIMIT_BYTES = 20 * 1024;
const STATE_SECTION = '## Current state';
const STATE_LIMIT_BYTES = 5 * 1024;
const LINE_LIMIT_CHARS = 1200;
const NESTED_LIMIT_BYTES = 12 * 1024;

let values;
try {
  ({ values } = parseArgs({ options: { file: { type: 'string' }, help: { type: 'boolean', short: 'h', default: false } } }));
} catch (err) {
  console.error(`claude-md-budget: ${err.message}`);
  process.exit(2);
}
if (values.help) {
  console.log('Usage: node scripts/claude-md-budget.mjs [--file <path>]   (limits and remedies: see the header comment)');
  process.exit(0);
}

const file = values.file ? path.resolve(values.file) : path.join(REPO_ROOT, 'CLAUDE.md');
let text;
try {
  text = readFileSync(file, 'utf8');
} catch (err) {
  console.error(`claude-md-budget: cannot read ${file}: ${err.message}`);
  process.exit(2);
}

const bytes = (s) => Buffer.byteLength(s, 'utf8');
const kb = (n) => `${(n / 1024).toFixed(1)} KB`;
const hits = [];

const total = bytes(text);
if (total > FILE_LIMIT_BYTES) hits.push(`whole file is ${kb(total)}, budget ${kb(FILE_LIMIT_BYTES)}`);

const lines = text.split('\n');
const start = lines.findIndex((l) => l.trim() === STATE_SECTION);
if (start === -1) {
  hits.push(`no "${STATE_SECTION}" heading — the section this gate watches is gone or renamed`);
} else {
  let end = lines.findIndex((l, i) => i > start && l.startsWith('## '));
  if (end === -1) end = lines.length;
  const section = bytes(lines.slice(start, end).join('\n'));
  if (section > STATE_LIMIT_BYTES) hits.push(`"${STATE_SECTION}" is ${kb(section)}, budget ${kb(STATE_LIMIT_BYTES)}`);
}

const checkLines = (label, ls) => ls.forEach((l, i) => {
  if (l.length > LINE_LIMIT_CHARS) hits.push(`${label}line ${i + 1} is ${l.length} chars, limit ${LINE_LIMIT_CHARS}: "${l.slice(0, 60)}…"`);
});
checkLines('', lines);

let nestedCount = 0;
if (!values.file) {
  const ls = spawnSync('git', ['ls-files', '-co', '--exclude-standard', '-z', '--', '*CLAUDE.md'], { cwd: REPO_ROOT, encoding: 'utf8' });
  if (ls.status !== 0) {
    console.error(`claude-md-budget: git ls-files failed: ${ls.stderr}`);
    process.exit(2);
  }
  for (const rel of ls.stdout.split('\0').filter((f) => f && f !== 'CLAUDE.md' && path.basename(f) === 'CLAUDE.md')) {
    let nested;
    try { nested = readFileSync(path.join(REPO_ROOT, rel), 'utf8'); } catch { continue; } // deleted, not yet committed
    nestedCount++;
    const n = bytes(nested);
    if (n > NESTED_LIMIT_BYTES) hits.push(`${rel} is ${kb(n)}, nested budget ${kb(NESTED_LIMIT_BYTES)}`);
    checkLines(`${rel} `, nested.split('\n'));
  }
}

if (hits.length === 0) {
  console.log(`claude-md-budget: ok (root ${kb(total)} of ${kb(FILE_LIMIT_BYTES)}; ${nestedCount} nested within ${kb(NESTED_LIMIT_BYTES)})`);
  process.exit(0);
}
console.error('claude-md-budget: over budget — every session pays for the root file, every session in a subtree for its nested one.');
for (const h of hits) console.error(`  - ${h}`);
console.error('Move text to its owner, never cut a rule: shipped narrative → the plan\'s Status; owed items → the owed ledger (CLAUDE.local.md names it);');
console.error('a rare-event runbook → a skill or dev-docs topic. Raising a limit is the user\'s decision.');
process.exit(1);
