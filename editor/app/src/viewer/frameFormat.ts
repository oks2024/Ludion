// Which codec this viewer asks the server for, and why.
//
// THE MEASUREMENT THAT DECIDES IT (real plaza frames, 2026-08-08). QOI: 5.37 ms server encode,
// 1.20 MB, 16.1 ms decode here. LZ4: 7.55 ms, 2.42 MB, 12.8 ms. RAW: no encode worth the name, 8.3 MB
// at 1080p, and a decode that is one `set` call. So the question is not "which codec is best" but
// "is the wire free": on loopback it is, and RAW wins at both ends by not being a codec; over a real
// network 8 MB a frame is not sendable and QOI wins, being both smaller AND cheaper to encode than
// LZ4 on this content. LZ4 is therefore nobody's default — it stays reachable only through the
// override, which is also how the numbers above get re-measured.
//
// `ludion-window` decides the same question the same way (`crates/ludion-window/src/reconnect.rs`,
// `default_stream_format`), and its unit test pins the same URLs as this module's.
//
// Pure and socket-free: the URL and the stored override come in as arguments so the policy is
// testable without a browser, a server or a `localStorage`.

import type { FrameFormat } from '../protocol/frames';

/** localStorage key holding a format name to use verbatim, whatever the URL says. */
export const FORMAT_KEY = 'ludion.editor.frameFormat';

const FORMATS: readonly FrameFormat[] = ['QOI', 'LZ4_RGBA', 'RAW_RGBA'];

/** A resolved codec together with the reason it was resolved — the reason is what gets announced,
 *  because a reading taken without knowing which codec produced it is not a reading. */
export interface FormatChoice {
  readonly format: FrameFormat;
  readonly why: 'forced' | 'loopback' | 'remote';
}

/**
 * Whether `url`'s host is one of the three loopback spellings — `127.0.0.1`, `localhost`, `[::1]`.
 *
 * Deliberately shallow, exactly like the Rust twin: a hostname that reaches a loopback address by
 * other means (a `hosts` alias, a tunnel) reads as remote and merely gets a codec. The cost of
 * guessing wrong here is bandwidth, never a failure — so a URL that will not even parse is remote.
 */
export function isLoopbackUrl(url: string | null): boolean {
  if (url === null || url === '') {
    return false;
  }
  let host: string;
  try {
    host = new URL(url).hostname; // already lowercased; an IPv6 literal keeps its brackets
  } catch {
    return false;
  }
  return host === 'localhost' || host === '127.0.0.1' || host === '[::1]';
}

/**
 * The codec to subscribe with for a server at `url`, given the raw `override` string read out of
 * `localStorage` (`null` when unset or unreadable).
 *
 * An override naming a real format wins outright — that is the A/B knob, and it can name any of the
 * three. Anything else is ignored rather than rejected: a typo in a developer key must not stop the
 * viewport from rendering.
 */
export function chooseFrameFormat(url: string | null, override: string | null): FormatChoice {
  const forced = FORMATS.find((format) => format === override);
  if (forced !== undefined) {
    return { format: forced, why: 'forced' };
  }
  return isLoopbackUrl(url)
    ? { format: 'RAW_RGBA', why: 'loopback' }
    : { format: 'QOI', why: 'remote' };
}

/** One line naming the resolved codec and what decided it. */
export function describeFormatChoice(choice: FormatChoice, url: string | null): string {
  const because =
    choice.why === 'forced'
      ? `forced by ${FORMAT_KEY}`
      : choice.why === 'loopback'
        ? `loopback ${url ?? '(unknown)'} — the wire is free, so the cheapest decode wins`
        : `remote ${url ?? '(unknown)'} — the wire is not free, so the smallest payload wins`;
  return `${choice.format} (${because})`;
}
