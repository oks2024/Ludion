#!/usr/bin/env node
// One-command local verify loop for Ludion.
//
// Runs every CI gate with the parallelizable work overlapped, prints only a compact pass/fail
// summary — plus the actual errors when something fails — and writes full per-step logs to
// `.build/` (throwaway, recreated each run) so any instance can dig into the detail on demand. The
// summary names every gate a mode ran.
//
// The canonical gate set is CI (`.github/workflows/ci.yml` + the generic gate scripts in
// `ci/scripts/`; the private TeamCity configs mirror the same set); this script is a low-noise
// local front-end to the same commands, not a new source of truth. If a gate is added there,
// mirror it here.
//
// Usage:  node scripts/check.mjs [--rust-only|--frontend-only] [--no-gpu] [--full] [--soak] [--fix] [--history]
//         node scripts/check.mjs --scope [--deps] [--no-gpu] [--fix] <paths...>
//
//   (no flag)        every gate: the cheap node gates, the Rust gates (fmt, clippy, nextest +
//                    doctests, the derived-import sync, the drift gates, the check-docs smoke, mobile,
//                    vigil's own workspace — fmt, clippy, nextest, doctests, run from games/vigil/ —
//                    the editor host, fuzz fmt + clippy) and the editor frontend (oxlint, tsc, vitest, schema-drift),
//                    then the quiet `fps-gate` tail: the 1080p delivery fps gates, alone on the box
//   --rust-only      the Rust gates only — fastest for Rust-only edits
//   --frontend-only  the editor frontend gates only
//                    (the cheap node gates — privacy, size, claude-md, ecs-budget, lists-in-step,
//                    diagnostic-prints, atomic-writes, schedule-comments — run in every mode: each
//                    costs well under a second, and a scoped run must not be the way a private
//                    token, a stray build artifact or a regrown CLAUDE.md slips through)
//   --no-gpu         the hosted `no-gpu` nextest profile (what CI runs on a runner without a GPU), no
//                    fps gates; needs cargo-nextest, refused with --full or --soak
//   --full           default suite + cargo-deny (licenses, advisories) + the determinism denylist
//                    canary (scratch crates under target/) + crates.io packaging (`cargo package`)
//                    + a workspace doc build + the 1/2/8-thread hash-identity gate over the named
//                    replay corpus (games/breakout/run.lrp at the root, games/vigil/{run,stress}.lrp
//                    from games/vigil/) + vigil's triage-probe campaign
//                    (games/vigil/tests/probe.rs, `--ignored`) + the public-tree lane
//                    (scripts/public-tree.mjs: the git-visible export, without vigil, built and
//                    tested on its own — `no-gpu` nextest `--locked`, then oxlint, tsc, vitest)
//   --soak           the vigil hash soak (games/vigil/tests/soak.rs, `--ignored`) — a full 2700 s
//                    replay driven twice and hashed every frame, ~35-60 minutes on a laptop. NOT
//                    part of `--full`: that lane's tail is already the CI budget, and this one scales
//                    with the run length, not with the workspace
//   --fix            re-mirror docs/ into ludion-cli/embedded-docs/ + regenerate shader_gen.rs
//                    + the clippy.toml denylists + `cargo fmt` (write) + `oxlint --fix` first, then
//                    the (check-only) gates
//   --history        print the per-step timings of recent runs (target/check-history.jsonl) and exit
//   --scope <paths>  a coder's inner loop over the gates the named paths reach (scripts/check-scope.mjs):
//                    the cheap node gates always; clippy, nextest and doctests over the paths' own
//                    crates (`-p`); vigil, the editor host, fuzz, the frontend, schema-drift, the
//                    drift gates and the script tests only when a path falls under them. `--deps`
//                    adds every crate that depends on a named one (vigil, through the facade). Never
//                    a commit gate: the default suite runs before the commit lands. Logs and scratch
//                    are per run (target/check-scope/<pid>/), so scoped runs overlap each other; one
//                    refuses while a default run holds the lock. A workspace-wide path (a manifest,
//                    the lockfile, toolchain or nextest config, vendor/) is refused: run the default
//                    suite
//
// games/vigil is a Lore-only game and its own Cargo workspace: its steps run with cwd games/vigil, and
// report as skipped when the directory is absent (a public clone).
//
// Every step has a 60-minute ceiling (the soak 200): past it the step's process tree is killed and the step fails.
// A second concurrent run exits 2 "busy" (target/check.lock) instead of deleting the first run's .build/.
// Refuses to start when LUDION_BLESS or BLESS_SCHEMA is set (a bless run rewrites baselines and reports green).
//
// Budget: CLAUDE.md §Working conventions. The test lane's floor is nextest's `gpu` group (its tests
// through 4 slots), not CPU. The dev profile is at opt-level 1 so committed-replay tests run
// optimized. The test lane runs under cargo-nextest (`cargo install cargo-nextest --locked` once per machine); without
// it, a slower serial `cargo test --workspace` gates the same set. A breach means re-measure, never
// delete or weaken a gate. Every run appends its timings to target/check-history.jsonl and the
// summary compares the total against the last green run of the same mode.
//
// Zero npm dependencies — Node built-ins only. Runs unchanged on Windows and macOS.

import {
  rmSync, mkdirSync, createWriteStream, readFileSync, writeFileSync, appendFileSync,
  readdirSync, existsSync, unlinkSync, copyFileSync, constants as fsConstants,
} from 'node:fs';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { acquireLock } from './check-lock.mjs';
import { classify, loadWorkspace } from './check-scope.mjs';
import { writeFileAtomic } from './atomic-write.mjs';
import { killTree, spawnTool } from './kill-tree.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
// A scoped run keeps its logs and scratch per process, so concurrent scoped runs and a default run
// never delete each other's.
const SCOPED = process.argv.slice(2).includes('--scope');
const SCOPE_ROOT = path.join(REPO_ROOT, 'target', 'check-scope');
const LOG_DIR = SCOPED ? path.join(SCOPE_ROOT, String(process.pid)) : path.join(REPO_ROOT, '.build');
const HISTORY = path.join(REPO_ROOT, 'target', 'check-history.jsonl');
const LOCK = path.join(REPO_ROOT, 'target', 'check.lock');
// Cargo never prunes stale artifacts, and macOS's default unpacked split-debuginfo leaves a loose `.o`
// per codegen unit in `target/debug/deps`. Past ~100k entries CoreAudio's per-boot directory scan
// makes the embed/ffi boot tests fail with L4022 (dev-docs/test-box-failures.md). A warning only.
const DEPS_WARN_FILES = Number(process.env.LUDION_DEPS_WARN_FILES ?? 60000);
function warnIfDepsBloated() {
  const deps = path.join(REPO_ROOT, 'target', 'debug', 'deps');
  if (!existsSync(deps)) return;
  const count = readdirSync(deps).length;
  if (count > DEPS_WARN_FILES) {
    console.log(`${YELLOW}check: target/debug/deps holds ${count} files (warn past ${DEPS_WARN_FILES}) — macOS boot tests may fail with L4022. Run \`cargo sweep --time 7\` or \`cargo clean\`.${RST}`);
  }
}

// Many tests never remove their pid-suffixed scratch under the temp dir, and none survive a kill,
// which filled C: over time. The test-running steps get one bounded temp root instead: wiped at the
// start of a run, removed at the end when every test step passed, kept for diagnosis otherwise.
// Named per checkout (the run lock is per checkout, so the wipe never touches another checkout's
// live scratch), and in the real temp dir, not under target/: some tests (ludion-cli's docs.rs
// `path_flag_outside_repo_is_l0008` and `devdocs_outside_a_checkout_is_l0008_for_every_form`) need
// scratch outside any checkout. Only these steps: build tools keep the real temp dir.
const TEST_TEMP = path.join(os.tmpdir(), `ludion-check-${createHash('sha256').update(REPO_ROOT).digest('hex').slice(0, 8)}${SCOPED ? `-scope-${process.pid}` : ''}`);
const TEST_TEMP_STEPS = new Set(['test', 'doctest', 'vigil-test', 'vigil-doctest', 'fps-gate', 'tauri-test', 'thread-identity', 'vigil-thread-identity', 'probe', 'public-tree', 'soak', 'mobile']);
const IS_WINDOWS = process.platform === 'win32';
const EXE = IS_WINDOWS ? '.exe' : '';
const VIGIL = 'games/vigil';
const hasVigil = existsSync(path.join(REPO_ROOT, VIGIL, 'Cargo.toml'));
const vigilAbsent = (name) => skipped(name, 'games/vigil absent (Lore-only game)');

