#!/usr/bin/env node
// Privacy canary — the gate that keeps private facts out of the public git mirror.
//
// The publication boundary is a path boundary (`.gitignore` minus `.loreignore`, see the header
// of either file), but the leak class actually found in the tree was PROSE: a server address in
// a notebook, a personal path in a plan, a CI URL in a commit message. This gate greps for those.
//
// Token list: `ci/teamcity/privacy-tokens.txt` (a private path, so a public clone has none) —
// one token per line, `#` comments and blank lines ignored, case-insensitive plain substring
// matching, no regex. Absent list with no `ci/teamcity/` ⇒ "skipped", exit 0: a public clone cannot
// leak what it was never given, and the gate must not turn red there. Absent list with
// `ci/teamcity/` present (a Lore checkout) ⇒ exit 3: the private paths are here, so a skip would
// pass a scan that never ran (distinct from exit 1, a hit, so a caller never tells the user to
// reword). An explicit `--tokens` path that does not exist ⇒ exit 2.
//
// Tree mode (default): the public set is `git ls-files -co --exclude-standard` minus everything
// `git check-ignore` reports — tracked or untracked, and not ignored — which is everything a
// mirror commit could publish. Untracked files count because the mirror lags Lore: a file Lore
// already holds is untracked in git until the next snapshot writes it into public history.
// Binary files are skipped by extension, then by a NUL in the first 8 KB. A hit prints
// `path:line: token <token>` — the token, never the line, which may hold the secret twice over.
//
// Message mode: `--message <text>` / `--message-file <path>` scans one commit message the same
// way; `scripts/commit.mjs` runs it before staging.
//
// Usage:  node scripts/privacy-canary.mjs [--tokens <path>]
//         node scripts/privacy-canary.mjs --message <text> | --message-file <path>
//         node scripts/privacy-canary.mjs --staged
//
//   --tokens <path>         token list to use instead of ci/teamcity/privacy-tokens.txt
//   --message <text>        scan this text instead of the tree
//   --message-file <path>   scan this file's content instead of the tree
//   --staged                scan only the files staged in git's index (what `commit.mjs --mirror-only`
//                           is about to publish) instead of the tree
//
// Exit 0 clean or skipped (public clone), 1 on any hit, 2 on a usage error, 3 on a missing list on
// a private checkout. Output is sorted by path.
// Zero npm dependencies — Node built-ins only. Runs unchanged on Windows and macOS.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { parseArgs } from 'node:util';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DEFAULT_TOKENS = 'ci/teamcity/privacy-tokens.txt';
const PRIVATE_DIR = 'ci/teamcity';
const BINARY_EXTENSIONS = new Set([
  '.png', '.qoi', '.lz4', '.rgba', '.lrp', '.wav', '.mp4', '.ico', '.icns', '.zst', '.gz', '.woff', '.woff2',
]);
const BINARY_SNIFF_BYTES = 8 * 1024;
const READ_BATCH = 64;

// ── args ────────────────────────────────────────────────────────────────────
let values;
try {
  ({ values } = parseArgs({
    options: {
      tokens: { type: 'string' },
      message: { type: 'string' },
      'message-file': { type: 'string' },
      staged: { type: 'boolean', default: false },
      help: { type: 'boolean', short: 'h', default: false },
    },
  }));
} catch (err) {
  console.error(`privacy-canary: ${err.message}`);
  process.exit(2);
}

if (values.help) {
  const help = [];
  for (const l of readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n')) {
    if (l.startsWith('#!')) continue;
    if (l.startsWith('//')) help.push(l.slice(3));
    else if (help.length) break;
  }
  console.log(help.join('\n'));
  process.exit(0);
}
if (values.message !== undefined && values['message-file'] !== undefined) {
  console.error('privacy-canary: --message and --message-file are mutually exclusive');
  process.exit(2);
}

// ── token list ──────────────────────────────────────────────────────────────
const tokensRel = values.tokens ?? DEFAULT_TOKENS;
const tokensPath = path.resolve(REPO_ROOT, tokensRel);
if (!existsSync(tokensPath)) {
  if (values.tokens !== undefined) {
    console.error(`privacy-canary: no token list at ${tokensRel}`);
    process.exit(2);
  }
  if (existsSync(path.join(REPO_ROOT, PRIVATE_DIR))) {
    console.error(`privacy-canary: ${PRIVATE_DIR}/ exists but the token list ${DEFAULT_TOKENS} is missing — a private checkout must not skip the scan; restore the list`);
    process.exit(3);
  }
  console.log(`privacy-canary: no token list at ${tokensRel} — skipped (public clone)`);
  process.exit(0);
}
const tokens = readFileSync(tokensPath, 'utf8')
  .split('\n')
  .map((l) => l.trim())
  .filter((l) => l && !l.startsWith('#'))
  .map((token) => ({ token, lower: token.toLowerCase() }));
