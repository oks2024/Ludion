import { describe, expect, it } from 'vitest';
import { initialSyncState, reduce, type SyncState } from './fileSync';

const clean = (mtime: number): SyncState => ({ phase: 'clean', baselineMtime: mtime, incoming: null });
const dirty = (mtime: number): SyncState => ({ phase: 'dirty', baselineMtime: mtime, incoming: null });

describe('fileSync.reduce', () => {
  it('loaded resets to clean at the file mtime', () => {
    const { state, effect } = reduce({ phase: 'dirty', baselineMtime: 1, incoming: null }, { type: 'loaded', mtime: 500 });
    expect(state).toEqual(clean(500));
    expect(effect).toEqual({ type: 'none' });
  });

  it('first edit moves clean → dirty; further edits are no-ops (same reference)', () => {
    const c = clean(100);
    const first = reduce(c, { type: 'edit' });
    expect(first.state).toEqual(dirty(100));
    const second = reduce(first.state, { type: 'edit' });
    expect(second.state).toBe(first.state);
  });

  it('external change while CLEAN silently reloads (setContent, stays clean, adopts mtime)', () => {
    const { state, effect } = reduce(clean(100), { type: 'externalChange', mtime: 200, content: 'new' });
    expect(state).toEqual(clean(200));
    expect(effect).toEqual({ type: 'setContent', content: 'new', mtime: 200 });
  });

  it('external change matching the baseline mtime (our own write) is ignored', () => {
    const c = clean(200);
    const { state, effect } = reduce(c, { type: 'externalChange', mtime: 200, content: 'x' });
    expect(state).toBe(c);
    expect(effect).toEqual({ type: 'none' });
  });

  it('external change while DIRTY raises a conflict holding the disk content, no effect', () => {
    const { state, effect } = reduce(dirty(100), { type: 'externalChange', mtime: 300, content: 'theirs' });
    expect(state).toEqual({ phase: 'conflict', baselineMtime: 100, incoming: { content: 'theirs', mtime: 300 } });
    expect(effect).toEqual({ type: 'none' });
  });

  it('a further external change while conflicting keeps the latest disk content', () => {
    const conflict = reduce(dirty(100), { type: 'externalChange', mtime: 300, content: 'v1' }).state;
    const { state } = reduce(conflict, { type: 'externalChange', mtime: 400, content: 'v2' });
    expect(state).toEqual({ phase: 'conflict', baselineMtime: 100, incoming: { content: 'v2', mtime: 400 } });
  });

  it('save adopts the returned mtime and goes clean (from dirty and from conflict)', () => {
    expect(reduce(dirty(100), { type: 'save', mtime: 999 }).state).toEqual(clean(999));
    const conflict = reduce(dirty(100), { type: 'externalChange', mtime: 300, content: 'x' }).state;
    expect(reduce(conflict, { type: 'save', mtime: 1000 }).state).toEqual(clean(1000));
  });

  it('reload (banner) applies the incoming disk content and goes clean', () => {
    const conflict = reduce(dirty(100), { type: 'externalChange', mtime: 300, content: 'theirs' }).state;
    const { state, effect } = reduce(conflict, { type: 'reload' });
    expect(state).toEqual(clean(300));
    expect(effect).toEqual({ type: 'setContent', content: 'theirs', mtime: 300 });
  });

  it('reload with nothing pending is a no-op', () => {
    const { state, effect } = reduce(clean(100), { type: 'reload' });
    expect(state).toEqual(clean(100));
    expect(effect).toEqual({ type: 'none' });
  });

  it('keepMine dismisses the banner, stays dirty, and acknowledges the disk mtime', () => {
    const conflict = reduce(dirty(100), { type: 'externalChange', mtime: 300, content: 'theirs' }).state;
    const { state, effect } = reduce(conflict, { type: 'keepMine' });
    expect(state).toEqual(dirty(300));
    expect(effect).toEqual({ type: 'none' });
    // The acknowledged external change no longer re-prompts (same mtime is ignored)...
    expect(reduce(state, { type: 'externalChange', mtime: 300, content: 'theirs' }).state).toBe(state);
    // ...but a genuinely newer external change conflicts again.
    expect(reduce(state, { type: 'externalChange', mtime: 400, content: 'newer' }).state.phase).toBe('conflict');
  });

  it('initial state is clean with no baseline', () => {
    expect(initialSyncState).toEqual({ phase: 'clean', baselineMtime: null, incoming: null });
    // With no baseline yet, the first external change reloads rather than falsely conflicting.
    const { effect } = reduce(initialSyncState, { type: 'externalChange', mtime: 10, content: 'first' });
    expect(effect.type).toBe('setContent');
  });
});
