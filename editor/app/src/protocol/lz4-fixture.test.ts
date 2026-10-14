// Cross-language LZ4 fixture check.
//
// `crates/ludion-server/examples/gen_frame_fixtures.rs` builds a small, deterministic synthetic RGBA
// image and compresses it with the same `lz4_flex` block API the server's frame-delivery path uses
// (`ludion_server::delivery::route_frame`), writing the raw pixels (`fixture.rgba`) and the LZ4 block
// encoding of them (`fixture.lz4`) into `__fixtures__/`. This test decompresses `fixture.lz4` with
// this package's own hand-rolled TS decoder (`./lz4.ts` — the one the viewer actually uses to decode
// `LZ4_RGBA` frames off the wire) and byte-compares the result against `fixture.rgba`. Nothing else
// in the workspace pins that the Rust compressor and the TS decompressor agree; this is that pin.
// Regenerate the fixture with `cargo run -p ludion-server --example gen_frame_fixtures`.
//
// The block carries no size of its own, so the output buffer is sized from `fixture.json` — standing
// in for the LFR1 header, which is where a real frame's dimensions come from.
//
// Reads the fixture files from disk with `node:fs`; safe here because vitest runs this suite under
// `environment: 'node'` (see `vite.config.ts`) — the browser-facing app code under `src/` never
// touches Node built-ins. Types for the Node built-ins used here come from the shared ambient shim
// in `../test-node-shims.d.ts` (see that file for why: the app's tsconfig intentionally omits
// `@types/node`'s globals).

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decompressLz4Block } from './lz4';

interface FixtureDimensions {
  readonly width: number;
  readonly height: number;
}

function fixturePath(name: string): string {
  return fileURLToPath(new URL(`./__fixtures__/${name}`, import.meta.url));
}

describe('cross-language LZ4 fixture', () => {
  it('decompresses fixture.lz4 (Rust `lz4_flex`) into exactly fixture.rgba (raw bytes)', () => {
    const lz4Bytes = readFileSync(fixturePath('fixture.lz4'));
    const rawBytes = readFileSync(fixturePath('fixture.rgba'));
    const dimensions = JSON.parse(
      new TextDecoder().decode(readFileSync(fixturePath('fixture.json'))),
    ) as FixtureDimensions;

    expect(rawBytes.length).toBe(dimensions.width * dimensions.height * 4);
    const out = new Uint8ClampedArray(dimensions.width * dimensions.height * 4);
    decompressLz4Block(lz4Bytes, out);

    // Byte-exact comparison, not a fuzzy one: this fixture pins bit-for-bit compatibility, not
    // approximate visual similarity. Compare as plain arrays so the `Uint8ClampedArray` the decoder
    // fills and the `Uint8Array` read from disk don't fail `toEqual` purely on constructor identity.
    expect(Array.from(out)).toEqual(Array.from(rawBytes));
  });
});
