import { describe, expect, it } from 'vitest';
import { componentSet } from './commands';

interface Ref {
  index: number;
  generation: number;
}

describe('componentSet', () => {
  it('builds a plain component.set command from an interface-typed entity', () => {
    const entity: Ref = { index: 4, generation: 2 };
    expect(componentSet(entity, 'Transform', { translation: [1, 2, 3] })).toEqual({
      method: 'component.set',
      params: {
        entity: { index: 4, generation: 2 },
        component: 'Transform',
        value: { translation: [1, 2, 3] },
      },
    });
  });
});
