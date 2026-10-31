#!/usr/bin/env node
// ludionCommit — the one commit verb: Lore is the source of truth, git its public mirror.
//
// Lore holds everything; git is a derived, write-only snapshot mirror of the public paths, pushed
// to `origin`'s `main` (decisions 1 and 5 of the private publication plan in `Plan-Private/`; the
// Lore setup is in CLAUDE.md §Source control). Never author git by hand — no `git commit`, no
// branches; this wrapper writes every mirror commit, and each carries a `[lore <rev#> | <signature>]`
// trailer. A commit on origin/main newer than its newest trailer commit is a contribution — a
// merged PR awaiting `--import`.
//
// A commit, in order:
//   0. privacy canary over the message (`scripts/privacy-canary.mjs --message-file`) — the
//      commit message is published verbatim by the git mirror, so a private token in it aborts
//      before anything is staged; no token list (a public clone) is a pass
//   1. contribution check — fetch origin/main; while it holds contributions the commit is refused
//      (exit 2, nothing staged), so a snapshot never lands over a PR Lore has not seen. An
//      unreachable remote warns and proceeds: Lore is never blocked by the public host
//   2. `lore status` preflight — refuses to touch an unresolved merge, refuses a staged set this
//      commit did not make (another instance's staging, or a failed run's leftovers: exit 2,
//      nothing touched), and syncs first when the remote has moved, so a commit is never
//      authored on a stale parent
//   3. `lore stage --scan <paths>` — stage adds/mods/deletions (bulk-staging guard below; output
//      it cannot read fails the guard closed). A failure after staging unstages only this
//      commit's own entries — `lore unstage .` is for `--all` alone
//   4. `lore commit <message>`     — the real commit; failure aborts everything
//   5. `lore push`                 — a failure is LOUD (exit 1), because a silently unpushed
//                                    revision is what lets the next commit widen the divergence
//   6. mirror commit               — the same message plus the trailer, staging only this
//                                    commit's paths (a git index already holding other staging
//                                    refuses it, untouched); built on a freshly fetched origin/main and
//                                    pushed, rebuilt on the new tip when the push is rejected
//                                    (3 attempts). A mirror that did not reach the remote is loud
//                                    (exit 1); its commit stays local and the next run carries it
//
// `--import` (decision 6): the same Lore preflight; refuses unless every path the contributions
// touched is byte-identical in the working tree to the newest mirror commit's version (or already
// to the contribution's, so a re-run is safe); writes the contributed bytes; one Lore commit
// titled by origin/main's tip subject, with a `Co-Authored-By:` per contributor and a `[git <sha>]`
// line; pushes Lore; then pushes an import marker — a mirror commit carrying the new revision's
// trailer — so step 1 finds nothing left to import.
//
// The tree itself is not scanned here — that is `scripts/check.mjs`'s `privacy` gate.
//
// Manual `lore commit`s (outside this wrapper) are legitimate — the wrapper is a convenience,
// not a gate — but they leave the git mirror behind. `--all` on a clean tree then creates a
// catch-up snapshot commit collapsing the manual revisions instead of failing.
//
// WHY THE PREFLIGHT AND `--all` EXIST (all four fired for real on 2026-07-29/30):
//   * No sync before staging meant every commit raced the other machine and *created* the
//     divergence its push then failed on. Syncing first is prevention instead of repair.
//   * A push failure that only warned let a second commit stack on the same stale parent, so one
//     repair became two.
//   * Staging over a pending merge cannot assert resolution (that is `lore branch merge
//     resolve`'s job) and risks burying one — a merge is repaired by hand, never through here.
//   * Path-less staging in a checkout several instances share is a footgun, not a convenience: one
//     run swept 4961 build artifacts (a nested `.build/` an anchored ignore rule missed, incl. a
//     22 MB exe) plus another instance's in-progress file into the history.
//
// WHY THE MIRROR IS BUILT IN THE INDEX: the working tree holds other instances' uncommitted files,
// so git never writes it — no merge, checkout or pull. `reset --soft` + `read-tree` put the index
// on origin/main, and only this commit's paths are staged from the working tree. Earlier mirror
// commits that never reached the remote collapse into it from their own trees (`Also carries:`
// lines keep their trailers), except where origin/main holds a later Lore revision of a path.
//
// Usage:  node scripts/commit.mjs "<message>" <paths...> [--no-push] [--all] [--force]
//         node scripts/commit.mjs --mirror-only "<message>" <paths...> [--paths-file <file>] [--no-push]
//         node scripts/commit.mjs --import
//
//   <message>   commit message (multi-line ok); required first argument
//   <paths...>  stage only these repo-relative paths; REQUIRED unless --all
//   --no-push   commit locally: skip `lore push` and the mirror push
//   --all       scan the whole tree instead of named paths (also the mirror catch-up mode).
//               Deliberate by design: several instances share this checkout, so "everything
//               dirty" is rarely one coherent change
//   --force     proceed past the bulk-staging guard (a staged set this large is usually an
//               ignore-rule hole, not an intended commit)
//   --import    bring origin/main's contributions into Lore; takes no message, paths or flags
//   --mirror-only  write ONLY the git mirror commit for the named paths, with the current Lore
//               revision in the trailer: no Lore stage, commit or push. For repairing a mirror
//               that lags Lore (`node scripts/mirror-gap.mjs` lists the gap). Refused when no
//               path is named, with --all, for a path under a private path (`.gitignore` minus
//               `.loreignore`), for a path `lore status --scan` lists as changed (it has no Lore
//               revision to mirror yet), or when the privacy canary hits the staged files or the
//               message. A named path git tracks but that is gone from disk is staged as a deletion
//   --paths-file <file>  with --mirror-only: more paths, one per line (a long list overflows argv)
//
// A private path is never mirrored: a named path under one is skipped, and --all or a named parent
// leaves the private content out. Only --mirror-only also untracks the private paths git still
// holds (`git rm --cached`; the files stay on disk and in Lore).
//
// Without an `origin` remote the mirror is local: commits land on the checked-out branch (`main`,
// or `master`) and nothing is fetched or pushed.
//
// Zero npm dependencies — Node built-ins only. Runs unchanged on Windows and macOS.

import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { writeFileAtomic } from './atomic-write.mjs';
import { foreignIndexPaths, foreignStaged, ownUnstageTargets, parseStagedCount, parseStagedSet } from './commit-lib.mjs';
import { isPrivate, overlaps, parseLoreStatus, privatePaths } from './mirror-lib.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const REMOTE = 'origin';
const MIRROR_BRANCH = 'main';
const REMOTE_MAIN = `refs/remotes/${REMOTE}/${MIRROR_BRANCH}`;
const PUSH_ATTEMPTS = 3;
const MAX_BUFFER = 1024 * 1024 * 1024;
// Every trailer in a message, a collapsed commit's `Also carries:` lines included.
const TRAILER = /\[lore (\d+) \| ([0-9a-f]{64})\]/g;
// The same pattern as a POSIX ERE for `git log --grep` (bracket expressions instead of escapes).
const TRAILER_ERE = '[[]lore [0-9]+ [|] [0-9a-f]{64}[]]';

