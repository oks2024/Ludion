// The model roster offered by the drawer's spawn picker, passed straight to `claude --model`
// (claude entries) or used to pick the opencode spawn path. Claude ids are the CLI's family
// aliases, which always resolve to the latest release, so a new model ships with no edit here;
// the labels are what the picker cards show.

export type TerminalKind = 'claude' | 'opencode';

export interface TerminalModel {
  readonly id: string;
  readonly kind: TerminalKind;
  readonly label: string;
  readonly sub: string;
}

export const TERMINAL_MODELS: readonly TerminalModel[] = [
  { id: 'fable', kind: 'claude', label: 'Fable', sub: 'hardest tasks' },
  { id: 'opus', kind: 'claude', label: 'Opus', sub: 'complex reasoning' },
  { id: 'sonnet', kind: 'claude', label: 'Sonnet', sub: 'everyday work' },
  { id: 'haiku', kind: 'claude', label: 'Haiku', sub: 'fast · simple' },
  { id: 'opencode-llamacpp', kind: 'opencode', label: 'opencode', sub: 'llama.cpp · local' },
];

export const DEFAULT_MODEL_ID = 'opus';

/** A stored versioned id (`claude-haiku-4-5-20251001`, the roster's pre-alias form) reads
 *  as its family alias, so a returning user keeps their model family. */
const VERSIONED_ID = /^claude-(fable|opus|sonnet|haiku)-/;

/** The last-used model id from storage, or the default when it is absent or no longer offered. */
export function resolveModelId(stored: string | null): string {
  const id = stored?.match(VERSIONED_ID)?.[1] ?? stored;
  return id !== null && TERMINAL_MODELS.some((m) => m.id === id) ? id : DEFAULT_MODEL_ID;
}

/** Display name for a model id; unknown ids pass through so a hand-set id still reads. */
export function modelLabel(id: string): string {
  return TERMINAL_MODELS.find((m) => m.id === id)?.label ?? id;
}

/** Which spawn path a model id takes; unknown ids default to `claude` (matches `modelLabel`'s
 *  passthrough — a hand-set id still spawns the claude CLI, the roster's only prior behavior). */
export function modelKind(id: string): TerminalKind {
  return TERMINAL_MODELS.find((m) => m.id === id)?.kind ?? 'claude';
}
