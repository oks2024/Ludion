import { describe, expect, it } from 'vitest';
import { DEFAULT_MODEL_ID, modelKind, modelLabel, resolveModelId, TERMINAL_MODELS } from './models';

describe('resolveModelId', () => {
  it('keeps a stored id that is still offered', () => {
    expect(resolveModelId('haiku')).toBe('haiku');
  });
  it('maps a stored versioned id to its family alias', () => {
    expect(resolveModelId('claude-haiku-4-5-20251001')).toBe('haiku');
    expect(resolveModelId('claude-opus-4-8[1m]')).toBe('opus');
    expect(resolveModelId('claude-sonnet-5')).toBe('sonnet');
    expect(resolveModelId('claude-fable-5')).toBe('fable');
  });
  it('falls back to the default for null and for a retired id', () => {
    expect(resolveModelId(null)).toBe(DEFAULT_MODEL_ID);
    expect(resolveModelId('claude-gone-1')).toBe(DEFAULT_MODEL_ID);
  });
  it('offers the default as one of the roster entries', () => {
    expect(TERMINAL_MODELS.some((m) => m.id === DEFAULT_MODEL_ID)).toBe(true);
  });
  it('accepts the opencode roster entry', () => {
    expect(resolveModelId('opencode-llamacpp')).toBe('opencode-llamacpp');
  });
});

describe('modelLabel', () => {
  it('names every offered model', () => {
    for (const model of TERMINAL_MODELS) {
      expect(modelLabel(model.id)).toBe(model.label);
    }
  });
  it('passes an unknown id through', () => {
    expect(modelLabel('claude-gone-1')).toBe('claude-gone-1');
  });
});

describe('modelKind', () => {
  it('reports the kind tagged on every roster entry', () => {
    for (const model of TERMINAL_MODELS) {
      expect(modelKind(model.id)).toBe(model.kind);
    }
  });
  it('falls back to claude for an unknown id', () => {
    expect(modelKind('claude-gone-1')).toBe('claude');
  });
});
