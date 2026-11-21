#!/usr/bin/env node
// Lists that must stay in step, checked mechanically. Each of these was a
// hand-kept copy that had already drifted, or was one edit away from it:
//
//   nextest   `.config/nextest.toml` selects GPU tests by name: the `gpu` test-group override and the
//             `no-gpu` profile's `default-filter` are both the one expression `test(/(^|::)needs_gpu_/)`.
//             Checks: both filters are exactly that text; every `binary_id(=...)` in any override or the
//             no-gpu filter names a test binary or package that exists; every `#[test]` under
//             `crates/*/{src,tests}` and `games/*/{src,tests}` whose body puts a device up (`Gpu::request`,
//             `Renderer::new`, `Presenter::new`, a no-adapter skip, L2003) is named `needs_gpu_*`; a
//             `needs_gpu_*` test sits in a file that touches a device; and every phase4 `#[test]` that
//             calls `run_fps_gate` matches the exclusive fps override. `LUDION_FORBID_GPU=1` is the runtime
//             half: the hosted no-gpu lane panics on device creation, catching a GPU test this scan misses.
//             `games/vigil/.config/nextest.toml` (its own workspace) gets the filter and binary checks
//             when present, and its manifest's `[profile.*]` tail must equal the root Cargo.toml's.
//   c-header  `crates/ludion-cli/templates/ios/ludion.h` is the C ABI a native host compiles against;
//             `crates/ludion/src/ffi.rs` is the Rust side. Checks: the LUDION_* constants (name and
//             value), the exported functions (parameter types and return type; parameter names need
//             not match) and each `#[repr(C)]` struct's field sequence (name and mapped type, in order)
//             agree. The one symbol only the header declares is the game's own (`ludion_game_start`).
//   require-gpu  every Rust file under `crates/*/{src,tests}` and `games/*/{src,tests}` that prints a
//             `SKIPPED` line for a missing adapter (`L2003` or "adapter" beside it) must route the skip
//             through `ludion_core::require_gpu`, or `LUDION_REQUIRE_GPU=1` cannot turn it into a failure.
//   bless     every read of `LUDION_BLESS*` / `BLESS_SCHEMA` under `crates/` and `games/` goes through
//             `ludion_core::bless`, so one parse (non-empty and not "0") decides every bless switch.
//   clippy    every `clippy.toml` is generated from `scripts/clippy-denylist.mjs` (the determinism
//             denylist: one source, three tiers). Checks: a strict/sim file is its hand-written header
//             plus the generated body byte for byte; a permissive file carries no list; every workspace
//             crate is classified in `CRATES` and every classified dir exists. `--write` there regenerates.
//   crate-allow  no strict or sim crate carries a crate-level `#![allow(..)]` / `#![expect(..)]` of
//             `clippy::disallowed_methods` / `disallowed_types`, which would switch the denylist off for
//             the whole crate; an item-level allow with a why-comment stays legal.
//   orphan    in a package with `autotests = false` (ludion-engine's merged `it` binary), every top-level
//             `tests/*.rs` is a `[[test]]` path or a `mod <stem>;` of one of the package's test roots, else
//             it never compiles or runs. Binary ids come from the manifest's `[[test]]` tables there.
//   proptest  every `proptest!` block opens with a `proptest_config` that is `deterministic_config()` or a
//             `RngSeed::Fixed` seed with `failure_persistence: None`, so a red run reproduces anywhere and
//             no `.proptest-regressions` file is written.
//   crate-graph  the edge listing in `dev-docs/crate-graph-edges.md` is generated from `cargo metadata`
//             by `scripts/crate-graph-edges.mjs`; the block must match the manifests byte for byte
//             (`--write` regenerates), and every workspace package needs a Layout line in
//             `dev-docs/crate-graph.md`. Needs the toolchain for `cargo metadata`.
//   manifests  no member crate writes a dependency version of its own (`foo.workspace = true` or a
//             path; `games/vigil` is a detached workspace and is skipped), and a game's plain
//             `[dependencies]` (and the `ludion new` template's) name no `ludion-*` crate, only the
//             `ludion` facade; `scripts/manifest-rules.mjs` holds both.
//   doc-citations  every `ludion docs <t>` / `ludion devdocs <t>` in `docs/`, `dev-docs/` and crate
//             sources names a topic in the right root (`docs/` vs `dev-docs/`), across a line wrap;
//             `scripts/doc-citations.mjs` holds it.
//
// Every violation is reported, not just the first.
//
// Usage:  node scripts/lists-in-step.mjs
//
// Exit 0 in step, 1 on drift. Zero npm dependencies — Node built-ins only.

