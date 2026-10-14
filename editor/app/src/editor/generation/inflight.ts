// Which generation verbs are in flight, and whether a finished verb's result still belongs on screen.
//
// A verb is a subprocess that takes minutes, and the view lets the human switch recipe meanwhile. One
// "busy" slot would be cleared by whichever verb finished first (re-enabling Generate under a live
// run), and a result would land on whatever recipe is selected by then. So the view counts verbs and
// stamps each with the scope it started in.

import type { GenVerb } from '../../shell/genHost';

/** In-flight verbs by kind. Immutable: every transition returns a new value. */
export type BusyCounts = Readonly<Partial<Record<GenVerb, number>>>;

export const IDLE: BusyCounts = {};

export function beginVerb(counts: BusyCounts, verb: GenVerb): BusyCounts {
  return { ...counts, [verb]: (counts[verb] ?? 0) + 1 };
}

/** One fewer `verb` in flight, never below zero. */
export function endVerb(counts: BusyCounts, verb: GenVerb): BusyCounts {
  return { ...counts, [verb]: Math.max(0, (counts[verb] ?? 0) - 1) };
}

/** Whether `verb` is in flight, or any verb when omitted. */
export function isBusy(counts: BusyCounts, verb?: GenVerb): boolean {
  if (verb !== undefined) {
    return (counts[verb] ?? 0) > 0;
  }
  return Object.values(counts).some((n) => n > 0);
}

/** What a verb's result is about: the project and recipe, and for `accept` also the run. */
export interface VerbScope {
  readonly projectDir: string;
  readonly recipe: string | null;
  readonly runId?: string | null;
}

/** Whether a verb started in `captured` may still show its result in `current`. */
export function isCurrent(captured: VerbScope, current: VerbScope): boolean {
  return (
    captured.projectDir === current.projectDir &&
    captured.recipe === current.recipe &&
    (captured.runId === undefined || captured.runId === current.runId)
  );
}
