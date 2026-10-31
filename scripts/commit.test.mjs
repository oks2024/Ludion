#!/usr/bin/env node
// Tests for scripts/commit.mjs against scratch git repositories and a fake `lore`.
//
// Every scenario builds its own world under the OS temp dir: a bare repository standing in for
// the public remote, workspaces holding copies of commit.mjs + privacy-canary.mjs (no token list,
// so the canary skips), and a Lore shim (LUDION_COMMIT_LORE_SHIM) backed by a JSON "server" file
// that emulates exactly the `lore` output shapes commit.mjs parses. Nothing touches this checkout.
//
// Usage:  node --test scripts/commit.test.mjs
//
// Zero npm dependencies — Node built-ins only. Runs unchanged on Windows and macOS.

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { foreignIndexPaths, foreignStaged, ownUnstageTargets, parseStagedCount, parseStagedSet } from './commit-lib.mjs';

const SCRIPTS = path.dirname(fileURLToPath(import.meta.url));
const UNRELATED = 'wip-other-instance.txt';

// The fake Lore: a server JSON file (revision, signature, committed files as base64, a commit log)
// and per-workspace state in `.lore/state.json`.
const SHIM = `
import { createHash, randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const serverFile = process.env.FAKE_LORE_SERVER;
const wsFile = path.join(process.cwd(), '.lore', 'state.json');
const load = (f) => JSON.parse(readFileSync(f, 'utf8'));
const save = (f, v) => writeFileSync(f, JSON.stringify(v, null, 1));
const server = load(serverFile);
const ws = load(wsFile);
const [verb, ...args] = process.argv.slice(2);
const SKIP = new Set(['.git', '.lore', 'scripts']);

function walk(rel, out) {
  const abs = path.join(process.cwd(), rel);
  if (!existsSync(abs)) return;
  if (statSync(abs).isDirectory()) {
    for (const e of readdirSync(abs)) if (!(rel === '' && SKIP.has(e))) walk(rel ? rel + '/' + e : e, out);
  } else out.add(rel);
}

function exit(code, text) { if (text) process.stdout.write(text + '\\n'); save(wsFile, ws); save(serverFile, server); process.exit(code); }

// The files under the named paths (none means the whole tree) whose disk bytes differ from the
// committed ones, with the lore change kind: M modified, A added, D deleted.
function changesUnder(targets) {
  const files = new Set();
  for (const t of targets.length ? targets : ['.']) {
    const rel = t === '.' ? '' : t;
    walk(rel, files);
    for (const known of Object.keys(ws.files)) if (!rel || known === rel || known.startsWith(rel + '/')) files.add(known);
  }
  const changes = [];
  for (const f of [...files].sort()) {
    const abs = path.join(process.cwd(), f);
    const now = existsSync(abs) ? readFileSync(abs).toString('base64') : null;
    if (now === (ws.files[f] ?? null)) continue;
    changes.push({ path: f, now, kind: now === null ? 'D' : f in ws.files ? 'M' : 'A' });
  }
  return changes;
}

if (verb === 'status') {
  let out = 'Repository ' + '0'.repeat(32) + '\\nOn branch main revision ' + ws.rev + ' -> ' + ws.sig + '\\nRemote revision ' + server.rev + ' -> ' + server.sig + '\\n';
  if (ws.pendingMerge) out += 'Pending merge, incoming revision ' + server.rev + ' -> ' + server.sig + '\\n';
  const stagedLines = Object.entries(ws.staged).map(([f, c]) => (c === null ? 'D' : f in ws.files ? 'M' : 'A') + ' ' + f + ' \\n');
  if (stagedLines.length) out += 'Changes staged for commit:\\n' + stagedLines.join('');
  out += 'Local branch in sync with remote\\n';
  const changes = changesUnder(args.filter((a) => a !== '--scan' && a !== '-P'));
  if (changes.length) {
    out += 'Changes not staged for commit:\\n' + changes.map((c) => c.kind + ' ' + c.path + '\\n').join('');
    out += 'Tracked changes: ' + changes.length + ' modified\\n';
  }
  exit(0, out);
}
if (verb === 'stage') {
  const changes = changesUnder(args.filter((a) => a !== '--scan'));
  for (const c of changes) ws.staged[c.path] = c.now;
  const count = (k) => changes.filter((c) => c.kind === k).length;
  exit(0, server.stageSummary ?? 'Staging file system changes\\nStaging ' + changes.length + ' files (' + count('M') + ' modified, ' + count('A') + ' added, ' + count('D') + ' deleted, 0 moved)\\nStaged repository state ' + '0'.repeat(64));
}
if (verb === 'repository') {
  if (server.failDump) exit(1, 'repository dump failed');
  exit(0, Object.keys(server.files).map((f) => f + ' id 1 parent 0 addr 0').join('\\n'));
}
if (verb === 'unstage') {
  (ws.unstageCalls ??= []).push(...args);
  for (const f of Object.keys(ws.staged)) if (args.some((a) => a === '.' || f === a || f.startsWith(a + '/'))) delete ws.staged[f];
  exit(0);
}
if (verb === 'commit') {
  if (server.failCommit) exit(1, 'commit failed: injected');
  if (server.mutateOnCommit) writeFileSync(server.mutateOnCommit.file, server.mutateOnCommit.content);
  if (!Object.keys(ws.staged).length) exit(1, 'Nothing staged for commit');
  const rev = ws.rev + 1;
  const sig = createHash('sha256').update(rev + args[0] + randomBytes(8).toString('hex')).digest('hex');
  for (const [f, c] of Object.entries(ws.staged)) { if (c === null) delete ws.files[f]; else ws.files[f] = c; }
  ws.unpushed.push({ rev, sig, message: args[0], changes: ws.staged });
  ws.staged = {}; ws.rev = rev; ws.sig = sig;
  exit(0, 'Revision : ' + rev + '\\nSignature : ' + sig);
}
if (verb === 'push') {
  if (server.failPush) exit(1, 'push failed: server unreachable');
  if (!ws.unpushed.length) exit(0);
  if (ws.unpushed[0].rev !== server.rev + 1) exit(1, 'push failed: diverged');
  for (const c of ws.unpushed) {
    for (const [f, v] of Object.entries(c.changes)) { if (v === null) delete server.files[f]; else server.files[f] = v; }
    server.rev = c.rev; server.sig = c.sig;
    server.log.push({ rev: c.rev, sig: c.sig, message: c.message, paths: Object.keys(c.changes) });
  }
  ws.unpushed = [];
  exit(0, 'Pushed');
}
if (verb === 'sync') {
  for (const f of new Set([...Object.keys(server.files), ...Object.keys(ws.files)])) {
    const abs = path.join(process.cwd(), f);
    const disk = existsSync(abs) ? readFileSync(abs).toString('base64') : null;
    if (disk !== (ws.files[f] ?? null)) continue; // a local edit is forwarded, not overwritten
    if (f in server.files) { mkdirSync(path.dirname(abs), { recursive: true }); writeFileSync(abs, Buffer.from(server.files[f], 'base64')); }
    else rmSync(abs, { force: true });
  }
  ws.files = { ...server.files }; ws.rev = server.rev; ws.sig = server.sig;
  exit(0, 'Synced to revision ' + server.rev);
}
exit(2, 'fake lore: unknown verb ' + verb);
`;

