#!/usr/bin/env node
// The physics resolved-feature gate (Plan/physics.md W1).
//
// Cargo unifies features across the whole workspace: one crate anywhere enabling a `simd-*` feature
// on `rapier3d`/`parry3d` would silently turn SIMD-width-dependent arithmetic on for every build,
// and clippy's `disallowed-types`/`disallowed-methods` cannot see this — a denylist bans a type
// *path in our code* and has never looked inside a dependency. This script is the mechanical stand-in: it reads `cargo metadata` and asserts the
// exact feature set both crates resolved to (`parallel` included: rapier's scheduler runs on the
// pool `ludion-physics` owns), plus that `rayon`/`rayon-core` are reachable from `ludion-physics`'s
// own (normal, non-dev, non-build) dependency closure only through the named carve-out, the
// `rapier3d -> rayon` and `parry3d -> rayon` edges (`Plan/deterministic-job-system.md`).
//
// Why `hashbrown` is NOT gated here, on purpose, after checking: `rapier3d`'s `std` feature pulls
// `alloc`, which turns on `parry3d/alloc`, which is what makes `hashbrown` a resolved dependency —
// unavoidable under the pinned feature set (`serde-serialize` itself requires `std`). Parry3d uses
// it as `hashbrown::hash_map::HashMap<K, V, foldhash::fast::FixedState>` (`parry3d/src/utils/
// hashmap.rs`) — an explicitly fixed-seed hasher, not std's randomized SipHash, the same
// "reproducible container" shape `enhanced-determinism`'s `IndexMap`/`IndexSet` already rely on.
// And it was never physics-specific to begin with: the root `Cargo.toml`'s
// `serde_json = { features = ["preserve_order"] }` already pulls `indexmap` (and so `hashbrown`)
// into every crate in the workspace, `ludion-core` included, well before `ludion-physics` existed.
// A hasher *choice* is a fact about source code, not about the dependency graph, so `cargo
// metadata` cannot see it either way — asserting "hashbrown absent" would be false on day one, and
// asserting "hashbrown present with a fixed hasher" is not something this script can check. What
// stands behind it instead is the per-scene cross-arch trace gate (`Plan/physics.md` gate 1): any
// non-deterministic container reachable from the solver would show up as a diverging hash on the
// second pinned machine, which is a stronger, real test of the property that actually matters.
//
// A fifth check exists because Cargo honours `[patch]` only in the root workspace: a game that
// depends on Ludion by path would build the registry rapier3d/parry3d, unpatched. The vendored
// crates are therefore path dependencies, and a scratch consumer of `ludion-physics` (under
// target/) is resolved to prove a path consumer gets vendor/rapier3d and vendor/parry3d too.
//
// Usage:  node scripts/physics-features.mjs
//
// Exit 0 clean, 1 on any violation (message names the exact mismatch or the dependency path),
// 2 on a usage error (cargo not found, metadata malformed). Zero npm dependencies — Node built-ins
// only. Runs unchanged on Windows and macOS.

import fs from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { parseArgs } from 'node:util';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