import { readdirSync, readFileSync, existsSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { CRATES, checkClippyLists, workspaceDirs } from './clippy-denylist.mjs';
import { checkCrateGraph } from './crate-graph-edges.mjs';
import { checkDocCitations } from './doc-citations.mjs';
import { checkManifests } from './manifest-rules.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const NEXTEST = path.join(REPO_ROOT, '.config/nextest.toml');
// vigil is a Lore-only game with its own workspace; its nextest config and manifest are checked only
// where it is present.
const VIGIL_NEXTEST = path.join(REPO_ROOT, 'games/vigil/.config/nextest.toml');
const VIGIL_MANIFEST = path.join(REPO_ROOT, 'games/vigil/Cargo.toml');
const FFI_RS = path.join(REPO_ROOT, 'crates/ludion/src/ffi.rs');
const LUDION_H = path.join(REPO_ROOT, 'crates/ludion-cli/templates/ios/ludion.h');
const GPU_MARKERS = /L2003|no_adapter|Gpu::request|Renderer::new/;
// Declared only in the header: the game exports it, the facade does not.
const HEADER_ONLY_FUNCTIONS = new Set(['ludion_game_start']);
// Binaries that match a GPU marker yet stay out of both lists on purpose.
const GPU_MARKER_EXEMPT = new Map([
  // Its scaffold `cargo test` half is the hosted-CI gate for `ludion new`, so no-gpu must keep it; the
  // bless half renders but skips on L2003, and the cargo-build group already serializes it.
  ['ludion-cli::new', 'runs on hosted CI; the render half skips without an adapter'],
]);

const read = (p) => readFileSync(p, 'utf8').replace(/\r\n/g, '\n');
const rel = (p) => path.relative(REPO_ROOT, p).split(path.sep).join('/');
const subdirs = (dir) => (existsSync(dir) ? readdirSync(dir).filter((n) => statSync(path.join(dir, n)).isDirectory()) : []);

// ── workspace scan ───────────────────────────────────────────────────────────

// Every workspace package (`crates/*`, `games/*`) and its integration-test binaries, by binary id.
function scanWorkspace() {
  const packages = new Set();
  const binaries = new Map(); // binary id -> source path
  const orphans = [];
  for (const group of ['crates', 'games']) {
    for (const dirName of subdirs(path.join(REPO_ROOT, group))) {
      const dir = path.join(REPO_ROOT, group, dirName);
      const manifest = path.join(dir, 'Cargo.toml');
      if (!existsSync(manifest)) continue;
      const name = /^\[package\][\s\S]*?^name\s*=\s*"([^"]+)"/m.exec(read(manifest))?.[1];
      if (!name) continue;
      packages.add(name);
      const manifestText = read(manifest);
      const declared = tomlBlocks(manifestText).filter((b) => b.header === '[[test]]').map((b) => ({
        name: /^name\s*=\s*"([^"]+)"/m.exec(b.body)?.[1],
        path: /^path\s*=\s*"([^"]+)"/m.exec(b.body)?.[1],
      }));
      for (const t of declared) {
        if (t.name) binaries.set(`${name}::${t.name}`, path.join(dir, t.path ?? `tests/${t.name}.rs`));
      }
      const tests = path.join(dir, 'tests');
      if (!existsSync(tests)) continue;
      if (/^autotests\s*=\s*false/m.test(manifestText.split(/^\[(?!package\])/m)[0])) {
        // A file nothing declares never compiles: it must be a `[[test]]` path or a `mod` of a declared root.
        const roots = declared.map((t) => path.join(dir, t.path ?? `tests/${t.name}.rs`));
        const rootText = roots.filter((r) => existsSync(r)).map(read).join('\n');
        for (const entry of readdirSync(tests)) {
          if (!entry.endsWith('.rs') || statSync(path.join(tests, entry)).isDirectory()) continue;
          const stem = entry.slice(0, -3);
          const full = path.join(tests, entry);
          if (roots.includes(full) || new RegExp(`^\\s*(pub\\s+)?mod\\s+(r#)?${stem}\\s*;`, 'm').test(rootText)) continue;
          orphans.push(`orphan: ${rel(full)} is neither a [[test]] path in ${rel(manifest)} nor a \`mod ${stem};\` in one of its test roots, so it never compiles, declare it`);
        }
        continue;
      }
      for (const entry of readdirSync(tests)) {
        const full = path.join(tests, entry);
        if (statSync(full).isDirectory()) {
          if (existsSync(path.join(full, 'main.rs'))) binaries.set(`${name}::${entry}`, path.join(full, 'main.rs'));
        } else if (entry.endsWith('.rs')) {
          binaries.set(`${name}::${entry.slice(0, -3)}`, full);
        }
      }
    }
  }
  return { packages, binaries, orphans };
}