// ── args ────────────────────────────────────────────────────────────────────
let values, positionals;
try {
  ({ values, positionals } = parseArgs({
    options: {
      'no-push': { type: 'boolean', default: false },
      all: { type: 'boolean', default: false },
      force: { type: 'boolean', default: false },
      import: { type: 'boolean', default: false },
      'mirror-only': { type: 'boolean', default: false },
      'paths-file': { type: 'string' },
      help: { type: 'boolean', short: 'h', default: false },
    },
    allowPositionals: true,
  }));
} catch (err) {
  console.error(`commit: ${err.message}`);
  process.exit(2);
}

if (!values.help && values['mirror-only'] && positionals.length === 0) {
  console.error('commit: --mirror-only needs a message and the paths to mirror.');
  process.exit(2);
}

if (values.help || (positionals.length === 0 && !values.import)) {
  const help = [];
  for (const l of readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n')) {
    if (l.startsWith('#!')) continue;
    if (l.startsWith('//')) help.push(l.slice(3));
    else if (help.length) break;
  }
  console.log(help.join('\n'));
  process.exit(values.help ? 0 : 2);
}

if (values.import && (positionals.length || values.all || values.force || values['no-push'])) {
  console.error('commit: --import takes no message, paths or other flags — its paths are the ones the');
  console.error('commit: contributions touched, and its message is built from their commits.');
  process.exit(2);
}

const [message, ...paths] = positionals;

if (values['paths-file'] !== undefined) {
  if (!values['mirror-only']) {
    console.error('commit: --paths-file is only valid with --mirror-only.');
    process.exit(2);
  }
  paths.push(...readFileSync(values['paths-file'], 'utf8').split(/\r?\n/).map((l) => l.trim()).filter(Boolean));
}

if (values['mirror-only']) {
  const clash = values.import ? '--import' : values.all ? '--all' : values.force ? '--force' : null;
  if (clash) {
    console.error(`commit: --mirror-only cannot be combined with ${clash}.`);
    process.exit(2);
  }
  if (!paths.length) {
    console.error('commit: --mirror-only needs the paths to mirror — name them (or --paths-file).');
    process.exit(2);
  }
}

// Explicit paths, unless the whole tree is asked for by name. Several instances share this
// checkout and always have unrelated work in it, so a path-less sweep commits other people's
// half-finished edits and anything an ignore rule failed to catch.
if (!values.import && !paths.length && !values.all) {
  console.error('commit: no paths given. Name the paths this change touches:');
  console.error('commit:   node scripts/commit.mjs "<message>" <paths...>');
  console.error('commit: or pass --all to deliberately scan the whole tree (also the mode that');
  console.error('commit: catches the git mirror up after a manual `lore commit`). Path-less runs');
  console.error('commit: are refused because this checkout is shared: `lore stage --scan .` sweeps');
  console.error("commit: every instance's uncommitted work, plus anything the ignore rules miss.");
  process.exit(2);
}

// ── runner ──────────────────────────────────────────────────────────────────
// No shell: the message is arbitrary text and must reach lore/git as one argv token.
// Output is echoed (unless quiet) AND captured (the Lore revision/signature are parsed from it).
function run(cmd, args, { quiet = false, input } = {}) {
  const r = spawnSync(cmd, args, { cwd: REPO_ROOT, encoding: 'utf8', input, maxBuffer: MAX_BUFFER });
  if (r.error) {
    console.error(`commit: failed to spawn ${cmd}: ${r.error.message}`);
    process.exit(2);
  }
  const stdout = r.stdout ?? '';
  const output = stdout + (r.stderr ?? '');
  if (!quiet) process.stdout.write(output);
  return { code: r.status ?? 1, output, stdout };
}

// LUDION_COMMIT_LORE_SHIM names a `.mjs` that stands in for `lore` (the seam `scripts/commit.test.mjs` drives).
const LORE_SHIM = process.env.LUDION_COMMIT_LORE_SHIM;
function lore(args, opts) {
  return LORE_SHIM ? run(process.execPath, [LORE_SHIM, ...args], opts) : run('lore', args, opts);
}

const git = (args, opts) => run('git', args, { quiet: true, ...opts });
const short = (sha) => sha.slice(0, 10);

function revParse(ref) {
  const r = git(['rev-parse', '--verify', '--quiet', `${ref}^{commit}`]);
  return r.code === 0 ? r.stdout.trim() : null;
}

// ── preflight ───────────────────────────────────────────────────────────────
for (const [dir, hint] of [
  ['.lore', 'this is not a Lore workspace — see CLAUDE.md §Source control'],
  ['.git', 'the git mirror is missing — restore it before committing'],
]) {
  if (!existsSync(path.join(REPO_ROOT, dir))) {
    console.error(`commit: no ${dir}/ at ${REPO_ROOT}: ${hint}`);
    process.exit(2);
  }
}
const remotes = git(['remote']).stdout.split(/\r?\n/).filter(Boolean);
const hasOrigin = remotes.includes(REMOTE);
// The mirror pushes only to `origin`; a remote under another name would keep every mirror commit
// local without a word.
if (!hasOrigin && remotes.length) {
  console.error(`commit: WARNING: git has remote(s) ${remotes.join(', ')} but no '${REMOTE}' — the mirror only`);
  console.error(`commit: pushes to '${REMOTE}', so this mirror commit stays local. Fix: git remote rename <name> ${REMOTE}`);
}
const PRIVATE = privatePaths(readOptional('.gitignore'), readOptional('.loreignore'));
// symbolic-ref (not rev-parse) so a freshly `git init`ed mirror — an unborn branch with no commits
// yet — passes the check and receives its first snapshot; a fresh checkout on a new machine
// bootstraps its mirror this way instead of needing history copied over.
const branch = git(['symbolic-ref', '--short', 'HEAD']).stdout.trim();
// `master` is the local lifeboat mirror that predates the public remote, accepted only while no
// `origin` exists; the remote's mirror is `main`.
if (branch !== MIRROR_BRANCH && !(branch === 'master' && !hasOrigin)) {
  console.error(`commit: git mirror is on branch '${branch}', not '${MIRROR_BRANCH}' — a switched branch means`);
  console.error('commit: git was authored by hand; reconcile it before mirroring (see the notebook).');
  process.exit(2);
}

