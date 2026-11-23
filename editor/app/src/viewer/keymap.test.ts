import { describe, expect, it } from 'vitest';
import { codeToEngineKey, HeldKeys, keyEdgeToInject, keyReleaseInjects } from './keymap';

describe('codeToEngineKey', () => {
  it('maps letter codes by stripping the Key prefix', () => {
    expect(codeToEngineKey('KeyW')).toBe('W');
    expect(codeToEngineKey('KeyA')).toBe('A');
    expect(codeToEngineKey('KeyZ')).toBe('Z');
  });

  it('passes digit codes through unchanged', () => {
    expect(codeToEngineKey('Digit0')).toBe('Digit0');
    expect(codeToEngineKey('Digit9')).toBe('Digit9');
  });

  it('renames the arrow codes to the engine names', () => {
    expect(codeToEngineKey('ArrowUp')).toBe('Up');
    expect(codeToEngineKey('ArrowDown')).toBe('Down');
    expect(codeToEngineKey('ArrowLeft')).toBe('Left');
    expect(codeToEngineKey('ArrowRight')).toBe('Right');
  });

  it('maps the named control keys', () => {
    expect(codeToEngineKey('Space')).toBe('Space');
    expect(codeToEngineKey('Enter')).toBe('Enter');
    expect(codeToEngineKey('Escape')).toBe('Escape');
  });

  it('folds both sides of each modifier to one name', () => {
    for (const [code, name] of [
      ['ShiftLeft', 'Shift'], ['ShiftRight', 'Shift'],
      ['ControlLeft', 'Ctrl'], ['ControlRight', 'Ctrl'],
      ['AltLeft', 'Alt'], ['AltRight', 'Alt'],
    ] as const) {
      expect(codeToEngineKey(code)).toBe(name);
    }
  });

  it('maps the appended keys by their identical DOM code', () => {
    for (const code of ['Tab', 'Backspace', 'CapsLock', 'F1', 'F5', 'F12', 'Minus', 'Backquote',
      'Slash', 'Insert', 'Delete', 'PageDown', 'Numpad0', 'Numpad9', 'NumpadAdd', 'NumpadEnter']) {
      expect(codeToEngineKey(code)).toBe(code);
    }
  });

  it('returns null for keys the engine does not know', () => {
    for (const code of ['PrintScreen', 'NumLock', 'F13', 'F0', 'Numpad10', 'MetaLeft', 'ContextMenu']) {
      expect(codeToEngineKey(code)).toBeNull();
    }
  });
});

describe('keyEdgeToInject', () => {
  it('builds a key_down inject for a fresh press', () => {
    expect(keyEdgeToInject({ code: 'KeyW', type: 'keydown', repeat: false }, new HeldKeys())).toEqual({
      event: { kind: 'key_down', key: 'W' },
    });
  });

  it('builds a key_up inject for a release', () => {
    expect(keyEdgeToInject({ code: 'ArrowLeft', type: 'keyup', repeat: false }, new HeldKeys())).toEqual({
      event: { kind: 'key_up', key: 'Left' },
    });
  });

  it('suppresses OS auto-repeat keydowns', () => {
    expect(keyEdgeToInject({ code: 'KeyD', type: 'keydown', repeat: true }, new HeldKeys())).toBeNull();
  });

  it('still forwards a keyup even if flagged repeat (releases never repeat, but be safe)', () => {
    expect(keyEdgeToInject({ code: 'KeyD', type: 'keyup', repeat: true }, new HeldKeys())).toEqual({
      event: { kind: 'key_up', key: 'D' },
    });
  });

  it('returns null for an unmapped key regardless of edge', () => {
    expect(keyEdgeToInject({ code: 'PrintScreen', type: 'keydown', repeat: false }, new HeldKeys())).toBeNull();
    expect(keyEdgeToInject({ code: 'PrintScreen', type: 'keyup', repeat: false }, new HeldKeys())).toBeNull();
  });

  it('folds two shift sides into one held key', () => {
    const fold = new HeldKeys();
    const edge = (code: string, type: 'keydown' | 'keyup') => keyEdgeToInject({ code, type, repeat: false }, fold);
    expect(edge('ShiftLeft', 'keydown')).toEqual({ event: { kind: 'key_down', key: 'Shift' } });
    expect(edge('ShiftRight', 'keydown')).toBeNull();
    expect(edge('ShiftLeft', 'keyup')).toBeNull();
    expect(edge('ShiftRight', 'keyup')).toEqual({ event: { kind: 'key_up', key: 'Shift' } });
    expect(edge('ShiftRight', 'keyup')).toBeNull();
  });
});

describe('focus-loss release', () => {
  const press = (held: HeldKeys, code: string, type: 'keydown' | 'keyup' = 'keydown') =>
    keyEdgeToInject({ code, type, repeat: false }, held);
  const up = (key: string) => ({ event: { kind: 'key_up', key } });

  it('keyReleaseInjects gives one key_up per key in order', () => {
    expect(keyReleaseInjects(['W', 'Space'])).toEqual([up('W'), up('Space')]);
  });

  it('keyReleaseInjects of nothing is empty', () => {
    expect(keyReleaseInjects([])).toEqual([]);
  });

  it('releases the held keys once', () => {
    const held = new HeldKeys();
    press(held, 'KeyW');
    press(held, 'Space');
    expect(held.release()).toEqual([up('W'), up('Space')]);
    expect(held.release()).toEqual([]);
  });

  it('releases nothing when nothing is pressed', () => {
    expect(new HeldKeys().release()).toEqual([]);
  });

  it('does not re-release a key already released', () => {
    const held = new HeldKeys();
    press(held, 'KeyW');
    press(held, 'KeyW', 'keyup');
    expect(held.release()).toEqual([]);
  });

  it('releases both shift sides as one key_up and clears the side map', () => {
    const held = new HeldKeys();
    press(held, 'ShiftLeft');
    press(held, 'ShiftRight');
    expect(held.release()).toEqual([up('Shift')]);
    expect(press(held, 'ShiftLeft')).toEqual({ event: { kind: 'key_down', key: 'Shift' } });
  });
});
