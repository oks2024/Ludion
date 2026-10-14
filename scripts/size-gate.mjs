#!/usr/bin/env node
// Large-file gate — keeps build artifacts and stray oversized files out of the public git mirror.
//
// Publication decision 20 (private plan, `Plan-Private/`): every publishable file over 1 MiB must
// be named in a committed allowlist. Why: the ignore delta is meant to be a pure privacy boundary,
// not a size filter — but the accident this gate exists for was a path-less sweep that once
// committed 4961 build artifacts, 70 MB rlibs included, to that same mirror. An allowlist (not a
// raised limit) keeps every large file in the tree a deliberate, reviewed choice.
//
// The candidate set is the same one scripts/privacy-canary.mjs scans: `git ls-files -co
// --exclude-standard -z` (tracked plus untracked-not-ignored — the git mirror lags Lore by many
// files, so tracked-only would miss a new one) minus anything `git check-ignore --no-index --stdin
// -z` reports (tracked private paths — same reason as the canary's comment) minus paths missing
// from disk (deleted, not yet committed — nothing left to publish).
//
// Allowlist: scripts/size-allowlist.txt (committed, public) — one glob pattern per line, `#`
// comments and blank lines ignored, each pattern carrying a short comment for why the file belongs.
// `*` matches within one path segment, `**` matches across segments, `?` one character, forward
// slashes only. An allowlist entry that matches nothing over the line is a hit too, so the list
// can't rot into permission nobody uses.
//
// Usage:  node scripts/size-gate.mjs [--allowlist <path>] [--list]
//
//   --allowlist <path>   allowlist to use instead of scripts/size-allowlist.txt
//   --list               print every file over the line with its size and its matching entry (or
//                         "not allowlisted"); always exits 0 — for maintaining the list
//
// Exit 0 clean, 1 on any hit (an unallowlisted file over the line, or a stale entry), 2 on a usage
// error. Zero npm dependencies — Node built-ins only. Runs unchanged on Windows and macOS.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { parseArgs } from 'node:util';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DEFAULT_ALLOWLIST = 'scripts/size-allowlist.txt';
const MIB = 1024 * 1024;
const SIZE_LIMIT_BYTES = MIB; // decision 20's 1 MB line

// ── args ────────────────────────────────────────────────────────────────────
let values;
try {
  ({ values } = parseArgs({
    options: {
      allowlist: { type: 'string' },
      list: { type: 'boolean', default: false },
      help: { type: 'boolean', short: 'h', default: false },
    },
  }));
} catch (err) {
  console.error(`size-gate: ${err.message}`);
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

// ── glob matching ─────────────────────────────────────────────────────────────
// Hand-rolled rather than `path.matchesGlob`, whose stability varies across the Node versions a
// contributor may run. A `**` followed by `/` matches zero or more whole path segments, so one
// pattern covers both a direct and a nested match; a bare trailing `**` matches the rest of the path.
function globToRegExp(glob) {
  let re = '';
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === '*' && glob[i + 1] === '*') {
      if (glob[i + 2] === '/') { re += '(?:.*/)?'; i += 2; }
      else { re += '.*'; i += 1; }
    } else if (c === '*') {
      re += '[^/]*';
    } else if (c === '?') {
      re += '[^/]';
    } else if ('.+^${}()|[]\\'.includes(c)) {
      re += '\\' + c;
    } else {
      re += c;
    }
  }
  return new RegExp(`^${re}$`);
}
const regexCache = new Map();
function matchesGlob(file, pattern) {
  let re = regexCache.get(pattern);
  if (!re) { re = globToRegExp(pattern); regexCache.set(pattern, re); }
  return re.test(file);
}

// ── allowlist ───────────────────────────────────────────────────────────────
function loadAllowlist(rel) {
  const abs = path.resolve(REPO_ROOT, rel);
  if (!existsSync(abs)) {
    console.error(`size-gate: no allowlist at ${rel}`);
    process.exit(2);
  }
  const entries = [];
  const lines = readFileSync(abs, 'utf8').split('\n');
  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const hashIdx = trimmed.indexOf('#');
    const pattern = (hashIdx === -1 ? trimmed : trimmed.slice(0, hashIdx)).trim();
    if (pattern) entries.push({ pattern, line: i + 1 });
  }
  return entries;
}

// ── candidate files ───────────────────────────────────────────────────────────
function git(args, input) {
  const r = spawnSync('git', args, { cwd: REPO_ROOT, input, maxBuffer: 64 * 1024 * 1024 });
  if (r.error) {
    console.error(`size-gate: failed to spawn git: ${r.error.message}`);
    process.exit(2);
  }
  return r;
}

// Tracked plus untracked-not-ignored, minus tracked-but-ignored — the set a mirror commit could
// publish; identical to privacy-canary.mjs's publicFiles().
function publicFiles() {
  const ls = git(['ls-files', '-co', '--exclude-standard', '-z']);
  if (ls.status !== 0) {
    console.error(`size-gate: git ls-files failed:\n${ls.stderr}`);
    process.exit(2);
  }
  const candidates = ls.stdout.toString('utf8').split('\0').filter(Boolean);
  const ci = git(['check-ignore', '--no-index', '--stdin', '-z'], ls.stdout);
  if (ci.status !== 0 && ci.status !== 1) {
    console.error(`size-gate: git check-ignore failed:\n${ci.stderr}`);
    process.exit(2);
  }
  const ignored = new Set(ci.stdout.toString('utf8').split('\0').filter(Boolean));
  return candidates.filter((f) => !ignored.has(f));
}

function overLineFiles() {
  const files = publicFiles();
  const over = [];
  for (const f of files) {
    let st;
    try { st = statSync(path.join(REPO_ROOT, f)); } catch { continue; } // missing from disk
    if (!st.isFile() || st.size <= SIZE_LIMIT_BYTES) continue;
    over.push({ file: f, size: st.size });
  }
  over.sort((a, b) => a.file.localeCompare(b.file));
  return { files, over };
}

const mb = (bytes) => (bytes / MIB).toFixed(2);

// ── main ──────────────────────────────────────────────────────────────────────
const allowlistRel = values.allowlist ?? DEFAULT_ALLOWLIST;
const entries = loadAllowlist(allowlistRel);
const { files, over } = overLineFiles();

if (values.list) {
  for (const { file, size } of over) {
    const hit = entries.find((e) => matchesGlob(file, e.pattern));
    console.log(`${file}: ${mb(size)} MB — ${hit ? hit.pattern : 'not allowlisted'}`);
  }
  console.log(`size-gate: ${over.length} file(s) over the line, ${files.length} checked`);
  process.exit(0);
}

const used = new Set();
const overLineHits = [];
for (const { file, size } of over) {
  const entry = entries.find((e) => matchesGlob(file, e.pattern));
  if (entry) { used.add(entry); continue; }
  overLineHits.push(`${file}: ${mb(size)} MB — over the 1 MB line and not in ${allowlistRel}`);
}
const staleHits = entries
  .filter((e) => !used.has(e))
  .map((e) => `${allowlistRel}:${e.line}: entry matches no file over the line — delete it`);

const hits = [...overLineHits, ...staleHits];
if (hits.length) {
  for (const h of hits) console.log(h);
  console.log(`size-gate: ${hits.length} hit(s) — files over the 1 MB line must be named in ${allowlistRel}`);
  process.exit(1);
}
console.log(`size-gate: ${files.length} files checked, ${over.length} allowlisted over the line, clean`);