const gitEnv = () => {
  const env = { ...process.env };
  for (const k of Object.keys(env)) if (k.startsWith('GIT_')) delete env[k];
  return env;
};

function git(cwd, args, input) {
  const r = spawnSync('git', args, { cwd, input, encoding: 'utf8', env: gitEnv() });
  assert.equal(r.status, 0, `git ${args.join(' ')} failed in ${cwd}:\n${r.stdout}${r.stderr}`);
  return r.stdout.trim();
}

const hex = (c) => c.repeat(64);

function configure(dir, name) {
  for (const [k, v] of [['user.name', name], ['user.email', `${name}@example.test`], ['commit.gpgsign', 'false'], ['core.autocrlf', 'false']]) {
    git(dir, ['config', k, v]);
  }
}

// A world: the bare remote seeded with a fresh-root mirror commit, and the Lore server at rev 1.
function makeWorld(t, { remote = true } = {}) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'ludion-commit-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const shim = path.join(root, 'lore-shim.mjs');
  writeFileSync(shim, SHIM);
  const serverFile = path.join(root, 'server.json');
  const seedFiles = { 'README.md': 'Ludion test world\n', '.gitignore': '.lore/\n' };
  const server = { rev: 1, sig: hex('1'), files: {}, log: [], failPush: false };
  for (const [f, c] of Object.entries(seedFiles)) server.files[f] = Buffer.from(c).toString('base64');
  writeFileSync(serverFile, JSON.stringify(server));
  const bare = path.join(root, 'origin.git');
  if (remote) {
    git(root, ['init', '-q', '--bare', '-b', 'main', bare]);
    const seed = path.join(root, 'seed');
    git(root, ['init', '-q', '-b', 'main', seed]);
    configure(seed, 'seed');
    for (const [f, c] of Object.entries(seedFiles)) writeFileSync(path.join(seed, f), c);
    git(seed, ['add', '-A']);
    git(seed, ['commit', '-q', '-m', `Initial public import\n\n[lore 1 | ${hex('1')}]`]);
    git(seed, ['push', '-q', bare, 'HEAD:main']);
  }
  const world = {
    root,
    bare,
    server: () => JSON.parse(readFileSync(serverFile, 'utf8')),
    setServer: (patch) => writeFileSync(serverFile, JSON.stringify({ ...world.server(), ...patch })),
    workspace(name, { init } = {}) {
      const dir = path.join(root, name);
      if (init) {
        git(root, ['init', '-q', '-b', init, dir]);
        for (const [f, c] of Object.entries(seedFiles)) writeFileSync(path.join(dir, f), c);
      } else {
        git(root, ['clone', '-q', '-c', 'core.autocrlf=false', bare, dir]);
      }
      configure(dir, name);
      mkdirSync(path.join(dir, 'scripts'));
      for (const s of ['commit.mjs', 'commit-lib.mjs', 'mirror-lib.mjs', 'privacy-canary.mjs', 'atomic-write.mjs']) copyFileSync(path.join(SCRIPTS, s), path.join(dir, 'scripts', s));
      mkdirSync(path.join(dir, '.lore'));
      const s = world.server();
      writeFileSync(path.join(dir, '.lore', 'state.json'), JSON.stringify({ rev: s.rev, sig: s.sig, files: s.files, staged: {}, unpushed: [] }));
      return {
        dir,
        write: (f, c) => {
          mkdirSync(path.dirname(path.join(dir, f)), { recursive: true });
          writeFileSync(path.join(dir, f), c);
        },
        read: (f) => readFileSync(path.join(dir, f)),
        lore: () => JSON.parse(readFileSync(path.join(dir, '.lore', 'state.json'), 'utf8')),
        setLore: (patch) => {
          const file = path.join(dir, '.lore', 'state.json');
          writeFileSync(file, JSON.stringify({ ...JSON.parse(readFileSync(file, 'utf8')), ...patch }));
        },
        git: (...args) => git(dir, args),
        script: (name, ...args) => {
          copyFileSync(path.join(SCRIPTS, name), path.join(dir, 'scripts', name));
          const r = spawnSync(process.execPath, [path.join(dir, 'scripts', name), ...args], {
            cwd: dir,
            encoding: 'utf8',
            env: { ...gitEnv(), LUDION_COMMIT_LORE_SHIM: shim, FAKE_LORE_SERVER: serverFile },
          });
          return { code: r.status, out: `${r.stdout}${r.stderr}` };
        },
        commit(...args) {
          const r = spawnSync(process.execPath, [path.join(dir, 'scripts', 'commit.mjs'), ...args], {
            cwd: dir,
            encoding: 'utf8',
            env: { ...gitEnv(), LUDION_COMMIT_LORE_SHIM: shim, FAKE_LORE_SERVER: serverFile },
          });
          return { code: r.status, out: `${r.stdout}${r.stderr}` };
        },
      };
    },
    remote: (...args) => git(bare, args),
  };
  return world;
}

