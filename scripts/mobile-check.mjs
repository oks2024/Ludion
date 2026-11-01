#!/usr/bin/env node
// The mobile feature-set gate (Plan/ios-target.md, decision 3c).
//
// The `ludion` facade's default `wire` + `window` features are the desktop dev shape. A build for a
// device with no desktop turns them off (`default-features = false`) and hosts through
// `ludion::launch` alone — and nothing else gates that build: the workspace clippy runs
// `--all-features`, so a feature-off `cfg` branch would rot unseen until the first cross-compile.
// Six checks on the host target (every crate gated out is target-independent), and a seventh on iOS:
//
//   1. clippy `-D warnings` on the facade and `ludion-assets` with every feature off — the shipped
//      shape: no listener, no viewer, no watcher (`ludion-assets`' opt-in `watch` feature stays off
//      because nothing in that tree enables it). `--no-deps` lints those two crates only; the rest
//      of the tree is compiled, not linted, so this costs one check-build, not a second clippy run.
//   2. the same on the facade with `wire` alone — the debug-build shape: a listener the agent
//      attaches to, still no viewer.
//   3. the same on the facade with `window` alone — the desktop player shape: `launch` opens the
//      window, with no listener and no argv shell.
//   4. clippy on vigil's lib in both shapes — the game a native host links: its features forward to
//      the facade's, so the shipped build is `--no-default-features` and the debug build is `wire`.
//      vigil is its own workspace (run from games/vigil/); every vigil check is skipped, with a
//      printed reason, where that directory is absent.
//   5. `cargo tree` of the feature-less facade and of feature-less vigil must name none of the
//      desktop-only crates below.
//   6. the facade's lib tests in the shipped and the debug shape: the `#[cfg(not(feature = "wire"))]`
//      `start`/`try_start` paths in `crates/ludion/src/ffi.rs` run here and nowhere else.
//   7. vigil's lib linted and its tree checked for `aarch64-apple-ios` in the shipped shape, wherever
//      vigil is present and that target and the iPhoneOS SDK are installed (`coreaudio-sys` runs
//      bindgen against the SDK, so the target alone is not enough); elsewhere it is skipped with a
//      printed reason (Plan/ios-target.md, decision 11).
//
// Run standalone (`node scripts/mobile-check.mjs`) or via the `mobile` step in scripts/check.mjs;
// mirrored as the `mobile feature set` step in .github/workflows/ci.yml and the `mobile` step in
// ci/teamcity/provision.mjs — keep the three in step.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const IOS_TARGET = 'aarch64-apple-ios';

// vigil is a Lore-only game with its own workspace: its cargo commands run from its directory, and
// are skipped where it is absent.
const VIGIL_DIR = path.join(REPO_ROOT, 'games', 'vigil');
const hasVigil = fs.existsSync(path.join(VIGIL_DIR, 'Cargo.toml'));

// What the feature-less facade must not carry: the four desktop-only third-party crates the plan
// names, and the three workspace crates that pull them in.
const DESKTOP_ONLY = ['winit', 'gilrs', 'notify', 'tokio', 'ludion-window', 'ludion-server', 'ludion-client'];

const LINT_STEPS = [
  {
    what: 'shipped shape (no wire, no window, no watch)',
    args: ['clippy', '-p', 'ludion', '-p', 'ludion-assets', '--no-default-features', '--no-deps', '--', '-D', 'warnings'],
  },
  {
    what: 'debug shape (wire, no window)',
    args: ['clippy', '-p', 'ludion', '--no-default-features', '--features', 'wire', '--no-deps', '--', '-D', 'warnings'],
  },
  {
    what: 'desktop player shape (window, no wire)',
    args: ['clippy', '-p', 'ludion', '--no-default-features', '--features', 'window', '--no-deps', '--', '-D', 'warnings'],
  },
  {
    what: "vigil's lib in the shipped shape (no wire, no window)",
    args: ['clippy', '-p', 'vigil', '--lib', '--no-default-features', '--no-deps', '--', '-D', 'warnings'],
    cwd: VIGIL_DIR,
  },
  {
    what: "vigil's lib in the debug shape (wire, no window)",
    args: ['clippy', '-p', 'vigil', '--lib', '--no-default-features', '--features', 'wire', '--no-deps', '--', '-D', 'warnings'],
    cwd: VIGIL_DIR,
  },
];