// ── privacy canary over a message ───────────────────────────────────────────
// The message travels through a temp file, never an argv string: it is long, multi-line and
// arbitrary text. Spawned through this process's own node so the two scripts cannot disagree on
// the runtime; the canary's own output names the token (never the line) when it refuses.
function canaryOverMessage(text) {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-commit-'));
  const messageFile = path.join(dir, 'message.txt');
  writeFileSync(messageFile, text);
  const canary = run(process.execPath, [path.join(REPO_ROOT, 'scripts', 'privacy-canary.mjs'), '--message-file', messageFile]);
  rmSync(dir, { recursive: true, force: true });
  if (canary.code !== 0) {
    reportCanaryFailure(canary.code, [
      'the commit message carries a private token (see above) — it would be',
      'published verbatim by the git mirror. Reword it, then commit again.',
    ]);
    process.exit(canary.code);
  }
}

// Exit 1 is a hit; any other code means the canary never scanned, which is not a reason to reword.
function reportCanaryFailure(code, hitLines) {
  if (code === 1) for (const l of hitLines) console.error(`commit: ${l}`);
  else console.error(`commit: the privacy canary could not run (exit ${code}, see above) — fix that, then commit again.`);
}

// ── mirror content guard ────────────────────────────────────────────────────
// Every flow that writes a mirror commit runs this over the staged git index before the commit.
// Lore has already committed by then (and keeps the private copy); a refusal leaves only the mirror
// behind, repaired by `--mirror-only` once the file is clean.
function hashWorking(entries) {
  const hashes = new Map();
  for (const e of entries) {
    if (e.path.endsWith('/')) continue;
    const abs = path.join(REPO_ROOT, e.path);
    hashes.set(e.path, existsSync(abs) ? createHash('sha256').update(readFileSync(abs)).digest('hex') : null);
  }
  return hashes;
}

// `staged` is what Lore staged for this commit (path -> hash), when this flow staged anything: a
// file whose bytes moved since would be mirrored as bytes Lore never committed.
function mirrorGuard(staged = new Map()) {
  return () => {
    const drifted = [...hashWorking([...staged.keys()].map((p) => ({ path: p }))).entries()].filter(([p, h]) => h !== staged.get(p));
    if (drifted.length) {
      console.error('commit: these files changed between Lore staging them and the mirror commit:');
      for (const [p] of drifted) console.error(`commit:   ${p}`);
      console.error('commit: Lore is committed but the git mirror is NOT — the mirror would publish bytes Lore does not hold.');
      console.error('commit: re-commit them with Lore, or run `node scripts/commit.mjs --mirror-only "<message>" <paths>`.');
      return false;
    }
    const canary = run(process.execPath, [path.join(REPO_ROOT, 'scripts', 'privacy-canary.mjs'), '--staged']);
    if (canary.code !== 0) {
      reportCanaryFailure(canary.code, ['the staged files carry a private token (see above) — mirror commit aborted.']);
      console.error('commit: Lore is committed but the git mirror is NOT; fix the file(s), then `node scripts/commit.mjs --mirror-only "<message>" <paths>`.');
    }
    return canary.code === 0;
  };
}

// ── Lore preflight — never author on a stale parent, never maul a merge ─────
// `lore status`, parsed. Signatures rather than revision numbers: two machines can both hold a
// local revision 405 whose contents differ, and only the signature distinguishes them.
function loreState() {
  const out = lore(['status'], { quiet: true }).output;
  const conflictBlock = out.match(/^Changes in conflict:\r?\n((?:[A-Z!].*\r?\n?)*)/m)?.[1] ?? '';
  const head = out.match(/^On branch \S+ revision (\d+) -> ([0-9a-f]{64})/m);
  return {
    rev: head?.[1] ?? null,
    local: head?.[2] ?? null,
    remote: out.match(/^Remote revision \d+ -> ([0-9a-f]{64})/m)?.[1] ?? null,
    pendingMerge: /^Pending merge/m.test(out),
    conflicts: conflictBlock.trim(),
    staged: parseStagedSet(out),
  };
}

// Undoes only this commit's own staging. Never mid-merge: an unstage there silently poisons it.
function unstageOwn(named) {
  const state = loreState();
  if (state.pendingMerge) {
    console.error('commit: WARNING: a merge is pending, so the staged files were left staged — resolve it by hand.');
    return;
  }
  const targets = ownUnstageTargets(state.staged, named);
  if (targets.length) lore(['unstage', ...targets], { quiet: true });
}

// Exit 2 on staging this commit did not make: committing would sweep it in, and unstaging it
// would undo someone else's work. `undo` first removes this commit's own entries.
function refuseForeignStaged(state, named, undo) {
  const foreign = foreignStaged(state.staged, named);
  if (!foreign.length) return;
  console.error('commit: refusing — Lore already holds staged entries this commit did not make:');
  for (const e of foreign) console.error(`commit:   ${e.kind} ${e.from ? `${e.from} -> ` : ''}${e.path}`);
  console.error('commit: they were staged by another instance, or left by a failed run; this commit would');
  console.error("commit: sweep them in. Commit them with their owner's paths, or `lore unstage <path>` any");
  console.error('commit: that are yours. Nothing was committed.');
  if (undo) unstageOwn(named);
  process.exit(2);
}

// Printed rather than attempted: resolving a merge means choosing content, which is the one thing
// a commit wrapper must not guess at.
function mergeRecipe() {
  console.error('commit: resolve it by hand, then commit — the wrapper deliberately will not:');
  console.error('commit:   1. find the markers: they are git-style `<<<<<<< ours` / `>>>>>>> theirs`');
  console.error('commit:      (a Rust file with them fails to build with "encountered diff marker")');
  console.error('commit:   2. edit each file to the content you want, then DELETE the');
  console.error('commit:      `<path>~base` / `~mine` / `~theirs` sidecars lore left beside it');
  console.error('commit:   3. `lore branch merge resolve <paths>`  <- exactly this verb. NOT');
  console.error('commit:      `lore stage merge` (ignores your edit, leaves the file in-conflict),');
  console.error('commit:      NOT `resolve mine`/`theirs` (picks the wrong side), and NEVER');
  console.error('commit:      `lore unstage` mid-merge (silently poisons the merge: self-parent');
  console.error('commit:      commit, and later syncs overwrite the other machine\'s edits).');
  console.error('commit:      Escape hatches: `lore branch merge restart <paths>` regenerates a');
  console.error('commit:      mauled conflict; `lore branch merge abort` backs out of the merge.');
  console.error('commit:   4. `lore commit "<message>"` directly, then `lore push`');
  console.error('commit:   5. re-run this wrapper with --all to catch the git mirror up');
  console.error('commit: Agents: load the `lore-merge` skill first (.claude/skills/lore-merge/SKILL.md) —');
  console.error('commit: it adds the escape for a merge that was already committed wrong.');
}

