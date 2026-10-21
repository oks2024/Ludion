// `check.mjs --scope <paths...>`: which gates a change to the named paths needs. A scoped run is the
// coder's inner loop, never a substitute for the default suite before a commit lands — it gives up
// everything a path cannot name: the dependents of a changed crate (unless `--deps`), generated
// files whose generator lives in an engine crate, and the timing baselines of a full run.
//
// Paths are named explicitly, like `commit.mjs`'s: a shared checkout's dirty set mixes several
// instances' work, so `git status` cannot say whose change this run is for.

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';

// Paths no gate reads beyond the cheap node gates that run in every mode.
const PROSE_ROOTS = ['Plan/', 'Plan-Private/', 'dev-docs/', 'licenses/', 'infra/', 'ci/teamcity/', '.exarch/', '.claude/'];
const PROSE_FILE = /^[^/]+\.md$|^LICENSE-/;

// The root workspace's members, read from `cargo metadata` (no build, no build lock), and the root
// crates the detached fuzz workspace depends on by path.
export function loadWorkspace(repoRoot) {
  const meta = JSON.parse(execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1'], { cwd: repoRoot, encoding: 'utf8', maxBuffer: 64 << 20 }));
  const members = meta.packages.map((p) => ({
    name: p.name,
    dir: path.relative(repoRoot, path.dirname(p.manifest_path)).split(path.sep).join('/'),
    lib: p.targets.some((t) => t.kind.some((k) => k === 'lib' || k === 'rlib' || k === 'proc-macro')),
    deps: p.dependencies.filter((d) => d.path).map((d) => d.name),
  }));
  const fuzzToml = readFileSync(path.join(repoRoot, 'fuzz', 'Cargo.toml'), 'utf8');
  const fuzzDeps = [...fuzzToml.matchAll(/^([\w-]+)\s*=\s*\{[^}]*path\s*=\s*"\.\.\/crates\//gm)].map((m) => m[1]);
  return { members, fuzzDeps };
}

// Classifies repo-relative, `/`-separated paths. Returns the selection, or `{ error }` naming a path
// whose reach is the whole workspace (a manifest, the lockfile, toolchain or lint config, vendored
// crates) or that no rule claims — either needs the default suite.
export function classify(paths, { members, fuzzDeps }, { deps = false } = {}) {
  const s = {
    packages: new Set(), vigil: false, vigilNode: false, tauri: false, fuzz: false,
    frontend: false, schema: false, scripts: false, drift: false,
  };
  const byDepth = [...members].sort((a, b) => b.dir.length - a.dir.length);
  for (const raw of paths) {
    const p = raw.replace(/\\/g, '/').replace(/^\.\//, '').replace(/\/$/, '');
    const under = (dir) => p === dir || p.startsWith(dir + '/');
    if (p === '' || p === '.' || p.startsWith('../')) return { error: `${raw}: not a path inside the checkout` };
    if (PROSE_ROOTS.some((r) => under(r.slice(0, -1))) || PROSE_FILE.test(p)) continue;
    if (under('games/vigil')) {
      s.vigil = true;
      if (under('games/vigil/art') || under('games/vigil/scripts')) s.vigilNode = true;
      if (!p.endsWith('.rs')) s.drift = true;
      continue;
    }
    if (under('editor/app/src-tauri')) { s.tauri = true; continue; }
    if (under('editor/app')) { s.frontend = true; continue; }
    if (under('editor/protocol')) { s.schema = true; continue; }
    if (under('fuzz')) { s.fuzz = true; continue; }
    if (under('scripts') || under('ci/scripts') || under('.github')) {
      s.scripts = true;
      if (/drift|import-sync|ludion-bin/.test(p)) s.drift = true;
      continue;
    }
    if (under('docs')) { s.packages.add('ludion-cli'); continue; }
    if (under('examples')) { s.drift = true; continue; }
    const member = byDepth.find((m) => under(m.dir));
    if (member) {
      s.packages.add(member.name);
      if (under('games') && !p.endsWith('.rs')) s.drift = true;
      continue;
    }
    return { error: `${raw}: reaches the whole workspace or no scope rule claims it — run the default suite` };
  }
  if (deps) {
    // Every workspace member that depends, directly or not, on a named one.
    for (let grew = true; grew;) {
      grew = false;
      for (const m of members) {
        if (!s.packages.has(m.name) && m.deps.some((d) => s.packages.has(d))) { s.packages.add(m.name); grew = true; }
      }
    }
    if (s.packages.has('ludion')) s.vigil = true;
    if (fuzzDeps.some((d) => s.packages.has(d))) s.fuzz = true;
  }
  if (s.packages.has('ludion-protocol')) s.schema = true;
  const libs = members.filter((m) => m.lib && s.packages.has(m.name)).map((m) => m.name);
  return {
    ...s,
    packages: [...s.packages].sort(),
    libs: libs.sort(),
    mobile: s.packages.has('ludion') || s.packages.has('ludion-assets'),
  };
}