// `null` when the iOS check can run here, else the one-line reason it cannot. The sysroot is asked
// of the pinned toolchain (run from the repo root) rather than of rustup.
function iosTargetReady() {
  const sysroot = spawnSync('rustc', ['--print', 'sysroot'], { cwd: REPO_ROOT, encoding: 'utf8' });
  const root = sysroot.status === 0 ? sysroot.stdout.trim() : '';
  if (!root || !fs.existsSync(path.join(root, 'lib', 'rustlib', IOS_TARGET))) {
    return `${IOS_TARGET} is not installed (rustup target add ${IOS_TARGET})`;
  }
  const sdk = spawnSync('xcrun', ['--sdk', 'iphoneos', '--show-sdk-path'], { encoding: 'utf8' });
  if (sdk.error || sdk.status !== 0 || !sdk.stdout.trim()) {
    return 'no iPhoneOS SDK (xcrun --sdk iphoneos fails): coreaudio-sys binds against it';
  }
  return null;
}

function lint(what, args, cwd = REPO_ROOT) {
  console.log(`mobile: cargo ${args.join(' ')}`);
  const run = spawnSync('cargo', args, { cwd, stdio: 'inherit' });
  if (run.status !== 0) {
    console.error(`\nMOBILE FEATURE SET: ${what} does not lint clean (see above).`);
    process.exit(1);
  }
}

if (!hasVigil) console.log("mobile: skipping vigil's lib checks — games/vigil absent");
for (const step of LINT_STEPS) {
  if (step.cwd && !hasVigil) continue;
  lint(`the ${step.what}`, step.args, step.cwd);
}

function test(what, args) {
  console.log(`mobile: cargo ${args.join(' ')}`);
  const run = spawnSync('cargo', args, { cwd: REPO_ROOT, stdio: 'inherit' });
  if (run.status !== 0) {
    console.error(`
MOBILE FEATURE SET: the ${what} tests fail (see above).`);
    process.exit(1);
  }
}

const libTests = ['--skip', 'needs_gpu_'];
test("facade lib in the shipped shape", ['test', '-p', 'ludion', '--no-default-features', '--lib', '--', ...libTests]);
test("facade lib in the debug shape", ['test', '-p', 'ludion', '--no-default-features', '--features', 'wire', '--lib', '--', ...libTests]);

const iosSkip = hasVigil ? iosTargetReady() : 'games/vigil absent';
if (iosSkip === null) {
  lint(
    `vigil's lib for ${IOS_TARGET} in the shipped shape`,
    ['clippy', '-p', 'vigil', '--lib', '--target', IOS_TARGET, '--no-default-features', '--no-deps', '--', '-D', 'warnings'],
    VIGIL_DIR,
  );
} else {
  console.log(`mobile: skipping the ${IOS_TARGET} check — ${iosSkip}`);
}

// `--prefix none` prints one `name vX.Y.Z [(path)] [(*)]` line per node, duplicates included;
// `-e normal` leaves build- and dev-dependencies out, which is the tree a game binary links.
function checkTree(pkg, target, cwd = REPO_ROOT) {
  const flags = target ? ['--target', target] : [];
  const where = target ? ` for ${target}` : '';
  const tree = spawnSync(
    'cargo',
    ['tree', '-p', pkg, '--no-default-features', '-e', 'normal', '--prefix', 'none', ...flags],
    { cwd, encoding: 'utf8' },
  );
  if (tree.status !== 0) {
    console.error(`\nMOBILE FEATURE SET: cargo tree -p ${pkg}${where} failed:\n${tree.stderr}`);
    process.exit(1);
  }
  const names = new Set(tree.stdout.split('\n').map((line) => line.trim().split(' ')[0]).filter(Boolean));
  const carried = DESKTOP_ONLY.filter((name) => names.has(name));
  if (carried.length > 0) {
    console.error(
      `\nMOBILE FEATURE SET: the feature-less \`${pkg}\` tree${where} carries desktop-only crates: ${carried.join(', ')}.\n` +
        'They must sit behind a feature the mobile build turns off (`wire`, `window`, or ' +
        `\`ludion-assets\`' \`watch\`); \`cargo tree -p ${pkg} --no-default-features -e normal ${flags.join(' ')} -i <crate>\` ` +
        'names the edge that pulls each one in.',
    );
    process.exit(1);
  }
  console.log(`mobile: feature-less ${pkg} tree${where} is clean (${names.size} crates, none of: ${DESKTOP_ONLY.join(', ')})`);
}

checkTree('ludion');
if (hasVigil) checkTree('vigil', undefined, VIGIL_DIR);
if (iosSkip === null) checkTree('vigil', IOS_TARGET, VIGIL_DIR);
