#!/usr/bin/env node
// The physics pin-upgrade ritual (Plan/physics.md W4), scripted. Run it on demand; it is not in
// check.mjs's default lane or CI.
//
// rapier3d is pinned with `=` because moving the pin is a hash-moving event, not a routine bump.
// The ritual, as this script runs it:
//   1. (--to <x.y.z>) move the pin: rewrite the one `rapier3d = { version = "=..."` line in the
//      root Cargo.toml, `cargo update -p rapier3d --precise <x.y.z>`, print what moved in
//      Cargo.lock, every rapier-mentioning text file that still names the old version, and the reminder to
//      re-audit the clippy determinism denylist against the new registry source.
//   2. the resolved-feature gate: `node scripts/physics-features.mjs`. A failure stops the run.
//   3-4. the physics gates under nextest, all CPU: every ludion-physics test binary, the
//      ludion-engine physics_* and ragdoll* test modules (of its merged `it` binary), the
//      ludion-cli `physics` module (of its merged `it` binary), then the
//      #[ignore]d thread_identity scenes. Steps 3 (cross-arch traces) and 4 (in-process
//      determinism) are the same runs: on ONE machine only the in-process tests carry meaning,
//      because the committed traces and replays were recorded by the previous pin.
//   5. (printed, never run) the re-bless order: schema snapshot, goldens, budgets, the .lrp corpus
//      last.
//
// Verdicts, and the exit code:
//   PASS   0  every gate is green: the pin does not move any hash.
//   FAIL   1  a gate failed that is not a committed-artifact comparison (or the build broke): the
//             pin does not move; record the finding — it is exit trigger (b) of Plan/physics.md.
//   MOVED  3  the only failures are tests that compare against a committed trace or replay
//             (COMMITTED_COMPARISONS below). That is an ordinary hash-moving change, not a defect.
//   usage  2  bad flags, no nextest, an unreadable Cargo.toml.
//
// Flags:
//   --to <x.y.z>     do step 1 first (a same-version --to is a no-op and says so). Refused while
//                    vendor/rapier3d or vendor/parry3d exists: the pin carries vendored patches — port each
//                    vendor/<crate>/LUDION-PATCH.md onto <x.y.z> first.
//   --bless-traces   on the FIRST machine: run steps 3-4 unblessed; only on a MOVED verdict re-record
//                    the five committed traces (LUDION_BLESS=1 on exactly the tests that write one)
//                    and re-run steps 3-4. (PASS: nothing needs blessing; FAIL: nothing is blessed.)
//                    The replay tests then pass — they re-derive from scene + seed + inputs and
//                    compare hashes to the trace, never bytes to the .lrp — but the two committed
//                    .lrp files still carry the OLD pin's physics blob in their keyframes and are
//                    stale until step 5 re-records them (the .lrp corpus last); the run lists them.
//                    The candidate pin and traces go to the second machine, which runs this script
//                    with no flags: green there is step 3 (cross-arch) passing. Only then step 5.
//   --help
//
// Shared checkout: moving the pin rewrites Cargo.toml and Cargo.lock and rebuilds rapier for every
// instance on the box; another lane running meanwhile tests the candidate pin. Clear the box first.
// Zero npm dependencies — Node built-ins only. Runs unchanged on Windows and macOS.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { writeFileAtomic } from './atomic-write.mjs';
import { parseArgs } from 'node:util';

import { parseFailedTests, scanLeftovers } from './physics-pin-ritual-lib.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BUILD_DIR = path.join(REPO_ROOT, '.build');
const CARGO_TOML = path.join(REPO_ROOT, 'Cargo.toml');
const CARGO_LOCK = path.join(REPO_ROOT, 'Cargo.lock');

// Tests that compare against a committed trace or replay; when every failure is one of these the
// verdict is MOVED, not FAIL. Keyed `binary::test`.
const COMMITTED_COMPARISONS = new Set([
  // the committed WorldHash traces (cross-arch oracles)
  'pile::the_pile_matches_the_committed_trace',
  'layers::the_layered_scene_matches_the_committed_trace',
  'physics_gallery::the_pile_matches_the_committed_cross_arch_trace',
  'physics_stress::the_stress_pile_matches_the_committed_cross_arch_trace',
  'ragdoll_scene::the_ragdoll_scene_matches_the_committed_cross_arch_trace',
  // snapshot restore continued against the committed trace
  'physics_stress::a_restored_stress_pile_reproduces_the_committed_trace_from_tick_300',
  'ragdoll_scene::a_restored_ragdoll_scene_reproduces_the_committed_trace_from_tick_300',
  // the committed .lrp replays
  'physics_stress::the_committed_stress_replay_reproduces_the_committed_trace',
  'ragdoll_scene::the_committed_ragdoll_replay_reproduces_the_committed_trace',
  // `ludion sim-hash` against the committed traces
  'physics::sim_hash_matches_the_committed_trace_at_two_frame_counts',
  'physics::sim_hash_matches_the_stress_trace',
  'physics::sim_hash_matches_the_ragdoll_trace',
]);