const expectCode = (r, code) => assert.equal(r.code, code, `expected exit ${code}, got ${r.code}:\n${r.out}`);

// origin/main is one chain with no merges, and its tip is a mirror commit. `contributions` names
// the trailer-less commits allowed in it; every other commit carries a trailer.
function assertLinearMirror(world, contributions = []) {
  const lines = world.remote('log', '--format=%H %P', 'main').split('\n');
  for (let i = 0; i < lines.length; i++) {
    const [, ...parents] = lines[i].split(' ');
    assert.equal(parents.length, i === lines.length - 1 ? 0 : 1, `merge or extra root in origin/main:\n${lines.join('\n')}`);
    if (parents.length) assert.equal(parents[0], lines[i + 1].split(' ')[0]);
  }
  for (const line of lines) {
    const sha = line.split(' ')[0];
    if (contributions.includes(sha)) continue;
    assert.match(world.remote('log', '-1', '--format=%B', sha), /\[lore \d+ \| [0-9a-f]{64}\]/, `mirror commit ${sha} without a trailer`);
  }
  return lines.length;
}

const remoteFile = (world, f) => world.remote('show', `main:${f}`);

function assertNeverPublished(world, file) {
  const everyPath = world.remote('log', '--all', '--name-only', '--format=');
  assert.ok(!everyPath.split('\n').includes(file), `${file} reached the remote`);
}

// A third clone merges a PR on the remote without a trailer.
function pushContribution(world, file, content, subject = `Merge pull request #1: add ${file}`) {
  const dir = path.join(world.root, `contributor-${file}`);
  git(world.root, ['clone', '-q', '-c', 'core.autocrlf=false', world.bare, dir]);
  configure(dir, 'contributor');
  writeFileSync(path.join(dir, file), content);
  git(dir, ['add', file]);
  git(dir, ['commit', '-q', '-m', subject]);
  git(dir, ['push', '-q', 'origin', 'HEAD:main']);
  return git(dir, ['rev-parse', 'HEAD']);
}

