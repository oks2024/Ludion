import { describe, expect, it } from 'vitest';
import { beginVerb, endVerb, IDLE, isBusy, isCurrent } from './inflight';

describe('in-flight verb counts', () => {
  it('keeps a run busy when a check starts and ends during it', () => {
    let counts = beginVerb(IDLE, 'run');
    counts = beginVerb(counts, 'check');
    counts = endVerb(counts, 'check');
    expect(isBusy(counts, 'run')).toBe(true);
    expect(isBusy(counts, 'check')).toBe(false);
    expect(isBusy(counts)).toBe(true);
  });

  it('counts two of the same verb and never goes below zero', () => {
    let counts = beginVerb(beginVerb(IDLE, 'run'), 'run');
    counts = endVerb(counts, 'run');
    expect(isBusy(counts, 'run')).toBe(true);
    counts = endVerb(endVerb(counts, 'run'), 'run');
    expect(isBusy(counts)).toBe(false);
  });

  it('is immutable', () => {
    const counts = beginVerb(IDLE, 'run');
    endVerb(counts, 'run');
    expect(isBusy(counts, 'run')).toBe(true);
    expect(isBusy(IDLE)).toBe(false);
  });
});

describe('isCurrent', () => {
  const captured = { projectDir: '/p', recipe: 'a.yaml' };

  it('holds while project and recipe match', () => {
    expect(isCurrent(captured, { projectDir: '/p', recipe: 'a.yaml', runId: 'r1' })).toBe(true);
  });

  it('drops on a recipe switch and on a project switch', () => {
    expect(isCurrent(captured, { projectDir: '/p', recipe: 'b.yaml' })).toBe(false);
    expect(isCurrent(captured, { projectDir: '/p', recipe: null })).toBe(false);
    expect(isCurrent(captured, { projectDir: '/q', recipe: 'a.yaml' })).toBe(false);
  });

  it('compares the run only when the capture named one', () => {
    const accept = { ...captured, runId: 'r1' };
    expect(isCurrent(accept, { projectDir: '/p', recipe: 'a.yaml', runId: 'r1' })).toBe(true);
    expect(isCurrent(accept, { projectDir: '/p', recipe: 'a.yaml', runId: 'r2' })).toBe(false);
    expect(isCurrent(accept, { projectDir: '/p', recipe: 'a.yaml', runId: null })).toBe(false);
  });
});
