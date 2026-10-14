// One graded band — `lift`, `gamma` or `gain` — presented the way it is directed: one intensity and
// one colour, over the `[f32; 3]` the engine stores and renders.
//
// WHY THE CHAIN OWNS IT AND NOT THE SHARED WIDGET LAYER. `FieldWidget` renders what a schema says a
// field IS: three numbers under one declared range, hence three tracks. That this particular triple
// is an ASC CDL band whose channels read as warmth and green is knowledge about one component, and
// `editor/art/` is where that knowledge lives — the inspector must keep working for a component it
// has never heard of.
//
// THE STORED TRIPLE IS THE SOURCE OF TRUTH. The split is derived from it on every render and never
// held as state. The only thing a gesture keeps is which split ITS writes are rebuilt from
// (`GradeGesture`), because the write path quantizes and re-deriving from it would creep the two
// handles nobody is touching.
//
// THE ROWS ARE `RangeInput`s — the same control every other slider in the editor is, so the drag
// cadence, the clamp, the click-to-type readout and the one-undo-entry-per-gesture rule are not
// restated here. Each row is one write of the WHOLE triple at the band's own field path; nothing
// commits per channel.

import { useRef } from 'react';
import { RangeInput, type FieldCommit } from '../../viewer/FieldWidgets';
import type { FieldPath } from '../../viewer/inspect-fields';
import { decompose, type GradeBand } from './grade';
import { GradeGesture, type GradeRow } from './gradeControl';

export function GradeBandControl({
  band,
  rows,
  label,
  path,
  value,
  commit,
}: {
  readonly band: GradeBand;
  readonly rows: readonly GradeRow[];
  /** The field's own label, which each row's accessible name is built from. */
  readonly label: string;
  /** The band's field path — every row writes the whole triple here. */
  readonly path: FieldPath;
  readonly value: unknown;
  readonly commit: FieldCommit;
}) {
  const gesture = useRef(new GradeGesture()).current;
  // A value that is not three finite numbers decomposes to the band's neutral rather than throwing,
  // so a malformed component still renders three handles a human can drag back out of it.
  const stored: readonly number[] = Array.isArray(value) ? (value as readonly number[]) : [];
  const split = decompose(band, stored);

  return (
    <div className="field-vector field-vector-stack">
      {rows.map((row) => (
        <div className="field-vector-row" key={row.axis}>
          {/* The row's name is said again in the track's accessible name, so it is decoration here. */}
          <span className="field-channel ad-grade-axis" aria-hidden="true">
            {row.axis}
          </span>
          <RangeInput
            range={row.range}
            integer={false}
            label={`${label} ${row.axis}`}
            value={split[row.axis]}
            send={(next, cadence) =>
              commit(path, gesture.write(band, row.axis, next, split, cadence), cadence)
            }
          />
        </div>
      ))}
    </div>
  );
}
