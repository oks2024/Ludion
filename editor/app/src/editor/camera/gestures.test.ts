import { describe, expect, it } from 'vitest';
import {
  LEFT_BUTTON,
  MIDDLE_BUTTON,
  NO_FLY_KEYS,
  RIGHT_BUTTON,
  flyAction,
  flyKeysFrom,
  fovDirection,
  isCameraDrag,
  resolveDrag,
  resolveWheel,
  viewCommand,
  type DragGesture,
} from './gestures';

/** Every button combination, so the table below is exhaustive by construction rather than by eye. */
const COMBINATIONS: readonly number[] = [
  0,
  LEFT_BUTTON,
  RIGHT_BUTTON,
  MIDDLE_BUTTON,
  LEFT_BUTTON | RIGHT_BUTTON,
  LEFT_BUTTON | MIDDLE_BUTTON,
  RIGHT_BUTTON | MIDDLE_BUTTON,
  LEFT_BUTTON | RIGHT_BUTTON | MIDDLE_BUTTON,
];

function drag(buttons: number, alt: boolean, plainLeftOrbits = false): DragGesture {
  return resolveDrag({ buttons, alt, plainLeftOrbits });
}

describe('resolveDrag', () => {
  it('binds the UE fly and pivot gestures', () => {
    // Plain: RMB looks, MMB and LMB+RMB pan.
    expect(drag(RIGHT_BUTTON, false)).toBe('fly-look');
    expect(drag(MIDDLE_BUTTON, false)).toBe('pan');
    expect(drag(LEFT_BUTTON | RIGHT_BUTTON, false)).toBe('pan');
    // Alt: the three pivot gestures.
    expect(drag(LEFT_BUTTON, true)).toBe('orbit');
    expect(drag(RIGHT_BUTTON, true)).toBe('dolly');
    expect(drag(MIDDLE_BUTTON, true)).toBe('pan');
  });

  it('gives a plain left drag to the view, or orbits where nothing is selectable', () => {
    expect(drag(LEFT_BUTTON, false, false)).toBe('pick');
    expect(drag(LEFT_BUTTON, false, true)).toBe('orbit');
    // The stage's plain-left orbit never overrides a modified gesture.
    expect(drag(LEFT_BUTTON, true, true)).toBe('orbit');
    expect(drag(LEFT_BUTTON | RIGHT_BUTTON, false, true)).toBe('pan');
    expect(drag(RIGHT_BUTTON, false, true)).toBe('fly-look');
  });

  it('resolves every combination, and only an empty one to nothing', () => {
    for (const buttons of COMBINATIONS) {
      for (const alt of [false, true]) {
        for (const plainLeftOrbits of [false, true]) {
          const gesture = drag(buttons, alt, plainLeftOrbits);
          expect(gesture === 'none').toBe(buttons === 0);
        }
      }
    }
  });

  it('never resolves a modified drag to the view-owned pick', () => {
    for (const buttons of COMBINATIONS) {
      expect(drag(buttons, true, false)).not.toBe('pick');
    }
    // Only a lone plain left is the view's.
    for (const buttons of COMBINATIONS) {
      if (buttons !== LEFT_BUTTON) {
        expect(drag(buttons, false, false)).not.toBe('pick');
      }
    }
  });

  it('ignores bits that are not the three buttons', () => {
    // A pen's barrel and eraser bits (8, 32) ride along in `buttons` and mean nothing here.
    expect(drag(RIGHT_BUTTON | 8 | 32, false)).toBe('fly-look');
    expect(drag(8, false)).toBe('none');
  });

  it('names the gestures the camera drives', () => {
    expect(isCameraDrag('fly-look')).toBe(true);
    expect(isCameraDrag('orbit')).toBe(true);
    expect(isCameraDrag('dolly')).toBe(true);
    expect(isCameraDrag('pan')).toBe(true);
    expect(isCameraDrag('pick')).toBe(false);
    expect(isCameraDrag('none')).toBe(false);
  });
});

