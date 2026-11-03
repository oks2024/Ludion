// Prunes the loose `.o` objects that macOS's unpacked split-debuginfo leaves in `target/<profile>/deps`.
// Cargo never collects them, and every entry slows each process's Metal device lookup by ~2 ms per 1000
// (dev-docs/test-box-failures.md). Only a directory past the threshold is touched.
//
// Each rustc run names its objects with a new suffix, and incremental rustc hard-links a reused codegen
// unit's object under the new name, so many names share one inode and no timestamp tells live from dead.
// The exact record of what a binary reads is its OSO stab entries (`nm -ap`): an object no OSO entry in
// any executable or dylib names is dead, and deleting it costs no backtrace line numbers.
//
// A build in flight would write objects the OSO scan has not seen, so the prune runs holding cargo's own
// lock on the artifact directory (`target/<profile>/.cargo-lock`): no cargo build can run rustc meanwhile.
// Node has no flock, so a perl child holds it and lets go when its stdin closes, even if this process dies.
//
// Never throws: a file that vanishes or refuses deletion mid-walk is skipped.
import { readdirSync, lstatSync, unlinkSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import path from 'node:path';

const OSO = ' OSO ';
const LOCK_SCRIPT = `use Fcntl qw(:flock); open(my $f, '>>', $ARGV[0]) or exit 3; $| = 1; my $end = time + $ARGV[1];
until (flock($f, LOCK_EX | LOCK_NB)) { exit 4 if time >= $end; select(undef, undef, undef, 0.25); }
print "locked\\n"; <STDIN>;`;

function entries(dir) {
  try {
    return readdirSync(dir);
  } catch {
    return null;
  }
}

/** The basename of the loose object an `nm -ap` OSO line names, or `null` (archive member, other line). */
export function parseOsoLine(line) {
  const at = line.indexOf(OSO);
  if (at < 0) return null;
  const target = line.slice(at + OSO.length).trimEnd();
  if (target.includes('(')) return null;
  return path.basename(target) || null;
}

/** Whether `name` is an executable (`<crate>-<16 hex>`) or a dylib: the files whose OSO entries count. */
export function isLinkedOutput(name) {
  return /-[0-9a-f]{16}$/.test(name) || name.endsWith('.dylib');
}

/** The `<crate>-<hash>.` prefix the objects of linked output `name` carry. */
export function ownerPrefix(name) {
  return `${name.replace(/^lib/, '').replace(/\.dylib$/, '')}.`;
}

function scan(file, referenced) {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn('nm', ['-ap', file], { stdio: ['ignore', 'pipe', 'ignore'] });
    } catch {
      resolve(false);
      return;
    }
    let done = false;
    const finish = (ok) => {
      if (!done) {
        done = true;
        resolve(ok);
      }
    };
    createInterface({ input: child.stdout }).on('line', (line) => {
      const object = parseOsoLine(line);
      if (object) referenced.add(object);
    });
    child.on('error', () => finish(false));
    child.on('close', (code) => finish(code === 0));
  });
}

/**
 * Collects the objects named by an OSO entry in any of `names` (files of `dir`). `unreadable` holds the
 * owner prefixes of outputs `nm` could not read: their objects must be kept.
 */
export async function referencedObjects(dir, names, { concurrency = 6 } = {}) {
  const referenced = new Set();
  const unreadable = [];
  let next = 0;
  const worker = async () => {
    while (next < names.length) {
      const name = names[next++];
      if (!(await scan(path.join(dir, name), referenced))) unreadable.push(ownerPrefix(name));
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, names.length) }, worker));
  return { referenced, unreadable };
}

/** Holds an exclusive flock on `lockPath`; resolves to `release()` or `null` when busy past `waitMs` or perl fails. */
export function holdCargoLock(lockPath, waitMs) {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn('/usr/bin/perl', ['-e', LOCK_SCRIPT, lockPath, String(Math.ceil(waitMs / 1000))], { stdio: ['pipe', 'pipe', 'ignore'] });
    } catch {
      resolve(null);
      return;
    }
    let held = false;
    const closed = new Promise((r) => child.on('close', r));
    child.stdin.on('error', () => {});
    child.on('error', () => resolve(null));
    createInterface({ input: child.stdout }).on('line', (line) => {
      if (line !== 'locked' || held) return;
      held = true;
      resolve(async () => {
        child.stdin.end();
        await closed;
      });
    });
    closed.then(() => {
      if (!held) resolve(null);
    });
  });
}

function liveLinkedOutputs(dir, names) {
  return names.filter((name) => {
    if (!isLinkedOutput(name)) return false;
    try {
      return lstatSync(path.join(dir, name)).isFile();
    } catch {
      return false;
    }
  });
}

/**
 * Returns `{ before, removed, after, busy }` entry counts for `dir` (`null` when it cannot be listed).
 * `busy` means cargo's lock stayed held past `lockWaitMs` and nothing was removed.
 */
export async function pruneDeps(dir, { threshold, lockWaitMs = 120000 }) {
  const first = entries(dir);
  if (!first) return null;
  const before = first.length;
  if (before <= threshold) return { before, removed: 0, after: before, busy: false };
  const release = await holdCargoLock(path.join(path.dirname(dir), '.cargo-lock'), lockWaitMs);
  if (!release) return { before, removed: 0, after: before, busy: true };
  let removed = 0;
  try {
    const names = entries(dir) ?? [];
    const { referenced, unreadable } = await referencedObjects(dir, liveLinkedOutputs(dir, names));
    for (const name of names) {
      if (!name.endsWith('.o') || referenced.has(name) || unreadable.some((p) => name.startsWith(p))) continue;
      const file = path.join(dir, name);
      try {
        if (!lstatSync(file).isFile()) continue;
        unlinkSync(file);
        removed += 1;
      } catch {
        // vanished or undeletable: skipped
      }
    }
  } catch {
    // a scan failure leaves the directory as it was
  } finally {
    await release();
  }
  return { before, removed, after: entries(dir)?.length ?? before - removed, busy: false };
}