// ── args ────────────────────────────────────────────────────────────────────
let values, positionals;
try {
  ({ values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      'rust-only': { type: 'boolean', default: false },
      'frontend-only': { type: 'boolean', default: false },
      'no-gpu': { type: 'boolean', default: false },
      full: { type: 'boolean', default: false },
      soak: { type: 'boolean', default: false },
      fix: { type: 'boolean', default: false },
      history: { type: 'boolean', default: false },
      scope: { type: 'boolean', default: false },
      deps: { type: 'boolean', default: false },
      help: { type: 'boolean', short: 'h', default: false },
    },
  }));
} catch (err) {
  console.error(`check: ${err.message}`);
  process.exit(2);
}

// check.mjs never blesses. A session-wide bless switch makes every golden test rewrite its file (or the
// schema test rewrite its snapshot) and report green, so refuse to start rather than run a lane that
// cannot fail.
const BLESS_SWITCHES = [
  ['LUDION_BLESS', 'every golden test rewrites its golden and passes'],
  ['BLESS_SCHEMA', 'the schema test rewrites the protocol schema snapshot'],
];
for (const [name, effect] of BLESS_SWITCHES) {
  const value = process.env[name];
  if (value && value !== '0') {
    console.error([
      `check: ${name} is set in the environment. check.mjs never blesses: with it set ${effect},`,
      'so the run would report green over rewritten baselines.',
      `Unset it (PowerShell: Remove-Item Env:${name}; POSIX: unset ${name}) and re-run.`,
    ].join('\n'));
    process.exit(1);
  }
}

if (values.help) {
  const help = [];
  for (const l of readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n')) {
    if (l.startsWith('#!')) continue;        // shebang
    if (l.startsWith('//')) help.push(l.slice(3));
    else if (help.length) break;             // stop at the blank line after the banner
  }
  console.log(help.join('\n'));
  process.exit(0);
}

const rustOnly = values['rust-only'];
const frontendOnly = values['frontend-only'];
if (rustOnly && frontendOnly) {
  console.error('check: --rust-only and --frontend-only are mutually exclusive');
  process.exit(2);
}

const noGpu = values['no-gpu'];
if (noGpu && (values.full || values.soak)) {
  console.error('check: --no-gpu cannot be combined with --full or --soak');
  process.exit(2);
}

if (positionals.length && !values.scope) {
  console.error(`check: unexpected argument ${positionals[0]} (paths are taken only with --scope)`);
  process.exit(2);
}
if (values.deps && !values.scope) {
  console.error('check: --deps widens a --scope run; it needs --scope');
  process.exit(2);
}
if (values.scope && (rustOnly || frontendOnly || values.full || values.soak)) {
  console.error('check: --scope picks its own gates; it cannot be combined with --rust-only, --frontend-only, --full or --soak');
  process.exit(2);
}

// The scoped run's selection (scripts/check-scope.mjs), or null for every other mode.
let scope = null;
if (values.scope && !values.history) {
  if (!positionals.length) {
    console.error('check: --scope needs the paths this change touches, e.g. `--scope crates/ludion-renderer editor/app/src/App.tsx`');
    process.exit(2);
  }
  const rel = positionals.map((p) => path.relative(REPO_ROOT, path.resolve(p)).split(path.sep).join('/'));
  scope = classify(rel, loadWorkspace(REPO_ROOT), { deps: values.deps });
  if (scope.error) {
    console.error(`check: --scope: ${scope.error}`);
    process.exit(2);
  }
  scope.paths = rel;
}
// Which gates a scoped run needs; every other mode runs every gate its flags select.
const SCOPE_GATES = scope && {
  fmt: scope.packages.length > 0, 'physics-features': scope.packages.length > 0,
  'vigil-fmt': scope.vigil, 'vigil-node': scope.vigilNode, 'vigil-lab': scope.vigilNode,
  'voxel-kit': scope.scripts, 'commit-wrapper': scope.scripts, 'gate-scripts': scope.scripts, 'pin-ritual': scope.scripts,
  heavy: scope.packages.length > 0 || scope.vigil || scope.drift,
  bins: scope.vigil || scope.drift || scope.packages.includes('ludion'),
  drift: scope.drift, 'check-docs': scope.drift || scope.packages.includes('ludion'),
  clippy: scope.packages.length > 0, doctest: scope.libs.length > 0, vigil: scope.vigil, mobile: scope.mobile,
  tauri: scope.tauri, fuzz: scope.fuzz,
};
const inScope = (gate) => !SCOPE_GATES || SCOPE_GATES[gate];
const outOfScope = (name) => skipped(name, 'out of scope');
// The root workspace's selection for clippy, nextest and the doctests.
const SELECT = scope ? scope.packages.flatMap((p) => ['-p', p]) : ['--workspace'];
const DOC_SELECT = scope ? scope.libs.flatMap((p) => ['-p', p]) : ['--workspace'];
// A scoped package may have no test of its own (or none left once the fps gates are filtered out).
const NO_TESTS = scope ? ['--no-tests=pass'] : [];

const runRust = scope ? Object.values(SCOPE_GATES).some(Boolean) : !frontendOnly;
const runFrontend = scope ? scope.frontend : !rustOnly;
const runSchema = scope ? scope.schema : !rustOnly && !frontendOnly;
// The denylist canary, crates.io packaging, the doc build, the thread-identity gate and the probe
// campaign all ride `--full`: none is part of the routine edit/verify loop. The last two are
// `#[ignore]`d tests, so they cost nothing in the default lane.
const runFullTail = values.full && runRust;
// The hash soak: its own switch, not folded into `--full` — a full-clock replay driven twice is
// 35-60 minutes on a laptop, well past even the `--full` lane's tail.
const runSoak = values.soak && runRust;
const mode = scope ? 'scope' : frontendOnly ? 'frontend-only' : rustOnly ? 'rust-only' : values.full ? 'full' : values.soak ? 'soak' : 'default';

// ── step runner ───────────────────────────────────────────────────────────────
// Tools spawn through `spawnTool` (scripts/kill-tree.mjs): on Windows via the shell, elsewhere in
// their own process group, so a timeout or a signal can reap the whole tree.

// A step that outlives its ceiling is killed (the whole tree: under `shell: true` a plain kill
// only reaches `cmd.exe`) and fails, so a hung test cannot hold the run forever.
const STEP_TIMEOUT_MIN = 60;

// Steps still running; a signal to check.mjs reaps them, since a detached group would outlive it.
const liveChildren = new Set();

function runStep(step) {
  return new Promise((resolve) => {
    const logPath = path.join(LOG_DIR, step.log);
    const stream = createWriteStream(logPath);
    const cwd = step.cwd ? path.join(REPO_ROOT, step.cwd) : REPO_ROOT;
    const chunks = [];
    const start = process.hrtime.bigint();
    stream.write(`$ ${step.cmd} ${step.args.join(' ')}    (cwd: ${cwd})\n\n`);

    const tempEnv = TEST_TEMP_STEPS.has(step.name) ? { TEMP: TEST_TEMP, TMP: TEST_TEMP, TMPDIR: TEST_TEMP } : null;
    const child = spawnTool(step.cmd, step.args, { cwd, env: step.env || tempEnv ? { ...process.env, ...tempEnv, ...step.env } : process.env });
    liveChildren.add(child);
    const onData = (d) => { chunks.push(d); stream.write(d); };
    child.stdout.on('data', onData);
    child.stderr.on('data', onData);
    child.on('error', (err) => {
      const msg = `\n[spawn error] ${err.message}\n`;
      chunks.push(Buffer.from(msg));
      stream.write(msg);
    });
    const limitMin = step.timeoutMin ?? STEP_TIMEOUT_MIN;
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      const msg = `
[timeout] ${step.name} exceeded ${limitMin} min — killed
`;
      chunks.push(Buffer.from(msg));
      stream.write(msg);
      killTree(child);
    }, limitMin * 60_000);
    child.on('close', (code) => {
      liveChildren.delete(child);
      clearTimeout(timer);
      const ms = Number(process.hrtime.bigint() - start) / 1e6;
      const output = Buffer.concat(chunks).toString('utf8');
      stream.end(() => resolve({ name: step.name, ok: code === 0 && !timedOut, code, ms, logPath, output }));
    });
  });
}

// Runs steps one after another; once one fails, the rest report as skipped with that reason. For a
// chain where a failure makes every later step a redundant recompile-failure.
async function runChain(steps) {
  const out = [];
  let failed = null;
  for (const step of steps) {
    if (failed) { out.push(skipped(step.name, `${failed} failed`)); continue; }
    const r = await runStep(step);
    out.push(r);
    if (!r.ok) failed = step.name;
  }
  return out;
}

const skipped = (name, reason) => ({ name, ok: true, skipped: true, reason, ms: 0 });

// A gate decided in-process: writes its log like a spawned step would.
function verdict(name, log, ok, output) {
  const logPath = path.join(LOG_DIR, log);
  writeFileSync(logPath, output);
  return { name, ok, code: ok ? 0 : 1, ms: 0, logPath, output };
}