describe('commit.mjs', { concurrency: true }, () => {
  test('1. without a remote, master mirrors locally; any other branch is refused', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.write('a.txt', 'one\n');
    expectCode(a.commit('add a', 'a.txt'), 0);
    assert.match(a.git('log', '-1', '--format=%B'), /^add a\n[\s\S]*\[lore 2 \| [0-9a-f]{64}\]$/);
    assert.deepEqual(a.git('ls-tree', '--name-only', 'HEAD').split('\n'), ['a.txt']);
    assert.equal(a.git('remote'), '');

    a.git('checkout', '-q', '-b', 'feature');
    a.write('b.txt', 'two\n');
    expectCode(a.commit('add b', 'b.txt'), 2);
    a.git('checkout', '-q', 'master');
    a.git('remote', 'add', 'origin', path.join(w.root, 'nowhere.git'));
    expectCode(a.commit('add b', 'b.txt'), 2); // master is the remote-less lifeboat only
    assert.equal(w.server().rev, 2);
  });

  test('1b. a remote under another name is named, not silently ignored', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'main' });
    a.git('remote', 'add', 'public', path.join(w.root, 'nowhere.git'));
    a.write('a.txt', 'one\n');
    const r = a.commit('add a', 'a.txt');
    expectCode(r, 0);
    assert.match(r.out, /remote\(s\) public but no 'origin'/);
  });

  test('2. two workspaces alternating keep origin/main linear', (t) => {
    const w = makeWorld(t);
    const a = w.workspace('a');
    const b = w.workspace('b');
    a.write(UNRELATED, 'half-finished\n');
    for (let round = 1; round <= 3; round++) {
      a.write('a.txt', `A${round}\n`);
      expectCode(a.commit(`a round ${round}`, 'a.txt'), 0);
      b.write('b.txt', `B${round}\n`);
      expectCode(b.commit(`b round ${round}`, 'b.txt'), 0);
    }
    assert.equal(assertLinearMirror(w), 7);
    assert.equal(remoteFile(w, 'a.txt'), 'A3');
    assert.equal(remoteFile(w, 'b.txt'), 'B3');
    assert.equal(remoteFile(w, 'README.md'), 'Ludion test world');
    assertNeverPublished(w, UNRELATED);
  });

  test('3. an offline mirror commit is carried by the next run', (t) => {
    const w = makeWorld(t);
    const a = w.workspace('a');
    const b = w.workspace('b');
    a.write(UNRELATED, 'half-finished\n');
    b.git('remote', 'set-url', 'origin', path.join(w.root, 'missing.git'));
    b.write('c.txt', 'C\n');
    const offline = b.commit('add c', 'c.txt');
    expectCode(offline, 1);
    assert.match(offline.out, /unreachable/);
    assert.match(offline.out, /git said:\n(commit: {3}.+\n)*commit: {3}.*missing\.git/);
    assert.equal(w.server().log.at(-1).message, 'add c');

    a.write('d.txt', 'D\n');
    expectCode(a.commit('add d', 'd.txt'), 0);
    const aTip = w.remote('rev-parse', 'main');
    assert.throws(() => remoteFile(w, 'c.txt'));

    b.git('remote', 'set-url', 'origin', w.bare);
    b.write('e.txt', 'E\n');
    expectCode(b.commit('add e', 'e.txt'), 0);
    assert.equal(w.remote('rev-parse', 'main~1'), aTip);
    const [c, d, e] = ['c.txt', 'd.txt', 'e.txt'].map((f) => remoteFile(w, f));
    assert.deepEqual([c, d, e], ['C', 'D', 'E']);
    const carried = w.server().log.find((l) => l.message === 'add c');
    assert.ok(w.remote('log', '-1', '--format=%B', 'main').includes(`Also carries: [lore ${carried.rev} | ${carried.sig}]`));
    assertLinearMirror(w);
    assertNeverPublished(w, UNRELATED);
  });

  test('4. a contribution on the remote refuses a normal commit before Lore stages', (t) => {
    const w = makeWorld(t);
    const a = w.workspace('a');
    a.write(UNRELATED, 'half-finished\n');
    pushContribution(w, 'f.txt', 'F\n');
    a.write('x.txt', 'X\n');
    const r = a.commit('add x', 'x.txt');
    expectCode(r, 2);
    assert.match(r.out, /--import/);
    assert.equal(w.server().rev, 1);
    assert.deepEqual(JSON.parse(readFileSync(path.join(a.dir, '.lore', 'state.json'), 'utf8')).staged, {});
    assertNeverPublished(w, UNRELATED);
  });

  test('5. --import round-trips a contribution into Lore and marks it', (t) => {
    const w = makeWorld(t);
    const a = w.workspace('a');
    const b = w.workspace('b');
    a.write(UNRELATED, 'half-finished\n');
    const bytes = Buffer.from([0x68, 0x69, 0x0d, 0x0a, 0xff, 0x00, 0x0a]);
    const sha = pushContribution(w, 'f.txt', bytes);
    expectCode(a.commit('add x', 'x.txt'), 2);

    expectCode(a.commit('--import'), 0);
    assert.ok(a.read('f.txt').equals(bytes));
    const imported = w.server().log.at(-1);
    assert.deepEqual(imported.paths, ['f.txt']);
    const lines = imported.message.split('\n');
    assert.equal(lines[0], 'Merge pull request #1: add f.txt');
    assert.ok(lines.includes('Co-Authored-By: contributor <contributor@example.test>'));
    assert.equal(lines.at(-1), `[git ${sha}]`);
    assert.equal(w.remote('rev-parse', 'main~1'), sha);
    assert.equal(w.remote('rev-parse', 'main^{tree}'), w.remote('rev-parse', 'main~1^{tree}'));
    assert.ok(w.remote('log', '-1', '--format=%B', 'main').includes(`[lore ${imported.rev} | ${imported.sig}]`));

    b.write('g.txt', 'G\n');
    expectCode(b.commit('add g', 'g.txt'), 0);
    assert.ok(b.read('f.txt').equals(bytes));
    assert.equal(remoteFile(w, 'g.txt'), 'G');
    assert.equal(w.remote('rev-parse', 'main~2'), sha);
    assertLinearMirror(w, [sha]);
    assertNeverPublished(w, UNRELATED);
  });

  test('6. --import refuses a touched path that differs from the last mirror commit', (t) => {
    const w = makeWorld(t);
    const a = w.workspace('a');
    pushContribution(w, 'h.txt', 'contributed\n');
    a.write('h.txt', 'local edit\n');
    const r = a.commit('--import');
    expectCode(r, 2);
    assert.match(r.out, /h\.txt/);
    assert.equal(a.read('h.txt').toString(), 'local edit\n');
    assert.equal(w.server().rev, 1);
    assert.equal(w.server().log.length, 0);
  });

  test('7. another instance\'s uncommitted file never reaches the remote', (t) => {
    const w = makeWorld(t);
    const a = w.workspace('a');
    const b = w.workspace('b');
    for (const ws of [a, b]) ws.write(UNRELATED, 'half-finished\n');
    a.write('a.txt', 'A\n');
    expectCode(a.commit('add a', 'a.txt'), 0);
    b.git('remote', 'set-url', 'origin', path.join(w.root, 'missing.git'));
    b.write('b.txt', 'B\n');
    expectCode(b.commit('add b', 'b.txt'), 1);
    b.git('remote', 'set-url', 'origin', w.bare);
    const sha = pushContribution(w, 'f.txt', 'F\n');
    expectCode(a.commit('--import'), 0);
    b.write('c.txt', 'C\n');
    expectCode(b.commit('add c', 'c.txt'), 0);
    a.write('a.txt', 'A\n');
    a.write('d.txt', 'D\n');
    expectCode(a.commit('add d', 'd.txt'), 0);
    assertLinearMirror(w, [sha]);
    assertNeverPublished(w, UNRELATED);
    assert.deepEqual(['a.txt', 'b.txt', 'c.txt', 'f.txt'].map((f) => remoteFile(w, f)), ['A', 'B', 'C', 'F']);
  });

  test('8. --import carries a mirror commit held while a contribution was pending', (t) => {
    const w = makeWorld(t);
    const b = w.workspace('b');
    b.git('remote', 'set-url', 'origin', path.join(w.root, 'missing.git'));
    b.write('b.txt', 'B\n');
    expectCode(b.commit('add b', 'b.txt'), 1);
    b.git('remote', 'set-url', 'origin', w.bare);
    const sha = pushContribution(w, 'f.txt', 'F\n');
    expectCode(b.commit('add c', 'c.txt'), 2);
    expectCode(b.commit('--import'), 0);
    assert.equal(w.remote('rev-parse', 'main~1'), sha);
    assert.equal(remoteFile(w, 'b.txt'), 'B');
    const held = w.server().log.find((l) => l.message === 'add b');
    assert.ok(w.remote('log', '-1', '--format=%B', 'main').includes(`Also carries: [lore ${held.rev} | ${held.sig}]`));
    assertLinearMirror(w, [sha]);
  });

  test('9. a deleted directory named by its path leaves the mirror too', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.write('spike/src/main.rs', 'fn main() {}\n');
    a.write('spike/Cargo.toml', '[package]\n');
    a.write('keep.txt', 'kept\n');
    expectCode(a.commit('add spike', 'spike', 'keep.txt'), 0);
    rmSync(path.join(a.dir, 'spike'), { recursive: true });
    expectCode(a.commit('delete spike', 'spike'), 0);
    assert.deepEqual(a.git('ls-files').split('\n'), ['keep.txt']);
    assert.equal(a.git('status', '--porcelain', '--', 'spike'), '');
  });

  test('10. only --mirror-only untracks a private path (gitignore minus loreignore)', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.write('secret/notes.md', 'private\n');
    expectCode(a.commit('add secret', 'secret'), 0);
    assert.ok(a.git('ls-files').split('\n').includes('secret/notes.md'));
    a.write('.gitignore', '.lore/\n/secret/\n');
    expectCode(a.commit('declare secret private', '.gitignore'), 0);
    assert.ok(a.git('ls-files').split('\n').includes('secret/notes.md'));
    a.write('z.txt', 'Z\n');
    expectCode(a.commit('add z', 'z.txt'), 0);
    a.git('reset', '-q', '--mixed', 'HEAD~1');
    expectCode(a.commit('--mirror-only', 'catch z up', 'z.txt'), 0);
    assert.ok(!a.git('ls-files').split('\n').includes('secret/notes.md'));
    assert.ok(existsSync(path.join(a.dir, 'secret', 'notes.md')));
    assert.ok(w.server().files['secret/notes.md']);
  });

  test('11. --mirror-only refuses no paths, --all and private paths', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.write('.gitignore', '.lore/\n/secret/\n');
    a.write('secret/notes.md', 'private\n');
    expectCode(a.commit('--mirror-only'), 2);
    expectCode(a.commit('--mirror-only', 'm'), 2);
    expectCode(a.commit('--mirror-only', 'm', '--all', 'x.txt'), 2);
    const priv = a.commit('--mirror-only', 'm', 'secret/notes.md');
    expectCode(priv, 2);
    assert.match(priv.out, /private/);
    assert.equal(w.server().rev, 1);
  });

  test('12. --mirror-only writes the mirror commit alone, at the current Lore revision', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.write('y.txt', 'Y\n');
    expectCode(a.commit('add y', 'y.txt'), 0);
    a.write('x.txt', 'X\n');
    expectCode(a.commit('add x', 'x.txt'), 0);
    a.git('reset', '-q', '--mixed', 'HEAD~1');
    assert.ok(!a.git('ls-files').split('\n').includes('x.txt'));
    expectCode(a.commit('--mirror-only', 'catch x up', 'x.txt'), 0);
    assert.ok(a.git('ls-files').split('\n').includes('x.txt'));
    assert.match(a.git('log', '-1', '--format=%B'), /^catch x up\n[\s\S]*\[lore 3 \| [0-9a-f]{64}\]$/);
    assert.equal(w.server().rev, 3);
  });

  test('13. a pending merge refuses a commit and a --mirror-only', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.setLore({ pendingMerge: true });
    a.write('x.txt', 'X\n');
    const commit = a.commit('add x', 'x.txt');
    expectCode(commit, 2);
    assert.match(commit.out, /merge is pending/);
    expectCode(a.commit('--mirror-only', 'm', 'README.md'), 2);
    assert.equal(w.server().rev, 1);
    assert.deepEqual(a.lore().staged, {});
  });

  test('14. --mirror-only refuses a path Lore reports as changed', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.git('add', '-A');
    a.git('commit', '-q', '-m', 'seed');
    a.write('README.md', 'edited\n');
    const r = a.commit('--mirror-only', 'm', 'README.md');
    expectCode(r, 2);
    assert.match(r.out, /reports as changed/);
    assert.match(r.out, /M README\.md/);
    assert.equal(a.git('rev-list', '--count', 'HEAD'), '1');
  });

  test('14b. --mirror-only of the whole tree (`.`) still refuses a Lore-changed path', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.git('add', '-A');
    a.git('commit', '-q', '-m', 'seed');
    a.write('README.md', 'edited\n');
    const r = a.commit('--mirror-only', 'm', '.');
    expectCode(r, 2);
    assert.match(r.out, /reports as changed/);
  });

  test('15. over 500 staged files is refused and unstaged', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    for (let i = 0; i < 501; i++) a.write(`bulk/f${i}.txt`, `${i}\n`);
    const r = a.commit('add bulk', 'bulk');
    expectCode(r, 2);
    assert.match(r.out, /over the 500-file guard/);
    assert.equal(w.server().rev, 1);
    assert.deepEqual(a.lore().staged, {});
    assert.ok(a.lore().unstageCalls.includes('bulk'));
    assert.ok(!a.lore().unstageCalls.includes('.'));
  });

  test('16. a failed local mirror commit exits 1 and names the repair', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    const hook = path.join(a.dir, '.git', 'hooks', 'pre-commit');
    mkdirSync(path.dirname(hook), { recursive: true });
    writeFileSync(hook, '#!/bin/sh\nexit 1\n');
    chmodSync(hook, 0o755);
    a.write('x.txt', 'X\n');
    const r = a.commit('add x', 'x.txt');
    expectCode(r, 1);
    assert.match(r.out, /--mirror-only "<message>" "x\.txt"/);
    assert.equal(w.server().rev, 2);
  });

  test('17. mirror-gap fails when lore fails', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.git('add', '-A');
    a.git('commit', '-q', '-m', 'seed');
    w.setServer({ failDump: true });
    const r = a.script('mirror-gap.mjs');
    expectCode(r, 2);
    assert.match(r.out, /failed/);
  });

  test('18. a failed mirror build against a remote names the repair, not a held local commit', (t) => {
    const w = makeWorld(t);
    const a = w.workspace('a');
    const hook = path.join(a.dir, '.git', 'hooks', 'pre-commit');
    mkdirSync(path.dirname(hook), { recursive: true });
    writeFileSync(hook, '#!/bin/sh\nexit 1\n');
    chmodSync(hook, 0o755);
    a.write('x.txt', 'X\n');
    const r = a.commit('add x', 'x.txt');
    expectCode(r, 1);
    assert.match(r.out, /--mirror-only "<message>" "x\.txt"/);
    assert.doesNotMatch(r.out, /collapses/);
    assert.doesNotMatch(r.out, /held locally/);
  });

  // A world where `ci/private` is tracked by git but declared private: a normal commit must never
  // publish its edits, whether it names the file, a parent directory, `.` or `--all`.
  function trackedPrivateWorld(t) {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.write('ci/scripts/a.sh', 'one\n');
    a.write('ci/private/n.md', 'orig\n');
    expectCode(a.commit('add ci', 'ci'), 0);
    a.write('.gitignore', '.lore/\n/ci/private/\n');
    expectCode(a.commit('declare private', '.gitignore'), 0);
    assert.ok(a.git('ls-files').split('\n').includes('ci/private/n.md'));
    return { w, a };
  }
  const loreHolds = (w, f, text) => Buffer.from(w.server().files[f], 'base64').toString() === text;

  test('19. a named parent path or `.` leaves tracked private content out of the mirror', (t) => {
    const { w, a } = trackedPrivateWorld(t);
    a.write('ci/scripts/a.sh', 'two\n');
    a.write('ci/private/n.md', 'edit1\n');
    const r = a.commit('edit', 'ci');
    expectCode(r, 0);
    assert.match(r.out, /mirror leaves out private content under: ci\/private/);
    assert.equal(a.git('show', 'HEAD:ci/private/n.md'), 'orig');
    assert.equal(a.git('show', 'HEAD:ci/scripts/a.sh'), 'two');
    assert.ok(loreHolds(w, 'ci/private/n.md', 'edit1\n'));
    assert.ok(a.git('ls-files').split('\n').includes('ci/private/n.md'));
    a.write('ci/private/n.md', 'edit2\n');
    expectCode(a.commit('edit all', '.'), 0);
    assert.equal(a.git('show', 'HEAD:ci/private/n.md'), 'orig');
    assert.ok(loreHolds(w, 'ci/private/n.md', 'edit2\n'));
  });

  test('20. --all leaves tracked private content out of the mirror and still mirrors the rest', (t) => {
    const { w, a } = trackedPrivateWorld(t);
    a.write('ci/private/n.md', 'edit1\n');
    a.write('z.txt', 'Z\n');
    const r = a.commit('sweep', '--all');
    expectCode(r, 0);
    assert.match(r.out, /mirror leaves out private content under: ci\/private/);
    assert.equal(a.git('show', 'HEAD:z.txt'), 'Z');
    assert.equal(a.git('show', 'HEAD:ci/private/n.md'), 'orig');
    assert.ok(a.git('ls-files').split('\n').includes('ci/private/n.md'));
    assert.ok(loreHolds(w, 'ci/private/n.md', 'edit1\n'));
  });

  test('21. a private checkout with no token list stops the commit with the canary\'s own exit code', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.write('ci/teamcity/README.md', 'private\n');
    a.write('x.txt', 'X\n');
    const r = a.commit('add x', 'x.txt');
    expectCode(r, 3);
    assert.match(r.out, /token list/);
    assert.match(r.out, /could not run/);
    assert.doesNotMatch(r.out, /Reword/);
    assert.deepEqual(a.lore().staged, {});
    assert.equal(w.server().rev, 1);
  });
  test('22. staged entries this commit did not make are refused, and left staged', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    a.write(UNRELATED, 'half-finished\n');
    a.setLore({ staged: { [UNRELATED]: Buffer.from('half-finished\n').toString('base64') } });
    a.write('x.txt', 'X\n');
    const r = a.commit('add x', 'x.txt');
    expectCode(r, 2);
    assert.match(r.out, /did not make/);
    assert.match(r.out, new RegExp(UNRELATED));
    assert.equal(w.server().rev, 1);
    assert.deepEqual(Object.keys(a.lore().staged), [UNRELATED]);
  });

  test('23. a failed lore commit unstages only its own entries', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    w.setServer({ failCommit: true });
    a.write('x.txt', 'X\n');
    const r = a.commit('add x', 'x.txt');
    assert.notEqual(r.code, 0);
    assert.equal(w.server().rev, 1);
    assert.deepEqual(a.lore().staged, {});
    assert.deepEqual(a.lore().unstageCalls, ['x.txt']);
  });

  test('24. a stage summary it cannot read fails the bulk guard closed', (t) => {
    const w = makeWorld(t, { remote: false });
    const a = w.workspace('a', { init: 'master' });
    w.setServer({ stageSummary: 'Staged everything, no count' });
    a.write('x.txt', 'X\n');
    const r = a.commit('add x', 'x.txt');
    expectCode(r, 2);
    assert.match(r.out, /could not read how many files/);
    assert.equal(w.server().rev, 1);
    assert.deepEqual(a.lore().staged, {});
  });

  for (const [label, opts] of [['no tip', { remote: false, init: 'master' }], ['a remote', { remote: true }]]) {
    test(`25. git staging the mirror did not make refuses the mirror commit untouched (${label})`, (t) => {
      const w = makeWorld(t, { remote: opts.remote });
      const a = w.workspace('a', opts.init ? { init: opts.init } : {});
      a.write('wip-git.txt', 'wip\n');
      a.git('add', 'wip-git.txt');
      a.write('x.txt', 'X\n');
      const r = a.commit('add x', 'x.txt');
      expectCode(r, 1);
      assert.match(r.out, /git index holds staging this mirror commit did not make/);
      assert.equal(w.server().rev, 2);
      assert.ok(a.git('diff', '--cached', '--name-only').split('\n').includes('wip-git.txt'));
    });
  }

  test('26. a commit naming only a tracked private path writes Lore but no mirror commit', (t) => {
    const { w, a } = trackedPrivateWorld(t);
    const head = a.git('rev-parse', 'HEAD');
    a.write('ci/private/n.md', 'edit1\n');
    const r = a.commit('edit private', 'ci/private/n.md');
    expectCode(r, 0);
    assert.match(r.out, /every path is private/);
    assert.equal(a.git('rev-parse', 'HEAD'), head);
    assert.ok(loreHolds(w, 'ci/private/n.md', 'edit1\n'));
  });

  describe('the message scan follows the mirror', () => {
    const TOKEN = 'zzsecretzz';
    function tokenWorld(t) {
      const { w, a } = trackedPrivateWorld(t);
      a.write('ci/teamcity/privacy-tokens.txt', `${TOKEN}\n`);
      return { w, a };
    }

    test('27. a message naming a token still commits when every path is private', (t) => {
      const { w, a } = tokenWorld(t);
      a.write('ci/private/n.md', 'edit1\n');
      const r = a.commit(`edit ${TOKEN} notes`, 'ci/private/n.md');
      expectCode(r, 0);
      assert.ok(loreHolds(w, 'ci/private/n.md', 'edit1\n'));
    });

    test('28. a message naming a token is refused when any path is public', (t) => {
      const { w, a } = tokenWorld(t);
      const rev = w.server().rev;
      a.write('ci/private/n.md', 'edit1\n');
      a.write('x.txt', 'X\n');
      const r = a.commit(`edit ${TOKEN} notes`, 'ci/private/n.md', 'x.txt');
      expectCode(r, 1);
      assert.match(r.out, /private token/);
      assert.equal(w.server().rev, rev);
    });

    test('28b. file contents holding a token block the mirror commit of a normal commit; Lore keeps them', (t) => {
      const { w, a } = tokenWorld(t);
      const head = a.git('rev-parse', 'HEAD');
      a.write('x.txt', `leak ${TOKEN}\n`);
      const r = a.commit('add x', 'x.txt');
      expectCode(r, 1);
      assert.match(r.out, /private token/);
      assert.match(r.out, /Lore is committed but the git mirror is NOT/);
      assert.ok(loreHolds(w, 'x.txt', `leak ${TOKEN}\n`));
      assert.equal(a.git('rev-parse', 'HEAD'), head);
    });

    test('28c. file contents holding a token block the --all-style catch-up mirror', (t) => {
      const { a } = tokenWorld(t);
      const head = a.git('rev-parse', 'HEAD');
      const text = `leak ${TOKEN}\n`;
      a.write('x.txt', text);
      a.setLore({ files: { ...a.lore().files, 'x.txt': Buffer.from(text).toString('base64') } });
      const r = a.commit('catch up', 'x.txt');
      expectCode(r, 1);
      assert.match(r.out, /private token/);
      assert.equal(a.git('rev-parse', 'HEAD'), head);
    });

    test('28d. a file changed between Lore staging and the mirror commit is refused', (t) => {
      const { w, a } = tokenWorld(t);
      const head = a.git('rev-parse', 'HEAD');
      a.write('x.txt', 'X\n');
      w.setServer({ mutateOnCommit: { file: path.join(a.dir, 'x.txt'), content: 'changed\n' } });
      const r = a.commit('add x', 'x.txt');
      expectCode(r, 1);
      assert.match(r.out, /changed between Lore staging/);
      assert.equal(a.git('rev-parse', 'HEAD'), head);
    });
  });
});

