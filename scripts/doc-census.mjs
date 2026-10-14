#!/usr/bin/env node
// Token census for the agent-docs migration (Plan/… doc rework, wave 0 onward).
//
// The rework moves and then condenses ~630 KB of markdown across `docs/` and `dev-docs/` over
// several waves. A diff review catches a paragraph vanishing; it does not reliably catch a single
// measured number or diagnostic code quietly evaporating three waves later, buried inside a
// rewritten paragraph. This script extracts every LOAD-BEARING token (diagnostic codes, source
// paths, shell commands, measured numbers, headings) from a set of markdown files so that loss is
// a mechanical diff instead of a careful read.
//
// Usage:
//   node scripts/doc-census.mjs <path...>                    human-readable census, by class
//   node scripts/doc-census.mjs <path...> --json             machine-readable, sorted, stable keys
//   node scripts/doc-census.mjs <path...> --baseline <f.json> regression check: exits non-zero and
//                                                              lists any baseline token absent now
//
// Migration workflow: before a wave touches a doc, snapshot it —
//   node scripts/doc-census.mjs docs dev-docs --json > before.json
// — do the edit, then gate the wave on:
//   node scripts/doc-census.mjs docs dev-docs --baseline before.json
//
// A path may be a file or a directory; a directory scans its `*.md` files non-recursively (the
// same shape both doc roots use — no nested topic directories). Zero npm dependencies, Node
// built-ins only, matching scripts/check.mjs's house style.

import { readFileSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';

// ── args ────────────────────────────────────────────────────────────────────
let values, positionals;
try {
  ({ values, positionals } = parseArgs({
    options: {
      json: { type: 'boolean', default: false },
      baseline: { type: 'string' },
      help: { type: 'boolean', short: 'h', default: false },
    },
    allowPositionals: true,
  }));
} catch (err) {
  console.error(`doc-census: ${err.message}`);
  process.exit(2);
}

if (values.help || positionals.length === 0) {
  console.log([
    'Usage: node scripts/doc-census.mjs <path...> [--json] [--baseline <file.json>]',
    '',
    '  <path...>            markdown files or directories (directories scan *.md, non-recursive)',
    '  --json               machine-readable output, sorted, stable key order',
    '  --baseline <file>    regression check: report+exit non-zero on any baseline token now missing',
  ].join('\n'));
  process.exit(values.help ? 0 : 2);
}

// ── token classes ─────────────────────────────────────────────────────────────
// Fixed display order for the human-readable report; JSON output sorts class keys alphabetically
// instead (see `toSortedObject`), so the two forms deliberately use different, both-deterministic
// orderings.
const CLASS_ORDER = ['code', 'path', 'command', 'measure', 'heading'];
const CLASS_LABEL = {
  code: 'diagnostic codes',
  path: 'source paths',
  command: 'shell commands',
  measure: 'measured numbers',
  heading: 'headings',
};

const CODE_RE = /\bL\d{4}\b/g;
// `Plan` is tracked because a doc citing a Plan file is citing the decision record for a rule that
// otherwise reads as an arbitrary omission — exactly the kind of load-bearing pointer a condensing
// pass drops without noticing.
const PATH_RE = /\b(?:crates|games|examples|editor|scripts|budgets|ci|docs|dev-docs|Plan)\/[\w./-]+/g;
// Inline single-backtick spans only — triple-backtick fences delimit blocks, not commands, and are
// handled separately (and only for heading suppression, per the spec below).
const BACKTICK_RE = /`([^`\n]+)`/g;
const COMMAND_PREFIXES = ['ludion ', 'cargo ', 'node '];
const UNIT_RE = /\d+(?:\.\d+)?\s*(?:ms|fps|px|KB|MB|GB|%)\b/g;
const MULT_RE = /\d+(?:\.\d+)?×/g;

// Extracts every load-bearing token from `text`, grouped by class, as arrays (with repeats — the
// caller tallies counts). Every class but `heading` scans the WHOLE file, fenced code blocks
// included: a `cargo test` inside a fenced example is still a real command, and a measured number
// in a YAML comment is still a real measurement.
function extractTokens(text) {
  const out = { code: [], path: [], command: [], measure: [], heading: [] };

  for (const m of text.matchAll(CODE_RE)) out.code.push(m[0]);
  for (const m of text.matchAll(PATH_RE)) out.path.push(m[0]);
  for (const m of text.matchAll(UNIT_RE)) out.measure.push(normalizeWhitespace(m[0]));
  for (const m of text.matchAll(MULT_RE)) out.measure.push(m[0]);

  for (const m of text.matchAll(BACKTICK_RE)) {
    const span = normalizeWhitespace(m[1].trim());
    if (COMMAND_PREFIXES.some((p) => span.startsWith(p))) out.command.push(span);
  }

  out.heading.push(...extractHeadings(text));
  return out;
}

function normalizeWhitespace(s) {
  return s.replace(/\s+/g, ' ');
}

// ATX headings, skipping fenced code blocks — mirrors `extract_section`'s fence-toggling scan in
// crates/ludion-cli/src/docs.rs line for line: the engine guide's YAML examples carry `#`-prefixed
// comments that would otherwise misread as headings. A fence opens on a line trimmed-of-leading-
// whitespace starting with ``` or ~~~, and closes on the next line starting with that SAME marker
// (matching docs.rs, which does not re-check fence length either).
function extractHeadings(text) {
  const headings = [];
  let fence = null;
  for (const raw of text.split('\n')) {
    const trimmed = raw.trimStart();
    if (fence !== null) {
      if (trimmed.startsWith(fence)) fence = null;
      continue;
    }
    if (trimmed.startsWith('```')) {
      fence = '```';
      continue;
    }
    if (trimmed.startsWith('~~~')) {
      fence = '~~~';
      continue;
    }
    let hashes = 0;
    while (hashes < trimmed.length && trimmed[hashes] === '#') hashes++;
    if (hashes >= 1 && hashes <= 6 && trimmed[hashes] === ' ') {
      headings.push(trimmed.slice(hashes + 1).trim());
    }
  }
  return headings;
}