// ── nextest ──────────────────────────────────────────────────────────────────

// The text of each top-level table (`[..]` / `[[..]]` header line to the next), comments dropped.
function tomlBlocks(text) {
  const blocks = [];
  let current = null;
  for (const line of text.split('\n')) {
    if (/^\s*#/.test(line)) continue;
    if (/^\[/.test(line)) {
      current = { header: line.trim(), body: '' };
      blocks.push(current);
    } else if (current) {
      current.body += `${line}\n`;
    }
  }
  return blocks;
}

const binaryIds = (text) => [...text.matchAll(/binary_id\(=([^)]+)\)/g)].map((m) => m[1].trim());

const GPU_PREFIX_FILTER = 'test(/(^|::)needs_gpu_/)';
// A test body that puts a device up (or skips for want of one) is a GPU test and must carry the prefix.
const GPU_BODY_MARKERS = /Gpu::request|Renderer::new|Presenter::new|skip_no_adapter|is_no_adapter|L2003/;
// A `needs_gpu_` test lives in a file that touches a device itself or through a spawned renderer.
const GPU_FILE_MARKERS = new RegExp(`${GPU_BODY_MARKERS.source}|require_gpu|ludion\\(\\)|Command::new|spawn`);
// Tests that match a body marker yet stay unprefixed on purpose, keyed `<file>::<test>`.
const GPU_BODY_EXEMPT = new Map([
  // The scaffolded project's `cargo test` half is the hosted-CI gate for `ludion new`, so no-gpu must
  // keep it; the bless half renders but skips on L2003, and `FORBID_GPU_ENV` is removed for the child.
  ['crates/ludion-cli/tests/new.rs::generated_project_passes_cargo_test', 'runs on hosted CI; the render half skips without an adapter'],
  // Their `L2003` / `is_no_adapter` is a code under test, not a device: one runs `ludion explain`, the
  // other asserts a validation error arrives before any adapter is requested.
  ['crates/ludion-cli/tests/render.rs::render_codes_are_explainable', 'names L2003 to explain it; no device'],
  ['crates/ludion-testkit/tests/gallery.rs::broken_voxel_model_document_fails_with_diagnostics_not_panic', 'asserts no adapter was requested; no device'],
]);
// `needs_gpu_` files with no marker of their own, with the real reason.
const GPU_FILE_EXEMPT = new Map([
  ['crates/ludion-engine/tests/local_shadow_fade.rs', 'profile_scene puts the device up inside the library call'],
  ['crates/ludion-renderer/src/render/tests/lit_additive.rs', 'skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/shaft_layer.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/medium_scatter.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/skin_bounds.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/rim.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/film.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/lut.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/mesh_medium.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/transparent_medium.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/transparent_dof.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/gpu_cull.rs', 'skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/viewmodel.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/silhouette.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/blended.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/rain.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/exposure.rs','skip_or_renderer in render.rs puts the device up'],
  ['crates/ludion-renderer/src/render/tests/exposure_view.rs','skip_or_renderer in render.rs puts the device up'],
]);

const normalize = (s) => s.replace(/\s+/g, ' ').trim();

// Every `.rs` under `crates/*/{src,tests}` and `games/*/{src,tests}`, as [relative path, text].
function rustSources() {
  const out = [];
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const p = path.join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (name.endsWith('.rs')) out.push([rel(p), read(p)]);
    }
  };
  for (const group of ['crates', 'games']) {
    for (const dirName of subdirs(path.join(REPO_ROOT, group))) {
      for (const sub of ['src', 'tests']) {
        const dir = path.join(REPO_ROOT, group, dirName, sub);
        if (existsSync(dir)) walk(dir);
      }
    }
  }
  return out;
}