describe('commit-lib', () => {
  // Verbatim `lore status` output from Lore 0.8.6: staged lines carry a trailing space.
  const STATUS = [
    `On branch main revision 1 -> ${'a'.repeat(64)}`,
    'Changes staged for commit:',
    'M a.txt ',
    'A n/ ',
    'A n/q.txt ',
    'Changes not staged for commit:',
    'M b.txt',
    'Untracked files:',
    '',
  ].join('\n');

  test('parseStagedSet reads only the staged block, trimming the trailing space', () => {
    assert.deepEqual(parseStagedSet(STATUS), [
      { kind: 'M', path: 'a.txt' },
      { kind: 'A', path: 'n/' },
      { kind: 'A', path: 'n/q.txt' },
    ]);
    assert.deepEqual(parseStagedSet(['On branch main', 'Changes not staged for commit:', 'M b.txt', ''].join('\n')), []);
  });

  // `lore stage move` lines (Lore 0.8.6): kind `V`, `old -> new`, trailing space.
  const MOVES = ['Changes staged for commit:', 'V a.txt -> c.txt ', 'V d/ -> dd/ ', ''].join('\n');

  test('a move line parses to its two ends and is owned only when both are named', () => {
    const staged = parseStagedSet(MOVES);
    assert.deepEqual(staged, [
      { kind: 'V', path: 'c.txt', from: 'a.txt' },
      { kind: 'V', path: 'dd/', from: 'd/' },
    ]);
    assert.deepEqual(foreignStaged(staged, ['a.txt', 'c.txt', 'd', 'dd']), []);
    assert.deepEqual(foreignStaged(staged, ['c.txt', 'd', 'dd']).map((e) => e.from), ['a.txt']);
    assert.equal(foreignStaged(staged, []).length, 2);
    // `lore unstage <new>` alone drops the whole `V` line (scratch-repo check), so no `from` target.
    assert.deepEqual(ownUnstageTargets(staged, ['c.txt']), ['c.txt']);
  });

  test('parseStagedCount reads the three real shapes and refuses the rest', () => {
    assert.equal(parseStagedCount('Staging 3 files (1 modified, 2 added)'), 3);
    assert.equal(parseStagedCount('Staging 1 directories (1 added),  2 files (2 added)'), 2);
    assert.equal(parseStagedCount('No changes staged'), 0);
    assert.equal(parseStagedCount(''), null);
    assert.equal(parseStagedCount('Staging nothing'), null);
  });

  test('foreignStaged owns the named paths and the directories above them', () => {
    const staged = parseStagedSet(STATUS);
    assert.deepEqual(foreignStaged(staged, ['n/q.txt']).map((e) => e.path), ['a.txt']);
    assert.deepEqual(foreignStaged(staged, ['a.txt', 'n']).map((e) => e.path), []);
    assert.deepEqual(foreignStaged(staged, ['.']), []);
    assert.deepEqual(foreignStaged(staged, null), []);
    assert.equal(foreignStaged(staged, []).length, 3);
  });

  test('ownUnstageTargets adds the staged ancestor directories, and --all is the whole tree', () => {
    const staged = parseStagedSet(STATUS);
    assert.deepEqual(ownUnstageTargets(staged, ['n/q.txt']), ['n/q.txt', 'n']);
    assert.deepEqual(ownUnstageTargets(staged, null), ['.']);
  });

  test('foreignIndexPaths skips owned and private paths', () => {
    assert.deepEqual(foreignIndexPaths(['x.txt', 'd/y.txt', 'ci/private/n.md'], ['x.txt'], ['ci/private']), ['d/y.txt']);
    assert.deepEqual(foreignIndexPaths(['d/y.txt'], null, []), []);
    assert.deepEqual(foreignIndexPaths(['d/y.txt'], [], []), ['d/y.txt']);
  });
});
