// Reading a `scene.save` outcome: the conflict refusal and the checker findings a successful save
// carries. Pure, so the SessionStrip's branches are testable without React.

import type { ResponseBody } from '@ludion/protocol';
import { fromWire, type DiagnosticView } from '../viewer/diagnostic';

/** The engine's refusal when the destination differs from what this session last loaded or saved. */
export const SCENE_SAVE_CONFLICT_CODE = 'L3058';

export function isSaveConflict(view: DiagnosticView): boolean {
  return view.code === SCENE_SAVE_CONFLICT_CODE;
}

/** The one finding to show after a successful save: an error over a warning, with the count in the
 *  message when there are several. Null when the save was clean. */
export function saveFindings(body: ResponseBody): DiagnosticView | null {
  const list = body.kind === 'scene_saved' ? (body.diagnostics ?? []) : [];
  if (list.length === 0) {
    return null;
  }
  const pick = list.find((d) => d.severity !== 'warning') ?? list[0];
  if (pick === undefined) {
    return null;
  }
  const view = fromWire(pick);
  return list.length === 1 ? view : { ...view, message: `${view.message} (${list.length} findings)` };
}
