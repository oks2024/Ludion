import { describe, expect, it } from 'vitest';
import { pickScene } from './sceneList';
import { sceneChangeOutcome } from './sceneChange';

describe('sceneChangeOutcome', () => {
  it('resets edit state and adopts the path for a foreign change', () => {
    expect(sceneChangeOutcome(false, 'main.yaml')).toEqual({ resetEditState: true, savePath: 'main.yaml' });
  });

  it('keeps edit state for our own save, still adopting the path', () => {
    expect(sceneChangeOutcome(true, 'main.yaml')).toEqual({ resetEditState: false, savePath: 'main.yaml' });
  });

  it('leaves savePath null when the event carries none', () => {
    expect(sceneChangeOutcome(false, null)).toEqual({ resetEditState: true, savePath: null });
    expect(sceneChangeOutcome(false, undefined)).toEqual({ resetEditState: true, savePath: null });
  });

  it('follows the picker load: never arms the own-save flag, so the change is foreign', async () => {
    let ownSave = false;
    await pickScene('main.yaml', {
      dirty: false,
      savePath: '',
      confirm: () => Promise.resolve(true),
      load: () => Promise.resolve(undefined),
    });
    expect(sceneChangeOutcome(ownSave, 'main.yaml')).toEqual({ resetEditState: true, savePath: 'main.yaml' });

    ownSave = true; // contrast: an armed own save keeps edit state
    expect(sceneChangeOutcome(ownSave, 'main.yaml')).toEqual({ resetEditState: false, savePath: 'main.yaml' });
  });
});