// `named` is the paths this commit stages (`null` for --all): any other staged entry is foreign.
function lorePreflight(named) {
  const before = loreState();
  if (before.pendingMerge || before.conflicts) {
    console.error('commit: a merge is pending in this workspace — refusing to stage over it.');
    if (before.conflicts) console.error(`commit: still in conflict:\n${before.conflicts}`);
    mergeRecipe();
    process.exit(2);
  }
  refuseForeignStaged(before, named, false);
  // The remote moved since this workspace last synced: sync now, so the commit is a child of what
  // the server actually holds. Committing first is what produces a divergence no push can undo.
  if (before.remote && before.local && before.remote !== before.local) {
    console.log('commit: remote has moved — syncing before staging so this commit is not authored');
    console.log('commit: on a stale parent (the divergence a later push could not repair).');
    const sync = lore(['sync']);
    if (sync.code !== 0) {
      console.error('commit: `lore sync` failed — resolve the workspace state, then commit again.');
      process.exit(sync.code);
    }
    const after = loreState();
    if (after.pendingMerge || after.conflicts) {
      console.error('commit: the sync left a merge to resolve, so nothing was staged or committed.');
      if (after.conflicts) console.error(`commit: in conflict:\n${after.conflicts}`);
      mergeRecipe();
      process.exit(2);
    }
    refuseForeignStaged(after, named, false);
    console.log('commit: synced. If that pulled changes into files you build, re-run');
    console.log('commit: `node scripts/check.mjs` before trusting this commit.');
  }
}

function parseLoreCommit(output) {
  return {
    rev: output.match(/^Revision\s*:\s*(\d+)\s*$/m)?.[1],
    sig: output.match(/^Signature\s*:\s*([0-9a-f]{64})\s*$/m)?.[1],
  };
}

// ── the public remote ───────────────────────────────────────────────────────
// Refreshes origin/main; false only when the remote is unreachable, git's own words kept in
// `remoteError` — git runs quiet, and "unreachable" alone hides an SSH key or host-key refusal.
// An empty remote (no `main` yet) is reachable and leaves no origin/main.
let remoteError = '';
function fetchRemote() {
  const failed = (r) => {
    remoteError = r.output.trim() || `git exited with code ${r.code} and printed nothing`;
    return false;
  };
  const ls = git(['ls-remote', REMOTE, `refs/heads/${MIRROR_BRANCH}`]);
  if (ls.code !== 0) return failed(ls);
  if (!ls.stdout.trim()) {
    if (revParse(REMOTE_MAIN)) git(['update-ref', '-d', REMOTE_MAIN]);
    return true;
  }
  const fetch = git(['fetch', '--quiet', '--no-tags', REMOTE, `+refs/heads/${MIRROR_BRANCH}:${REMOTE_MAIN}`]);
  return fetch.code === 0 || failed(fetch);
}

function printRemoteError() {
  if (!remoteError) return;
  console.error('commit: git said:');
  for (const line of remoteError.split(/\r?\n/)) console.error(`commit:   ${line}`);
}

// origin/main's tip, its newest mirror commit, and the contributions after it. Mirror commits form
// one chain, so the first trailer commit in topological order is the newest, and a PR branch
// authored before it is still reached through the merge that brought it in.
function remoteState() {
  const tip = revParse(REMOTE_MAIN);
  if (!tip) return { tip: null, lastTrailer: null, contributions: [] };
  const lastTrailer =
    git(['log', '--topo-order', '-E', `--grep=${TRAILER_ERE}`, '-n', '1', '--format=%H', tip]).stdout.trim() || null;
  const contributions = git(['log', '--format=%H%x00%an%x00%ae%x00%s%x01', lastTrailer ? `${lastTrailer}..${tip}` : tip])
    .stdout.split('\x01')
    .map((record) => record.replace(/^\r?\n/, ''))
    .filter(Boolean)
    .map((record) => {
      const [sha, name, email, subject] = record.split('\0');
      return { sha, name, email, subject };
    });
  return { tip, lastTrailer, contributions };
}

function printContributions(contributions) {
  for (const c of contributions.slice(0, 20)) console.error(`commit:   ${short(c.sha)} ${c.subject}`);
  if (contributions.length > 20) console.error(`commit:   … and ${contributions.length - 20} more`);
}

// The commits of a range, children before parents, each with its trailers, its Lore revision and
// the paths it touched.
function commitsIn(range) {
  const out = git(['log', '--topo-order', '--no-renames', '--name-only', '-z', '--format=%x01%H %P%x02%B%x02', range]).stdout;
  const commits = out.split('\x01').slice(1).map((chunk) => {
    const [ids, body = '', names = ''] = chunk.split('\x02');
    const [sha, ...parents] = ids.trim().split(' ');
    const trailers = [...body.matchAll(TRAILER)].map((m) => ({ text: m[0], rev: Number(m[1]), sig: m[2] }));
    return {
      sha,
      parents,
      trailers,
      children: [],
      rev: trailers.length ? Math.max(...trailers.map((t) => t.rev)) : undefined,
      paths: names.split('\0').map((n) => n.replace(/^\r?\n/, '')).filter(Boolean),
    };
  });
  const bySha = new Map(commits.map((c) => [c.sha, c]));
  for (const c of commits) for (const p of c.parents) bySha.get(p)?.children.push(c);
  // A commit without a trailer (a contribution) takes the revision of the earliest mirror commit
  // built on it: the import marker that brought its content into Lore.
  for (const c of commits) c.rev ??= Math.min(...c.children.map((child) => child.rev));
  return commits;
}

// Newest Lore revision per path over some commits; `built` overrides a commit's revision per path
// for commits this run collapsed, whose trailer is newer than the content they carry.
function newestRevByPath(commits, built) {
  const revs = new Map();
  for (const c of commits) {
    for (const p of c.paths) {
      const rev = built?.get(c.sha)?.get(p) ?? c.rev;
      if (!(revs.get(p) >= rev)) revs.set(p, rev);
    }
  }
  return revs;
}

function lsTree(ref) {
  const tree = new Map();
  for (const record of git(['ls-tree', '-r', '-z', '--full-tree', ref]).stdout.split('\0')) {
    const tab = record.indexOf('\t');
    if (tab < 0) continue;
    const [mode, type, oid] = record.slice(0, tab).split(' ');
    tree.set(record.slice(tab + 1), { mode, type, oid });
  }
  return tree;
}

// Blob bytes, exactly as stored: read as Buffers, no line-ending or encoding translation.
function readBlobs(oids) {
  const blobs = new Map();
  if (!oids.length) return blobs;
  const r = spawnSync('git', ['cat-file', '--batch'], { cwd: REPO_ROOT, input: `${oids.join('\n')}\n`, maxBuffer: MAX_BUFFER });
  if (r.error || r.status !== 0) {
    console.error(`commit: git cat-file failed: ${r.error?.message ?? r.stderr}`);
    process.exit(2);
  }
  let pos = 0;
  while (pos < r.stdout.length) {
    const eol = r.stdout.indexOf(0x0a, pos);
    const [oid, , size] = r.stdout.subarray(pos, eol).toString('utf8').split(' ');
    blobs.set(oid, r.stdout.subarray(eol + 1, eol + 1 + Number(size)));
    pos = eol + 1 + Number(size) + 1;
  }
  return blobs;
}