// Is a tool on PATH? Spawned rather than resolved by hand so Windows `.cmd` shims, cargo's
// `cargo-<sub>` subcommand lookup and PATH all behave exactly as they would interactively — one
// mechanism instead of a `where`/`which` fork. Output is discarded; only the exit code matters.
function available(cmd, args) {
  return new Promise((resolve) => {
    const child = spawnTool(cmd, args, { stdio: 'ignore' });
    child.on('error', () => resolve(false));
    child.on('close', (code) => resolve(code === 0));
  });
}

let nextestProbe;
const hasNextest = () => (nextestProbe ??= available('cargo', ['nextest', '--version']));

// ── step definitions (commands match CI exactly) ──────────────────────────────
// The privacy canary (decision 4 of the private publication plan, `Plan-Private/`): every tracked or
// untracked, not-ignored file — the set a mirror commit could publish — grepped against the private token
// list in ci/teamcity/privacy-tokens.txt (server addresses, VPN names, personal paths, the CI
// URL). With no token list (a public clone) it reports "skipped" and passes. Mirrors ci.yml's
// `privacy canary`.
const privacyStep = { name: 'privacy', cmd: 'node', args: ['scripts/privacy-canary.mjs'], log: 'privacy.log' };
// The large-file gate (decision 20 of the private publication plan): every file over 1 MB in the
// same tracked-plus-untracked-not-ignored set the privacy canary scans must be named in
// scripts/size-allowlist.txt. Mirrors ci.yml's `size gate`.
const sizeStep    = { name: 'size',    cmd: 'node', args: ['scripts/size-gate.mjs'], log: 'size.log' };
// CLAUDE.md's size budget: the file is loaded into every session and subagent, and it regrows by
// wave-close narrative unless something pushes back. Mirrors ci.yml's `CLAUDE.md budget`.
const claudeMdStep = { name: 'claude-md', cmd: 'node', args: ['scripts/claude-md-budget.mjs'], log: 'claude-md.log' };
// The freeze-listed ECS crate's code-line budget (engine-architecture.md §4). Mirrors ci.yml's `ECS budget`.
const ecsBudgetStep = { name: 'ecs-budget', cmd: 'node', args: ['scripts/ecs-budget.mjs'], log: 'ecs-budget.log' };
// The lists that must agree: nextest's `gpu` override and `no-gpu` filter (both the `needs_gpu_` name),
// the tests that put a device on the card, and `ludion.h` vs `ffi.rs`. Mirrors ci.yml's `lists in step`.
const listsInStepStep = { name: 'lists-in-step', cmd: 'node', args: ['scripts/lists-in-step.mjs'], log: 'lists-in-step.log' };
// No hand-written `error:` / `warning:` print outside the two usage helpers: a runtime failure is a coded
// `Diagnostic` printed through `Display`. Mirrors ci.yml's `diagnostic prints`.
const diagnosticPrintsStep = { name: 'diagnostic-prints', cmd: 'node', args: ['ci/scripts/diagnostic-prints.mjs'], log: 'diagnostic-prints.log' };
// No direct `fs::write` / `fs::copy` / `File::create` in non-test code of the five writer crates: files go through
// `ludion_core::atomic`. Mirrors ci.yml's `atomic writes`.
const atomicWritesStep = { name: 'atomic-writes', cmd: 'node', args: ['ci/scripts/atomic-writes.mjs'], log: 'atomic-writes.log' };
// No schedule talk (relative waves, later phases, milestone tags, forward wave refs) in source, docs or
// strings: a comment describes state, never schedule. Mirrors ci.yml's `schedule comments`.
const scheduleCommentsStep = { name: 'schedule-comments', cmd: 'node', args: ['ci/scripts/schedule-comments.mjs'], log: 'schedule-comments.log' };
// The physics resolved-feature gate (Plan/physics.md W1): `rapier3d`/`parry3d`'s resolved feature
// sets must match the pinned ones exactly and `rayon`/`rayon-core` must be unreachable from
// `ludion-physics`, over `cargo metadata --locked` — no build lock, so it gets a lane of its own.
// Mirrors ci.yml's `physics features`.
const physicsFeaturesStep = { name: 'physics-features', cmd: 'node', args: ['scripts/physics-features.mjs'], log: 'physics-features.log' };
const fmtStep    = { name: 'fmt',    cmd: 'cargo', args: ['fmt', '--all', '--', '--check'], log: 'fmt.log' };
// The voxel generator kit's own tests: pure Node (`node:test`), no cargo, so a lane of its own.
// Named literally: a glob that matches nothing makes `node --test` pass with zero tests.
const voxelKitStep = { name: 'voxel-kit', cmd: 'node', args: ['--test', 'scripts/voxel/grid.test.mjs'], log: 'voxel-kit.log' };
// Vigil's pure-Node tests (town kit, generators): skipped with the game.
const vigilNodeStep = { name: 'vigil-node', cmd: 'node', args: ['--test', 'games/vigil/art/town/lib/kits.test.mjs', 'games/vigil/art/town/lib/labcheck.test.mjs', 'games/vigil/art/town/lib/baseline.test.mjs', 'games/vigil/art/town/lib/sheet.test.mjs', 'games/vigil/art/town/lib/review.test.mjs', 'games/vigil/art/town/lib/shadowbox.test.mjs', 'games/vigil/art/town/lib/stage.test.mjs', 'games/vigil/art/town/lib/scene.test.mjs', 'games/vigil/scripts/generators.test.mjs', 'games/vigil/scripts/city-map.test.mjs', 'games/vigil/scripts/map-layout.test.mjs'], log: 'vigil-node.log' };
const vigilLabStep = { name: 'vigil-lab', cmd: 'node', args: ['games/vigil/art/town/lib/labcheck.mjs'], log: 'vigil-lab.log' };
// The commit wrapper's own tests: pure Node plus git over scratch repositories and a fake `lore`.
// Mirrors ci.yml's `commit wrapper tests`.
const commitWrapperStep = { name: 'commit-wrapper', cmd: 'node', args: ['--test', 'scripts/commit.test.mjs', 'scripts/mirror-lib.test.mjs', 'scripts/public-tree.test.mjs'], log: 'commit-wrapper.log' };
// The public-tree lane (scripts/public-tree.mjs): the git-visible export — no vigil, no private
// paths — must build and pass the `no-gpu` tests and the frontend gates on its own. `--full` only:
// it builds a second workspace. Not in ci.yml: hosted CI checks out the public repository, which
// already is that tree.
const publicTreeStep = { name: 'public-tree', cmd: 'node', args: ['scripts/public-tree.mjs'], log: 'public-tree.log' };
// The physics pin ritual's parser and leftover scan (scripts/physics-pin-ritual-lib.mjs). Mirrors
// ci.yml's `physics pin ritual tests`.
const pinRitualStep = { name: 'pin-ritual', cmd: 'node', args: ['--test', 'scripts/physics-pin-ritual.test.mjs'], log: 'pin-ritual.log' };
const clippyStep = { name: 'clippy', cmd: 'cargo', args: ['clippy', ...SELECT, '--all-targets', '--all-features', '--', '-D', 'warnings'], log: 'clippy.log' };
// vigil's own workspace: the same commands as the root's, run from games/vigil/ so its
// `.cargo/config.toml` (the shared `target/`) and `.config/nextest.toml` apply.
const vigilFmtStep = { name: 'vigil-fmt', cmd: 'cargo', args: ['fmt', '--all', '--', '--check'], cwd: VIGIL, log: 'vigil-fmt.log' };
const vigilClippyStep = { name: 'vigil-clippy', cmd: 'cargo', args: ['clippy', '--workspace', '--all-targets', '--all-features', '--', '-D', 'warnings'], cwd: VIGIL, log: 'vigil-clippy.log' };
// The test lane, in two shapes.
//
// With cargo-nextest installed (the normal case, and what CI does) the workspace's tests schedule
// across every core instead of one test binary at a time; `.config/nextest.toml` bounds the handful
// that must not be co-scheduled. `--no-fail-fast` because one failure would otherwise cancel every
// test still queued, and a fix loop wants every failure from one run. nextest CANNOT run doctests,
// so `doctestStep` is not optional garnish — it is the other half of the same gate. The two share
// cargo's exclusive build lock, so they run sequentially.
//
// Without nextest (a fresh checkout that has not run `cargo install cargo-nextest`) the lane falls
// back to plain `cargo test --workspace`, which runs both halves itself — serially, and slower, but
// gating exactly the same set. `doctest` then reports as skipped rather than pretending to run.
//
// Both shapes set `LUDION_REQUIRE_GPU=1`: this lane gates images (unless `--no-gpu`), so a box
// without a GPU adapter fails here instead of printing SKIPPED and passing (`ludion_core::require_gpu`).
// On a box carrying the reference GPU they also set `LUDION_REQUIRE_REFERENCE_ADAPTER=1`, so wgpu
// picking another adapter fails the byte-exact goldens instead of skipping them (`referenceGpuEnv`).
//
// Both shapes leave out the fps gates (`fpsGateFilter`): they measure wall-clock fps, which the parallel
// lanes saturate, so the `fps-gate` step runs them after every lane has finished.
// The hosted lane's test step: the `no-gpu` profile with the GPU forbidden (`.github/workflows/ci.yml`).
const noGpuStep = { name: 'test', cmd: 'cargo', args: ['nextest', 'run', ...SELECT, ...NO_TESTS, '--profile', 'no-gpu', '--no-fail-fast'], log: 'test.log', env: { LUDION_FORBID_GPU: '1' } };
const referenceGpuEnv = () => {
  const [cmd, args, name] = IS_WINDOWS
    ? ['nvidia-smi', ['--query-gpu=name', '--format=csv,noheader'], 'RTX 3090']
    : ['sysctl', ['-n', 'machdep.cpu.brand_string'], 'Apple M4'];
  try {
    return execFileSync(cmd, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).includes(name)
      ? { LUDION_REQUIRE_REFERENCE_ADAPTER: '1' } : {};
  } catch { return {}; }
};
const nextestStep = (fps) => ({ name: 'test', cmd: 'cargo', args: ['nextest', 'run', ...SELECT, ...NO_TESTS, '--no-fail-fast', ...(fps.expr ? ['-E', `not (${fps.expr})`] : [])], log: 'test.log', env: { LUDION_REQUIRE_GPU: '1', ...referenceGpuEnv() } });
// vigil's test lane takes the root lane's shape (the nextest `no-gpu` profile, nextest, or plain `cargo test`) and env.
const vigilTestStep = (shape) => ({
  name: 'vigil-test', cwd: VIGIL, log: 'vigil-test.log',
  ...(shape === 'no-gpu' ? { cmd: 'cargo', args: ['nextest', 'run', '--workspace', '--profile', 'no-gpu', '--no-fail-fast'], env: { LUDION_FORBID_GPU: '1' } }
    : shape === 'nextest' ? { cmd: 'cargo', args: ['nextest', 'run', '--workspace', '--no-fail-fast'], env: { LUDION_REQUIRE_GPU: '1', ...referenceGpuEnv() } }
    : { cmd: 'cargo', args: ['test', '--workspace', '--no-fail-fast'], env: { LUDION_REQUIRE_GPU: '1', ...referenceGpuEnv() } }),
});
const vigilDoctestStep = { name: 'vigil-doctest', cmd: 'cargo', args: ['test', '--workspace', '--doc', '--no-fail-fast'], cwd: VIGIL, log: 'vigil-doctest.log' };
const doctestStep = { name: 'doctest', cmd: 'cargo', args: ['test', ...DOC_SELECT, '--doc', '--no-fail-fast'], log: 'doctest.log' };
const cargoTestStep = (fps) => ({ name: 'test', cmd: 'cargo', args: ['test', ...SELECT, '--no-fail-fast', ...(fps.prefix ? ['--', '--skip', fps.prefix] : [])], log: 'test.log', env: { LUDION_REQUIRE_GPU: '1', ...referenceGpuEnv() } });

