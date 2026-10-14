// The single-run lock for scripts/check.mjs: two concurrent runs share `.build/` and the workspace,
// and the second would delete the first's logs. Zero dependencies, Node built-ins only.

import { linkSync, mkdirSync, readFileSync, renameSync, statSync, unlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';

// A lock file that does not parse is a holder mid-write or a crash; it is only stale once it has sat this long.
const UNREADABLE_GRACE_MS = 60_000;
let seq = 0;

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return err.code === 'EPERM';
  }
}

function readHolder(lockPath) {
  try {
    return JSON.parse(readFileSync(lockPath, 'utf8'));
  } catch {
    return null;
  }
}

const holderAlive = (holder) => Boolean(holder) && Number.isInteger(holder.pid) && alive(holder.pid);

function ageMs(file) {
  try {
    return Date.now() - statSync(file).mtimeMs;
  } catch {
    return Infinity;
  }
}

const quietUnlink = (file) => {
  try {
    unlinkSync(file);
  } catch { /* already gone */ }
};

// Returns `{ ok: true, release }`, or `{ ok: false, holder }` when a live process holds the lock.
// A lock left by a dead holder is taken over; one that does not parse counts as live while young.
// The record is written to a temp file and published with `linkSync`, so the lock is never
// observable empty; a stale lock is renamed aside and re-read before it is deleted, so a takeover
// cannot remove a lock another run acquired in between.
export function acquireLock(lockPath, info) {
  mkdirSync(path.dirname(lockPath), { recursive: true });
  const unique = `${process.pid}.${seq++}`;
  const temp = `${lockPath}.${unique}.tmp`;
  writeFileSync(temp, JSON.stringify({ pid: process.pid, startedAt: new Date().toISOString(), ...info }));
  try {
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        linkSync(temp, lockPath);
        return {
          ok: true,
          release() {
            if (readHolder(lockPath)?.pid !== process.pid) return;
            quietUnlink(lockPath);
          },
        };
      } catch (err) {
        if (err.code !== 'EEXIST') throw err;
      }
      const holder = readHolder(lockPath);
      if (holderAlive(holder) || (!Number.isInteger(holder?.pid) && ageMs(lockPath) < UNREADABLE_GRACE_MS)) {
        return { ok: false, holder: holder ?? {} };
      }
      const aside = `${lockPath}.stale.${unique}`;
      try {
        renameSync(lockPath, aside);
      } catch (err) {
        if (err.code === 'ENOENT') continue;
        throw err;
      }
      if (holderAlive(readHolder(aside))) {
        const grabbed = readHolder(aside);
        try {
          linkSync(aside, lockPath);
        } catch { /* a third run published its own meanwhile */ }
        quietUnlink(aside);
        return { ok: false, holder: grabbed };
      }
      quietUnlink(aside);
    }
    return { ok: false, holder: readHolder(lockPath) ?? {} };
  } finally {
    quietUnlink(temp);
  }
}