// The only tests that write a committed trace under LUDION_BLESS=1 (none writes a PNG golden).
const BLESS_TRACE_TESTS = [
  'pile::the_pile_matches_the_committed_trace',
  'layers::the_layered_scene_matches_the_committed_trace',
  'physics_gallery::the_pile_matches_the_committed_cross_arch_trace',
  'physics_stress::the_stress_pile_matches_the_committed_cross_arch_trace',
  'ragdoll_scene::the_ragdoll_scene_matches_the_committed_cross_arch_trace',
];

// The committed replays whose keyframes hold the previous pin's physics blob after a blessed move.
const STALE_REPLAYS = ['examples/gallery/physics-stress.lrp', 'examples/gallery/ragdoll.lrp'];

const GATE_FILTER =
  'package(ludion-physics) | (binary_id(=ludion-engine::it) & test(/^(physics_|ragdoll)/)) | (binary_id(=ludion-cli::it) & test(/^physics::/))';
const THREAD_FILTER = 'package(ludion-engine) & binary(thread_identity)';

function usage(msg) {
  console.error(`physics-pin-ritual: ${msg}`);
  process.exit(2);
}

let values;
try {
  ({ values } = parseArgs({
    options: {
      help: { type: 'boolean', short: 'h', default: false },
      to: { type: 'string' },
      'bless-traces': { type: 'boolean', default: false },
    },
  }));
} catch (err) {
  usage(err.message);
}
if (values.help) {
  const help = [];
  for (const l of fs.readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n')) {
    if (l.startsWith('#!')) continue;
    if (l.startsWith('//')) help.push(l.slice(3));
    else if (help.length) break;
  }
  console.log(help.join('\n'));
  process.exit(0);
}
if (values.to !== undefined && !/^\d+\.\d+\.\d+$/.test(values.to)) {
  usage(`--to wants a plain x.y.z version, got "${values.to}"`);
}
const vendoredPatches = ['rapier3d', 'parry3d'].filter((c) => fs.existsSync(path.join(REPO_ROOT, 'vendor', c)));
if (values.to !== undefined && vendoredPatches.length > 0) {
  usage(
    `the pin carries vendored patches — port ${vendoredPatches.map((c) => `vendor/${c}/LUDION-PATCH.md`).join(' and ')} ` +
    `onto ${values.to} first`,
  );
}

const rows = []; // { step, verdict, secs }
let pinMove = null; // { from, to } once --to has moved the pin

function record(step, verdict, startedAt) {
  const secs = (performance.now() - startedAt) / 1000;
  rows.push({ step, verdict, secs });
  console.log(`[${step}] ${verdict} (${secs.toFixed(1)}s)`);
}

// Runs a command with output captured to `.build/<log>` and returned; never a shell string.
function run(cmd, args, log, env = {}) {
  fs.mkdirSync(BUILD_DIR, { recursive: true });
  const res = spawnSync(cmd, args, {
    cwd: REPO_ROOT,
    encoding: 'utf8',
    maxBuffer: 1024 * 1024 * 1024,
    env: { ...process.env, ...env },
  });
  if (res.error) usage(`failed to spawn ${cmd}: ${res.error.message}`);
  const out = `${res.stdout ?? ''}${res.stderr ?? ''}`;
  fs.writeFileSync(path.join(BUILD_DIR, log), out);
  return { status: res.status, out };
}

function tail(text, lines = 40) {
  return text.trimEnd().split('\n').slice(-lines).join('\n');
}

