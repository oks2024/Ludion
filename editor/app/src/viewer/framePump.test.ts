import { describe, expect, it } from 'vitest';
import { chooseRenderPath, createFramePump, type FrameJob, type FramePathCaps } from './framePump';

function job(frameIndex: number): FrameJob {
  return { frameIndex, width: 4, height: 4, format: 'LZ4_RGBA', payload: new ArrayBuffer(8) };
}

const EVERYTHING: FramePathCaps = {
  worker: true,
  offscreenCanvas: true,
  transferControl: true,
  offscreen2d: true,
};

describe('the latest-wins pump', () => {
  it('posts a frame that arrives while the decoder is idle', () => {
    const pump = createFramePump();
    const first = job(1);
    expect(pump.offer(first)).toBe(first);
    expect(pump.busy).toBe(true);
  });

  it('parks a frame that arrives while the decoder is busy', () => {
    const pump = createFramePump();
    pump.offer(job(1));
    expect(pump.offer(job(2))).toBeNull();
    expect(pump.dropped).toBe(0);
  });

  it('keeps the newest of several frames that arrive during one decode, dropping the rest', () => {
    const pump = createFramePump();
    pump.offer(job(1));
    pump.offer(job(2));
    pump.offer(job(3));
    pump.offer(job(4));
    // Two replacements, so two frames were shed; the survivor is the last one offered.
    expect(pump.dropped).toBe(2);
    expect(pump.complete()?.frameIndex).toBe(4);
  });

  it('never queues more than one frame no matter how far behind it falls', () => {
    const pump = createFramePump();
    pump.offer(job(0));
    for (let i = 1; i <= 500; i++) {
      expect(pump.offer(job(i))).toBeNull();
    }
    expect(pump.complete()?.frameIndex).toBe(500);
    expect(pump.complete()).toBeNull();
    expect(pump.dropped).toBe(499);
  });

  it('stays busy while the parked frame it just handed back is decoded', () => {
    const pump = createFramePump();
    pump.offer(job(1));
    pump.offer(job(2));
    expect(pump.complete()?.frameIndex).toBe(2);
    expect(pump.busy).toBe(true);
    // The next arrival is parked behind frame 2, not posted alongside it.
    expect(pump.offer(job(3))).toBeNull();
  });

  it('goes idle when a decode finishes with nothing waiting', () => {
    const pump = createFramePump();
    pump.offer(job(1));
    expect(pump.complete()).toBeNull();
    expect(pump.busy).toBe(false);
    const next = job(2);
    expect(pump.offer(next)).toBe(next);
  });

  it('tolerates a completion that outlives its subscription', () => {
    const pump = createFramePump();
    pump.offer(job(1));
    pump.complete();
    // A failed decode's reply arriving twice, or one arriving after teardown: the pump must not
    // latch busy, or the stream would never resume.
    expect(pump.complete()).toBeNull();
    expect(pump.busy).toBe(false);
  });

  it('drops the parked frame on discard without counting it as shed', () => {
    const pump = createFramePump();
    pump.offer(job(1));
    pump.offer(job(2));
    pump.discard();
    expect(pump.dropped).toBe(0);
    // The in-flight decode is still owed a completion, and it now has nothing to hand back.
    expect(pump.complete()).toBeNull();
    expect(pump.busy).toBe(false);
  });

  it('keeps a discarded pump usable', () => {
    const pump = createFramePump();
    pump.offer(job(1));
    pump.discard();
    pump.complete();
    const fresh = job(9);
    expect(pump.offer(fresh)).toBe(fresh);
  });

  it('starts idle, empty, and having dropped nothing', () => {
    const pump = createFramePump();
    expect(pump.busy).toBe(false);
    expect(pump.dropped).toBe(0);
    expect(pump.complete()).toBeNull();
  });

  it('keeps two pumps independent', () => {
    const a = createFramePump();
    const b = createFramePump();
    a.offer(job(1));
    a.offer(job(2));
    expect(b.busy).toBe(false);
    expect(b.dropped).toBe(0);
  });
});

describe('choosing a render path', () => {
  it('puts the canvas in the worker when everything is available', () => {
    expect(chooseRenderPath(EVERYTHING)).toBe('offscreen');
  });

  it('falls back to decoding in the worker and blitting here without OffscreenCanvas', () => {
    expect(chooseRenderPath({ ...EVERYTHING, offscreenCanvas: false })).toBe('worker-blit');
  });

  it('falls back when the canvas cannot be transferred', () => {
    expect(chooseRenderPath({ ...EVERYTHING, transferControl: false })).toBe('worker-blit');
  });

  it('falls back when an OffscreenCanvas yields no 2D context', () => {
    // The transfer is permanent, so a canvas handed to a worker that cannot draw on it is lost for
    // good — this capability is probed rather than inferred from the constructor's existence.
    expect(chooseRenderPath({ ...EVERYTHING, offscreen2d: false })).toBe('worker-blit');
  });

  it('decodes inline only when there are no workers at all', () => {
    expect(chooseRenderPath({ ...EVERYTHING, worker: false })).toBe('inline');
    expect(chooseRenderPath({ worker: false, offscreenCanvas: false, transferControl: false, offscreen2d: false })).toBe(
      'inline',
    );
  });
});
