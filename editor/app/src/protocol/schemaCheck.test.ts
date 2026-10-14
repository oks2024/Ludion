import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { SCHEMA_HASH } from './schemaHash';
import { schemaMismatch } from './schemaCheck';

describe('schemaMismatch', () => {
  it('is null when the hashes agree', () => {
    expect(schemaMismatch('1ebb24651f97ed0f', '1ebb24651f97ed0f')).toBeNull();
  });

  it('names both hashes, carries L3003 and a fix', () => {
    const d = schemaMismatch('aaaaaaaaaaaaaaaa', 'bbbbbbbbbbbbbbbb');
    expect(d?.code).toBe('L3003');
    expect(d?.message).toContain('aaaaaaaaaaaaaaaa');
    expect(d?.message).toContain('bbbbbbbbbbbbbbbb');
    expect(d?.help).toContain('npm run generate');
  });
});

describe('SCHEMA_HASH', () => {
  it("equals the hash the Rust test pins for the same snapshot (two XXH3 implementations)", () => {
    const rust = readFileSync(
      fileURLToPath(new URL('../../../../crates/ludion-protocol/tests/schema_snapshot.rs', import.meta.url)),
      'utf8',
    );
    const pinned = /EXPECTED_SCHEMA_HASH[^"]*"([0-9a-f]{16})"/.exec(rust);
    expect(pinned).not.toBeNull();
    expect(SCHEMA_HASH).toBe(pinned?.[1]);
  });
});
