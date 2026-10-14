import { describe, expect, it } from 'vitest';
import { chooseFrameFormat, describeFormatChoice, isLoopbackUrl } from './frameFormat';

// The two URL lists are the ones `crates/ludion-window/src/reconnect.rs` pins in
// `loopback_urls_stream_raw_and_everything_else_streams_qoi`. They are duplicated rather than shared
// on purpose: the point of the test is that two independently written parsers agree.
const LOOPBACK = [
  'ws://127.0.0.1:9411',
  'ws://localhost:9411',
  'ws://LocalHost:9411/stream',
  'ws://[::1]:9411',
  'ws://127.0.0.1',
];

const REMOTE = [
  'ws://203.0.113.5:9411',
  'ws://myhost:9411',
  'wss://viewer.example.com:9411/stream',
  // The trap: a domain that merely STARTS with the loopback address. A prefix match would send 8 MB
  // a frame to a host on the other side of a real network.
  'ws://127.0.0.1.example.com:9411',
];

describe('isLoopbackUrl', () => {
  it.each(LOOPBACK)('%s is loopback', (url) => {
    expect(isLoopbackUrl(url)).toBe(true);
  });

  it.each(REMOTE)('%s is remote', (url) => {
    expect(isLoopbackUrl(url)).toBe(false);
  });

  it('treats an absent or unparseable URL as remote', () => {
    expect(isLoopbackUrl(null)).toBe(false);
    expect(isLoopbackUrl('')).toBe(false);
    expect(isLoopbackUrl('not a url')).toBe(false);
    expect(isLoopbackUrl('/relative/path')).toBe(false);
  });
});

describe('chooseFrameFormat', () => {
  it('takes RAW_RGBA on loopback, where the wire is free', () => {
    expect(chooseFrameFormat('ws://127.0.0.1:9411', null)).toEqual({
      format: 'RAW_RGBA',
      why: 'loopback',
    });
  });

  it('takes QOI against a remote host — smaller AND cheaper to encode than LZ4 on this content', () => {
    expect(chooseFrameFormat('wss://viewer.example.com:9411', null)).toEqual({
      format: 'QOI',
      why: 'remote',
    });
  });

  it.each(['QOI', 'LZ4_RGBA', 'RAW_RGBA'])('lets an override name %s verbatim', (override) => {
    // Including against the URL's own preference — forcing RAW remote and QOI on loopback are both
    // legitimate A/B runs, and LZ4 is reachable ONLY this way now that it is nobody's default.
    expect(chooseFrameFormat('ws://127.0.0.1:9411', override)).toEqual({ format: override, why: 'forced' });
    expect(chooseFrameFormat('ws://example.com:9411', override)).toEqual({ format: override, why: 'forced' });
  });

  it('ignores an override that does not name a format', () => {
    expect(chooseFrameFormat('ws://127.0.0.1:9411', 'lz4')).toEqual({
      format: 'RAW_RGBA',
      why: 'loopback',
    });
    expect(chooseFrameFormat('ws://127.0.0.1:9411', '')).toEqual({
      format: 'RAW_RGBA',
      why: 'loopback',
    });
  });
});

describe('describeFormatChoice', () => {
  it('names the format and what decided it', () => {
    const url = 'ws://127.0.0.1:9411';
    expect(describeFormatChoice(chooseFrameFormat(url, null), url)).toMatch(/^RAW_RGBA \(loopback /);
    expect(describeFormatChoice(chooseFrameFormat(url, 'QOI'), url)).toMatch(
      /^QOI \(forced by ludion\.editor\.frameFormat\)$/,
    );
    expect(describeFormatChoice(chooseFrameFormat('ws://example.com', null), 'ws://example.com')).toMatch(
      /^QOI \(remote /,
    );
  });
});