// ── file/directory resolution ──────────────────────────────────────────────────
// Directories scan their `*.md` files non-recursively — the shape both `docs/` and `dev-docs/`
// actually have (flat topic files, no nested subdirectories).
function resolveMarkdownFiles(inputPaths) {
  const files = [];
  for (const p of inputPaths) {
    const stat = statSync(p);
    if (stat.isDirectory()) {
      for (const entry of readdirSync(p)) {
        if (!entry.endsWith('.md')) continue;
        const full = path.join(p, entry);
        if (statSync(full).isFile()) files.push(full);
      }
    } else {
      files.push(p);
    }
  }
  files.sort();
  return [...new Set(files)];
}

// ── census ──────────────────────────────────────────────────────────────────────
// Builds { class: { token: count } }, every class's tokens deduped and sorted.
function census(files) {
  const counts = Object.fromEntries(CLASS_ORDER.map((c) => [c, new Map()]));
  for (const file of files) {
    const text = readFileSync(file, 'utf8');
    const tokens = extractTokens(text);
    for (const cls of CLASS_ORDER) {
      for (const tok of tokens[cls]) {
        counts[cls].set(tok, (counts[cls].get(tok) ?? 0) + 1);
      }
    }
  }
  const result = {};
  for (const cls of CLASS_ORDER) {
    result[cls] = Object.fromEntries([...counts[cls].entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)));
  }
  return result;
}

// Same data, class keys AND token keys sorted alphabetically — the `--json` shape, so a byte diff
// between two runs is meaningful and never an artifact of Map/object insertion order.
function toSortedObject(result) {
  const out = {};
  for (const cls of Object.keys(result).sort()) {
    const tokens = result[cls];
    const sortedTokens = {};
    for (const tok of Object.keys(tokens).sort()) sortedTokens[tok] = tokens[tok];
    out[cls] = sortedTokens;
  }
  return out;
}

// ── reporting ───────────────────────────────────────────────────────────────────
function printCensus(result) {
  for (const cls of CLASS_ORDER) {
    const tokens = result[cls];
    const keys = Object.keys(tokens);
    if (keys.length === 0) continue;
    const total = keys.reduce((sum, k) => sum + tokens[k], 0);
    console.log(`${CLASS_LABEL[cls].toUpperCase()} (${keys.length} unique, ${total} total)`);
    for (const k of keys) console.log(`  ${String(tokens[k]).padStart(4)}  ${k}`);
    console.log('');
  }
}

// Every token in `baseline` absent from `current` (presence only — counts dropping is not itself a
// regression, a token disappearing entirely is). Returns { class: [token...] }, only classes with
// at least one miss.
function missingTokens(baseline, current) {
  const missing = {};
  for (const cls of Object.keys(baseline)) {
    const baseTokens = Object.keys(baseline[cls] ?? {});
    const curTokens = new Set(Object.keys(current[cls] ?? {}));
    const gone = baseTokens.filter((t) => !curTokens.has(t)).sort();
    if (gone.length) missing[cls] = gone;
  }
  return missing;
}

// ── main ──────────────────────────────────────────────────────────────────────
function main() {
  const files = resolveMarkdownFiles(positionals);
  const result = census(files);

  if (values.baseline) {
    const baseline = JSON.parse(readFileSync(values.baseline, 'utf8'));
    const missing = missingTokens(baseline, result);
    const classes = Object.keys(missing);
    if (classes.length === 0) {
      if (values.json) console.log(JSON.stringify({}, null, 2));
      else console.log('doc-census: no baseline tokens missing.');
      process.exit(0);
    }
    if (values.json) {
      console.log(JSON.stringify(missing, null, 2));
    } else {
      console.log('doc-census: tokens present in the baseline but missing from the current scan:\n');
      for (const cls of CLASS_ORDER) {
        if (!missing[cls]) continue;
        console.log(`${CLASS_LABEL[cls].toUpperCase()} (${missing[cls].length} missing)`);
        for (const tok of missing[cls]) console.log(`  ${tok}`);
        console.log('');
      }
    }
    process.exit(1);
  }

  if (values.json) {
    console.log(JSON.stringify(toSortedObject(result), null, 2));
  } else {
    printCensus(result);
  }
  process.exit(0);
}

main();