// The fps gates' filter, read from the exclusive override in `.config/nextest.toml` — the one place it
// is written (`scripts/lists-in-step.mjs` checks the block and its callers). The expression keeps its
// spaces: `spawnTool` double-quotes only whitespace-bearing tokens, which is what makes `&` safe on Windows.
function fpsGateFilter() {
  const toml = readFileSync(path.join(REPO_ROOT, '.config', 'nextest.toml'), 'utf8');
  const block = toml.split('[[profile.default.overrides]]').find((b) => /^threads-required\s*=\s*"num-test-threads"/m.test(b));
  const expr = block?.match(/^filter\s*=\s*'([^'\n]+)'/m)?.[1];
  if (!expr) return { error: 'no exclusive `threads-required = "num-test-threads"` override with a single-quoted filter' };
  const bin = expr.match(/binary_id\(=([\w-]+)::(\w+)\)/);
  const prefix = expr.match(/test\(\/\^(\w+)\/\)/)?.[1];
  if (!bin || !prefix) return { error: `filter \`${expr}\` has no binary_id(=crate::binary) / test(/^prefix/) parts` };
  if (expr.includes('"')) return { error: `filter \`${expr}\` contains a double quote, which the Windows spawn cannot pass` };
  return { expr, crate: bin[1], binary: bin[2], prefix };
}
let fpsGate = {};
// The mobile feature set (Plan/ios-target.md): the facade with its default `wire` + `window`
// features off must lint clean, pass its lib tests, and carry no desktop-only crate (winit, gilrs, notify, tokio) —
// the one build shape the `--all-features` clippy above never sees; vigil's lib is also checked for
// aarch64-apple-ios where that target and the iPhoneOS SDK are installed; the facade's lib tests run in
// the shipped and debug shapes too. Cargo-lock bound, so it rides
// the cargo chain. Mirrors the `mobile feature set` step in .github/workflows/ci.yml.
const mobileStep = { name: 'mobile', cmd: 'node', args: ['scripts/mobile-check.mjs'], log: 'mobile.log' };

// The binaries the drift gates and the check-docs smoke run. Built once, then copied out of
// `target/` so the gates can run beside clippy and the test build: nothing they run holds cargo's
// lock, and a relink of `target/debug/ludion` mid-run cannot pull a binary out from under them.
const binsStep = { name: 'bins', cmd: 'cargo', args: ['build', '-q', '-p', 'ludion-cli', '-p', 'hello-paddle'], log: 'bins.log' };
const BIN_DIR = path.join(LOG_DIR, 'bin');
// `ludion sync` every project's `[[import]]` tables into their ignored derived trees (scripts/import-sync.mjs).
// Mirrors the `derived imports sync` step in .github/workflows/ci.yml; it runs before `test`.
const importSyncStep = { name: 'import-sync', cmd: 'node', args: ['scripts/import-sync.mjs'], log: 'import-sync.log' };
// The drift gates: every committed generated file must match a fresh run of its generator. All four
// start from the sources (`*.map.yaml` and marked `.mjs` generators, `*.atlas-source.yaml`/
// `*.font-source.yaml`, `*.mesh-edit.yaml`, `*.voxel.gen.mjs`), pair each source's outputs with
// the `generated_from:` claims both ways, and fail on an empty enumeration. Each spawns the
// prebuilt `ludion` named by LUDION_BIN (scripts/ludion-bin.mjs).
const DRIFT_GATES = ['map-drift', 'voxel-drift', 'atlas-drift', 'mesh-edit-drift'];
// `ludion::run`'s `--check-docs` mode, exercised through an existing game binary: `hello-paddle`
// registers no doc kind, so this walks the "a game that never opted in" path end to end — real
// process boundary, real exit code, real "no server bound" behavior.
const checkDocsArgs = ['--check-docs', '--scene', 'games/hello-paddle/scenes/paddle.yaml'];

// The editor's Tauri host. `editor/app/src-tauri` is a SEPARATE cargo workspace (an empty
// `[workspace]` in its manifest detaches it from the root's `crates/*` + `games/*` members), so the
// `--workspace` gates above cannot see it: without these three steps the PTY host, the asset
// watcher, and the host's wire client are gated by nobody. Same three commands as the engine's, aimed
// through `--manifest-path`. Mirrors the `editor host` steps in .github/workflows/ci.yml `check`.
const TAURI_MANIFEST = ['--manifest-path', 'editor/app/src-tauri/Cargo.toml'];
const tauriFmtStep    = { name: 'tauri-fmt',    cmd: 'cargo', args: ['fmt', ...TAURI_MANIFEST, '--all', '--', '--check'], log: 'tauri-fmt.log' };
const tauriClippyStep = { name: 'tauri-clippy', cmd: 'cargo', args: ['clippy', ...TAURI_MANIFEST, '--all-targets', '--all-features', '--', '-D', 'warnings'], log: 'tauri-clippy.log' };
const tauriTestStep   = { name: 'tauri-test',   cmd: 'cargo', args: ['test', ...TAURI_MANIFEST], log: 'tauri-test.log' };

// The fuzz crate, fmt + clippy. `fuzz/` is a detached workspace with its own lock and target dir and
// a nightly pin in its own toolchain file; run from the root, `--manifest-path` leaves rustup on the
// root's stable pin, so this lints the targets without a nightly or a sanitizer runtime (clippy
// subsumes a plain check). Running the fuzzers stays scheduled (ci/scripts/fuzz.*). Mirrors the
// `fuzz` steps in .github/workflows/ci.yml `check`.
const FUZZ_MANIFEST = ['--manifest-path', 'fuzz/Cargo.toml'];
const fuzzFmtStep = { name: 'fuzz-fmt', cmd: 'cargo', args: ['fmt', ...FUZZ_MANIFEST, '--all', '--', '--check'], log: 'fuzz-fmt.log' };
const fuzzClippyStep = { name: 'fuzz-clippy', cmd: 'cargo', args: ['clippy', ...FUZZ_MANIFEST, '--all-targets', '--locked', '--', '-D', 'warnings'], log: 'fuzz-clippy.log' };

