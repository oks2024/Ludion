import type { Command } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import { ARCHETYPES, spawnCommand, type Archetype } from './palette';

function paramsOf(command: Command): Record<string, unknown> {
  return (command as { params?: Record<string, unknown> }).params ?? {};
}

describe('palette archetypes', () => {
  it('is exactly the curated list, in order', () => {
    expect(ARCHETYPES.map((a) => a.id)).toEqual([
      'sprite',
      'mesh',
      'light',
      'spot',
      'sun',
      'camera',
      'text',
      'hud-panel',
      'ui-list',
      'particles',
      'reflection-probe',
    ]);
  });

  it('gives every archetype a Transform', () => {
    for (const a of ARCHETYPES) {
      expect(a.components).toHaveProperty('Transform');
    }
  });

  it('never authors Name as a component (it rides the spawn `name` param instead)', () => {
    for (const a of ARCHETYPES) {
      expect(a.components).not.toHaveProperty('Name');
      expect(a.name).not.toBe('');
    }
  });

  it('references no external asset path (nothing that could wedge the render loop)', () => {
    const sprite = ARCHETYPES.find((a) => a.id === 'sprite')!;
    // A textureless Sprite is a plain white quad — no missing-texture failure (L1101).
    expect((sprite.components.Sprite as Record<string, unknown>).texture).toBeUndefined();
    const mesh = ARCHETYPES.find((a) => a.id === 'mesh')!;
    expect(mesh.components).not.toHaveProperty('Sprite');
  });

  it('ships the Spot light archetype with a valid cone (inner <= outer) and no GiDynamic marker', () => {
    const spot = ARCHETYPES.find((a) => a.id === 'spot')!;
    const light = spot.components.SpotLight as { inner_angle: number; outer_angle: number };
    expect(light.inner_angle).toBeLessThanOrEqual(light.outer_angle);
    expect(spot.components).not.toHaveProperty('GiDynamic');
  });

  it('ships the Reflection probe archetype with a positive box extent and a non-negative blend', () => {
    const probe = ARCHETYPES.find((a) => a.id === 'reflection-probe')!;
    const p = probe.components.ReflectionProbe as { shape: { box: { extent: number[] } }; blend: number };
    expect(p.shape.box.extent).toHaveLength(3);
    for (const e of p.shape.box.extent) expect(e).toBeGreaterThan(0);
    expect(p.blend).toBeGreaterThanOrEqual(0);
  });

  it('ships the Text archetype with non-empty required text', () => {
    const text = ARCHETYPES.find((a) => a.id === 'text')!;
    expect((text.components.Text as { text?: unknown }).text).toBe('TEXT');
  });

  it('ships the Menu list archetype with non-empty items, one disabled', () => {
    const uiList = ARCHETYPES.find((a) => a.id === 'ui-list')!;
    const items = (uiList.components.UiList as { items: Array<{ text: string; enabled: boolean }> }).items;
    expect(items.length).toBeGreaterThan(0);
    expect(items.some((item) => item.enabled)).toBe(true);
    expect(items.some((item) => !item.enabled)).toBe(true);
  });
});

describe('spawnCommand', () => {
  it('builds an entity.spawn with the name param and default components', () => {
    const archetype: Archetype = { id: 'x', label: 'X', name: 'X', components: { Transform: {} } };
    const command = spawnCommand(archetype);
    expect((command as { method: string }).method).toBe('entity.spawn');
    expect(paramsOf(command)).toEqual({ name: 'X', components: { Transform: {} } });
  });
});
