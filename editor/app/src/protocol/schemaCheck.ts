// The editor's check that the engine it is attached to speaks the protocol its types were generated
// from. A mismatch is a warning, never a refusal: the session stays usable and the caller shows the
// diagnostic persistently. Pure so it is testable without a socket or a render.

import type { DiagnosticView } from '../viewer/diagnostic';

/** `null` when the engine's announced schema hash is the one the editor's types came from. */
export function schemaMismatch(expected: string, announced: string): DiagnosticView | null {
  if (expected === announced) return null;
  return {
    code: 'L3003',
    message: `schema drift: the engine announces schema ${announced}, but this editor's protocol types were generated from ${expected}`,
    help: "rebuild the game against this checkout's engine, or regenerate the editor types (npm run generate in editor/protocol) from the engine's schema snapshot; commands whose shapes changed may fail until then",
    related: [],
  };
}