// The gate scripts' own tests (ci/scripts/gate-scripts.test.mjs): the canary's probes, that no gate
// writes a tracked file (schema-drift's own test runs in the node lane), the single-run lock, and the
// `--scope` path classifier (scripts/check-scope.test.mjs).
// Mirrors ci.yml's `Gate script tests` step.
const gateScriptsStep = { name: 'gate-scripts', cmd: 'node', args: ['--test', 'ci/scripts/gate-scripts.test.mjs', 'scripts/check-scope.test.mjs'], log: 'gate-scripts.log' };
// Needs editor/protocol's installed dependencies, so it runs in the node lane behind the npm-deps verdict.
const schemaDriftTestStep = { name: 'schema-drift-test', cmd: 'node', args: ['--test', 'ci/scripts/schema-drift.test.mjs'], log: 'schema-drift-test.log' };

// The gate scripts in ci/scripts/ are the one copy of each gate; CI, TeamCity and this file call them.
const shaderDriftStep = { name: 'shader-drift', cmd: 'node', args: ['ci/scripts/shader-drift.mjs'], log: 'shader-drift.log' };
// `--fast`: the snapshot test runs in the test lane and the npm-deps verdict covers the install.
const schemaDriftStep = { name: 'schema-drift', cmd: 'node', args: ['ci/scripts/schema-drift.mjs', '--fast'], log: 'schema-drift.log' };

const oxlintStep = { name: 'oxlint', cmd: 'npx', args: ['oxlint'], cwd: 'editor/app', log: 'oxlint.log' };
const tscStep    = { name: 'tsc',    cmd: 'npx', args: ['tsc', '-b'], cwd: 'editor/app', log: 'tsc.log' };
const vitestStep = { name: 'vitest', cmd: 'npx', args: ['vitest', 'run'], cwd: 'editor/app', log: 'vitest.log' };

const canaryStep = { name: 'denylist', cmd: 'node', args: ['ci/scripts/denylist-canary.mjs'], log: 'denylist.log' };

// Dependency licenses and RustSec advisories over both cargo workspaces (decision 10 of the private
// publication plan, `Plan-Private/`); deny.toml holds the allowlist and the reasoned ignores. Mirrors
// ci.yml's `deny` job.
const denySteps = [
  { name: 'deny', cmd: 'cargo', args: ['deny', 'check', 'advisories', 'licenses'], log: 'deny.log' },
  {
    name: 'deny-tauri',
    cmd: 'cargo',
    args: ['deny', '--manifest-path', 'editor/app/src-tauri/Cargo.toml', '--config', 'deny.toml', 'check', 'advisories', 'licenses'],
    log: 'deny-tauri.log',
  },
];

// Games are `publish = false` and excluded here for the same reason: `cargo package` packages a
// workspace member regardless of `publish = false` (that flag only gates `cargo publish`), so a
// games/* crate would otherwise hit this gate's dependency-version check for no reason — it never
// ships to crates.io.
const GAMES = ['breakout', 'hello-paddle', 'shmup-stress', 'sokoban'];
const excludeGames = GAMES.flatMap((g) => ['--exclude', g]);
// Full verify (each crate rebuilt from its isolated tarball): possible because `ludion-cli`
// vendors its embedded docs inside the crate (`embedded-docs/`, drift-gated against `docs/`), so
// every tarball is self-contained. Workspace-wide is load-bearing — a per-crate `--dry-run` would
// try to resolve the not-yet-published ludion deps against crates.io and fail.
//
// The verify builds run in their own `target/package-verify` (CARGO_TARGET_DIR) with the ludion-*
// entries purged first. Both halves matter: cargo fingerprints registry-sourced dependencies as
// immutable — a fair assumption for crates.io, wrong for our re-staged unpublished crates, which
// change content at a constant 0.1.0 — so without the purge a prior run's ludion rmeta is reused
// as "Fresh" and the gate verifies stale code (false greens included). The isolated dir keeps the
// purge from also invalidating the day-to-day `target/` cache; third-party deps stay warm in it.
const packageStep = {
  name: 'package', cmd: 'cargo',
  args: ['package', '--workspace', '--allow-dirty', ...excludeGames],
  env: { CARGO_TARGET_DIR: path.join(REPO_ROOT, 'target', 'package-verify') },
  log: 'package.log',
};

