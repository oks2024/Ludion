import assert from 'node:assert/strict';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { DOCS_TESTS, classify, testSelection } from './check-scope.mjs';

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');

const WS = {
  members: [
    { name: 'ludion-core', dir: 'crates/ludion-core', lib: true, deps: [] },
    { name: 'ludion-protocol', dir: 'crates/ludion-protocol', lib: true, deps: ['ludion-core'] },
    { name: 'ludion', dir: 'crates/ludion', lib: true, deps: ['ludion-core', 'ludion-protocol'] },
    { name: 'ludion-scene', dir: 'crates/ludion-scene', lib: true, deps: ['ludion-core'] },
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

test('game assets select the drift gates', () => {
  const game = classify(['games/breakout/scenes/level.yaml'], WS);
  assert.deepEqual(game.packages, ['breakout']);
  assert.equal(game.drift, true);
  assert.equal(classify(['games/breakout/src/main.rs'], WS).drift, false);
});

const DOCS_FILTER = '(binary_id(=ludion-cli::it) & test(/^docs::/)) | (binary_id(=ludion-cli::it) & test(/^explain::/)) | binary_id(=ludion-cli::new) | binary_id(=ludion-protocol::strict_sweep) | binary_id(=ludion-scene::kind_precheck)';

test('docs/ and its embedded mirror build ludion-cli and run only the docs-reading tests', () => {
  for (const p of ['docs/scenes.md', 'crates/ludion-cli/embedded-docs/x.md']) {
    const s = classify([p], WS);
    assert.deepEqual(s.packages, [], p);
    assert.equal(s.docs, true, p);
    assert.deepEqual(testSelection(s), { clippy: ['ludion-cli'], test: ['ludion-cli', 'ludion-protocol', 'ludion-scene'], filter: DOCS_FILTER }, p);
  }
});

test('a whole package keeps its tests in the docs filter; both whole packages need none', () => {
  const cli = testSelection(classify(['docs/scenes.md', 'crates/ludion-cli/src/main.rs'], WS));
  assert.equal(cli.filter, 'package(=ludion-cli) | binary_id(=ludion-protocol::strict_sweep) | binary_id(=ludion-scene::kind_precheck)');
  const both = testSelection(classify(['docs/scenes.md', 'crates/ludion-cli/src/main.rs', 'crates/ludion-scene/src/lib.rs', 'crates/ludion-protocol/src/lib.rs'], WS));
  assert.equal(both.filter, null);
  assert.deepEqual(both.test, ['ludion-cli', 'ludion-protocol', 'ludion-scene']);
});

test('without a docs path the selection is the packages and no filter', () => {
  const s = classify(['crates/ludion-core/src/lib.rs'], WS);
  assert.deepEqual(testSelection(s), { clippy: ['ludion-core'], test: ['ludion-core'], filter: null });
});

// A package's integration-test targets from its manifest: `[[test]]` tables (name, path defaulting
// to tests/<name>.rs) and whether `autotests = false` turns auto-discovery off.
function testTargets(manifest) {
  const autotests = !/^autotests\s*=\s*false/m.test(manifest);
  const targets = manifest.split(/^\[\[test\]\]\s*$/m).slice(1).map((table) => {
    const body = table.split(/^\[/m)[0];
    const name = /^name\s*=\s*"([^"]+)"/m.exec(body)[1];
    return { name, path: /^path\s*=\s*"([^"]+)"/m.exec(body)?.[1] ?? `tests/${name}.rs` };
  });
  return { autotests, targets };
}

// Every integration test file that reads docs/ must be in DOCS_TESTS, and no entry may be stale.
// A file is keyed `pkg::binary`, or `pkg::binary::module` when a [[test]] root declares it as a module.
test('DOCS_TESTS lists exactly the integration tests that read docs/', () => {
  const rs = (dir) => readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? rs(path.join(dir, e.name)) : e.name.endsWith('.rs') ? [path.join(dir, e.name)] : []));
  const pkgName = (manifest) => /^name\s*=\s*"([^"]+)"/m.exec(manifest)[1];
  const hits = new Set();
  for (const group of ['crates', 'games', 'examples']) {
    for (const e of readdirSync(path.join(ROOT, group), { withFileTypes: true })) {
      const dir = path.join(ROOT, group, e.name);
      if (!e.isDirectory() || (group === 'games' && e.name === 'vigil') || !existsSync(path.join(dir, 'tests'))) continue;
      const manifest = readFileSync(path.join(dir, 'Cargo.toml'), 'utf8');
      const { autotests, targets } = testTargets(manifest);
      for (const file of rs(path.join(dir, 'tests'))) {
        const reads = readFileSync(file, 'utf8').split('\n').some((l) => !l.trim().startsWith('//') && /"docs[/"]|embedded-docs/.test(l));
        if (!reads) continue;
        const rel = path.relative(path.join(dir, 'tests'), file).split(path.sep);
        assert.equal(rel.length, 1, `${file} reads docs/ from a nested test file: map it by hand in DOCS_TESTS in scripts/check-scope.mjs`);
        const stem = rel[0].replace(/\.rs$/, '');
        const pkg = pkgName(manifest);
        const root = targets.find((t) => t.path === `tests/${stem}.rs`);
        if (autotests || root) { hits.add(`${pkg}::${root ? root.name : stem}`); continue; }
        const host = targets.find((t) => new RegExp(`^mod (r#)?${stem};`, 'm').test(readFileSync(path.join(dir, t.path), 'utf8')));
        assert.ok(host, `${file} is not declared in any [[test]] root of ${pkg}`);
        hits.add(`${pkg}::${host.name}::${stem}`);
      }
    }
  }
  const listed = new Set(DOCS_TESTS.map((entry) => entry.join('::')));
  for (const h of hits) assert.ok(listed.has(h), `${h} reads docs/: add it to DOCS_TESTS in scripts/check-scope.mjs`);
  for (const l of listed) assert.ok(hits.has(l), `${l} is in DOCS_TESTS but no longer reads docs/: remove it from scripts/check-scope.mjs`);
});

test('--fail-fast without --scope is refused', () => {
  const r = spawnSync('node', ['scripts/check.mjs', '--fail-fast'], { cwd: ROOT, encoding: 'utf8' });
  assert.equal(r.status, 2);
  assert.match(r.stderr, /--fail-fast.*--scope/);
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
