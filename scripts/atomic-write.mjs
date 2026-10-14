// The one atomic file write for scripts: bytes land in a hidden temp sibling, then rename over the
// target, so a reader or a kill sees the old file or the new one, never a prefix. The script twin of
// `ludion_core::atomic::write_atomic` (same `.{name}.{pid}.{n}.tmp` temp name, parents created).
//
// Zero npm dependencies — Node built-ins only.

import { mkdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';

let nextTemp = 0;

// `options` is passed to the temp file's `writeFileSync` (e.g. `'utf8'`).
export function writeFileAtomic(file, data, options) {
  const dir = path.dirname(file);
  mkdirSync(dir, { recursive: true });
  const temp = path.join(dir, `.${path.basename(file)}.${process.pid}.${nextTemp++}.tmp`);
  try {
    writeFileSync(temp, data, options);
    renameSync(temp, file);
  } catch (err) {
    rmSync(temp, { force: true });
    throw err;
  }
}