// ── the mirror commit ───────────────────────────────────────────────────────
// Stages this commit's paths from the working tree; `null` is the whole tree (`--all`).
// The mirror's path list is the Lore list minus what git refuses: any path matching an ignore
// rule (a private path — `.gitignore` minus `.loreignore` — is Lore-only by design, and git
// refuses an explicit ignored path even when it still tracks the file), and a path git never
// held that no longer exists (a Lore-only file deleted in this commit). One refusal fails the
// whole snapshot. `check-ignore --no-index` answers "does an ignore rule match" regardless of
// tracking, which is the question git add asks. `ls-files` answers a directory with the files
// under it, never the directory, so a deleted directory counts as held by its prefix.
function stageWorkingTree(paths, announce) {
  // `git add -A` stages changes to a tracked file even when it is ignored, which is how Lore-only
  // content got into the mirror. Every add is followed by resetting the private paths in the index
  // (an exclude pathspec naming an ignored path makes `git add` fail); only --mirror-only untracks
  // what git still holds.
  const resetPrivate = () => !PRIVATE.length || run('git', ['reset', '-q', '--', ...PRIVATE]).code === 0;
  const untrackPrivate = () =>
    !values['mirror-only'] || !PRIVATE.length || run('git', ['rm', '-r', '--cached', '--ignore-unmatch', '--quiet', '--', ...PRIVATE]).code === 0;
  const announceHeld = (covering) => {
    if (!announce || values['mirror-only'] || !PRIVATE.length) return;
    const held = git(['ls-files', '-z', '--', ...PRIVATE]).stdout.split('\0').filter(Boolean);
    const left = PRIVATE.filter((q) => held.some((f) => f === q || f.startsWith(`${q}/`)) && covering(q));
    if (left.length) console.log(`commit: mirror leaves out private content under: ${left.join(', ')}`);
  };
  if (paths === null) {
    announceHeld(() => true);
    if (run('git', ['add', '-A']).code !== 0 || !resetPrivate()) return 'failed';
    return untrackPrivate() ? 'staged' : 'failed';
  }
  if (!paths.length) return 'none';
  const listed = (args, input) => new Set(git(args, { input }).stdout.split('\0').filter(Boolean));
  const tracked = [...listed(['ls-files', '-z', '--', ...paths])];
  const ignored = listed(['check-ignore', '--no-index', '--stdin', '-z'], paths.join('\0'));
  const held = (p) => {
    const dir = `${p.replaceAll('\\', '/').replace(/\/+$/, '')}/`;
    return tracked.some((f) => f === p || f.startsWith(dir));
  };
  const mirrorPaths = paths.filter(
    (p) => !ignored.has(p) && !isPrivate(p, PRIVATE) && (held(p) || existsSync(path.join(REPO_ROOT, p))),
  );
  const skipped = paths.filter((p) => !mirrorPaths.includes(p));
  if (announce && skipped.length) console.log(`commit: mirror skips ${skipped.length} Lore-only path(s): ${skipped.join(', ')}`);
  announceHeld((q) => mirrorPaths.some((p) => overlaps(q, [p])));
  if (mirrorPaths.length && (run('git', ['add', '-A', '--', ...mirrorPaths]).code !== 0 || !resetPrivate())) return 'failed';
  if (!untrackPrivate()) return 'failed';
  return mirrorPaths.length ? 'staged' : 'private';
}

function readOptional(rel) {
  const file = path.join(REPO_ROOT, rel);
  return existsSync(file) ? readFileSync(file, 'utf8') : '';
}

// Builds one mirror commit. With a `tip` (origin/main) the branch and index move onto it — HEAD
// and index only, the working tree is never written — and the local commits not on it are overlaid
// from their own trees, unless the remote holds a later Lore revision of the path. Without one the
// commit lands on the local HEAD. Returns { sha } for a new commit, { summary } when the tree
// already matches, { failed } after restoring the branch and index.
function buildMirrorCommit(tip, { message, sig, paths, built = new Map(), announce = true, allowEmpty = false, guard }) {
  const head = revParse('HEAD');
  // Staging this commit did not make would be committed (no tip) or discarded by read-tree (tip).
  // Refuse before anything moves — restore() itself would wipe it.
  const indexed = git(['diff', '--cached', '--name-only', '-z', '--no-renames', ...(head ? [head] : [])]).stdout.split('\0').filter(Boolean);
  const foreignIndex = foreignIndexPaths(indexed, paths, PRIVATE);
  if (foreignIndex.length) {
    console.error('commit: the git index holds staging this mirror commit did not make:');
    for (const p of foreignIndex) console.error(`commit:   ${p}`);
    console.error('commit: nobody authors git by hand — the mirror commit was not written. If it is a leftover,');
    console.error('commit: `git reset -q -- <path>` it, then run `node scripts/commit.mjs --mirror-only` for these paths.');
    return { failed: true };
  }
  const restore = () => {
    if (head) {
      git(['update-ref', 'HEAD', head]);
      git(['read-tree', head]);
    }
    return { failed: true };
  };
  const carried = [];
  const overlaid = new Map();
  if (tip && tip !== head) {
    const ours = head ? commitsIn(`${tip}..${head}`) : [];
    const fork = ours.length ? git(['merge-base', head, tip]).stdout.trim() : '';
    const theirs = fork ? commitsIn(`${fork}..${tip}`) : [];
    if (git(['update-ref', 'HEAD', tip]).code !== 0 || git(['read-tree', tip]).code !== 0) return restore();
    if (ours.length) {
      const theirRevs = newestRevByPath(theirs);
      const tree = lsTree(head);
      const entries = [];
      for (const [p, rev] of newestRevByPath(ours, built)) {
        if ((theirRevs.get(p) ?? -Infinity) > rev) continue;
        const entry = tree.get(p);
        entries.push(entry ? `${entry.mode} ${entry.oid}\t${p}` : `0 ${'0'.repeat(head.length)}\t${p}`);
        overlaid.set(p, rev);
      }
      if (entries.length && git(['update-index', '-z', '--index-info'], { input: `${entries.join('\0')}\0` }).code !== 0) {
        return restore();
      }
      const seen = new Set([sig]);
      for (const c of [...ours].reverse()) {
        for (const t of c.trailers) {
          if (!seen.has(t.sig)) carried.push(t.text);
          seen.add(t.sig);
        }
      }
    }
  }
  const staged = stageWorkingTree(paths, announce);
  if (staged === 'failed') return restore();
  if (guard && !guard()) return restore();
  const base = tip ?? head;
  if (!allowEmpty && git(['diff', '--cached', '--quiet', ...(base ? [base] : [])]).code === 0) {
    return { summary: staged === 'private' ? 'skipped — every path is private (Lore-only)' : 'skipped — git tree already matches' };
  }
  const full = carried.length ? `${message}\n${carried.map((t) => `Also carries: ${t}`).join('\n')}` : message;
  if (run('git', ['commit', ...(allowEmpty ? ['--allow-empty'] : []), '-m', full]).code !== 0) return restore();
  const sha = revParse('HEAD');
  built.set(sha, overlaid);
  return { sha };
}

