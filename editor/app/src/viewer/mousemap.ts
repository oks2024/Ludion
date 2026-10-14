// Maps browser pointer events to `input.inject` command payloads, the mouse sibling of `keymap.ts`.
//
// The engine's mouse model (ludion-sim's `InputEvent`) speaks render-target pixels (origin top-left,
// +Y down) — the same space the streamed frame lives in — and a pinned bit-exact wire form: a move is
// `{"kind": "mouse_move", "position_bits": [<u32>, <u32>]}` where each component is the IEEE-754 bit
// pattern of the `f32` position (analog input must replay bit-exact, so it never crosses serde as a
// JSON number), and a button edge is `{"kind": "mouse_down" | "mouse_up", "button": "left" | "right"
// | "middle"}`. The client→render-target coordinate mapping is NOT here: it is `picking.ts`'s
// `clientToEnginePoint`, reused verbatim so the game's cursor lands on the same pixel a pick would.

import type { InputInjectParams } from '@ludion/protocol';
import type { Point } from './picking';

/** The mouse button names the engine accepts (mirrors `ludion_sim::MouseButton`'s snake_case serde). */
export type EngineMouseButton = 'left' | 'right' | 'middle';

// A one-element f32 view whose bytes we reinterpret as a u32 — the JS equivalent of Rust's
// `f32::to_bits`, so the position matches the window host's `position[0].to_bits()` byte-for-byte.
const F32 = new Float32Array(1);
const U32 = new Uint32Array(F32.buffer);

/** The IEEE-754 bit pattern of `x` narrowed to `f32`, as an unsigned 32-bit integer. */
export function f32Bits(x: number): number {
  F32[0] = x;
  return U32[0];
}

/** Maps a DOM `PointerEvent.button` index to the engine's button name, or `null` for a button the
 *  milestone-minimal set has no name for (`0` left, `1` middle, `2` right; back/forward/eraser drop). */
export function pointerButtonName(button: number): EngineMouseButton | null {
  switch (button) {
    case 0:
      return 'left';
    case 1:
      return 'middle';
    case 2:
      return 'right';
    default:
      return null;
  }
}

/** Builds the `input.inject` params for a cursor move to `position` (already in render-target pixels),
 *  encoding the position as bit patterns in the engine's pinned `position_bits` shape. */
export function mouseMoveInject(position: Point): InputInjectParams {
  return { event: { kind: 'mouse_move', position_bits: [f32Bits(position.x), f32Bits(position.y)] } };
}

/** Builds the `input.inject` params for a mouse button transition. */
export function mouseButtonInject(button: EngineMouseButton, edge: 'down' | 'up'): InputInjectParams {
  return { event: { kind: edge === 'down' ? 'mouse_down' : 'mouse_up', button } };
}

/** The button-up injects that release a set of held buttons, in iteration order — sent when the pointer
 *  leaves the viewport or play mode ends mid-hold so the engine's held-state can't stick down. */
export function releaseInjects(held: Iterable<EngineMouseButton>): InputInjectParams[] {
  return [...held].map((button) => mouseButtonInject(button, 'up'));
}
