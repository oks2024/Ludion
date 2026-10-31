import { describe, expect, it } from 'vitest';
import { codeToEngineKey, keyEdgeToInject, ModifierFold } from './keymap';

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

  it('returns null for keys the engine does not know', () => {
    expect(codeToEngineKey('Tab')).toBeNull();
    expect(codeToEngineKey('CapsLock')).toBeNull();
    expect(codeToEngineKey('Numpad1')).toBeNull();
    expect(codeToEngineKey('F5')).toBeNull();
  });
});

describe('keyEdgeToInject', () => {
  it('builds a key_down inject for a fresh press', () => {
    expect(keyEdgeToInject({ code: 'KeyW', type: 'keydown', repeat: false }, new ModifierFold())).toEqual({
      event: { kind: 'key_down', key: 'W' },
    });
  });

  it('builds a key_up inject for a release', () => {
    expect(keyEdgeToInject({ code: 'ArrowLeft', type: 'keyup', repeat: false }, new ModifierFold())).toEqual({
      event: { kind: 'key_up', key: 'Left' },
    });
  });

  it('suppresses OS auto-repeat keydowns', () => {
    expect(keyEdgeToInject({ code: 'KeyD', type: 'keydown', repeat: true }, new ModifierFold())).toBeNull();
  });

  it('still forwards a keyup even if flagged repeat (releases never repeat, but be safe)', () => {
    expect(keyEdgeToInject({ code: 'KeyD', type: 'keyup', repeat: true }, new ModifierFold())).toEqual({
      event: { kind: 'key_up', key: 'D' },
    });
  });

  it('returns null for an unmapped key regardless of edge', () => {
    expect(keyEdgeToInject({ code: 'Tab', type: 'keydown', repeat: false }, new ModifierFold())).toBeNull();
    expect(keyEdgeToInject({ code: 'Tab', type: 'keyup', repeat: false }, new ModifierFold())).toBeNull();
  });

  it('folds two shift sides into one held key', () => {
    const fold = new ModifierFold();
    const edge = (code: string, type: 'keydown' | 'keyup') => keyEdgeToInject({ code, type, repeat: false }, fold);
    expect(edge('ShiftLeft', 'keydown')).toEqual({ event: { kind: 'key_down', key: 'Shift' } });
    expect(edge('ShiftRight', 'keydown')).toBeNull();
    expect(edge('ShiftLeft', 'keyup')).toBeNull();
    expect(edge('ShiftRight', 'keyup')).toEqual({ event: { kind: 'key_up', key: 'Shift' } });
    expect(edge('ShiftRight', 'keyup')).toBeNull();
  });
});
