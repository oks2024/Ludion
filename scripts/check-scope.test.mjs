import assert from 'node:assert/strict';
import test from 'node:test';
import { classify } from './check-scope.mjs';

const WS = {
  members: [
    { name: 'ludion-core', dir: 'crates/ludion-core', lib: true, deps: [] },
    { name: 'ludion-protocol', dir: 'crates/ludion-protocol', lib: true, deps: ['ludion-core'] },
    { name: 'ludion', dir: 'crates/ludion', lib: true, deps: ['ludion-core', 'ludion-protocol'] },
    { name: 'ludion-cli', dir: 'crates/ludion-cli', lib: false, deps: ['ludion'] },
    { name: 'breakout', dir: 'games/breakout', lib: false, deps: ['ludion'] },
  ],
  fuzzDeps: ['ludion-protocol'],
};

test('a crate path selects its package and nothing that depends on it', () => {
  const s = classify(['crates/ludion-core/src/lib.rs'], WS);
  assert.deepEqual(s.packages, ['ludion-core']);
  assert.deepEqual(s.libs, ['ludion-core']);
  assert.equal(s.vigil, false);
  assert.equal(s.fuzz, false);
});

test('--deps adds every dependent, vigil through the facade and fuzz through its path deps', () => {
  const s = classify(['crates/ludion-protocol'], WS, { deps: true });
  assert.deepEqual(s.packages, ['breakout', 'ludion', 'ludion-cli', 'ludion-protocol']);
  assert.deepEqual(s.libs, ['ludion', 'ludion-protocol']);
  assert.equal(s.vigil, true);
  assert.equal(s.fuzz, true);
  assert.equal(s.mobile, true);
  assert.equal(s.schema, true);
});

test('the protocol crate brings the schema-drift gate along', () => {
  assert.equal(classify(['crates/ludion-protocol/src/lib.rs'], WS).schema, true);
});

test('prose needs only the cheap gates', () => {
  const s = classify(['Plan/owed.md', 'dev-docs/determinism.md', 'README.md'], WS);
  assert.deepEqual(s.packages, []);
  assert.equal(s.drift || s.scripts || s.frontend || s.vigil, false);
});

test('docs/ is ludion-cli (the embedded mirror), game assets the drift gates', () => {
  assert.deepEqual(classify(['docs/scenes.md'], WS).packages, ['ludion-cli']);
  const game = classify(['games/breakout/scenes/level.yaml'], WS);
  assert.deepEqual(game.packages, ['breakout']);
  assert.equal(game.drift, true);
  assert.equal(classify(['games/breakout/src/main.rs'], WS).drift, false);
});

test('the detached workspaces and the frontend each select their own lane', () => {
  assert.equal(classify(['editor/app/src-tauri/src/main.rs'], WS).tauri, true);
  const app = classify(['editor/app/src/App.tsx'], WS);
  assert.equal(app.frontend, true);
  assert.equal(app.tauri, false);
  assert.equal(classify(['editor/protocol/src/index.ts'], WS).schema, true);
  assert.equal(classify(['fuzz/fuzz_targets/scene_parse.rs'], WS).fuzz, true);
  const vigil = classify(['games/vigil/art/town/lib/png.mjs'], WS);
  assert.equal(vigil.vigil && vigil.vigilNode, true);
});

test('scripts select the script tests; a drift script the drift gates too', () => {
  const s = classify(['scripts/commit.mjs'], WS);
  assert.equal(s.scripts, true);
  assert.equal(s.drift, false);
  assert.equal(classify(['scripts/map-drift.mjs'], WS).drift, true);
});

test('workspace-wide and unclaimed paths are refused', () => {
  for (const p of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.config/nextest.toml', 'vendor/rapier3d/src/lib.rs', 'budgets/macos.toml', 'crates/not-a-member/x.rs', '../elsewhere']) {
    assert.ok(classify([p], WS).error, p);
  }
});