// 'ok', 'rejected' (origin/main moved: rebuild and retry), or 'failed'.
function pushMirror() {
  const r = run('git', ['push', '--porcelain', REMOTE, `HEAD:refs/heads/${MIRROR_BRANCH}`]);
  if (r.code === 0) return 'ok';
  return /^!\t[^\t]*\t\[rejected\]/m.test(r.stdout) ? 'rejected' : 'failed';
}

// Returns { summary, problem? }. A problem means the mirror did not reach the remote, which the
// caller reports loudly; the local commit it leaves is collapsed into the next run's.
function mirrorCommit({ message, sig, paths, online, push, guard }) {
  const local = () => {
    const build = buildMirrorCommit(null, { message, sig, paths, guard });
    return build.failed ? 'FAILED' : build.summary ?? `${short(build.sha)} (local)`;
  };
  // A failed build leaves no local commit, so it carries no held-locally problem: callers print the repair.
  const held = (problem, extra) => {
    const summary = local();
    return summary === 'FAILED' ? { summary } : { summary, problem, ...extra };
  };
  if (!online) return held(hasOrigin ? 'unreachable' : null);
  const built = new Map();
  for (let attempt = 1; ; attempt++) {
    if (!fetchRemote()) return held('unreachable');
    const state = remoteState();
    // A PR merged while this run was committing to Lore: the mirror commit waits locally, and the
    // import marker carries it onto the remote.
    if (state.contributions.length) return held('contributions', { contributions: state.contributions });
    const build = buildMirrorCommit(state.tip, { message, sig, paths, built, announce: attempt === 1, guard });
    if (build.failed) return { summary: 'FAILED' };
    if (!build.sha) return { summary: build.summary };
    if (!push) return { summary: `${short(build.sha)} (not pushed, --no-push)` };
    const pushed = pushMirror();
    if (pushed === 'ok') return { summary: `${short(build.sha)} (pushed)` };
    if (pushed === 'rejected' && attempt < PUSH_ATTEMPTS) {
      console.log(`commit: mirror push rejected — ${REMOTE}/${MIRROR_BRANCH} moved; rebuilding on its new tip`);
      continue;
    }
    return { summary: `${short(build.sha)} (NOT pushed)`, problem: pushed };
  }
}

function reportMirrorProblem({ problem, contributions }) {
  if (!problem) return false;
  const reason = {
    unreachable: `${REMOTE} was unreachable, so the mirror commit is held locally.`,
    contributions: `${REMOTE}/${MIRROR_BRANCH} gained contributions while Lore committed, so the mirror commit is held locally.`,
    rejected: `${PUSH_ATTEMPTS} mirror pushes were rejected (${REMOTE}/${MIRROR_BRANCH} kept moving).`,
    failed: 'the mirror push failed (see above).',
  }[problem];
  console.error(`\ncommit: Lore is committed, but the git mirror did NOT reach ${REMOTE}: ${reason}`);
  if (problem === 'unreachable') printRemoteError();
  if (contributions) printContributions(contributions);
  if (problem === 'contributions') {
    console.error('commit: run `node scripts/commit.mjs --import` — its import marker carries the held commit.');
  } else {
    console.error('commit: the next run collapses the local mirror commit into its own and pushes it.');
  }
  console.error('commit: Exiting non-zero so this cannot pass unnoticed.');
  return true;
}

// Nothing new for Lore, but manual lore commits may have left the git mirror behind: catch it
// up to the current revision with one snapshot commit (the given message is not used).
function catchUpMirror(online, paths) {
  const { rev: curRev, local: curSig } = loreState();
  if (!curRev) {
    console.error('commit: nothing to commit, and `lore status` did not yield the current revision');
    process.exit(1);
  }
  const catchUp =
    `Mirror catch-up: snapshot at Lore revision ${curRev}\n\n` +
    'Collapses Lore-native commits made since the last mirror (Lore is the\n' +
    'source of truth); see `lore history` for the individual revisions.\n\n' +
    `[lore ${curRev} | ${curSig}]`;
  const result = mirrorCommit({ message: catchUp, sig: curSig, paths: paths.length ? paths : null, online, push: !values['no-push'], guard: mirrorGuard() });
  if (result.summary.startsWith('skipped')) {
    console.log(`commit: nothing to commit — tree and git mirror both match Lore revision ${curRev}`);
    reportMirrorProblem(result);
    process.exit(1);
  }
  if (result.summary === 'FAILED' && !result.problem) {
    console.error('commit: git catch-up commit failed — repair it with');
    console.error(
      paths.length
        ? `commit:   node scripts/commit.mjs --mirror-only "<message>" ${paths.map((p) => JSON.stringify(p)).join(' ')}`
        : 'commit:   node scripts/mirror-gap.mjs --paths-file <file>, then commit.mjs --mirror-only "<message>" --paths-file <file>',
    );
    process.exit(1);
  }
  console.log(`\nLore revision ${curRev} (unchanged) · git mirror caught up: ${result.summary}`);
  process.exit(reportMirrorProblem(result) ? 1 : 0);
}

