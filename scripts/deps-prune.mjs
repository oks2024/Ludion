// Prunes the loose `.o` objects that macOS's unpacked split-debuginfo leaves in `target/<profile>/deps`.
// Cargo never collects them, and past ~100k entries CoreAudio's per-process directory scan slows every
// audio boot (dev-docs/test-box-failures.md). Only a directory past the threshold is touched, and only
// objects older than the floor: a link in flight by another instance reads fresh objects.
//
// Age is the newer of mtime and ctime. Incremental rustc hard-links a reused codegen unit's object into
// `deps` from the incremental cache, and a hard link keeps the old mtime but moves ctime.
//
// Never throws: a file that vanishes or refuses deletion mid-walk is skipped.
import { readdirSync, lstatSync, unlinkSync } from 'node:fs';
import path from 'node:path';

export const MIN_AGE_MS = 60 * 60 * 1000;

function entries(dir) {
  try {
    return readdirSync(dir);
  } catch {
    return null;
  }
}

/** Returns `{ before, removed, after }` entry counts for `dir` (`null` when it cannot be listed). */
export function pruneDeps(dir, { threshold, minAgeMs = MIN_AGE_MS, now = Date.now() }) {
  const names = entries(dir);
  if (!names) return null;
  const before = names.length;
  if (before <= threshold) return { before, removed: 0, after: before };
  let removed = 0;
  for (const name of names) {
    if (!name.endsWith('.o')) continue;
    const file = path.join(dir, name);
    try {
      const st = lstatSync(file);
      if (!st.isFile() || now - Math.max(st.mtimeMs, st.ctimeMs) < minAgeMs) continue;
      unlinkSync(file);
      removed += 1;
    } catch {
      // vanished or undeletable: skipped
    }
  }
  return { before, removed, after: entries(dir)?.length ?? before - removed };
}
