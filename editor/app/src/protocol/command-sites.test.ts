// Class gate for typed commands: a cast to `Command` hides a wrong params shape from tsc, and a
// hand-built `component.set` is the main reason one was needed. Commands are built as typed
// literals, and every `component.set` goes through `componentSet` (`commands.ts`).

import { readFileSync, readdirSync } from 'node:fs';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_SRC_DIR = fileURLToPath(new URL('../', import.meta.url));
const COMMAND_CAST = /\bas\s+(unknown\s+as\s+)?Command\b/;
const COMPONENT_SET_LITERAL = /method:\s*['"]component\.set['"]/;

function isAuditableSource(path: string): boolean {
  if (!path.endsWith('.ts') && !path.endsWith('.tsx')) return false;
  return !path.endsWith('.d.ts') && !path.includes('.test.');
}

function sources(): { path: string; text: string }[] {
  return readdirSync(APP_SRC_DIR, { recursive: true })
    .filter((entry) => extname(entry) !== '')
    .map((entry) => entry.split('\\').join('/'))
    .filter(isAuditableSource)
    .map((path) => ({ path, text: readFileSync(join(APP_SRC_DIR, path)).toString() }));
}

describe('command construction sites', () => {
  it('no source casts to Command', () => {
    expect(
      sources()
        .filter(({ text }) => COMMAND_CAST.test(text))
        .map(({ path }) => path),
    ).toEqual([]);
  });

  it('only protocol/commands.ts spells a component.set literal', () => {
    expect(
      sources()
        .filter(({ path, text }) => path !== 'protocol/commands.ts' && COMPONENT_SET_LITERAL.test(text))
        .map(({ path }) => path),
    ).toEqual([]);
  });
});
