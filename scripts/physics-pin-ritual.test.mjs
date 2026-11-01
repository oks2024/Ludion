// Tests for the pure helpers of scripts/physics-pin-ritual.mjs — `node --test scripts/physics-pin-ritual.test.mjs`.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { parseFailedTests, scanLeftovers } from './physics-pin-ritual-lib.mjs';

test('a FAIL line with the padded counter parses', () => {
  const out = '        FAIL [   0.053s] ( 2/12) ludion-physics::pile the_pile_matches_the_committed_trace\n';
  assert.deepEqual(parseFailedTests(out), ['pile::the_pile_matches_the_committed_trace']);
});

test('a FAIL line without a counter parses', () => {
  const out = '        FAIL [   0.053s] ludion-engine::physics_stress a_thing\n';
  assert.deepEqual(parseFailedTests(out), ['physics_stress::a_thing']);
});

test('a FAIL line from the merged engine binary parses to stem::test', () => {
  const out = '        FAIL [   0.053s] (  2/12) ludion-engine::it physics_stress::a_thing\n';
  assert.deepEqual(parseFailedTests(out), ['physics_stress::a_thing']);
});

test('a counter of several digits parses', () => {
  const out = '        FAIL [   1.500s] (123/456) ludion-engine::ragdoll_scene some_test\n';
  assert.deepEqual(parseFailedTests(out), ['ragdoll_scene::some_test']);
});

test('SIGSEGV, TIMEOUT and LEAK-FAIL lines parse, with and without a counter', () => {
  const out = [
    '     SIGSEGV [   0.010s] ( 1/9) ludion-physics::a t_one',
    '     TIMEOUT [ 120.000s] ludion-physics::b t_two',
    '   LEAK-FAIL [   0.020s] ( 3/9) ludion-cli::physics t_three',
  ].join('\n');
  assert.deepEqual(parseFailedTests(out), ['a::t_one', 'b::t_two', 'physics::t_three']);
});

test('lines that are not failures do not match', () => {
  const out = [
    '        PASS [   0.030s] ( 1/12) ludion-physics::pile ok_test',
    '     Starting 12 tests across 4 binaries',
    '        FAIL [   0.053s] ( 2/12) only-one-token',
    'FAIL: something else entirely',
    '',
  ].join('\n');
  assert.deepEqual(parseFailedTests(out), []);
});

test('duplicates collapse and the result is sorted', () => {
  const out = [
    'FAIL [ 0.1s] ( 2/2) ludion-x::b t',
    'FAIL [ 0.1s] ( 1/2) ludion-x::a t',
    'FAIL [ 0.1s] ( 1/2) ludion-x::a t',
  ].join('\n');
  assert.deepEqual(parseFailedTests(out), ['a::t', 'b::t']);
});

test('scanLeftovers finds rapier-mentioning text, skipping build dirs, the lockfile and binaries', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pin-ritual-'));
  try {
    const put = (rel, data) => {
      fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
      fs.writeFileSync(path.join(root, rel), data);
    };
    put('docs/a.md', 'rapier3d 0.35.3 is pinned\nunrelated 0.35.3\n');
    put('docs/b.md', 'no mention of the physics engine 0.35.3\n');
    put('target/x.md', 'rapier 0.35.3\n');
    put('node_modules/y.md', 'rapier 0.35.3\n');
    put('Cargo.lock', 'rapier 0.35.3\n');
    put('crates/ludion-physics/Cargo.toml', 'rapier 0.35.3\n');
    put('blob.bin', Buffer.from([0x72, 0x61, 0x70, 0x69, 0x65, 0x72, 0, 0x30, 0x2e, 0x33, 0x35, 0x2e, 0x33]));
    put('bad.txt', Buffer.from([0xff, 0xfe, 0x72, 0x61, 0x70, 0x69, 0x65, 0x72]));
    assert.deepEqual(scanLeftovers(root, '0.35.3'), [
      '  docs/a.md:1: rapier3d 0.35.3 is pinned',
      '  docs/a.md:2: unrelated 0.35.3',
    ]);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('a failing ludion-cli test of the merged it binary keys as its module path', () => {
  const out = '        FAIL [   0.500s] ( 2/9) ludion-cli::it physics::cli_trace_matches\n';
  assert.deepEqual(parseFailedTests(out), ['physics::cli_trace_matches']);
});
