// Class gate for lossless component values: `JSON.parse` rounds a u64 above 2^53 and
// `JSON.stringify` throws on the bigint `parseWire` yields instead. The sites that read or write
// component values go through `wireJson.ts`; this pins them.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_SRC_DIR = fileURLToPath(new URL('../', import.meta.url));
const JSON_CALL = /\bJSON\.(parse|stringify)\(/g;

function read(path: string): string {
  return readFileSync(`${APP_SRC_DIR}${path}`).toString();
}

describe('component-value JSON sites', () => {
  it.each([
    'viewer/pendingBase.ts',
    'viewer/Inspector.tsx',
    'viewer/FieldWidgets.tsx',
    'editor/undo.ts',
    'editor/prefab-actions.ts',
  ])('%s uses parseWire/stringifyWire', (path) => {
    expect(read(path).match(JSON_CALL) ?? []).toEqual([]);
  });

  it('protocol/client.ts keeps JSON only for the hello handshake', () => {
    // The hello send and its reply parse; every response and request goes through the wire helpers.
    expect(read('protocol/client.ts').match(JSON_CALL)).toHaveLength(2);
  });
});
