---
description: Viewer-side frame decode: TS worker/pump/inline paths, codec switch, Rust consumers' decode
---

# frame-streaming-viewer

**Scope:** the decode leg of the frame path — `editor/app`'s TS viewer, `ludion-client`'s and
`ludion-window`'s Rust consumers. Everything upstream (egress, pump, routers, forwarder, LFR1 on the wire)
is `ludion devdocs frame-streaming`; codec cost numbers are `ludion devdocs frame-streaming-codecs`.

## Layout

- `editor/app/src/protocol/frames.ts` — `decodeFrameHeader`, the TS mirror of `frame.rs`.
- `editor/app/src/protocol/decode.ts` — `decodeFramePayload`, the one codec switch over all three
  formats, shared by the worker and the no-worker inline sink so it has exactly one owner.
- `editor/app/src/protocol/qoi.ts` — the QOI pixel decoder.
- `editor/app/src/protocol/lz4.ts` — the LZ4_RGBA block decoder.
- `editor/app/src/viewer/frameFormat.ts` — `chooseFrameFormat`, the TS mirror of
  `default_stream_format`, plus the `ludion.editor.frameFormat` localStorage override.
- `editor/app/src/viewer/frameWorker.ts` / `frameWorkerProtocol.ts` / `framePump.ts` — decode off the
  main thread: the worker itself (offscreen-canvas or buffer-recycling blit), the request/reply types
  it exchanges with its host, and the two-deep latest-wins pump gating what reaches it.
- `editor/app/src/viewer/useFrameStream.ts` — the React hook: subscribes with the chosen format,
  picks a decode path (`inline`/`worker-blit`/`offscreen`), and blits to a canvas.

## Dataflow

**Decode.** `protocol/client.ts` calls its frame listeners inline on the WebSocket message
   event, so everything `useFrameStream.ts` does there is time a keydown waits before it can reach
   `ws.send` — kept to a header check, a defensive format guard (`canDecodeFormat`, so a codec added
   to the wire but absent from this build degrades to a still viewport instead of an exception per
   frame), a payload copy, and a `postMessage`. The actual decode runs off that thread, in
   `frameWorker.ts`, gated by `framePump.ts`'s two-deep latest-wins pump: a frame that arrives while
   the worker is busy REPLACES whatever was parked rather than queueing behind it, so the client's
   backlog never exceeds one decoding plus one waiting no matter how far behind it falls. The worker
   runs one of two paths (`chooseRenderPath` in `framePump.ts`, resolved once per document):
   `offscreen`, where a transferred `OffscreenCanvas` lets the worker decode *and* blit with nothing
   on the main thread, or `worker-blit`, where the worker decodes into a recycled pixel buffer and
   the main thread does one `putImageData`. A browser with no `Worker` falls back to `inline` —
   decode and blit synchronously on the message event, the pre-worker behavior and what the other two
   paths are measured against. Whichever path runs, the actual format switch is one function,
   `protocol/decode.ts`'s `decodeFramePayload(header.format, payload, out)` — QOI via `qoi.ts`,
   LZ4_RGBA via `lz4.ts`, RAW_RGBA as a length-checked `out.set(payload)` — shared by the worker and
   the inline sink so the codec list has exactly one owner. `useFrameStream.ts` resizes the canvas to
   the frame's intrinsic pixel size and computes a rolling fps for the status overlay regardless of
   path. `ludion-client`'s `next_frame` is the Rust-side equivalent — it decodes the LFR1 header
   (`FrameHeader::decode`) but deliberately leaves the payload encoded, matching the `screenshot()`
   precedent; a Rust caller that wants pixels decodes it itself (as
   `crates/ludion-client/tests/streams.rs` does with the `qoi` crate), matching `header.format` to a
   decoder. `crates/ludion-window` decodes its own subscribed format the same way, synchronously,
   since it is not a browser and has no message-event budget to protect.

## Verify

TS-side (`editor/app`): the decode and hook unit tests (`src/protocol/frames.test.ts`,
`src/protocol/decode.test.ts`, `src/protocol/qoi.test.ts`, `src/protocol/qoi-fixture.test.ts`,
`src/protocol/lz4.test.ts`, `src/protocol/lz4-fixture.test.ts`, `src/viewer/*.test.ts` — including
`frameFormat.test.ts` and `framePump.test.ts`) and the parity audit (`src/protocol/parity.test.ts`,
which mechanically proves the viewer only ever calls methods from the generated protocol union and
only ever opens the wire through `src/protocol/client.ts`) — see `editor/app`'s own test runner for
the exact command.