let values;
try {
  ({ values } = parseArgs({ options: { help: { type: 'boolean', short: 'h', default: false } } }));
} catch (err) {
  console.error(`physics-features: ${err.message}`);
  process.exit(2);
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

// The exact feature set `rapier3d` must resolve to under `[workspace.dependencies]`'s pin
// (`dim3 f32 std enhanced-determinism parallel serde-serialize`, `default-features = false`): those
// six plus `alloc`, which rapier3d's own `std` feature unconditionally implies. A resolved set that
// differs — larger (something unexpectedly on, e.g. a `simd*` feature) or smaller (a pin
// regression, `parallel` dropped) — is exactly the silent-drift case this gate exists for.
const EXPECTED_RAPIER3D_FEATURES = ['alloc', 'dim3', 'enhanced-determinism', 'f32', 'parallel', 'serde-serialize', 'std'];

// parry3d's feature set as `rapier3d` 0.35.3 actually resolves it today (inspected via `cargo
// metadata` against the pinned lock, not derived by reading manifests): `required-features` (its
// own `dim3`+`f32` gate), the six it shares with rapier3d's request, plus what `alloc`,
// `enhanced-determinism` and `parallel` each turn on transitively inside parry3d itself
// (`downcast-rs`, `ena`, `hashbrown`, `indexmap`, `rstar`, `slab`, `smallvec`, `serde`,
// `serde_arrays`, and `rayon`, the implicit feature of parry's `parallel = ["rayon"]`). Any drift here —
// added or missing — means the pin moved under us or parry3d's own feature wiring changed, either
// of which is a stop-and-recheck event, not something to wave through silently.
const EXPECTED_PARRY3D_FEATURES = [
  'alloc', 'dim3', 'downcast-rs', 'ena', 'enhanced-determinism', 'f32', 'hashbrown', 'indexmap',
  'parallel', 'rayon', 'required-features', 'rstar', 'serde', 'serde-serialize', 'serde_arrays', 'slab', 'smallvec', 'std',
];

// glamx (rapier3d's math layer) and the glam it builds on: `scalar-math` keeps every glam operation
// off SIMD, `libm`/`nostd-libm` route transcendentals through pure-Rust libm. Recorded as resolved
// today (the glam below is the one glamx depends on; other glam versions in the graph belong to
// other crates). `fast-math` and `core-simd` trade bit-exactness for speed and are denied on every
// glam in the graph, whoever turns them on.
const EXPECTED_GLAMX_FEATURES = [
  'approx', 'glamx-serde', 'i32', 'libm', 'nalgebra', 'nostd-libm', 'scalar-math', 'serde', 'std',
];
const EXPECTED_GLAM_FEATURES = ['approx', 'f64', 'i32', 'libm', 'nostd-libm', 'scalar-math', 'serde', 'std', 'u32'];
const DENIED_GLAM_FEATURES = ['fast-math', 'core-simd'];

const FORBIDDEN_NORMAL_DEPS = new Set(['rayon', 'rayon-core']);
// The one named carve-out: rapier's internal scheduler (Serious Isaac 0020). Only these crates may
// depend on rayon; the pool they run on is owned by `ludion-physics` and sized `par::threads()`.
const ALLOWED_RAYON_EDGES = new Set(['rapier3d', 'parry3d']);

function fail(msg) {
  console.error(`physics-features: ${msg}`);
  process.exit(1);
}

function usageError(msg) {
  console.error(`physics-features: ${msg}`);
  process.exit(2);
}

function setsEqual(a, b) {
  const sa = [...a].sort();
  const sb = [...b].sort();
  return sa.length === sb.length && sa.every((v, i) => v === sb[i]);
}

function diffSets(expected, actual) {
  const e = new Set(expected);
  const a = new Set(actual);
  const missing = expected.filter((f) => !a.has(f));
  const extra = actual.filter((f) => !e.has(f));
  return { missing, extra };
}

function runMetadata(args, what) {
  const result = spawnSync('cargo', ['metadata', '--format-version', '1', ...args], {
    cwd: REPO_ROOT,
    maxBuffer: 256 * 1024 * 1024,
    encoding: 'utf8',
  });
  if (result.error) usageError(`failed to spawn cargo: ${result.error.message}`);
  if (result.status !== 0) {
    usageError(`cargo metadata for ${what} failed (exit ${result.status}); run \`cargo check -p ludion-physics\` to refresh Cargo.lock:\n${result.stderr}`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch (err) {
    usageError(`cargo metadata produced unparseable JSON: ${err.message}`);
  }
}

function indexPackages(meta) {
  const byName = new Map();
  for (const pkg of meta.packages) {
    if (!byName.has(pkg.name)) byName.set(pkg.name, []);
    byName.get(pkg.name).push(pkg);
  }
  return byName;
}

const metadata = runMetadata(['--locked'], 'the workspace');
const packagesByName = indexPackages(metadata);
const nodesById = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));

function singlePackage(name, byName = packagesByName) {
  const pkgs = byName.get(name) ?? [];
  if (pkgs.length === 0) fail(`no "${name}" package in the resolve graph — is it still a dependency of ludion-physics?`);
  if (pkgs.length > 1) {
    fail(`${pkgs.length} distinct "${name}" versions in the resolve graph (${pkgs.map((p) => p.version).join(', ')}) — a pin drifted; only one is expected`);
  }
  return pkgs[0];
}

// `name` must be exactly one path package under vendor/<name>/; `how` names what keeps it there.
function assertVendored(byName, name, how, where) {
  const pkg = singlePackage(name, byName);
  const manifest = pkg.manifest_path.replaceAll('\\', '/');
  if (pkg.source !== null || !manifest.endsWith(`/vendor/${name}/Cargo.toml`)) {
    fail(
      `${name}@${pkg.version} resolved from ${pkg.source ?? manifest}, not vendor/${name}, ${where} — ${how} has fallen ` +
      `away, so the patch (vendor/${name}/LUDION-PATCH.md) is not in the build`,
    );
  }
  return pkg;
}

function resolvedFeatures(pkg) {
  const node = nodesById.get(pkg.id);
  if (!node) fail(`"${pkg.name}@${pkg.version}" has no resolve node — malformed cargo metadata output`);
  return node.features;
}

// 1-2. rapier3d: exactly one version, from the vendored patch, exactly the expected feature set.
// The pin is a vendored patch (vendor/rapier3d/LUDION-PATCH.md): a path package, never the registry one.
const rapier = assertVendored(
  packagesByName, 'rapier3d', 'the path on the workspace rapier3d entry in the root Cargo.toml', 'in the workspace',
);
const rapierFeatures = resolvedFeatures(rapier);
if (!setsEqual(rapierFeatures, EXPECTED_RAPIER3D_FEATURES)) {
  const { missing, extra } = diffSets(EXPECTED_RAPIER3D_FEATURES, rapierFeatures);
  fail(
    `rapier3d@${rapier.version} resolved features do not match the pinned set.\n` +
    `  expected: ${EXPECTED_RAPIER3D_FEATURES.join(', ')}\n` +
    `  actual:   ${[...rapierFeatures].sort().join(', ')}\n` +
    (missing.length ? `  missing:  ${missing.join(', ')}\n` : '') +
    (extra.length ? `  extra:    ${extra.join(', ')} (something turned this on — check for a feature unified in from elsewhere in the workspace)\n` : ''),
  );
}

// 3. parry3d: exactly one version, `enhanced-determinism` present (checked first, for a pointed
// message — it is the feature that selects the fixed-hasher containers), then the exact set.
// Also a vendored patch (vendor/parry3d/LUDION-PATCH.md): a path package, never the registry one.
const parry = assertVendored(
  packagesByName, 'parry3d', 'the path on the parry3d dependency in vendor/rapier3d/Cargo.toml', 'in the workspace',
);
const parryFeatures = resolvedFeatures(parry);
if (!parryFeatures.includes('enhanced-determinism')) {
  fail(
    `parry3d@${parry.version} resolved WITHOUT "enhanced-determinism" — this is the feature that ` +
    `selects parry3d's fixed-seed hashbrown containers over an unordered default; without it the ` +
    `solver's iteration order is not guaranteed reproducible. Resolved features: ${[...parryFeatures].sort().join(', ')}`,
  );
}
if (!setsEqual(parryFeatures, EXPECTED_PARRY3D_FEATURES)) {
  const { missing, extra } = diffSets(EXPECTED_PARRY3D_FEATURES, parryFeatures);
  fail(
    `parry3d@${parry.version} resolved features do not match the recorded set.\n` +
    `  expected: ${EXPECTED_PARRY3D_FEATURES.join(', ')}\n` +
    `  actual:   ${[...parryFeatures].sort().join(', ')}\n` +
    (missing.length ? `  missing:  ${missing.join(', ')}\n` : '') +
    (extra.length ? `  extra:    ${extra.join(', ')} (something turned this on — check for a feature unified in from elsewhere in the workspace)\n` : ''),
  );
}

// 3b. glamx and its glam: the exact feature sets, and no glam anywhere with a denied feature.
const glamx = singlePackage('glamx');
const glamxNode = nodesById.get(glamx.id);
const glamxFeatures = resolvedFeatures(glamx);
const glamId = glamxNode.deps.find((dep) => dep.name === 'glam')?.pkg;
if (!glamId) fail(`glamx@${glamx.version} has no glam dependency in the resolve graph — the math layer moved under us`);
const glam = metadata.packages.find((p) => p.id === glamId);
const checkSet = (pkg, expected, features) => {
  if (setsEqual(features, expected)) return;
  const { missing, extra } = diffSets(expected, features);
  fail(
    `${pkg.name}@${pkg.version} resolved features do not match the recorded set.\n` +
    `  expected: ${expected.join(', ')}\n  actual:   ${[...features].sort().join(', ')}\n` +
    (missing.length ? `  missing:  ${missing.join(', ')}\n` : '') +
    (extra.length ? `  extra:    ${extra.join(', ')}\n` : ''),
  );
};
checkSet(glamx, EXPECTED_GLAMX_FEATURES, glamxFeatures);
checkSet(glam, EXPECTED_GLAM_FEATURES, resolvedFeatures(glam));
for (const pkg of packagesByName.get('glam') ?? []) {
  const denied = resolvedFeatures(pkg).filter((f) => DENIED_GLAM_FEATURES.includes(f));
  if (denied.length) {
    fail(`glam@${pkg.version} resolved with ${denied.join(', ')} — fast-math and core-simd change float results across targets`);
  }
}

// 4. rayon / rayon-core reachable from ludion-physics's own normal dependency closure only through
// the carve-out edges (rapier3d/parry3d -> rayon), both of which must exist. A dep_kind
// entry with `kind: null` (or an empty `dep_kinds` array, which cargo treats the same way) is a
// normal edge; dev/build-only edges are not walked, since neither reaches a shipped build.
const physicsPkgs = packagesByName.get('ludion-physics') ?? [];
if (physicsPkgs.length !== 1) fail(`expected exactly one "ludion-physics" package in the resolve graph, found ${physicsPkgs.length}`);
const physicsRoot = nodesById.get(physicsPkgs[0].id);
if (!physicsRoot) fail('ludion-physics has no resolve node — malformed cargo metadata output');

const isNormalEdge = (dep) => dep.dep_kinds.length === 0 || dep.dep_kinds.some((k) => k.kind === null);
const nameById = new Map(metadata.packages.map((p) => [p.id, p.name]));
const parentOf = new Map();
const visited = new Set([physicsRoot.id]);
const queue = [physicsRoot.id];
const allowedEdgesSeen = new Set();
while (queue.length) {
  const id = queue.shift();
  const node = nodesById.get(id);
  for (const dep of node.deps) {
    if (!isNormalEdge(dep)) continue;
    if (ALLOWED_RAYON_EDGES.has(nameById.get(id)) && FORBIDDEN_NORMAL_DEPS.has(nameById.get(dep.pkg))) {
      allowedEdgesSeen.add(`${nameById.get(id)} -> ${nameById.get(dep.pkg)}`);
      continue;
    }
    if (visited.has(dep.pkg)) continue;
    visited.add(dep.pkg);
    parentOf.set(dep.pkg, id);
    queue.push(dep.pkg);
  }
}

for (const id of visited) {
  const name = nameById.get(id);
  if (!FORBIDDEN_NORMAL_DEPS.has(name)) continue;
  const chain = [id];
  let cur = id;
  while (parentOf.has(cur)) {
    cur = parentOf.get(cur);
    chain.push(cur);
  }
  chain.reverse();
  const path_ = chain.map((pid) => `${nameById.get(pid)}@${metadata.packages.find((p) => p.id === pid)?.version ?? '?'}`).join(' -> ');
  fail(`"${name}" is reachable from ludion-physics's normal dependency closure:\n  ${path_}\nRayon is allowed only through rapier3d and parry3d (the \`parallel\` carve-out); something else in the closure now reaches it.`);
}
for (const edge of ['rapier3d -> rayon', 'parry3d -> rayon']) {
  if (!allowedEdgesSeen.has(edge)) {
    fail(
      `${edge.split(' ')[0]} no longer reaches rayon — the \`parallel\` pin changed? Expected rapier3d -> rayon and ` +
      `parry3d -> rayon (seen: ${[...allowedEdgesSeen].join(', ') || 'none'})`,
    );
  }
}

// 5. A path consumer of ludion-physics resolves the vendored crates. Only the scratch dir under
// target/ is written; its empty [workspace] keeps cargo from adopting it into Ludion's workspace.
const probeDir = path.join(REPO_ROOT, 'target', 'physics-consumer-probe');
fs.rmSync(probeDir, { recursive: true, force: true });
fs.mkdirSync(path.join(probeDir, 'src'), { recursive: true });
fs.writeFileSync(
  path.join(probeDir, 'Cargo.toml'),
  '[package]\nname = "physics-consumer-probe"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n' +
  `[dependencies]\nludion-physics = { path = "${REPO_ROOT.replaceAll('\\', '/')}/crates/ludion-physics" }\n\n[workspace]\n`,
);
fs.writeFileSync(path.join(probeDir, 'src', 'lib.rs'), '');
fs.copyFileSync(path.join(REPO_ROOT, 'Cargo.lock'), path.join(probeDir, 'Cargo.lock'));
const probePackages = indexPackages(
  runMetadata(['--manifest-path', path.join(probeDir, 'Cargo.toml')], 'the scratch path consumer'),
);
for (const name of ['rapier3d', 'parry3d']) {
  assertVendored(
    probePackages, name, 'the path dependency that carries it (a path consumer of Ludion would build unpatched physics, external-game request 0023)',
    'for a path consumer of ludion-physics',
  );
}

console.log(
  `physics-features: ok — rapier3d@${rapier.version} and parry3d@${parry.version} resolved to their ` +
  `pinned feature sets, glam/glamx on their recorded sets with no fast-math or core-simd, rayon/rayon-core reachable from ludion-physics only through rapier3d and parry3d (the parallel carve-out), and a path consumer resolves both from vendor/`,
);