describe('resolveWheel', () => {
  it('dollies when not flying and changes fly speed when it is', () => {
    expect(resolveWheel(-100, false)).toEqual({ kind: 'dolly', steps: -1 });
    expect(resolveWheel(100, false)).toEqual({ kind: 'dolly', steps: 1 });
    expect(resolveWheel(-100, true)).toEqual({ kind: 'fly-speed', steps: 1 });
    expect(resolveWheel(100, true)).toEqual({ kind: 'fly-speed', steps: -1 });
  });

  it('reads the sign only, so a line-unit and a pixel-unit wheel agree', () => {
    expect(resolveWheel(3, false).steps).toBe(resolveWheel(240, false).steps);
    expect(resolveWheel(-0.01, true).steps).toBe(resolveWheel(-1200, true).steps);
  });

  it('is a no-op for a wheel that did not move', () => {
    expect(resolveWheel(0, false).steps).toBe(0);
    expect(resolveWheel(0, true).steps).toBe(0);
  });
});

describe('key bindings', () => {
  it('maps the fly keys by physical position', () => {
    expect(flyAction('KeyW')).toBe('forward');
    expect(flyAction('KeyS')).toBe('back');
    expect(flyAction('KeyA')).toBe('left');
    expect(flyAction('KeyD')).toBe('right');
    expect(flyAction('KeyQ')).toBe('down');
    expect(flyAction('KeyE')).toBe('up');
    expect(flyAction('KeyZ')).toBe('fov-narrow');
    expect(flyAction('KeyC')).toBe('fov-widen');
    expect(flyAction('KeyB')).toBeNull();
    // `code`, not `key`: the character a layout produces is never consulted.
    expect(flyAction('w')).toBeNull();
    expect(flyAction('W')).toBeNull();
  });

  it('maps the three standalone commands and nothing else', () => {
    expect(viewCommand('KeyF')).toBe('frame');
    expect(viewCommand('Home')).toBe('home');
    expect(viewCommand('KeyG')).toBe('toggle-game');
    expect(viewCommand('KeyW')).toBeNull();
    expect(viewCommand('End')).toBeNull();
  });

  it('never reads one key as both a command and a fly action', () => {
    for (const code of ['KeyF', 'Home', 'KeyG']) {
      expect(flyAction(code)).toBeNull();
    }
    for (const code of ['KeyW', 'KeyS', 'KeyA', 'KeyD', 'KeyQ', 'KeyE', 'KeyZ', 'KeyC']) {
      expect(viewCommand(code)).toBeNull();
    }
  });
});

describe('flyKeysFrom', () => {
  it('is empty for no keys, and ignores what is not movement', () => {
    expect(flyKeysFrom(new Set())).toEqual(NO_FLY_KEYS);
    expect(flyKeysFrom(new Set(['KeyZ', 'KeyC', 'KeyF', 'ShiftLeft']))).toEqual(NO_FLY_KEYS);
  });

  it('collects every held movement key', () => {
    expect(flyKeysFrom(new Set(['KeyW', 'KeyD', 'KeyE']))).toEqual({
      ...NO_FLY_KEYS,
      forward: true,
      right: true,
      up: true,
    });
  });

  it('reports opposites as both held — the movement math cancels them, not this table', () => {
    expect(flyKeysFrom(new Set(['KeyW', 'KeyS']))).toEqual({
      ...NO_FLY_KEYS,
      forward: true,
      back: true,
    });
  });
});

describe('fovDirection', () => {
  it('narrows on Z, widens on C, and cancels when both are held', () => {
    expect(fovDirection(new Set(['KeyZ']))).toBe(-1);
    expect(fovDirection(new Set(['KeyC']))).toBe(1);
    expect(fovDirection(new Set(['KeyZ', 'KeyC']))).toBe(0);
    expect(fovDirection(new Set(['KeyW', 'ShiftLeft']))).toBe(0);
    expect(fovDirection(new Set())).toBe(0);
  });
});