function finish(verdict) {
  if (verdict === 'FAIL' && pinMove) {
    console.error(`\nthe run moved the pin (=${pinMove.from} -> =${pinMove.to}) and it did not hold. Revert with:`);
    console.error(`  node scripts/physics-pin-ritual.mjs --to ${pinMove.from}`);
    console.error('(or restore Cargo.toml and Cargo.lock by hand). Do not commit Cargo.toml or Cargo.lock at the candidate');
    console.error('pin in the shared checkout; nothing was reverted for you.');
  }
  const width = Math.max(...rows.map((r) => r.step.length), 4);
  console.log('\nphysics-pin-ritual summary');
  console.log(`  ${'step'.padEnd(width)}  verdict  wall`);
  for (const r of rows) console.log(`  ${r.step.padEnd(width)}  ${r.verdict.padEnd(7)}  ${r.secs.toFixed(1)}s`);
  const total = rows.reduce((s, r) => s + r.secs, 0);
  console.log(`  ${'total'.padEnd(width)}  ${verdict.padEnd(7)}  ${total.toFixed(1)}s`);
  console.log(`\nverdict: ${verdict}`);
  process.exit({ PASS: 0, FAIL: 1, MOVED: 3 }[verdict]);
}

// ---- step 1: move the pin ----

const PIN_LINE = /^rapier3d = \{ version = "=([^"]+)"/m;

