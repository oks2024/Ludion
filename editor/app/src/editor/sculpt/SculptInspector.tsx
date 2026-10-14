// The Sculpt inspector: one slider per number in the open `.sculpt.yaml`, grouped by the part it
// belongs to.
//
// IT IS A VIEW OF THE FILE, NOT OF A SCHEMA. The entity inspector's widgets (`viewer/FieldWidgets`)
// render `FieldDescriptor`s derived from a component's JSON Schema and commit through the wire; this
// one renders positions in a text document and commits by splicing that text. Nothing is shared but
// the look, so it reuses those stylesheet classes and none of the code.
//
// PURE OVER `doc`. Every decision about layout, naming and the value a drag may write lives in
// `rows.ts` and `ranges.ts`, tested without a DOM; this file turns them into controls and reports
// `(pointer, value, commit)` upwards. It holds no draft of its own — the working document IS the
// live value, because `useSculptEdit` applies an edit before the disk sees it.
//
// A DRAG IS LIVE, AND IT ENDS HONESTLY. `onChange` fires all the way through the travel so the stage
// re-meshes while the handle moves (the whole point of a slider over a number box), and the pointer
// release re-reports the same value as a COMMIT, so the file always ends the gesture holding exactly
// what the human left on screen — the posture the camera throttle and `save.ts` both take.

import { type ChangeEvent, type PointerEvent } from 'react';
import type { SculptDoc } from './doc';
import { sliderRange } from './ranges';
import { inspectorGroups, quantize } from './rows';

export interface SculptInspectorProps {
  /** The working document — the inspector redraws from it after every edit. */
  readonly doc: SculptDoc;
  /** `commit` is the end of a gesture; see the module doc. */
  readonly onChange: (pointer: string, value: number, commit: boolean) => void;
  /** No host, or an unresolved conflict: every track greys out. */
  readonly disabled?: boolean;
}

/** The readout's text. `String` is shortest round-trip for a double, which is the same rule the
 *  file's own writer follows — so what the row shows is what a commit would write. */
function readout(value: number): string {
  return String(value);
}

export function SculptInspector({ doc, onChange, disabled = false }: SculptInspectorProps) {
  const groups = inspectorGroups(doc);

  return (
    <div className="component-fields">
      {groups.map((group) => (
        <div key={group.id} className="fx-group">
          <div className="fx-group-head">{group.head}</div>
          {group.rows.map((row) => {
            const range = sliderRange(row.field, doc);
            // The travel must be able to hold the value that is already there: a hand-authored
            // number outside the authored range would otherwise snap the moment the row rendered.
            const min = Math.min(range.min, row.field.value);
            const max = Math.max(range.max, row.field.value);
            const report = (event: { currentTarget: { value: string } }, commit: boolean): void => {
              onChange(row.field.pointer, quantize(Number(event.currentTarget.value), { ...range, min, max }), commit);
            };
            return (
              <div key={row.field.pointer} className="field">
                <span className="field-label" title={row.field.pointer}>
                  {row.label}
                </span>
                <div className="field-slider">
                  <input
                    type="range"
                    className="field-range"
                    min={min}
                    max={max}
                    step={range.step}
                    value={row.field.value}
                    disabled={disabled}
                    aria-label={`${group.head} ${row.label}`}
                    onChange={(event: ChangeEvent<HTMLInputElement>) => report(event, false)}
                    onPointerUp={(event: PointerEvent<HTMLInputElement>) => report(event, true)}
                    // A pointer that ends anywhere but on the track — cancelled, or focus taken by
                    // another control — still ends the gesture, so a throttled write can never be
                    // the last word.
                    onPointerCancel={(event: PointerEvent<HTMLInputElement>) => report(event, true)}
                    onBlur={(event) => report(event, true)}
                    onKeyUp={(event) => report(event, true)}
                  />
                  <span className="field-input field-readout num">{readout(row.field.value)}</span>
                </div>
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}