// ── a commit ────────────────────────────────────────────────────────────────
function commitFlow() {
  // A commit whose every path is private is never mirrored, so its message is never published.
  if (!(paths.length && paths.every((p) => isPrivate(p, PRIVATE)))) canaryOverMessage(message);

  let online = false;
  if (hasOrigin) {
    online = fetchRemote();
    if (!online) {
      console.error(`commit: WARNING: ${REMOTE} is unreachable — committing to Lore anyway; the mirror commit`);
      console.error('commit: stays local and the next run carries it.');
    } else {
      const { contributions } = remoteState();
      if (contributions.length) {
        console.error(`commit: refusing to commit — ${REMOTE}/${MIRROR_BRANCH} holds contributions Lore has not imported:`);
        printContributions(contributions);
        console.error('commit: import them first (nothing was staged): node scripts/commit.mjs --import');
        process.exit(2);
      }
    }
  }

  const named = paths.length ? paths : null;
  lorePreflight(named);

  // `--scan` in both forms: without it, directory paths stage only files already marked dirty via
  // `lore dirty` — edits made externally (i.e. all of ours) are invisible and stage nothing.
  const stage = lore(['stage', '--scan', ...(paths.length ? paths : ['.'])]);
  if (stage.code !== 0) {
    unstageOwn(named);
    process.exit(stage.code);
  }
  refuseForeignStaged(loreState(), named, true);

  // A staged set this large is an ignore-rule hole far more often than an intended commit: the run
  // that motivated this guard staged 4961 files out of a nested `.build/` no rule covered. Counted
  // from lore's own summary line rather than a dry run, because `lore stage --dry-run` stages anyway.
  const STAGED_FILE_LIMIT = 500;
  const stagedFiles = parseStagedCount(stage.output);
  if (stagedFiles === null) {
    console.error(`commit: ${stage.output.trim().split(/\r?\n/).slice(0, 5).join('\ncommit: ')}`);
    console.error('commit: could not read how many files were staged; refusing rather than skipping the bulk guard.');
    unstageOwn(named);
    process.exit(2);
  }
  if (stagedFiles > STAGED_FILE_LIMIT && !values.force) {
    console.error(`commit: ${stagedFiles} files staged, over the ${STAGED_FILE_LIMIT}-file guard.`);
    console.error('commit: that is usually a build directory or generated tree an ignore rule misses');
    console.error('commit: (check .gitignore AND .loreignore — lore reads only the root .loreignore,');
    console.error('commit: so a rule added to one must be mirrored into the other by hand).');
    console.error('commit: unstaging so the workspace is left as it was found; name explicit paths,');
    console.error('commit: or pass --force if a change this wide really is intended.');
    unstageOwn(named);
    process.exit(2);
  }

  // Private paths never reach the mirror, so their bytes are not tracked.
  const stagedHashes = hashWorking(loreState().staged.filter((e) => !isPrivate(e.path, PRIVATE)));
  const commit = lore(['commit', message]);
  if (commit.code !== 0) {
    if (commit.output.includes('Nothing staged for commit')) catchUpMirror(online, paths); // exits
    unstageOwn(named);
    process.exit(commit.code);
  }
  const { rev, sig } = parseLoreCommit(commit.output);
  if (!rev || !sig) {
    console.error('commit: Lore committed, but its revision/signature could not be parsed from the');
    console.error('commit: output above — mirror by hand: git add -A && git commit with the message');
    console.error('commit: plus a "[lore <rev#> | <signature>]" trailer (values via `lore branch info`).');
    process.exit(1);
  }

  // A failed push used to warn and carry on, which is how one stale-parent commit became two: the
  // next run happily authored another revision on the same unpushed base. The mirror below still
  // runs (the commit is real and must be backed up), but the exit code is loud.
  let pushed = false;
  let pushFailed = false;
  if (values['no-push']) {
    console.log('commit: push skipped (--no-push); remember `lore push` — CI and backups only see the server');
  } else {
    pushed = lore(['push']).code === 0;
    pushFailed = !pushed;
  }

  const mirrorMessage =
    `${message.trimEnd()}\n\n` +
    'Mirror of the Lore-native commit (Lore is the source of truth).\n\n' +
    `[lore ${rev} | ${sig}]`;
  const mirror = mirrorCommit({ message: mirrorMessage, sig, paths: paths.length ? paths : null, online, push: !values['no-push'], guard: mirrorGuard(stagedHashes) });
  const mirrorFailed = mirror.summary === 'FAILED' && !mirror.problem;
  if (mirrorFailed) {
    console.error('commit: git mirror commit failed — Lore is committed and authoritative, but the mirror');
    console.error('commit: holds none of this commit. Repair it with:');
    if (paths.length) {
      console.error(`commit:   node scripts/commit.mjs --mirror-only "<message>" ${paths.map((p) => JSON.stringify(p)).join(' ')}`);
    } else {
      console.error('commit:   node scripts/mirror-gap.mjs --paths-file <file>');
      console.error('commit:   node scripts/commit.mjs --mirror-only "<message>" --paths-file <file>');
    }
  }

  console.log(`\nLore revision ${rev}${pushed ? ' (pushed)' : ' (LOCAL ONLY)'} · git mirror: ${mirror.summary}`);

  if (pushFailed) {
    console.error('\ncommit: the commit LANDED but the push did NOT — the server, CI and every other');
    console.error('commit: machine cannot see it. Do not stack another commit on top: run `lore push`');
    console.error('commit: once the server is reachable, or `lore sync` first if it reports a');
    console.error('commit: divergence — that sync is a real merge: load the `lore-merge` skill');
    console.error('commit: (.claude/skills/lore-merge/SKILL.md) before it. Exiting non-zero so this cannot pass unnoticed.');
  }
  const mirrorProblem = reportMirrorProblem(mirror);
  if (pushFailed || mirrorProblem || mirrorFailed) process.exit(1);
}

// ── --mirror-only ───────────────────────────────────────────────────────────
// The mirror commit alone, for paths Lore already holds. The trailer names the current revision.
function mirrorOnlyFlow() {
  const inPrivate = paths.filter((p) => isPrivate(p, PRIVATE));
  if (inPrivate.length) {
    console.error(`commit: --mirror-only refuses private paths (${PRIVATE.join(', ')}); they are Lore-only:`);
    for (const p of inPrivate) console.error(`commit:   ${p}`);
    process.exit(2);
  }
  canaryOverMessage(message);

  const state = loreState();
  if (state.pendingMerge || state.conflicts) {
    console.error('commit: a merge is pending in this workspace — refusing to mirror over it.');
    mergeRecipe();
    process.exit(2);
  }
  if (!state.rev || !state.local) {
    console.error('commit: `lore status` did not yield the current revision.');
    process.exit(2);
  }
  // A path with uncommitted Lore changes has no revision to mirror: the snapshot would publish bytes
  // the trailer's revision does not hold.
  const status = lore(['status', '--scan', ...paths], { quiet: true }).output;
  const dirty = parseLoreStatus(status).filter((c) => overlaps(c.path, paths));
  if (dirty.length) {
    console.error('commit: --mirror-only refuses paths Lore reports as changed (commit them with Lore first):');
    for (const c of dirty) console.error(`commit:   ${c.kind} ${c.path}`);
    process.exit(2);
  }

  let online = false;
  if (hasOrigin) {
    online = fetchRemote();
    if (online) {
      const { contributions } = remoteState();
      if (contributions.length) {
        console.error(`commit: refusing — ${REMOTE}/${MIRROR_BRANCH} holds contributions Lore has not imported:`);
        printContributions(contributions);
        console.error('commit: import them first (nothing was staged): node scripts/commit.mjs --import');
        process.exit(2);
      }
    }
  }

  const mirrorMessage =
    `${message.trimEnd()}\n\n` +
    'Mirror of Lore-tracked content (Lore is the source of truth).\n\n' +
    `[lore ${state.rev} | ${state.local}]`;
  const mirror = mirrorCommit({ message: mirrorMessage, sig: state.local, paths, online, push: !values['no-push'], guard: mirrorGuard() });
  console.log(`\nLore revision ${state.rev} (unchanged) · git mirror: ${mirror.summary}`);
  const problem = reportMirrorProblem(mirror);
  if (problem || (mirror.summary === 'FAILED' && !mirror.problem)) process.exit(1);
}