// Removes every cached ludion-* artifact a packaging verify could wrongly reuse: the staging dir
// (tmp tarballs + overlay registry), the ludion fingerprints/artifacts in the verify target dir,
// and the staged unpacks under CARGO_HOME's registry/src (cargo re-extracts them from the fresh
// tarballs; deleting is always safe).
function purgePackageVerifyCaches() {
  const verifyRoot = path.join(REPO_ROOT, 'target', 'package-verify');
  // Both staging locations: `target/package` (default CARGO_TARGET_DIR) and the one cargo actually
  // uses under the isolated dir. The overlay registry index inside survives across runs and goes
  // stale the same way the fingerprints do ("no hash listed for <crate>" cargo internal error).
  rmSync(path.join(REPO_ROOT, 'target', 'package'), { recursive: true, force: true });
  rmSync(path.join(verifyRoot, 'package'), { recursive: true, force: true });
  if (existsSync(verifyRoot)) {
    for (const profile of readdirSync(verifyRoot)) {
      for (const sub of ['.fingerprint', 'deps']) {
        const dir = path.join(verifyRoot, profile, sub);
        if (!existsSync(dir)) continue;
        for (const entry of readdirSync(dir)) {
          if (entry.startsWith('ludion') || entry.startsWith('libludion')) {
            rmSync(path.join(dir, entry), { recursive: true, force: true });
          }
        }
      }
    }
  }
  const registrySrc = path.join(process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo'), 'registry', 'src');
  if (existsSync(registrySrc)) {
    for (const reg of readdirSync(registrySrc)) {
      for (const entry of readdirSync(path.join(registrySrc, reg))) {
        if (entry.startsWith('ludion')) {
          rmSync(path.join(registrySrc, reg, entry), { recursive: true, force: true });
        }
      }
    }
  }
}
// Warnings are errors: docs.rs is a publication surface — a broken intra-doc link is a defect there, not a nicety.
const docStep = {
  name: 'doc', cmd: 'cargo',
  args: ['doc', '--workspace', '--no-deps', ...excludeGames],
  env: { RUSTDOCFLAGS: '-D warnings' },
  log: 'doc.log',
};

// The `#[ignore]`d campaigns, each one test binary run with `--ignored`. Each prefers nextest's
// ignored-only filter (what CI uses) and falls back to a plain `cargo test --workspace --test <binary> -- --ignored`
// (every member with a test binary of that name) when nextest is not installed. Mirror ci.yml's jobs of the same names.
//   thread-identity  the 1/2/8-thread hash-identity gate (Plan/deterministic-job-system.md J0): a
//                    full `run.lrp` replay driven once per thread count, hashed every frame.
//   probe            vigil's triage-probe campaign (a private plan): five
//                    quest probes assert the triage-forcing invariant and that each quest completes
//                    on its intended route; the rest are measurement harnesses that fail only on a
//                    panic.
//   soak             the hash soak (a private plan, decision 11): two engines through the
//                    full 2700 s shipped clock, per-frame `WorldHash` traces compared.
// `thread-identity` runs at the root (breakout's corpus); `vigil-thread-identity`, `probe` and `soak`
// run from games/vigil/ (`cwd`) and skip when it is absent.
async function ignoredStep(name, binary, timeoutMin, cwd) {
  if (cwd === VIGIL && !hasVigil) return vigilAbsent(name);
  return runStep(
    (await hasNextest())
      ? { name, cmd: 'cargo', args: ['nextest', 'run', '-E', `binary(${binary})`, '--run-ignored', 'ignored-only'], log: `${name}.log`, timeoutMin, cwd }
      : { name, cmd: 'cargo', args: ['test', '--workspace', '--test', binary, '--', '--ignored'], log: `${name}.log`, timeoutMin, cwd },
  );
}

// ── lanes ─────────────────────────────────────────────────────────────────────
// The engine workspace's single `target/` serializes every cargo build behind one exclusive lock,
// so the cargo chain (clippy → test → doctest → mobile) is the long pole. Everything else that
// needs the workspace's code runs off a prebuilt binary beside it.
async function heavyLane() {
  // shader-drift goes first and stands alone: stale bindings may break the renderer's build, and
  // the gate's own message is the better report than the compile error that follows.
  const out = [await runStep(shaderDriftStep)];
  // Only vigil carries `[[import]]` tables and only the drift gates and check-docs run the binaries,
  // so a scoped run over root crates alone needs neither the build nor the sync.
  if (!inScope('bins')) {
    out.push(outOfScope('bins'));
    return [...out, ...[...DRIFT_GATES, 'vigil-textures', 'check-docs'].map(outOfScope), ...await cargoChain(Promise.resolve(outOfScope('import-sync')))];
  }
  const bins = await runStep(binsStep);
  out.push(bins);
  const copied = bins.ok ? copyBins() : null;
  // The derived import trees must be fresh before the test step; the sync runs off the copied
  // binary, so it overlaps clippy rather than waiting for it.
  const syncDone = copied
    ? runStep({ ...importSyncStep, env: { LUDION_BIN: copied.ludion } })
    : Promise.resolve(skipped('import-sync', 'bins failed'));
  const [binResults, cargoResults] = await Promise.all([
    copied ? binLane(copied, syncDone) : [...DRIFT_GATES, 'vigil-textures', 'check-docs'].map((n) => skipped(n, 'bins failed')),
    cargoChain(syncDone),
  ]);
  return [...out, ...binResults, ...cargoResults];
}

// Copies the built binaries out of `target/`; the lanes that spawn them use the copies.
function copyBins() {
  mkdirSync(BIN_DIR, { recursive: true });
  const targetDir = path.join(process.env.CARGO_TARGET_DIR ?? path.join(REPO_ROOT, 'target'), 'debug');
  const bin = (name) => {
    const dst = path.join(BIN_DIR, name + EXE);
    copyFileSync(path.join(targetDir, name + EXE), dst, fsConstants.COPYFILE_FICLONE);
    return dst;
  };
  return { ludion: bin('ludion'), helloPaddle: bin('hello-paddle') };
}

// The drift gates and the check-docs smoke, off the prebuilt binaries — cheap, and none holds a
// cargo lock, so the lane hides under clippy. The script-generated scenes and prefabs read the derived
// import trees, so the lane waits for the sync.
async function binLane(copied, syncDone) {
  const env = { LUDION_BIN: copied.ludion };
  await syncDone;
  const out = [];
  for (const name of DRIFT_GATES) {
    out.push(inScope('drift') ? await runStep({ name, cmd: 'node', args: [`scripts/${name}.mjs`], env, log: `${name}.log` }) : outOfScope(name));
  }
  // Vigil is Lore-only: its texture-set gate is skipped with the game, and is not in DRIFT_GATES (ci.yml mirrors those).
  out.push(!hasVigil ? vigilAbsent('vigil-textures') : (inScope('drift') || scope.packages.some((p) => ['ludion-texgen', 'ludion-cli'].includes(p))) ? await runStep({ name: 'vigil-textures', cmd: 'node', args: ['scripts/vigil-texture-drift.mjs'], env, log: 'vigil-textures.log' }) : outOfScope('vigil-textures'));
  out.push(inScope('check-docs') ? await runStep({ name: 'check-docs', cmd: copied.helloPaddle, args: checkDocsArgs, log: 'smoke-check-docs.log' }) : outOfScope('check-docs'));
  return out;
}

// A compile failure in clippy makes the test build a redundant recompile-failure; a test failure
// leaves doctest and mobile independent, so they still run.
const VIGIL_CARGO_STEPS = ['vigil-clippy', 'vigil-test', 'vigil-doctest'];
async function cargoChain(syncDone) {
  const root = inScope('clippy');
  const clippy = root ? await runStep(clippyStep) : outOfScope('clippy');
  if (!clippy.ok) return [clippy, await syncDone, ...['test', 'doctest', ...VIGIL_CARGO_STEPS, 'mobile'].map((n) => (hasVigil || !VIGIL_CARGO_STEPS.includes(n) ? skipped(n, 'clippy failed') : vigilAbsent(n)))];
  const useNextest = await hasNextest();
  const synced = await syncDone;
  if (noGpu && !useNextest) {
    const missing = verdict('test', 'test.log', false, '--no-gpu runs the nextest `no-gpu` profile: install cargo-nextest (`cargo install cargo-nextest --locked`)');
    return [clippy, synced, missing, skipped('doctest', 'no cargo-nextest'), ...VIGIL_CARGO_STEPS.map((n) => (hasVigil ? skipped(n, 'no cargo-nextest') : vigilAbsent(n))), await runStep(mobileStep)];
  }
  const out = [clippy, synced];
  if (root) {
    out.push(await runStep(noGpu ? noGpuStep : useNextest ? nextestStep(fpsGate) : cargoTestStep(fpsGate)));
    out.push(!useNextest ? skipped('doctest', 'ran inside cargo test (no cargo-nextest)') : inScope('doctest') ? await runStep(doctestStep) : skipped('doctest', 'no library in scope'));
  } else {
    out.push(outOfScope('test'), outOfScope('doctest'));
  }
  out.push(...(inScope('vigil') ? await vigilChain(useNextest) : VIGIL_CARGO_STEPS.map(outOfScope)));
  out.push(inScope('mobile') ? await runStep(mobileStep) : outOfScope('mobile'));
  return out;
}

// vigil's clippy, then its tests (a failed clippy skips only those two). Runs after the root's
// doctests: the same cargo lock over the shared `target/`, and after the import sync.
async function vigilChain(useNextest) {
  if (!hasVigil) return VIGIL_CARGO_STEPS.map(vigilAbsent);
  const clippy = await runStep(vigilClippyStep);
  if (!clippy.ok) return [clippy, skipped('vigil-test', 'vigil-clippy failed'), skipped('vigil-doctest', 'vigil-clippy failed')];
  const test = await runStep(vigilTestStep(noGpu ? 'no-gpu' : useNextest ? 'nextest' : 'cargo'));
  return [clippy, test, useNextest ? await runStep(vigilDoctestStep) : skipped('vigil-doctest', 'ran inside cargo test (no cargo-nextest)')];
}

// The editor's Tauri host: its detached workspace has its own `target/`, so it takes a DIFFERENT
// lock than the heavy lane and genuinely overlaps it.
const tauriLane = () => runChain([tauriFmtStep, tauriClippyStep, tauriTestStep]);

// npm installs are not part of the gate (CI runs `npm ci`; locally it is a manual step), so a
// dependency added to a package.json and not yet installed would surface as a cryptic tsc or
// vitest error. Checked up front instead: every declared dependency must be in node_modules.
function npmDepsVerdict(dirs) {
  const missing = [];
  for (const dir of dirs) {
    const manifest = JSON.parse(readFileSync(path.join(REPO_ROOT, dir, 'package.json'), 'utf8'));
    for (const dep of Object.keys({ ...manifest.dependencies, ...manifest.devDependencies })) {
      if (!existsSync(path.join(REPO_ROOT, dir, 'node_modules', dep, 'package.json'))) missing.push({ dir, dep });
    }
  }
  if (!missing.length) return null;
  const dirsToFix = [...new Set(missing.map((m) => m.dir))];
  return verdict('npm-deps', 'npm-deps.log', false, [
    `NPM DEPS: declared but not installed: ${missing.map((m) => `${m.dep} (${m.dir})`).join(', ')}.`,
    `Install them: ${dirsToFix.map((d) => `cd ${d} && npm ci`).join('; ')}.`,
    '',
  ].join('\n'));
}

// The editor frontend checks are independent, so all run and each is reported.
async function nodeLane() {
  const dirs = [...(runFrontend ? ['editor/app'] : []), ...(runSchema ? ['editor/protocol'] : [])];
  const deps = npmDepsVerdict(dirs);
  if (deps) {
    const names = [...(runFrontend ? ['oxlint', 'tsc', 'vitest'] : []), ...(runSchema ? ['schema-drift', 'schema-drift-test'] : [])];
    return [deps, ...names.map((n) => skipped(n, 'npm-deps failed'))];
  }
  const out = [];
  if (runFrontend) {
    out.push(await runStep(oxlintStep));
    out.push(await runStep(tscStep));
    out.push(await runStep(vitestStep));
  }
  if (runSchema) {
    out.push(await runStep(schemaDriftStep));
    out.push(await runStep(schemaDriftTestStep));
  }
  return out;
}

// ── --fix (opt-in, mutates the tree) ──────────────────────────────────────────

// Re-mirrors docs/*.md into crates/ludion-cli/embedded-docs/ — the vendored copy that ships inside
// the crates.io tarball and that `ludion docs` embeds. `ludion-cli`'s build script only CHECKS this
// mirror (a build script must not write into the committed tree); this is the writing half, and the
// build failure names this command. Pure Node on purpose: clearing the failure must never require
// the cargo build that is failing.
function syncEmbeddedDocs() {
  const src = path.join(REPO_ROOT, 'docs');
  const dst = path.join(REPO_ROOT, 'crates', 'ludion-cli', 'embedded-docs');
  if (!existsSync(src) || !existsSync(dst)) return;
  const topics = readdirSync(src).filter((f) => f.endsWith('.md'));
  let changed = 0;
  for (const topic of topics) {
    const body = readFileSync(path.join(src, topic));
    const mirrored = path.join(dst, topic);
    // Write only on a real difference, so unchanged docs keep their mtime and cargo stays quiet.
    if (existsSync(mirrored) && readFileSync(mirrored).equals(body)) continue;
    writeFileAtomic(mirrored, body);
    changed++;
  }
  for (const stale of readdirSync(dst).filter((f) => f.endsWith('.md') && !topics.includes(f))) {
    unlinkSync(path.join(dst, stale));
    changed++;
  }
  if (changed) console.log(`  (embedded-docs: re-mirrored ${changed} file(s) from docs/)`);
}

async function applyFixes() {
  if (runRust) syncEmbeddedDocs();
  const steps = [];
  // Regenerate shader_gen.rs BEFORE fmt/the Rust lane (decision 12) so the build sees fresh bindings.
  if (runRust) steps.push({ name: 'shader-gen', cmd: 'cargo', args: ['run', '-q', '-p', 'ludion-shader-bindgen'], log: 'fix-shader-gen.log' });
  // The clippy.toml denylists are generated; regenerate them before the Rust lane lints against them.
  if (runRust) steps.push({ name: 'clippy-denylist', cmd: 'node', args: ['scripts/clippy-denylist.mjs', '--write'], log: 'fix-clippy-denylist.log' });
  if (runRust) steps.push({ name: 'fmt-fix', cmd: 'cargo', args: ['fmt', '--all'], log: 'fix-fmt.log' });
  // The detached editor-host and vigil workspaces are invisible to the `--all` above.
  if (runRust && hasVigil) steps.push({ name: 'vigil-fmt-fix', cmd: 'cargo', args: ['fmt', '--all'], cwd: VIGIL, log: 'fix-vigil-fmt.log' });
  if (runRust) steps.push({ name: 'tauri-fmt-fix', cmd: 'cargo', args: ['fmt', ...TAURI_MANIFEST, '--all'], log: 'fix-tauri-fmt.log' });
  if (runFrontend) steps.push({ name: 'oxlint-fix', cmd: 'npx', args: ['oxlint', '--fix'], cwd: 'editor/app', log: 'fix-oxlint.log' });
  for (const s of steps) {
    const r = await runStep(s);
    if (!r.ok) console.log(`  (${s.name} reported issues — see ${path.relative(REPO_ROOT, r.logPath)})`);
  }
}

// ── timing history ──────────────────────────────────────────────────────────
// One JSON line per run in target/ (machine-local, never committed: timings are a property of the
// box). Only a green run is a baseline — a failure skips steps and shortens the run.
function readHistory() {
  try {
    return readFileSync(HISTORY, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));
  } catch {
    return [];
  }
}