// Each `#[test]` fn in `text`: its name, 1-based line and body (the `fn` line to the first `}` line at
// the fn's own indentation).
function testFns(text) {
  const lines = text.split('\n');
  const fns = [];
  for (let i = 0; i < lines.length; i++) {
    if (!/^\s*#\[(?:\w+::)*test\b/.test(lines[i])) continue;
    for (let j = i + 1; j < Math.min(i + 8, lines.length); j++) {
      const m = /^(\s*)(?:pub\s+)?(?:async\s+)?fn\s+(\w+)/.exec(lines[j]);
      if (!m) continue;
      let end = j;
      while (end + 1 < lines.length && lines[end] !== `${m[1]}}`) end++;
      fns.push({ name: m[2], line: j + 1, body: lines.slice(j, end + 1).join('\n') });
      break;
    }
  }
  return fns;
}

// `file` is a nextest config; only the root's drives the test-body scan and the fps override.
function checkNextest({ packages, binaries }, file = NEXTEST) {
  const isRoot = file === NEXTEST;
  const errors = [];
  const blocks = tomlBlocks(read(file));
  const overrides = blocks.filter((b) => b.header === '[[profile.default.overrides]]');
  const gpuBlock = overrides.find((b) => /^test-group\s*=\s*'gpu'/m.test(b.body));
  const noGpuBlock = blocks.find((b) => b.header === '[profile.no-gpu]');
  if (!gpuBlock) errors.push(`nextest: no [[profile.default.overrides]] with test-group = 'gpu' in ${rel(file)}`);
  if (!noGpuBlock || !/^default-filter\s*=/m.test(noGpuBlock.body)) {
    errors.push(`nextest: no [profile.no-gpu] default-filter in ${rel(file)}`);
  }
  if (errors.length) return { errors, count: 0 };

  const filterOf = (body, key) => {
    const entries = [...body.matchAll(/^([\w-]+)\s*=\s*(?:'([^']*)'|"{3}([\s\S]*?)"{3})/gm)];
    const m = entries.find((e) => e[1] === key);
    return m ? normalize(m[2] ?? m[3]) : null;
  };
  const gpuFilter = filterOf(gpuBlock.body, 'filter');
  const noGpuFilter = filterOf(noGpuBlock.body, 'default-filter');
  if (gpuFilter !== GPU_PREFIX_FILTER) {
    errors.push(`nextest: ${rel(file)}'s gpu override filter is \`${gpuFilter}\`, expected exactly \`${GPU_PREFIX_FILTER}\` — GPU tests are selected by the \`needs_gpu_\` name, not by binary`);
  }
  if (noGpuFilter !== `not ${GPU_PREFIX_FILTER}`) {
    errors.push(`nextest: ${rel(file)}'s no-gpu default-filter is \`${noGpuFilter}\`, expected exactly \`not ${GPU_PREFIX_FILTER}\``);
  }

  // Every binary a filter names must exist (a renamed binary silently stops matching).
  const named = new Set([...overrides, noGpuBlock].flatMap((b) => binaryIds(b.body)));
  for (const id of named) {
    const exists = id.includes('::') ? binaries.has(id) : packages.has(id);
    if (!exists) errors.push(`nextest: \`${id}\` in ${rel(file)} names no ${id.includes('::') ? 'test binary' : 'workspace package'}`);
  }
  if (!isRoot) return { errors, count: 0 };

  let scanned = 0;
  for (const [file, text] of rustSources()) {
    const fns = testFns(text);
    scanned += fns.length;
    const fileMarker = GPU_FILE_MARKERS.exec(text)?.[0];
    for (const fn of fns) {
      const marker = GPU_BODY_MARKERS.exec(fn.body)?.[0];
      const prefixed = fn.name.startsWith('needs_gpu_');
      if (marker && !prefixed && !GPU_BODY_EXEMPT.has(`${file}::${fn.name}`)) {
        errors.push(`nextest: ${file}:${fn.line} \`${fn.name}\` matches \`${marker}\` (it puts a device on the card) but is not named \`needs_gpu_*\` — rename it so the \`no-gpu\` profile excludes it`);
      } else if (prefixed && !fileMarker && !GPU_FILE_EXEMPT.has(file)) {
        errors.push(`nextest: ${file}:${fn.line} \`${fn.name}\` is named \`needs_gpu_*\` but nothing in the file touches a device — drop the prefix, or exempt the file in scripts/lists-in-step.mjs with its reason`);
      }
    }
  }
  errors.push(...checkFpsOverride(overrides, binaries));
  return { errors, count: scanned };
}

// Every `#[test]` in phase4 that calls `run_fps_gate` must match the exclusive fps override's regex,
// or it measures a saturated box — the drift that made the `_lz4` and `_raw` twins flake.
function checkFpsOverride(overrides, binaries) {
  const block = overrides.find((b) => /^threads-required\s*=\s*"num-test-threads"/m.test(b.body));
  const source = binaries.get('ludion-server::phase4');
  if (!block) return [`nextest: no [[profile.default.overrides]] with threads-required = "num-test-threads" (the exclusive fps override) in ${rel(NEXTEST)}`];
  if (!source) return ['nextest: ludion-server::phase4 names no test binary, so the fps override cannot be checked'];
  const matchers = [...block.body.matchAll(/test\((?:=(\w+)|\/(.+?)\/)\)/g)].map((m) => (m[1] ? (n) => n === m[1] : (n) => new RegExp(m[2]).test(n)));
  if (matchers.length === 0) return [`nextest: the exclusive fps override's filter has no test(=name) or test(/regex/) in ${rel(NEXTEST)}`];
  const filter = { test: (n) => matchers.some((m) => m(n)) };
  const callers = read(source)
    .split('#[test]')
    .slice(1)
    .map((chunk) => ({ name: /fn (\w+)/.exec(chunk)?.[1], calls: chunk.includes('run_fps_gate(') }))
    .filter((t) => t.calls);
  if (callers.length === 0) return [`nextest: no #[test] in ${rel(source)} calls run_fps_gate — the fps-override scan in scripts/lists-in-step.mjs has gone stale`];
  return callers
    .filter((t) => !filter.test(t.name))
    .map((t) => `nextest: \`${t.name}\` calls run_fps_gate but the exclusive fps override's filter does not match it — it would measure a loaded box; name it needs_gpu_frame_stream_delivers_at_least_30_fps_at_1080p_*`);
}

// The profiles vigil's own manifest repeats must stay byte-identical to the root's: cargo reads profiles
// from the workspace root only, so a drifted copy silently changes how vigil's tests are built.
function checkVigilProfiles() {
  const tail = (file) => {
    const text = read(file);
    const at = text.indexOf('[profile.dev]');
    return at < 0 ? null : text.slice(at);
  };
  const root = tail(path.join(REPO_ROOT, 'Cargo.toml'));
  const vigil = tail(VIGIL_MANIFEST);
  if (root === null) return ['profiles: the root Cargo.toml has no [profile.dev]'];
  if (vigil === null) return [`profiles: ${rel(VIGIL_MANIFEST)} has no [profile.dev] — copy the root Cargo.toml's [profile.*] tail into it`];
  return root === vigil ? [] : [`profiles: ${rel(VIGIL_MANIFEST)} from [profile.dev] to the end differs from Cargo.toml's — vigil's own workspace ignores the root profiles, so the two must stay identical`];
}

// ── c-header ─────────────────────────────────────────────────────────────────

const PRIMITIVES = {
  u32: 'uint32_t', i32: 'int32_t', u64: 'uint64_t', i64: 'int64_t', f32: 'float', bool: 'bool',
};
const cType = (t) => t.replace(/\s+/g, ' ').replace(/\s*\*\s*/g, ' *').trim();

// Rust FFI type -> the C spelling; `null` for a type the map does not know (reported by the caller).
function rustToC(ty, cNames) {
  ty = ty.trim();
  const ptr = /^\*(const|mut)\s+(.+)$/.exec(ty);
  if (ptr) {
    const [, mutability, pointee] = ptr;
    const inner = pointee === 'c_char' ? 'char' : pointee === 'c_void' ? 'void' : PRIMITIVES[pointee] ?? (cNames.has(pointee) ? pointee : null);
    if (inner === null) return null;
    return cType(`${mutability === 'const' ? 'const ' : ''}${inner} *`);
  }
  return PRIMITIVES[ty] ?? null;
}

// `Type name` / `const T *name` -> the type part, canonical.
function paramType(decl) {
  const m = /^(.*?)\s*(\*+)?\s*(\w+)$/.exec(decl.trim());
  return m ? cType(`${m[1]}${m[2] ? ` ${m[2]}` : ''}`) : decl.trim();
}

const stripCComments = (text) => text.replace(/\/\/.*$/gm, '');

function parseRust(text, cNames, errors) {
  const constants = new Map();
  for (const m of text.matchAll(/^pub const (LUDION_\w+): \w+ = (-?\d+);/gm)) constants.set(m[1], m[2]);

  const structs = new Map();
  for (const m of text.matchAll(/#\[repr\(C\)\]\s*pub struct (\w+) \{([\s\S]*?)\n\}/g)) {
    const fields = [];
    for (const line of m[2].split('\n')) {
      const f = /^\s*pub (\w+): ([^,]+),\s*$/.exec(line);
      if (!f) continue;
      const ty = rustToC(f[2], cNames);
      if (ty === null) errors.push(`c-header: ffi.rs struct ${m[1]} field \`${f[1]}\` has type \`${f[2].trim()}\`, which the gate's Rust→C type map does not know — add it to PRIMITIVES/rustToC in scripts/lists-in-step.mjs`);
      fields.push([f[1], ty ?? f[2].trim()]);
    }
    structs.set(m[1], fields);
  }

  const functions = new Map();
  for (const m of text.matchAll(/^pub unsafe extern "C" fn (\w+)\(([^)]*)\)\s*(?:->\s*([^{]+?))?\s*\{/gm)) {
    const params = m[2].split(',').map((p) => p.trim()).filter(Boolean).map((p) => {
      const ty = p.slice(p.indexOf(':') + 1);
      const mapped = rustToC(ty, cNames);
      if (mapped === null) errors.push(`c-header: ffi.rs fn ${m[1]} has a parameter of type \`${ty.trim()}\`, which the gate's Rust→C type map does not know — add it to PRIMITIVES/rustToC in scripts/lists-in-step.mjs`);
      return mapped ?? ty.trim();
    });
    let ret = 'void';
    if (m[3]) {
      ret = rustToC(m[3], cNames);
      if (ret === null) {
        errors.push(`c-header: ffi.rs fn ${m[1]} returns \`${m[3].trim()}\`, which the gate's Rust→C type map does not know — add it to PRIMITIVES/rustToC in scripts/lists-in-step.mjs`);
        ret = m[3].trim();
      }
    }
    functions.set(m[1], { params, ret });
  }
  return { constants, structs, functions };
}

function parseHeader(text) {
  const code = stripCComments(text);
  const constants = new Map();
  for (const m of code.matchAll(/^#define (LUDION_\w+) (-?\d+)\s*$/gm)) constants.set(m[1], m[2]);

  const structs = new Map();
  const cNames = new Set();
  for (const m of code.matchAll(/typedef struct (\w+) \1;/g)) cNames.add(m[1]);
  for (const m of code.matchAll(/typedef struct \{([\s\S]*?)\}\s*(\w+);/g)) {
    cNames.add(m[2]);
    const fields = [];
    for (const decl of m[1].split(';').map((d) => d.trim()).filter(Boolean)) {
      const names = decl.split(',').map((n) => n.trim());
      const first = /^(.*?)\s*(\*+)?\s*(\w+)$/.exec(names[0]);
      const base = first[1];
      fields.push([first[3], cType(`${base}${first[2] ? ` ${first[2]}` : ''}`)]);
      for (const extra of names.slice(1)) fields.push([extra, cType(base)]);
    }
    structs.set(m[2], fields);
  }

  const functions = new Map();
  for (const m of code.matchAll(/^([A-Za-z_][\w ]*?[ *]+)(ludion_\w+)\(([^)]*)\);/gm)) {
    const params = m[3].split(',').map((p) => p.trim()).filter(Boolean).map(paramType);
    functions.set(m[2], { params, ret: cType(m[1]) });
  }
  return { constants, structs, functions, cNames };
}

function checkHeader() {
  const errors = [];
  const header = parseHeader(read(LUDION_H));
  const rust = parseRust(read(FFI_RS), header.cNames, errors);
  const h = rel(LUDION_H);
  const r = rel(FFI_RS);

  for (const [name, value] of rust.constants) {
    if (!header.constants.has(name)) errors.push(`c-header: ${r} defines ${name} = ${value}, ${h} has no #define for it`);
    else if (header.constants.get(name) !== value) errors.push(`c-header: ${name} is ${value} in ${r} but ${header.constants.get(name)} in ${h}`);
  }
  for (const name of header.constants.keys()) {
    if (!rust.constants.has(name)) errors.push(`c-header: ${h} defines ${name}, ${r} has no matching const`);
  }

  for (const [name, fn] of rust.functions) {
    const c = header.functions.get(name);
    if (!c) { errors.push(`c-header: ${r} exports \`${name}\`, ${h} does not declare it`); continue; }
    if (c.ret !== fn.ret) errors.push(`c-header: \`${name}\` returns ${fn.ret} in ${r} but ${c.ret} in ${h}`);
    if (c.params.join(', ') !== fn.params.join(', ')) errors.push(`c-header: \`${name}\` takes (${fn.params.join(', ')}) in ${r} but (${c.params.join(', ')}) in ${h}`);
  }
  for (const name of header.functions.keys()) {
    if (!rust.functions.has(name) && !HEADER_ONLY_FUNCTIONS.has(name)) errors.push(`c-header: ${h} declares \`${name}\`, ${r} exports no such function`);
  }

  for (const [name, fields] of rust.structs) {
    const c = header.structs.get(name);
    if (!c) { errors.push(`c-header: ${r} has #[repr(C)] struct ${name}, ${h} has no typedef for it`); continue; }
    const show = (fs) => fs.map(([n, t]) => `${t} ${n}`).join('; ');
    if (show(fields) !== show(c)) errors.push(`c-header: struct ${name} differs — ${r}: {${show(fields)}} vs ${h}: {${show(c)}}`);
  }
  for (const name of header.structs.keys()) {
    if (!rust.structs.has(name)) errors.push(`c-header: ${h} typedefs ${name}, ${r} has no #[repr(C)] struct of that name`);
  }

  return { errors, count: rust.constants.size + rust.functions.size + rust.structs.size };
}

// ── main ─────────────────────────────────────────────────────────────────────

// A no-adapter skip that bypasses `ludion_core::require_gpu` passes on a lane that must gate images.
// Nextest never logs a passing test's stderr, so a log scan cannot catch it;
// the source can.
function checkRequireGpu() {
  const errors = [];
  let count = 0;
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const p = path.join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (name.endsWith('.rs')) {
        if (rel(p) === 'crates/ludion-core/src/require_gpu.rs') continue;
        const text = read(p);
        // A test that branches on the profile report's `gpu.available` is a no-adapter skip in disguise.
        if (rel(p).includes('/tests/') && /\b(?:if|while)\s+!?\s*[\w.]*\bgpu\.available\b/.test(text) && !text.includes('require_gpu')) {
          errors.push(`require-gpu: ${rel(p)} branches on gpu.available but never calls ludion_core::require_gpu — route the branch through \`skip_no_adapter(!report.gpu.available)\``);
        }
        if (!/SKIPPED/.test(text) || !/L2003|adapter/.test(text)) continue;
        count++;
        if (!text.includes('require_gpu')) {
          errors.push(`require-gpu: ${rel(p)} prints SKIPPED for a missing adapter but never calls ludion_core::require_gpu — route the skip through \`ludion_core::require_gpu::skip_no_adapter\` / \`is_no_adapter\``);
        }
      }
    }
  };
  for (const group of ['crates', 'games']) {
    for (const dirName of subdirs(path.join(REPO_ROOT, group))) {
      for (const sub of ['src', 'tests']) {
        const dir = path.join(REPO_ROOT, group, dirName, sub);
        if (existsSync(dir)) walk(dir);
      }
    }
  }
  return { errors, count };
}

// A bless switch read with its own parse (`var_os(..).is_some()` blesses on `LUDION_BLESS=0`; `var(..)`
// does not) makes one switch mean two things across tests. Every read goes
// through `ludion_core::bless`.
const BLESS_HELPER = 'crates/ludion-core/src/bless.rs';
const BLESS_READ = /\bvar(?:_os)?\(\s*"(?:LUDION_BLESS\w*|BLESS_SCHEMA)"/;

function checkBless() {
  const errors = [];
  let count = 0;
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      if (name === 'target') continue;
      const p = path.join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (name.endsWith('.rs') && rel(p) !== BLESS_HELPER) {
        count++;
        read(p).split('\n').forEach((line, i) => {
          if (BLESS_READ.test(line)) {
            errors.push(`bless: ${rel(p)}:${i + 1} reads a bless switch directly — use \`ludion_core::bless::bless_requested\` / \`switch_set\` so \`=0\` means off everywhere`);
          }
        });
      }
    }
  };
  for (const group of ['crates', 'games']) {
    for (const dirName of subdirs(path.join(REPO_ROOT, group))) walk(path.join(REPO_ROOT, group, dirName));
  }
  return { errors, count };
}

// A proptest on the default config draws a fresh seed per run and writes regression files beside the
// source; a red run then cannot be reproduced. Every block pins its seed and turns persistence off.
function checkProptest() {
  const errors = [];
  let count = 0;
  for (const [file, text] of rustSources()) {
    const lines = text.split('\n');
    lines.forEach((line, i) => {
      if (!/^\s*proptest!\s*\{/.test(line)) return;
      count++;
      const next = lines.slice(i + 1).find((l) => l.trim() !== '') ?? '';
      const ok = /#!\[proptest_config\(/.test(next)
        && (/deterministic_config\(\)/.test(next) || (/RngSeed::Fixed\(/.test(next) && /failure_persistence:\s*None/.test(next)));
      if (!ok) errors.push(`proptest: ${file}:${i + 1} \`proptest!\` must open with \`#![proptest_config(..)]\` using a fixed seed and \`failure_persistence: None\` (\`ludion devdocs ludion-testkit\`)`);
    });
  }
  return { errors, count };
}

// A crate-level inner attribute silences the denylist for the whole crate; the generated clippy.toml
// is then a decoration. Only item-level allows (with a why-comment) may carve an exception.
// The `cfg_attr` wrapper and a manifest `[lints.clippy]` / `[workspace.lints.clippy]` entry set to
// allow or expect do the same from outside the source tree.
const CRATE_ALLOW = /#!\[[^\]]*?\b(?:allow|expect)\s*\([^\]]*?clippy::disallowed_(?:methods|types)/;
const MANIFEST_ALLOW = /^\s*disallowed_(?:methods|types)\s*=\s*(?:"(?:allow|expect)"|\{[^}]*\blevel\s*=\s*"(?:allow|expect)")/m;

function checkCrateAllows() {
  const errors = [];
  let count = 0;
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const p = path.join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (name.endsWith('.rs')) {
        count++;
        if (CRATE_ALLOW.test(read(p))) {
          errors.push(`crate-allow: ${rel(p)} carries a crate-level allow/expect of clippy::disallowed_methods or disallowed_types — move it onto the one item, with a why-comment`);
        }
      }
    }
  };
  const checkManifest = (p) => {
    count++;
    if (MANIFEST_ALLOW.test(read(p))) {
      errors.push(`crate-allow: ${rel(p)} sets clippy disallowed_methods or disallowed_types to allow/expect in [lints] � remove it; carve an exception on the one item, with a why-comment`);
    }
  };
  checkManifest(path.join(REPO_ROOT, 'Cargo.toml'));
  for (const [dir, tier] of Object.entries(CRATES)) {
    if (tier === 'permissive') continue;
    if (existsSync(path.join(REPO_ROOT, dir, 'Cargo.toml'))) checkManifest(path.join(REPO_ROOT, dir, 'Cargo.toml'));
    for (const sub of ['src', 'tests']) {
      const d = path.join(REPO_ROOT, dir, sub);
      if (existsSync(d)) walk(d);
    }
  }
  return { errors, count };
}

const workspace = scanWorkspace();
const nextest = checkNextest(workspace);
const vigilPresent = existsSync(VIGIL_MANIFEST);
if (vigilPresent) {
  if (existsSync(VIGIL_NEXTEST)) nextest.errors.push(...checkNextest(workspace, VIGIL_NEXTEST).errors);
  else nextest.errors.push(`nextest: ${rel(VIGIL_NEXTEST)} is missing — vigil's own workspace needs its gpu/no-gpu config`);
  nextest.errors.push(...checkVigilProfiles());
}
const header = checkHeader();
const clippy = checkClippyLists(REPO_ROOT, workspaceDirs(REPO_ROOT));
const crateGraph = checkCrateGraph(REPO_ROOT);
const manifests = checkManifests(REPO_ROOT, workspaceDirs(REPO_ROOT));
const docCitations = checkDocCitations(REPO_ROOT);
const requireGpu = checkRequireGpu();
const bless = checkBless();
const crateAllow = checkCrateAllows();
const proptest = checkProptest();
const errors = [...workspace.orphans, ...nextest.errors, ...requireGpu.errors, ...bless.errors, ...crateAllow.errors, ...proptest.errors, ...header.errors, ...clippy.errors, ...crateGraph.errors, ...manifests.errors, ...docCitations.errors];

if (errors.length === 0) {
  console.log(`lists-in-step: OK (nextest ${nextest.count} tests scanned, c-header ${header.count} items, clippy ${clippy.count} files, crate-graph ${crateGraph.count} packages, manifests ${manifests.count} files, doc-citations ${docCitations.count} citations, require-gpu ${requireGpu.count} files, bless ${bless.count} files, crate-allow ${crateAllow.count} files, proptest ${proptest.count} blocks)`);
  process.exit(0);
}
console.error(`lists-in-step: ${errors.length} list(s) out of step`);
for (const e of errors) console.error(`  - ${e}`);
process.exit(1);