// ── --import ────────────────────────────────────────────────────────────────
function importFlow() {
  if (!hasOrigin) {
    console.error(`commit: --import needs the '${REMOTE}' remote — without it there is nothing to import.`);
    process.exit(2);
  }
  lorePreflight([]);
  if (!fetchRemote()) {
    console.error(`commit: could not fetch ${REMOTE} — --import needs the remote; try again once it is reachable.`);
    printRemoteError();
    process.exit(2);
  }
  const { tip, lastTrailer, contributions } = remoteState();
  if (!contributions.length) {
    console.log(`commit: nothing to import — ${REMOTE}/${MIRROR_BRANCH} holds no commit after its newest mirror commit.`);
    process.exit(0);
  }
  if (!lastTrailer) {
    console.error(`commit: ${REMOTE}/${MIRROR_BRANCH} holds no mirror commit (no [lore <rev#> | <signature>] trailer)`);
    console.error('commit: to import against — its root must be one.');
    process.exit(2);
  }

  // Clean means each touched path still holds what the newest mirror commit published, so writing
  // the contribution overwrites nothing Lore or a local edit holds that the mirror does not. Already
  // holding the contribution's bytes also counts: that is a re-run after a failed marker push.
  const touched = git(['diff', '--name-only', '-z', '--no-renames', lastTrailer, tip]).stdout.split('\0').filter(Boolean);
  const published = lsTree(lastTrailer);
  const contributed = lsTree(tip);
  const blobOid = (tree, p) => (tree.get(p)?.type === 'blob' ? tree.get(p).oid : null);
  const blobs = readBlobs([...new Set(touched.flatMap((p) => [blobOid(published, p), blobOid(contributed, p)]).filter(Boolean))]);
  const onDisk = (p) => {
    try {
      return readFileSync(path.join(REPO_ROOT, p));
    } catch (err) {
      return err.code === 'ENOENT' ? null : undefined;
    }
  };
  const holds = (bytes, oid) => (oid ? Buffer.isBuffer(bytes) && bytes.equals(blobs.get(oid)) : bytes === null);
  const dirty = touched.filter((p) => {
    const bytes = onDisk(p);
    return !holds(bytes, blobOid(published, p)) && !holds(bytes, blobOid(contributed, p));
  });
  if (dirty.length) {
    console.error('commit: refusing to import — these paths the contributions touched differ in the working tree');
    console.error('commit: from the newest mirror commit (a local edit, or Lore state the mirror has not published):');
    for (const p of dirty) console.error(`commit:   ${p}`);
    console.error('commit: nothing was written. Commit or set aside a local edit; Lore state the contribution');
    console.error('commit: conflicts with is merged by hand.');
    process.exit(2);
  }

  const subject = git(['log', '-1', '--format=%s', tip]).stdout.trim();
  const self = git(['config', 'user.email']).stdout.trim().toLowerCase();
  const coAuthors = new Map();
  for (const c of [...contributions].reverse()) {
    const key = c.email.toLowerCase();
    if (key !== self && !coAuthors.has(key)) coAuthors.set(key, `Co-Authored-By: ${c.name} <${c.email}>`);
  }
  const importMessage = [
    subject,
    '',
    `Imported from the public git mirror: ${short(lastTrailer)}..${short(tip)}.`,
    '',
    ...coAuthors.values(),
    `[git ${tip}]`,
  ].join('\n');
  canaryOverMessage(importMessage);

  for (const p of touched) {
    const file = path.join(REPO_ROOT, p);
    const oid = blobOid(contributed, p);
    if (oid) {
      writeFileAtomic(file, blobs.get(oid));
    } else {
      rmSync(file, { force: true });
    }
  }
  console.log(`commit: wrote ${touched.length} contributed path(s) from ${short(lastTrailer)}..${short(tip)}`);

  if (touched.length) {
    const stage = lore(['stage', '--scan', ...touched]);
    if (stage.code !== 0) {
      unstageOwn(touched);
      process.exit(stage.code);
    }
  }
  const commit = lore(['commit', importMessage]);
  let rev, sig;
  if (commit.code === 0) {
    ({ rev, sig } = parseLoreCommit(commit.output));
    if (!rev || !sig) {
      console.error('commit: Lore committed the import, but its revision/signature could not be parsed from');
      console.error('commit: the output above — push Lore, then run --import again to write the marker.');
      process.exit(1);
    }
    if (lore(['push']).code !== 0) {
      console.error(`\ncommit: the import LANDED as Lore revision ${rev} but the push did NOT — no machine can see it.`);
      console.error('commit: run `lore push` once the server is reachable (`lore sync` first on a divergence),');
      console.error('commit: then `node scripts/commit.mjs --import` again to publish the import marker.');
      console.error('commit: Exiting non-zero so this cannot pass unnoticed.');
      process.exit(1);
    }
  } else if (commit.output.includes('Nothing staged for commit')) {
    ({ rev, local: sig } = loreState());
    if (!rev) {
      console.error('commit: nothing to commit, and `lore status` did not yield the current revision');
      process.exit(1);
    }
    console.log(`commit: Lore already holds the contributed content — marking revision ${rev} as the import`);
  } else {
    unstageOwn(touched);
    process.exit(commit.code);
  }

  // The marker keeps the contribution rule a plain "commits after the newest trailer". It is built
  // on the imported tip, so it also carries any local mirror commit that never reached the remote.
  const marker = buildMirrorCommit(tip, {
    message: `Imported into Lore as revision ${rev}\n\n[lore ${rev} | ${sig}]`,
    sig,
    paths: [],
    allowEmpty: true,
  });
  const pushed = marker.failed ? 'failed' : pushMirror();
  if (pushed !== 'ok') {
    console.error(`\ncommit: Lore revision ${rev} holds the import, but the import marker did NOT reach ${REMOTE}`);
    console.error(`commit: (${pushed === 'rejected' ? `${REMOTE}/${MIRROR_BRANCH} moved` : 'see above'}), so the contributions still read as`);
    console.error('commit: unimported. Run `node scripts/commit.mjs --import` again: it re-marks without');
    console.error('commit: re-committing. Exiting non-zero so this cannot pass unnoticed.');
    process.exit(1);
  }
  console.log(`\nLore revision ${rev} (pushed) · imported ${contributions.length} commit(s) · git mirror: ${short(marker.sha)} (pushed)`);
}

if (values.import) importFlow();
else if (values['mirror-only']) mirrorOnlyFlow();
else commitFlow();
