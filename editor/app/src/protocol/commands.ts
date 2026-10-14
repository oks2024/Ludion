// Typed builders for commands whose params interfaces would otherwise need a cast. The generated
// `Command` members carry an index signature, which an interface-typed value (a Transform, an
// entity id read off a query) lacks, so it is not assignable; a fresh object literal is.

import type { Command, ComponentSetParams } from '@ludion/protocol';

/** A fresh `{ index, generation }` literal, assignable where the wire's entity id is expected. */
export function entityRef(e: { readonly index: number; readonly generation: number }) {
  return { index: e.index, generation: e.generation };
}

/** The `component.set` command for `component` on `entity`. */
export function componentSet(
  entity: { readonly index: number; readonly generation: number },
  component: string,
  value: object,
): Command {
  return {
    method: 'component.set',
    params: { entity: entityRef(entity), component, value: value as ComponentSetParams['value'] },
  };
}
