// Class gate for document writes: a write that does not go through the synced-file session skips its
// stat-before-write conflict check and its dispose-flush, so an agent's terminal edit is reverted or a
// pending write is lost on a document switch. `useSyncedFile.ts` is the one writer of an open
// document; `CodePanel.tsx` (CodeMirror owns that doc) and `PatternEditor.tsx`'s `create()` (it writes
// a brand-new file before opening it) are the two allow-listed exceptions.

import { readFileSync, readdirSync } from 'node:fs';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const EDITOR_DIR = fileURLToPath(new URL('../', import.meta.url));
const ALLOWED = new Map([
  ['systems/useSyncedFile.ts', 1],
  ['systems/CodePanel.tsx', 1],
  ['effects/PatternEditor.tsx', 1],
]);
const WRITE_CALL = /writeEditorFile\(/g;

function isAuditableSource(path: string): boolean {
  if (!path.endsWith('.ts') && !path.endsWith('.tsx')) return false;
  return !path.endsWith('.d.ts') && !path.includes('.test.');
}

describe('document write sites', () => {
  it('only the synced-file hook, the code panel and pattern creation write a file', () => {
    const counts: Record<string, number> = {};
    for (const entry of readdirSync(EDITOR_DIR, { recursive: true })) {
      if (extname(entry) === '') continue;
      const path = entry.split('\\').join('/');
      if (!isAuditableSource(path)) continue;
      const found = readFileSync(join(EDITOR_DIR, path)).toString().match(WRITE_CALL)?.length ?? 0;
      if (found > 0) counts[path] = found;
    }
    expect(counts).toEqual(Object.fromEntries(ALLOWED));
  });
});