function recordHistory(results, totalMs, ok) {
  const steps = Object.fromEntries(results.filter((r) => !r.skipped).map((r) => [r.name, Math.round(r.ms)]));
  try {
    mkdirSync(path.dirname(HISTORY), { recursive: true });
    appendFileSync(HISTORY, JSON.stringify({ at: new Date().toISOString(), mode, ok, totalMs: Math.round(totalMs), steps, ...(scope ? { note: scope.paths.join(' ') } : {}) }) + '\n');
  } catch { /* history is a convenience; never fail a run over it */ }
}

function printHistory() {
  const runs = readHistory().slice(-10);
  if (!runs.length) {
    console.log(`check: no runs recorded yet in ${path.relative(REPO_ROOT, HISTORY)}`);
    return;
  }
  // Steps a past run had that this script no longer runs still print, after the current ones.
  const seen = [...new Set(runs.flatMap((r) => Object.keys(r.steps)))];
  const names = [...ORDER.filter((n) => seen.includes(n)), ...seen.filter((n) => !ORDER.includes(n))];
  const cell = (ms) => (ms === undefined ? '—' : (ms / 1000).toFixed(1)).padStart(7);
  console.log(`${'step'.padEnd(17)}${runs.map((_, i) => `#${i + 1}`.padStart(7)).join('')}`);
  for (const n of names) console.log(`${n.padEnd(17)}${runs.map((r) => cell(r.steps[n])).join('')}`);
  console.log(`${'TOTAL'.padEnd(17)}${runs.map((r) => cell(r.totalMs)).join('')}`);
  console.log('');
  runs.forEach((r, i) => console.log(`  #${i + 1}  ${r.at}  ${r.mode}  ${r.ok ? 'green' : 'FAILED'}${r.note ? `  ${r.note}` : ''}`));
}

// ── reporting ───────────────────────────────────────────────────────────────
const ORDER = ['privacy', 'size', 'claude-md', 'ecs-budget', 'lists-in-step', 'diagnostic-prints', 'atomic-writes', 'schedule-comments', 'physics-features', 'fmt', 'vigil-fmt', 'voxel-kit', 'vigil-node', 'vigil-lab', 'commit-wrapper', 'pin-ritual', 'shader-drift', 'bins', 'import-sync', ...DRIFT_GATES, 'vigil-textures', 'check-docs', 'clippy', 'test', 'doctest', 'vigil-clippy', 'vigil-test', 'vigil-doctest', 'mobile', 'tauri-fmt', 'tauri-clippy', 'tauri-test', 'fuzz-fmt', 'fuzz-clippy', 'gate-scripts', 'npm-deps', 'oxlint', 'tsc', 'vitest', 'schema-drift', 'schema-drift-test', 'fps-gate', 'denylist', 'package', 'doc', 'thread-identity', 'vigil-thread-identity', 'probe', 'public-tree', 'soak'];
const YELLOW = '\x1b[33m'; const GREEN = '\x1b[32m'; const RED = '\x1b[31m'; const DIM = '\x1b[2m'; const BOLD = '\x1b[1m'; const RST = '\x1b[0m';

function line(r) {
  const name = r.name.padEnd(16);
  if (r.skipped) return `${DIM}⊘ ${name} skipped (${r.reason})${RST}`;
  const secs = `${(r.ms / 1000).toFixed(1)}s`.padStart(7);
  return r.ok ? `${GREEN}✓${RST} ${name}${DIM}${secs}${RST}` : `${RED}✗ ${name}${secs}${RST}`;
}

function report(results, totalMs, previous) {
  const sorted = [...results].sort((a, b) => ORDER.indexOf(a.name) - ORDER.indexOf(b.name));
  console.log('');
  const unscoped = sorted.filter((r) => r.skipped && r.reason === 'out of scope');
  for (const r of sorted) if (!unscoped.includes(r)) console.log('  ' + line(r));
  if (unscoped.length) console.log(`  ${DIM}⊘ out of scope: ${unscoped.map((r) => r.name).join(', ')}${RST}`);
  console.log('');

  // Surface nextest's slow-test list (final-status-level = "slow") in the summary itself, so the
  // cost of a test can never go invisible again — the pre-nextest suite grew past 18 minutes with
  // nobody able to name the expensive tests from the default output.
  const test = sorted.find((r) => r.name === 'test' && r.ok && !r.skipped && r.output);
  // 'SLOW [>' is the live in-progress marker; the final-status line carries the measured time.
  const slow = test ? test.output.split('\n').filter((l) => l.includes('SLOW [') && !l.includes('SLOW [>')) : [];
  if (slow.length) {
    console.log(`  ${DIM}slow tests (>60s):${RST}`);
    for (const s of slow) console.log(`  ${DIM}  ${s.trim()}${RST}`);
    console.log('');
  }

  const failures = sorted.filter((r) => !r.ok && !r.skipped);
  const total = `${(totalMs / 1000).toFixed(1)}s`;
  if (failures.length === 0) {
    const delta = previous
      ? `, last green ${mode} run ${(previous.totalMs / 1000).toFixed(1)}s — node scripts/check.mjs --history`
      : '';
    console.log(`${GREEN}${BOLD}ALL GREEN${RST} ${DIM}(${total}${delta})${RST}`);
    return 0;
  }
  for (const f of failures) {
    const rel = path.relative(REPO_ROOT, f.logPath);
    console.log(`\n${RED}${BOLD}── ${f.name} ──${RST} ${DIM}(full log: ${rel})${RST}`);
    const lines = f.output.trimEnd().split('\n');
    const CAP = 300;
    if (lines.length > CAP) {
      console.log(`${DIM}… showing last ${CAP} of ${lines.length} lines; full detail in ${rel}${RST}`);
      console.log(lines.slice(-CAP).join('\n'));
    } else {
      console.log(lines.join('\n'));
    }
  }
  console.log(`\n${RED}${BOLD}FAILED${RST}: ${failures.map((f) => f.name).join(', ')} ${DIM}(${total})${RST}`);
  return 1;
}

// ── scoped-run coordination ───────────────────────────────────────────────────
function pidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return err.code === 'EPERM';
  }
}

