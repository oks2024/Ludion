// Maps a browser keyboard event to an `input.inject` command payload.
//
// The engine's input model (ludion-sim's `InputEvent` / `Key`) is deliberately small and
// layout-independent: `{"kind": "key_down" | "key_up", "key": "<name>"}` where the key name is the
// abstract variant name (`"W"`, `"Left"`, `"Space"`, `"Digit1"`). The browser reports physical keys
// via `KeyboardEvent.code` (`"KeyW"`, `"ArrowLeft"`, `"Space"`, `"Digit1"`), which is exactly the
// layout-independent identity we want — so this maps `code`, never `key`. Unmapped keys return
// `null` (the engine only knows the milestone-minimal set), and OS auto-repeat is suppressed so a
// held key produces exactly one `key_down` edge, mirroring the engine's own no-repeat semantics.
//
// Shift, Ctrl and Alt are folded: left and right are one engine key, held while either side is down.
// `HeldKeys` remembers the sides so a second side or one of two releases sends no spurious edge, and
// records every forwarded key still down so a focus loss can release exactly those (`release()`).

import type { InputInjectParams } from '@ludion/protocol';

/** The sim key names the engine accepts (mirrors `ludion_sim::Key`'s serde form). */
export type EngineKey =
  | 'A' | 'B' | 'C' | 'D' | 'E' | 'F' | 'G' | 'H' | 'I' | 'J' | 'K' | 'L' | 'M'
  | 'N' | 'O' | 'P' | 'Q' | 'R' | 'S' | 'T' | 'U' | 'V' | 'W' | 'X' | 'Y' | 'Z'
  | 'Digit0' | 'Digit1' | 'Digit2' | 'Digit3' | 'Digit4'
  | 'Digit5' | 'Digit6' | 'Digit7' | 'Digit8' | 'Digit9'
  | 'Up' | 'Down' | 'Left' | 'Right'
  | 'Space' | 'Enter' | 'Escape'
  | 'Shift' | 'Ctrl' | 'Alt';

const ARROWS: Readonly<Record<string, EngineKey>> = {
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
};

const NAMED: Readonly<Record<string, EngineKey>> = {
  Space: 'Space',
  Enter: 'Enter',
  Escape: 'Escape',
  ShiftLeft: 'Shift',
  ShiftRight: 'Shift',
  ControlLeft: 'Ctrl',
  ControlRight: 'Ctrl',
  AltLeft: 'Alt',
  AltRight: 'Alt',
};

/**
 * Translates a `KeyboardEvent.code` to the engine's abstract key name, or `null` if the engine has
 * no such key. Handles the four families the milestone set covers: `KeyA`–`KeyZ`, `Digit0`–`Digit9`,
 * the four arrows, Space/Enter/Escape, and either side of Shift/Ctrl/Alt (folded by `HeldKeys`).
 */
export function codeToEngineKey(code: string): EngineKey | null {
  if (/^Key[A-Z]$/.test(code)) {
    return code.slice(3) as EngineKey;
  }
  if (/^Digit[0-9]$/.test(code)) {
    return code as EngineKey;
  }
  return ARROWS[code] ?? NAMED[code] ?? null;
}

/** A keyboard edge to translate: which physical key, whether it is a press or a release, and
 *  whether the browser flagged it as an OS auto-repeat. */
export interface KeyEdge {
  readonly code: string;
  readonly type: 'keydown' | 'keyup';
  readonly repeat: boolean;
}

/**
 * The editor's held-key record. Tracks which physical sides of each modifier are down, so left and
 * right fold into one key, and which engine keys were forwarded down and not yet up, so a focus loss
 * (`release()`) can send the up edges the browser will never deliver.
 */
export class HeldKeys {
  private readonly down = new Map<string, Set<string>>();
  private readonly held = new Set<EngineKey>();

  /** Bookkeeping for a forwarded edge; never filters what is forwarded. */
  note(key: EngineKey, kind: 'key_down' | 'key_up'): void {
    if (kind === 'key_down') {
      this.held.add(key);
    } else {
      this.held.delete(key);
    }
  }

  /** The key_up injects for everything still held; forgets the keys and the modifier sides. */
  release(): InputInjectParams[] {
    const params = keyReleaseInjects(this.held);
    this.held.clear();
    this.down.clear();
    return params;
  }

  /** Whether this edge changes the folded key: the first side down, or the last side up. */
  changes(key: EngineKey, code: string, type: 'keydown' | 'keyup'): boolean {
    const sides = this.down.get(key) ?? new Set<string>();
    this.down.set(key, sides);
    const wasDown = sides.size > 0;
    if (type === 'keydown') {
      sides.add(code);
    } else {
      sides.delete(code);
    }
    return wasDown !== sides.size > 0;
  }
}

const MODIFIER_KEYS: ReadonlySet<EngineKey> = new Set<EngineKey>(['Shift', 'Ctrl', 'Alt']);

/**
 * Builds the `input.inject` params for a keyboard edge, or `null` when nothing should be sent:
 * an unmapped key, an auto-repeat `keydown` (the engine treats a re-asserted held key as no new
 * edge, so forwarding repeats would be pure noise), or a modifier edge that leaves the folded key
 * unchanged. The event carries no `frame`, so the engine applies it at the next tick boundary.
 */
export function keyEdgeToInject(edge: KeyEdge, fold: HeldKeys): InputInjectParams | null {
  if (edge.type === 'keydown' && edge.repeat) {
    return null;
  }
  const key = codeToEngineKey(edge.code);
  if (key === null) {
    return null;
  }
  if (MODIFIER_KEYS.has(key) && !fold.changes(key, edge.code, edge.type)) {
    return null;
  }
  const kind = edge.type === 'keydown' ? 'key_down' : 'key_up';
  fold.note(key, kind);
  return { event: { kind, key } };
}

/** The key-up injects that release a set of held keys, in iteration order — sent when the viewer or
 *  preview loses focus or play mode ends mid-hold so the engine's held-state can't stick down. */
export function keyReleaseInjects(held: Iterable<EngineKey>): InputInjectParams[] {
  return [...held].map((key) => ({ event: { kind: 'key_up', key } }));
}

/**
 * Whether a keyboard event originated inside a text-editing surface, where the browser's own editing
 * (undo, caret motion) must win over the editor's window-level shortcuts (Ctrl+Z/Y, the W/E/R gizmo
 * modes). Shared by every window-level key handler so the rule lives in exactly one place.
 */
export function isTextEditingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tag = target.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable;
}
