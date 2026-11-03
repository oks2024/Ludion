// node --test scripts/deps-prune.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, existsSync, linkSync, rmSync } from 'node:fs';
import { execFileSync, spawnSync } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import { pruneDeps, parseOsoLine, isLinkedOutput, ownerPrefix, holdCargoLock } from './deps-prune.mjs';

const darwin = process.platform === 'darwin' ? false : 'needs the macOS toolchain (nm, clang, perl)';
const HASH = '0123456789abcdef';

/** A `target/debug/deps` directory in a fresh temp tree. */
function fixture() {
  const root = mkdtempSync(path.join(os.tmpdir(), 'deps-prune-'));
  const dir = path.join(root, 'debug', 'deps');
  mkdirSync(dir, { recursive: true });
  return { root, dir, lock: path.join(root, 'debug', '.cargo-lock') };
}

test('parseOsoLine reads loose objects only', () => {
  assert.equal(parseOsoLine('0000000000000000 - 00 0001 OSO /abs/deps/x-0123.a.b.rcgu.o'), 'x-0123.a.b.rcgu.o');
  assert.equal(parseOsoLine('0000000000000000 - 00 0001 OSO /abs/dir with space/deps/x y.rcgu.o'), 'x y.rcgu.o');
  assert.equal(parseOsoLine('0000000000000000 - 00 0001 OSO /abs/libx.rlib(member.o)'), null);
  assert.equal(parseOsoLine('0000000100000000 T _main'), null);
});

test('isLinkedOutput and ownerPrefix', () => {
  assert.ok(isLinkedOutput(`app-${HASH}`));
  assert.ok(isLinkedOutput(`libmacro-${HASH}.dylib`));
  assert.ok(!isLinkedOutput(`libdep-${HASH}.rlib`));
  assert.ok(!isLinkedOutput(`app-${HASH}.d`));
  assert.ok(!isLinkedOutput(`app-${HASH}.cgu.s1.rcgu.o`));
  assert.equal(ownerPrefix(`app-${HASH}`), `app-${HASH}.`);
  assert.equal(ownerPrefix(`libmacro-${HASH}.dylib`), `macro-${HASH}.`);
});

test('under the threshold nothing is deleted and no lock is taken', { skip: darwin }, async () => {
  const { root, dir, lock } = fixture();
  writeFileSync(path.join(dir, 'a.o'), 'x');
  const hold = await holdCargoLock(lock, 1000);
  assert.ok(hold);
  const r = await pruneDeps(dir, { threshold: 5, lockWaitMs: 10 });
  assert.deepEqual(r, { before: 1, removed: 0, after: 1, busy: false });
  assert.ok(existsSync(path.join(dir, 'a.o')));
  await hold();
  rmSync(root, { recursive: true });
});

test('a missing directory is not fatal', async () => {
  assert.equal(await pruneDeps(path.join(os.tmpdir(), 'deps-prune-absent-xyz', 'deps'), { threshold: 1 }), null);
});

function buildApp(dir) {
  const c = path.join(dir, 'app.c');
  writeFileSync(c, 'int main(void){return 0;}\n');
  const live = path.join(dir, `app-${HASH}.cgu.s1.rcgu.o`);
  execFileSync('clang', ['-g', '-c', c, '-o', live]);
  const exe = path.join(dir, `app-${HASH}`);
  execFileSync('clang', ['-g', live, '-o', exe]);
  rmSync(c);
  return { live, exe };
}

test('only objects no executable names are removed', { skip: darwin }, async () => {
  const { root, dir } = fixture();
  const { live, exe } = buildApp(dir);
  // A stale name on the live object's inode, fresh by every timestamp: age cannot tell it from a live one.
  const stale = path.join(dir, `app-${HASH}.cgu.s0.rcgu.o`);
  linkSync(live, stale);
  const dead = path.join(dir, `dep-${HASH}.x.y.rcgu.o`);
  writeFileSync(dead, 'x');
  const rlib = path.join(dir, `libdep-${HASH}.rlib`);
  writeFileSync(rlib, 'x');
  const r = await pruneDeps(dir, { threshold: 1 });
  assert.equal(r.busy, false);
  assert.equal(r.removed, 2);
  assert.ok(existsSync(live));
  assert.ok(!existsSync(stale));
  assert.ok(!existsSync(dead));
  assert.ok(existsSync(rlib));
  assert.ok(existsSync(exe));
  assert.equal(spawnSync(exe).status, 0);
  rmSync(root, { recursive: true });
});

test('objects of an output nm cannot read are kept', { skip: darwin }, async () => {
  const { root, dir } = fixture();
  writeFileSync(path.join(dir, `bad-${HASH}`), 'not a mach-o file');
  const obj = path.join(dir, `bad-${HASH}.x.y.rcgu.o`);
  writeFileSync(obj, 'x');
  const r = await pruneDeps(dir, { threshold: 1 });
  assert.equal(r.removed, 0);
  assert.ok(existsSync(obj));
  rmSync(root, { recursive: true });
});

test('a busy cargo lock prunes nothing, and the wait polls', { skip: darwin }, async () => {
  const { root, dir, lock } = fixture();
  const dead = path.join(dir, `dep-${HASH}.x.y.rcgu.o`);
  writeFileSync(dead, 'x');
  writeFileSync(path.join(dir, 'pad'), 'x');
  const hold = await holdCargoLock(lock, 1000);
  assert.ok(hold);
  const busy = await pruneDeps(dir, { threshold: 1, lockWaitMs: 500 });
  assert.deepEqual(busy, { before: 2, removed: 0, after: 2, busy: true });
  assert.ok(existsSync(dead));
  const waiting = pruneDeps(dir, { threshold: 1, lockWaitMs: 5000 });
  await new Promise((r) => setTimeout(r, 600));
  await hold();
  const served = await waiting;
  assert.equal(served.busy, false);
  assert.equal(served.removed, 1);
  assert.ok(!existsSync(dead));
  rmSync(root, { recursive: true });
});