function lockPairs() {
  const text = fs.readFileSync(CARGO_LOCK, 'utf8');
  const pairs = new Set();
  for (const m of text.matchAll(/\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"/g)) {
    pairs.add(`${m[1]} ${m[2]}`);
  }
  return pairs;
}

function movePin(target) {
  const started = performance.now();
  const manifest = fs.readFileSync(CARGO_TOML, 'utf8');
  const matches = manifest.match(new RegExp(PIN_LINE.source, 'gm')) ?? [];
  if (matches.length !== 1) usage(`expected exactly one \`rapier3d = { version = "=..."\` line in Cargo.toml, found ${matches.length}`);
  const old = manifest.match(PIN_LINE)[1];
  if (old === target) {
    console.log(`step 1: the pin is already =${target}; nothing to move`);
    record('1 move pin', 'PASS', started);
    return;
  }
  const before = lockPairs();
  pinMove = { from: old, to: target };
  writeFileAtomic(CARGO_TOML, manifest.replace(PIN_LINE, (line, v) => line.replace(`"=${v}"`, `"=${target}"`)));
  const upd = run('cargo', ['update', '-p', 'rapier3d', '--precise', target], 'physics-pin-ritual-update.log');
  if (upd.status !== 0) {
    console.error(`physics-pin-ritual: cargo update to ${target} failed — Cargo.toml is left edited; restore it by hand.\n${tail(upd.out)}`);
    record('1 move pin', 'FAIL', started);
    finish('FAIL');
  }
  const after = lockPairs();
  console.log(`step 1: rapier3d =${old} -> =${target}; Cargo.lock moved:`);
  for (const p of [...before].filter((p) => !after.has(p)).sort()) console.log(`  - ${p}`);
  for (const p of [...after].filter((p) => !before.has(p)).sort()) console.log(`  + ${p}`);

  const todo = scanLeftovers(REPO_ROOT, old);
  console.log(`to do: ${todo.length} line(s) in rapier-mentioning text files still name ${old}:`);
  for (const l of todo) console.log(l);
  console.log('to do: re-audit the clippy determinism denylist against the new registry source.');
  record('1 move pin', 'PASS', started);
}

// ---- steps 3-4: nextest ----

function nextest(filter, log, { ignored = false, env = {} } = {}) {
  const args = ['nextest', 'run', '--no-fail-fast', '--color', 'never', '--final-status-level', 'fail', '-E', filter];
  if (ignored) args.push('--run-ignored', 'ignored-only');
  const res = run('cargo', args, log, env);
  return { ...res, failed: parseFailedTests(res.out) };
}

// PASS / MOVED / FAIL for one nextest run, printing the evidence.
function judge(res, name) {
  if (res.status === 0) return 'PASS';
  if (res.failed.length === 0) {
    console.error(`${name}: nextest exited ${res.status} with no failing test — a build or harness error:\n${tail(res.out)}`);
    return 'FAIL';
  }
  const inProcess = res.failed.filter((t) => !COMMITTED_COMPARISONS.has(t));
  if (inProcess.length) {
    console.error(`${name}: ${inProcess.length} in-process gate(s) failed (not a committed-artifact comparison):`);
    for (const t of inProcess) console.error(`  ${t}`);
    return 'FAIL';
  }
  console.log(`${name}: only committed-artifact comparisons failed (${res.failed.length}):`);
  for (const t of res.failed) console.log(`  ${t}`);
  return 'MOVED';
}

const worse = (a, b) => ['PASS', 'MOVED', 'FAIL'][Math.max(['PASS', 'MOVED', 'FAIL'].indexOf(a), ['PASS', 'MOVED', 'FAIL'].indexOf(b))];

function printStep5(verdict, { blessed = false } = {}) {
  if (verdict === 'FAIL') {
    console.log('\nthe pin does not move; record the finding — it is exit trigger (b) of Plan/physics.md.');
    return;
  }
  if (verdict === 'MOVED') {
    console.log('\nMOVED protocol: on the first machine run this script with --bless-traces (re-records the five committed');
    console.log('traces, re-runs the gates), carry the candidate pin + traces to the second machine and run this script');
    console.log('there with no flags; both green, then step 5. The committed .lrp files stay stale until step 5.');
  }
  if (blessed) {
    console.log('\nthe replay tests pass after blessing, but these committed replays still hold the old pin\'s physics blob in');
    console.log('their keyframes (the tests re-derive and compare hashes, never bytes) — stale until step 5 re-records them:');
    for (const r of STALE_REPLAYS) console.log(`  ${r}`);
  }
  console.log('\nstep 5, by hand, in order (nothing here is run):');
  console.log('  1. the schema snapshot (crates/ludion-protocol/tests/schema_snapshot.rs) — expected unmoved');
  console.log('  2. the goldens, on each pinned GPU');
  console.log('  3. the budgets');
  console.log(`  4. the .lrp corpus, last (${STALE_REPLAYS.map((r) => path.basename(r)).join(', ')}, then the games')`);
}

// ---- main ----

if (values.to !== undefined) movePin(values.to);

let started = performance.now();
const features = run(process.execPath, [path.join('scripts', 'physics-features.mjs')], 'physics-pin-ritual-features.log');
process.stdout.write(features.out);
if (features.status !== 0) {
  record('2 features', 'FAIL', started);
  console.error('\nphysics-pin-ritual: the resolved feature set drifted — later steps are meaningless until it is fixed.');
  printStep5('FAIL');
  finish('FAIL');
}
record('2 features', 'PASS', started);

const nextestOk = spawnSync('cargo', ['nextest', '--version'], { cwd: REPO_ROOT, encoding: 'utf8' });
if (nextestOk.error || nextestOk.status !== 0) usage('cargo-nextest is required (`cargo install cargo-nextest --locked`)');

// Steps 3-4 once; `suffix` labels the summary rows of a re-run.
function runGates(suffix = '') {
  let t0 = performance.now();
  const gates = nextest(GATE_FILTER, 'physics-pin-ritual-gates.log');
  const gatesVerdict = judge(gates, 'gates');
  record(`3-4 gates${suffix}`, gatesVerdict, t0);

  t0 = performance.now();
  const threads = nextest(THREAD_FILTER, 'physics-pin-ritual-threads.log', { ignored: true });
  // thread identity compares only in-process runs: any failure there is a real FAIL.
  const threadsVerdict = threads.status === 0 ? 'PASS' : 'FAIL';
  if (threadsVerdict === 'FAIL') console.error(`thread-identity failed:\n${tail(threads.out)}`);
  record(`4 thread-identity${suffix}`, threadsVerdict, t0);
  return worse(gatesVerdict, threadsVerdict);
}

let verdict = runGates();
let blessed = false;

if (values['bless-traces']) {
  if (verdict === 'PASS') {
    console.log('--bless-traces: every gate passed unblessed; nothing needs blessing.');
  } else if (verdict === 'FAIL') {
    console.log('--bless-traces: the verdict is FAIL; nothing is blessed.');
  } else {
    started = performance.now();
    const filter = BLESS_TRACE_TESTS.map((t) => {
      const [bin, test] = t.split('::');
      // Matches whether the file is still its own binary or a module of a merged `it` binary.
      return `((binary(=${bin}) & test(=${test})) | (binary(=it) & test(=${t})))`;
    }).join(' | ');
    const res = nextest(filter, 'physics-pin-ritual-bless.log', { env: { LUDION_BLESS: '1' } });
    const blessVerdict = res.status === 0 ? 'PASS' : 'FAIL';
    if (blessVerdict === 'FAIL') console.error(`bless run failed:\n${tail(res.out)}`);
    record('bless traces', blessVerdict, started);
    if (blessVerdict === 'FAIL') finish('FAIL');
    console.log('traces re-recorded; review them with `lore diff` before carrying them to the second machine.');
    blessed = true;
    verdict = runGates(' (re-run)');
  }
}

printStep5(verdict, { blessed });
finish(verdict);