if (!tokens.length) {
  console.log(`privacy-canary: token list ${tokensRel} is empty — nothing to scan for`);
  process.exit(0);
}

// ── scanning ────────────────────────────────────────────────────────────────
// Whole-text substring test first, so the per-line walk only runs for a file that actually
// contains a token — the common case is one `includes` per token per file.
function scanText(text) {
  const lower = text.toLowerCase();
  const present = tokens.filter((t) => lower.includes(t.lower));
  if (!present.length) return [];
  const hits = [];
  const lines = lower.split('\n');
  for (let i = 0; i < lines.length; i++) {
    for (const t of present) {
      if (lines[i].includes(t.lower)) hits.push({ line: i + 1, token: t.token });
    }
  }
  return hits;
}

function isBinary(file, buf) {
  if (BINARY_EXTENSIONS.has(path.extname(file).toLowerCase())) return true;
  return buf.subarray(0, BINARY_SNIFF_BYTES).includes(0);
}

function git(args, input) {
  const r = spawnSync('git', args, { cwd: REPO_ROOT, input, maxBuffer: 64 * 1024 * 1024 });
  if (r.error) {
    console.error(`privacy-canary: failed to spawn git: ${r.error.message}`);
    process.exit(2);
  }
  return r;
}

// Tracked-or-untracked minus ignored. `--no-index` is load-bearing: without it check-ignore never
// reports a tracked path, and the private block in `.gitignore` covers paths the current mirror
// still tracks. Exit 1 from check-ignore means "nothing ignored", not an error.
function stagedFiles() {
  const r = git(['diff', '--cached', '--name-only', '-z', '--diff-filter=ACMR']);
  if (r.status !== 0) {
    console.error(`privacy-canary: git diff --cached failed:\n${r.stderr}`);
    process.exit(2);
  }
  return r.stdout.toString('utf8').split('\0').filter(Boolean);
}

function publicFiles() {
  const ls = git(['ls-files', '-co', '--exclude-standard', '-z']);
  if (ls.status !== 0) {
    console.error(`privacy-canary: git ls-files failed:\n${ls.stderr}`);
    process.exit(2);
  }
  const tracked = ls.stdout.toString('utf8').split('\0').filter(Boolean);
  const ci = git(['check-ignore', '--no-index', '--stdin', '-z'], ls.stdout);
  if (ci.status !== 0 && ci.status !== 1) {
    console.error(`privacy-canary: git check-ignore failed:\n${ci.stderr}`);
    process.exit(2);
  }
  const ignored = new Set(ci.stdout.toString('utf8').split('\0').filter(Boolean));
  return tracked.filter((f) => !ignored.has(f));
}

async function scanTree() {
  const files = (values.staged ? stagedFiles() : publicFiles()).sort();
  const hits = [];
  let scanned = 0;
  for (let i = 0; i < files.length; i += READ_BATCH) {
    const batch = files.slice(i, i + READ_BATCH);
    const bufs = await Promise.all(batch.map((f) => readFile(path.join(REPO_ROOT, f)).catch(() => null)));
    for (let j = 0; j < batch.length; j++) {
      const buf = bufs[j];
      // A tracked file missing from the working tree (deleted, not yet committed) has nothing
      // to publish.
      if (!buf || isBinary(batch[j], buf)) continue;
      scanned++;
      for (const h of scanText(buf.toString('utf8'))) hits.push({ file: batch[j], ...h });
    }
  }
  return { hits, scanned };
}

function report(hits, label) {
  for (const h of hits) console.log(`${h.file}:${h.line}: token <${h.token}>`);
  console.log(`privacy-canary: ${hits.length} hit(s) in ${label} — private tokens must not reach the public mirror`);
  process.exit(1);
}

if (values.message !== undefined || values['message-file'] !== undefined) {
  const text = values.message ?? readFileSync(path.resolve(values['message-file']), 'utf8');
  const hits = scanText(text).map((h) => ({ file: 'message', ...h }));
  if (hits.length) report(hits, 'the commit message');
  console.log('privacy-canary: message clean');
  process.exit(0);
}

const { hits, scanned } = await scanTree();
if (hits.length) report(hits, `${scanned} scanned files`);
console.log(`privacy-canary: ${scanned} files scanned, clean`);
