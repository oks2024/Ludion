#!/usr/bin/env node
// The schedule-comment lint: CLAUDE.md says "a comment describes state, never
// schedule" — no "this wave", "lands in Wave 3", "a later phase", "(M1)". Sequencing belongs in `Plan/`;
// a comment that dates itself is stale the day the plan moves. This gate fails on schedule phrasing in
// source, docs and user-facing strings.
//
// Four rules, each tuned against the tree (one example each):
//   current-wave      "this wave", "the wave's exit criterion", "a later wave"
//   later-phase       "a later phase", "in later milestones", "this milestone", "a future editor"
//   milestone-tag     "(M1)", "until M2"                                    (case-sensitive)
//   forward-wave      "Wave 4 will emit", "lands in Wave 3", "until W2"     (case-sensitive)
//
// Deliberately NOT banned: bare provenance citations — "(Phase 8 Wave 3)", "`Plan/x.md` Wave 4", fixture
// names such as `gi-wave2`. They name where something shipped, not when something will. "phase" alone is
// a domain word (`Phase::Sim`, particle phases) and "wave" alone is vigil's spawn wave, so neither is
// banned bare; "a future <noun>" is banned only for a closed list of nouns.
//
// Scope: `git ls-files -co --exclude-standard` filtered to the source/doc extensions under crates/,
// games/, the editor's src and src-tauri, scripts/, ci/scripts/, docs/, dev-docs/, examples/, budgets/,
// fuzz/ and .github/. `Plan/`, `Plan-Private/`, `.exarch/`, the root design docs and `ci/teamcity/` are
// historical records or private and sit outside by construction; `embedded-docs/` mirrors `docs/`, and
// generated files are skipped.
//
// Allowlist (path + needle; `--help` prints the reasons): a doc comment that feeds the schema hash, and
// the gate's own files, which name the phrases.
//
// Every hit is reported as `path:line: [rule] snippet`. Mirrors the `schedule comments` step in
// `.github/workflows/ci.yml` and `schedule-comments` in `scripts/check.mjs`.
//
// Usage:  node ci/scripts/schedule-comments.mjs [--help]
//
// Exit 0 clean, 1 on any hit (or when the walk saw no files). Zero npm dependencies — Node built-ins only.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

export const RULES = [
  { name: 'current-wave', pattern: /\b(?:this|next|that|the current|a later|an earlier|the previous) wave\b|\bthe wave['’]s\b/gi },
  {
    name: 'later-phase',
    pattern: /\ba later (?:phase|milestone)\b|\b(?:in|for|until) later (?:waves|phases|milestones)\b|\bthis milestone(?:['’]s)?\b|\ba future (?:wave|phase|milestone|editor|asset watcher|consumer)\b/gi,
  },
  { name: 'milestone-tag', pattern: /\(M[0-9]+\)|\b(?:in|until|after|before|for) M[0-9]+\b/g },
  {
    name: 'forward-wave',
    pattern: /\b(?:Wave|W) ?[0-9]+[a-z]? (?:will|lands|adds)\b|\b(?:lands|ships|arrives) in (?:Wave|W) ?[0-9]+|\buntil (?:Wave|W) ?[0-9]+/g,
  },
];

export const ALLOWLIST = [
  { file: 'crates/ludion-protocol/src/response.rs', needle: 'in M1.', reason: 'the doc feeds the schema hash; restated at the next schema bump' },
  { file: 'ci/scripts/schedule-comments.mjs', needle: null, reason: 'names the banned phrases' },
  { file: 'ci/scripts/gate-scripts.test.mjs', needle: null, reason: 'names the banned phrases' },
];

const EXTENSIONS = new Set(['.rs', '.ts', '.tsx', '.wgsl', '.mjs', '.js', '.md', '.toml', '.yml', '.yaml', '.sh', '.ps1']);
const ROOTS = ['crates/', 'games/', 'editor/app/src/', 'editor/app/src-tauri/', 'scripts/', 'ci/scripts/', 'docs/', 'dev-docs/', 'examples/', 'budgets/', 'fuzz/', '.github/'];
const SKIPPED = ['crates/ludion-cli/embedded-docs/', 'editor/protocol/src/protocol.d.ts', '/node_modules/', '/target/'];

// Every schedule phrase in `text`, with its 1-based line, rule name and the matched snippet.
export function findScheduleText(text) {
  const hits = [];
  for (const { name, pattern } of RULES) {
    for (const match of text.matchAll(pattern)) {
      const line = text.slice(0, match.index).split('\n').length;
      hits.push({ line, rule: name, snippet: match[0] });
    }
  }
  return hits.sort((a, b) => a.line - b.line);
}

// Every file the lint reads under `repoRoot`, repo-relative with forward slashes.
export function scanFiles(repoRoot) {
  const ls = spawnSync('git', ['ls-files', '-co', '--exclude-standard', '-z'], { cwd: repoRoot, encoding: 'utf8', maxBuffer: 1 << 28 });
  if (ls.status !== 0) throw new Error(`git ls-files failed: ${ls.stderr}`);
  return ls.stdout
    .split('\0')
    .filter((rel) => rel && EXTENSIONS.has(path.extname(rel)))
    .filter((rel) => ROOTS.some((r) => rel.startsWith(r)))
    .filter((rel) => !SKIPPED.some((s) => rel.includes(s) || rel.endsWith(s)))
    .filter((rel) => !path.basename(rel).startsWith('shader_gen.'))
    .filter((rel) => existsSync(path.join(repoRoot, rel)));
}

// The hits in `text` of repo-relative file `rel` that the allowlist does not suppress.
export function unallowedHits(rel, text) {
  const entries = ALLOWLIST.filter((e) => e.file === rel);
  if (entries.some((e) => e.needle === null)) return [];
  const lines = text.split('\n');
  return findScheduleText(text).filter(({ line }) => !entries.some((e) => lines[line - 1].includes(e.needle)));
}

function main() {
  if (process.argv.includes('--help')) {
    console.log('Usage: node ci/scripts/schedule-comments.mjs   (rules and scope: see the header comment)\nAllowlist:');
    for (const e of ALLOWLIST) console.log(`  ${e.file}${e.needle ? ` / ${e.needle}` : ''} — ${e.reason}`);
    return;
  }
  const failures = [];
  const files = scanFiles(REPO_ROOT);
  for (const rel of files) {
    const text = readFileSync(path.join(REPO_ROOT, rel), 'utf8').replace(/\r\n/g, '\n');
    for (const { line, rule, snippet } of unallowedHits(rel, text)) failures.push(`${rel}:${line}: [${rule}] ${snippet}`);
  }
  if (files.length === 0) failures.push('the walk saw no files');
  for (const failure of failures) console.error(failure);
  if (failures.length > 0) console.error('schedule-comments: restate as state; keep plan pointers without the schedule tag.');
  else console.log(`schedule-comments: ok (${files.length} files)`);
  process.exit(failures.length === 0 ? 0 : 1);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