// A scoped run refuses while a default run holds target/check.lock, and prunes the log dirs of dead
// scoped runs; its own log dir (named by its pid) is what marks it live.
function scopeLock() {
  let holder = null;
  try { holder = JSON.parse(readFileSync(LOCK, 'utf8')); } catch { /* no default run */ }
  if (holder && Number.isInteger(holder.pid) && pidAlive(holder.pid)) return { ok: false, holder };
  if (existsSync(SCOPE_ROOT)) {
    for (const entry of readdirSync(SCOPE_ROOT)) {
      const pid = Number(entry);
      if (pid !== process.pid && !(Number.isInteger(pid) && pidAlive(pid))) rmSync(path.join(SCOPE_ROOT, entry), { recursive: true, force: true });
    }
  }
  return { ok: true, release() {} };
}

function warnIfScopedRunsLive() {
  if (!existsSync(SCOPE_ROOT)) return;
  const live = readdirSync(SCOPE_ROOT).map(Number).filter((pid) => Number.isInteger(pid) && pidAlive(pid));
  if (live.length) console.log(`${YELLOW}check: scoped run(s) live (pid ${live.join(', ')}) — they share the box, so this run's wall-clock gates and timings may suffer.${RST}`);
}

// ── main ──────────────────────────────────────────────────────────────────────
async function main() {
  if (values.history) {
    printHistory();
    return 0;
  }
  // A scoped run takes no lock of its own (it shares nothing but cargo's build lock), but waits out a
  // default run: that run's wall-clock gates need the box. A default run only warns about live scoped runs.
  const lock = scope ? scopeLock() : acquireLock(LOCK, { mode });
  if (!lock.ok) {
    const h = lock.holder;
    console.error(`check: busy — another check.mjs run (pid ${h.pid}, ${h.mode}, started ${h.startedAt}) holds target/check.lock. Wait for it; if that process is gone, delete the file.`);
    return 2;
  }
  if (!scope) warnIfScopedRunsLive();
  process.on('exit', lock.release);
  for (const [sig, code] of [['SIGINT', 130], ['SIGTERM', 143], ['SIGHUP', 129]]) process.on(sig, () => { for (const c of liveChildren) killTree(c); lock.release(); process.exit(code); });
  rmSync(LOG_DIR, { recursive: true, force: true });
  mkdirSync(LOG_DIR, { recursive: true });
  if (runRust) {
    try {
      rmSync(TEST_TEMP, { recursive: true, force: true, maxRetries: 3 });
    } catch (error) {
      console.log(`${DIM}could not clear ${TEST_TEMP}: ${error.message}${RST}`);
    }
    mkdirSync(TEST_TEMP, { recursive: true });
  }

  console.log(`Ludion check ${DIM}— ${mode} — logs in ${path.relative(REPO_ROOT, LOG_DIR)}/${RST}`);
  if (scope) {
    console.log(`${DIM}scope: ${scope.paths.join(' ')}${values.deps ? ' (with dependents)' : ''}${scope.packages.length ? ` → ${scope.packages.join(', ')}` : ''}${RST}`);
    console.log(`${YELLOW}check: a scoped run is the inner loop, not a commit gate — the default suite still runs before the commit lands.${RST}`);
  }
  if (runRust) warnIfDepsBloated();
  const start = process.hrtime.bigint();

  if (values.fix) await applyFixes();

  fpsGate = fpsGateFilter();
  const single = (step) => runStep(step).then((r) => [r]);
  const lanes = [];
  // The cheap node gates, every mode; sequenced so their logs never interleave.
  lanes.push(runChainAll([privacyStep, sizeStep, claudeMdStep, ecsBudgetStep, listsInStepStep, diagnosticPrintsStep, atomicWritesStep, scheduleCommentsStep]));
  if (runRust) {
    // No build lock among these, so each gets a lane of its own.
    const vigilLane = (step) => (hasVigil ? single(step) : Promise.resolve([vigilAbsent(step.name)]));
    const gate = (step, lane = single) => (inScope(step.name) ? lane(step) : Promise.resolve([outOfScope(step.name)]));
    lanes.push(gate(fmtStep), gate(vigilFmtStep, vigilLane), gate(voxelKitStep), gate(vigilNodeStep, vigilLane), gate(vigilLabStep, vigilLane), gate(commitWrapperStep), gate(gateScriptsStep), gate(pinRitualStep), gate(physicsFeaturesStep));
    lanes.push(inScope('heavy') ? heavyLane() : Promise.resolve(['shader-drift', 'bins', 'import-sync', ...DRIFT_GATES, 'vigil-textures', 'check-docs', 'clippy', 'test', 'doctest', ...VIGIL_CARGO_STEPS, 'mobile'].map(outOfScope)));
    lanes.push(inScope('tauri') ? tauriLane() : Promise.resolve(['tauri-fmt', 'tauri-clippy', 'tauri-test'].map(outOfScope)));
    // detached workspace, own target/ and lock
    lanes.push(inScope('fuzz') ? runChainAll([fuzzFmtStep, fuzzClippyStep]) : Promise.resolve(['fuzz-fmt', 'fuzz-clippy'].map(outOfScope)));
  }
  if (runFrontend || runSchema) lanes.push(nodeLane());

  const results = (await Promise.all(lanes)).flat();

  // The fps gates, alone on the box: every parallel lane has finished, so nothing steals the CPU the
  // wall-clock measurement needs. A failed `test` still runs them.
  if (runRust) {
    const test = results.find((r) => r.name === 'test');
    if (noGpu) {
      results.push(skipped('fps-gate', '--no-gpu'));
    } else if (scope && !fpsGate.error && !scope.packages.includes(fpsGate.crate)) {
      results.push(outOfScope('fps-gate'));
    } else if (test?.skipped) {
      results.push(skipped('fps-gate', test.reason));
    } else if (fpsGate.error) {
      results.push(verdict('fps-gate', 'fps-gate.log', false, `fps-gate: cannot read the filter from .config/nextest.toml: ${fpsGate.error}\nthe exclusive override must stay as scripts/lists-in-step.mjs expects`));
    } else {
      const args = (await hasNextest())
        ? ['nextest', 'run', ...(scope ? ['-p', fpsGate.crate] : ['--workspace']), '--no-fail-fast', '-E', fpsGate.expr]
        : ['test', ...(scope ? ['-p', fpsGate.crate] : ['--workspace']), '--test', fpsGate.binary, '--', fpsGate.prefix, '--test-threads=1'];
      results.push(await runStep({ name: 'fps-gate', cmd: 'cargo', args, log: 'fps-gate.log', env: { LUDION_REQUIRE_GPU: '1' } }));
    }
  }

  // The `--full` tail shares cargo's exclusive build lock, so it is serialized, last.
  if (runFullTail) {
    if (await available('cargo', ['deny', '--version'])) {
      for (const step of denySteps) results.push(await runStep(step));
    } else {
      results.push(verdict('deny', 'deny.log', false, 'cargo-deny is not installed: run `cargo install cargo-deny --locked` once per machine'));
    }
    results.push(await runStep(canaryStep));
    purgePackageVerifyCaches();
    results.push(await runStep(packageStep));
    results.push(await runStep(docStep));
    results.push(await ignoredStep('thread-identity', 'thread_identity'));
    results.push(await ignoredStep('vigil-thread-identity', 'thread_identity', undefined, VIGIL));
    results.push(await ignoredStep('probe', 'probe', undefined, VIGIL));
    results.push(await runStep(publicTreeStep));
  }
  if (runSoak) results.push(await ignoredStep('soak', 'soak', 200, VIGIL));

  if (runRust && results.filter((r) => TEST_TEMP_STEPS.has(r.name)).every((r) => r.ok)) {
    try {
      rmSync(TEST_TEMP, { recursive: true, force: true, maxRetries: 3 });
    } catch (error) {
      console.log(`${DIM}could not remove ${TEST_TEMP}: ${error.message}${RST}`);
    }
  }

  const totalMs = Number(process.hrtime.bigint() - start) / 1e6;
  const previous = readHistory().filter((r) => r.mode === mode && r.ok && (!scope || r.note === scope.paths.join(' '))).at(-1);
  const code = report(results, totalMs, previous);
  if (!values.fix) recordHistory(results, totalMs, code === 0);
  return code;
}

// Runs every step in order regardless of failures (independent gates, one lane for tidy logs).
async function runChainAll(steps) {
  const out = [];
  for (const step of steps) out.push(await runStep(step));
  return out;
}

main().then((code) => process.exit(code), (err) => {
  console.error(err);
  process.exit(2);
});
