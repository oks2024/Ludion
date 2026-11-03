import { describe, expect, it } from 'vitest';
import { pickScene } from './sceneList';
import { sceneChangeOutcome } from './sceneChange';

const none: ReadonlyMap<number, string> = new Map();

describe('sceneChangeOutcome', () => {
  it('switch with no path resets and clears the Save path', () => {
    expect(sceneChangeOutcome({ kind: 'switch' }, none, 'a.yaml')).toEqual({
      resetEditState: true,
      clearDirty: true,
      savePath: '',
      ownSaveId: null,
    });
  });

  it('switch with a path resets and adopts it', () => {
    expect(sceneChangeOutcome({ kind: 'switch', path: 'b.yaml' }, none, 'a.yaml')).toMatchObject({
      resetEditState: true,
      clearDirty: true,
      savePath: 'b.yaml',
    });
  });

  it('load with a path resets and adopts it', () => {
    expect(sceneChangeOutcome({ kind: 'load', path: 'b.yaml' }, none, 'a.yaml')).toMatchObject({
      resetEditState: true,
      clearDirty: true,
      savePath: 'b.yaml',
    });
  });

  it('a load with no path keeps the Save path', () => {
    expect(sceneChangeOutcome({ kind: 'load' }, none, 'a.yaml')).toMatchObject({ resetEditState: true, savePath: null });
  });

  it('a restore with no path clears the Save path', () => {
    expect(sceneChangeOutcome({ kind: 'restore', path: null }, none, 'a.yaml')).toMatchObject({
      resetEditState: true,
      clearDirty: true,
      savePath: '',
    });
  });

  it('a load is a replaced world', () => {
    expect(sceneChangeOutcome({ kind: 'load', path: 'main.yaml' }, none, '')).toEqual({
      resetEditState: true,
      clearDirty: true,
      savePath: 'main.yaml',
      ownSaveId: null,
    });
  });

  it('own save keeps edit state, clears dirty, adopts the path and names its id', () => {
    expect(sceneChangeOutcome({ kind: 'save', path: 'a.yaml', request_id: 7 }, new Map([[7, 'a.yaml']]), 'a.yaml')).toEqual({
      resetEditState: false,
      clearDirty: true,
      savePath: 'a.yaml',
      ownSaveId: 7,
    });
  });

  it('foreign save of the served path clears dirty and keeps edit state', () => {
    expect(sceneChangeOutcome({ kind: 'save', path: 'a.yaml', request_id: 9 }, new Map([[7, 'a.yaml']]), 'a.yaml')).toEqual({
      resetEditState: false,
      clearDirty: true,
      savePath: null,
      ownSaveId: null,
    });
  });

  it('foreign save with no Save path yet adopts the path, dirty untouched', () => {
    expect(sceneChangeOutcome({ kind: 'save', path: 'a.yaml' }, none, '')).toEqual({
      resetEditState: false,
      clearDirty: false,
      savePath: 'a.yaml',
      ownSaveId: null,
    });
  });

  it('foreign save to another path changes nothing', () => {
    expect(sceneChangeOutcome({ kind: 'save', path: 'other.yaml' }, none, 'a.yaml')).toEqual({
      resetEditState: false,
      clearDirty: false,
      savePath: null,
      ownSaveId: null,
    });
  });

  it('a foreign save with our id and another path changes nothing and leaves the id pending', () => {
    const pending = new Map([[5, 'a.yaml']]);
    expect(sceneChangeOutcome({ kind: 'save', path: 'other.yaml', request_id: 5 }, pending, 'a.yaml')).toEqual({
      resetEditState: false,
      clearDirty: false,
      savePath: null,
      ownSaveId: null,
    });
    expect(pending.has(5)).toBe(true);
    expect(sceneChangeOutcome({ kind: 'save', path: 'a.yaml', request_id: 5 }, pending, 'a.yaml')).toMatchObject({
      clearDirty: true,
      ownSaveId: 5,
    });
  });

  it('a foreign load with an own id pending resets and leaves the id; the real save then matches', () => {
    const pending = new Map([[5, 'b.yaml']]);
    const load = sceneChangeOutcome({ kind: 'load', path: 'b.yaml' }, pending, 'a.yaml');
    expect(load).toMatchObject({ resetEditState: true, ownSaveId: null });
    expect(pending.has(5)).toBe(true);
    expect(sceneChangeOutcome({ kind: 'save', path: 'b.yaml', request_id: 5 }, pending, 'b.yaml')).toMatchObject({
      resetEditState: false,
      clearDirty: true,
      ownSaveId: 5,
    });
  });

  it('a picker load carries no own id, so it resets', async () => {
    await pickScene('main.yaml', {
      dirty: false,
      savePath: '',
      confirm: () => Promise.resolve(true),
      load: () => Promise.resolve(undefined),
    });
    expect(sceneChangeOutcome({ kind: 'load', path: 'main.yaml' }, none, '')).toMatchObject({ resetEditState: true });
  });
});
