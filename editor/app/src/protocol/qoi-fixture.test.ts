// Cross-language QOI fixture check.
//
// `crates/ludion-server/examples/gen_frame_fixtures.rs` builds a small, deterministic synthetic RGBA
// image and encodes it with the same `qoi` crate the server's frame-delivery path uses
// (`ludion_server::delivery::route_frame`), writing the raw pixels (`fixture.rgba`) and the QOI
// encoding of them (`fixture.qoi`) into `__fixtures__/`. This test decodes `fixture.qoi` with this
// package's own hand-rolled TS decoder (`./qoi.ts` — the one the viewer actually uses to decode
// frames off the wire) and byte-compares the result against `fixture.rgba`. Nothing else in the
// workspace pins that the Rust encoder and the TS decoder agree across every QOI op class; this is
// that pin. Regenerate the fixture with `cargo run -p ludion-server --example gen_frame_fixtures`.
//
// Reads the fixture files from disk with `node:fs`; safe here because vitest runs this suite under
// `environment: 'node'` (see `vite.config.ts`) — the browser-facing app code under `src/` never
// touches Node built-ins. Types for the Node built-ins used here come from the shared ambient shim
// in `../test-node-shims.d.ts` (see that file for why: the app's tsconfig intentionally omits
// `@types/node`'s globals).

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeQoi } from './qoi';

interface FixtureDimensions {
  readonly width: number;
  readonly height: number;
}

function fixturePath(name: string): string {
  return fileURLToPath(new URL(`./__fixtures__/${name}`, import.meta.url));
}

describe('cross-language QOI fixture', () => {
  it('decodes fixture.qoi (Rust `qoi` crate) into exactly fixture.rgba (raw bytes)', () => {
    const qoiBytes = readFileSync(fixturePath('fixture.qoi'));
    const rawBytes = readFileSync(fixturePath('fixture.rgba'));
    const dimensions = JSON.parse(
      new TextDecoder().decode(readFileSync(fixturePath('fixture.json'))),
    ) as FixtureDimensions;

    const decoded = decodeQoi(qoiBytes);

    expect(decoded.width).toBe(dimensions.width);
    expect(decoded.height).toBe(dimensions.height);
    expect(decoded.rgba.length).toBe(rawBytes.length);
    expect(decoded.rgba.length).toBe(dimensions.width * dimensions.height * 4);
    // Byte-exact comparison, not a fuzzy one: this fixture pins bit-for-bit compatibility, not
    // approximate visual similarity. Compare as plain arrays so the `Uint8ClampedArray` the
    // decoder returns and the `Uint8Array` read from disk don't fail `toEqual` purely on
    // constructor identity.
    expect(Array.from(decoded.rgba)).toEqual(Array.from(rawBytes));
  });
});
