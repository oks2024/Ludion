// The one widget layer over a `FieldDescriptor`: schema-derived field in, editable input out.
//
// WHY IT IS ITS OWN MODULE. `deriveFields` (`inspect-fields.ts`) turns a component's JSON Schema into
// typed field descriptors, and two surfaces now render them — the entity inspector's component forms
// and the Art Director's chain rows. A second implementation of the same six kinds would drift
// silently (the two would disagree about what a rejected draft does, or which edits are worth
// sending), so there is exactly one, and each caller keeps only its own chrome around it: the
// inspector its label/layout, the chain its dimming and shipped-default tag.
//
// THE DRAFT-AND-COMMIT CONTRACT, shared by every text-backed kind (number, string, JSON, and each
// component of a vector):
//   - the input holds a local text draft while focused; a refreshed `text` — both callers poll their
//     values at ~3 Hz — is applied ONLY when the field is not being edited, so a refresh can never
//     clobber what the human is typing;
//   - Enter (single-line) or blur commits; Escape reverts and sends nothing. Escape sets a `reverting`
//     ref *before* blurring, because the draft state update has not flushed when the synchronous blur
//     fires;
//   - leaving the field always re-syncs the draft to `text`, which is what restores the displayed
//     value after a rejected or reverted edit without a second code path;
//   - a commit that would not change the value is not sent. The comparison is per kind and is
//     deliberately semantic, not textual: `1.00` over a `1.0` and a reformatted JSON object are both
//     no-ops, and sending them would put an entry on the undo stack and dirty the scene for an edit
//     that did not happen.
//
// THE SLIDER CONTRACT (`RangeInput`, for a numeric field whose schema declares a range). The track is
// a second gesture producing the same commit, so it keeps both rules above: the no-op skip, and the
// refusal to be clobbered by a refresh — a gesture in progress displays its own value rather than the
// polled one, and its readout is the very same `DraftInput` when clicked into, so there is still one
// text path. Where the two deliberately differ: a DRAGGED value is quantized and clamped to the
// schema's legal bounds, because a gesture must not be able to author a value the validator would
// reject; a value TYPED into the readout is sent exactly as written, like any other number box, and
// lets the engine's own diagnostic speak.
//
// ONE TRACK PER LINE. A vector's components share one declared range, so a ranged vector is N tracks;
// side by side in the chain's column each would get a few pixels of travel, so they stack, one row per
// component, tagged with the channel that row writes (`channelName`). This is layout only — every
// component still commits to its own index, at the same cadences.
//
// A DRAG IS LIVE, AND IT IS ONE EDIT. The frame has to move while the handle does — that is the whole
// point of a slider over a number box — but `App.handleMutation` makes one undo entry per report, so
// the travel is sent at the `'preview'` cadence (throttled, latest-wins, no report) and only the value
// the gesture ended on is committed and reported. `commit` carries that distinction down from the
// caller's `useComponentCommit`; the gesture's own policy — the throttle, the always-send-the-final
// rule, the captured base Escape restores — is `SliderDrag` in `slider.ts`, so it is testable without
// a DOM.

import { Fragment, useEffect, useRef, useState, type ReactNode } from 'react';
import type { DiagnosticView } from './diagnostic';
import type { FieldDescriptor, FieldPath, FieldRange, VariantOption } from './inspect-fields';
import { niceStep, SliderDrag, widen, type SliderInstruction } from './slider';
import type { CommitCadence } from './useComponentCommit';

// --- the widgets' pure logic ---------------------------------------------------------------------
//
// Exported and directly tested (`FieldWidgets.logic.test.ts`): this repo runs vitest with
// `environment: 'node'` and collects `*.test.ts` only, so a widget's decision logic lives in pure
// functions rather than being reachable through DOM rendering. (The slider's arithmetic — travel,
// step, and what a drag is allowed to write — lives one module over, in `slider.ts`.)

/** Whether a value can carry a slider handle. A track has nowhere honest to put its handle for a
 *  missing or non-numeric value, so such a field degrades to the plain number box instead of the
 *  handle inventing a position — the same fail-soft posture `deriveField` takes for a malformed
 *  range. */
export function isSliderValue(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/** Channel names for the tracks of a vector: `R`/`G`/`B`/`A` at 3 and 4 wide, 1-based indices at any
 *  other width.
 *
 *  WHY RGB IS THE HONEST GUESS AT THOSE TWO WIDTHS, given that this widget knows a vector's length and
 *  nothing about its meaning: only a vector whose schema declares slider travel is ever named here,
 *  and travel is declared field by field in the engine. A position, a size, a world-space offset has
 *  no defensible ceiling to declare and so never gets a track; what is left at 3 and 4 wide is
 *  color-space triples — the ASC CDL grade `lift`/`gamma`/`gain` — and RGBA. A width outside that
 *  falls back to indices, which are always true if never evocative. A wrong guess costs a mislabelled
 *  row and never a wrong write: what a component commits to is its index, not its name.
 *
 *  The Art Director presents those same three triples as intensity-and-colour instead
 *  (`editor/art/GradeBandControl.tsx`), which is knowledge about one component and stays out of here;
 *  the inspector renders them channel by channel, and that is what these names are for. */
export function channelName(index: number, length: number): string {
  const rgba = 'RGBA';
  const named = (length === 3 || length === 4) && index >= 0 && index < length;
  return named ? rgba[index] : String(index + 1);
}

/** `<select>` value prefix for a data-carrying (`'asset'`-kind) variant, keyed by its property so two
 *  object variants (e.g. `MeshSource`'s `asset` and `voxel`, Phase 10 Wave 5) get distinct tags instead
 *  of colliding on one sentinel. Distinct from every unit value the engine actually emits (bare
 *  lowercase identifiers like `cube`), so it never collides with a real variant name either. */
const VARIANT_OBJECT_TAG_PREFIX = '__variant_object__:';

/** The `<select>` tag for the object variant with this property. */
export function variantObjectTag(property: string): string {
  return `${VARIANT_OBJECT_TAG_PREFIX}${property}`;
}

/** The property named by an object-variant tag, or `null` if `tag` isn't one (a unit value or `''`). */
function propertyFromObjectTag(tag: string): string | null {
  return tag.startsWith(VARIANT_OBJECT_TAG_PREFIX) ? tag.slice(VARIANT_OBJECT_TAG_PREFIX.length) : null;
}

function objectOptions(
  options: readonly VariantOption[],
): readonly Extract<VariantOption, { kind: 'asset' }>[] {
  return options.filter((o): o is Extract<VariantOption, { kind: 'asset' }> => o.kind === 'asset');
}

/** The object variant `value` is already shaped as (its one property is a key of `value`), or
 *  `undefined` if `value` doesn't match any of the field's declared object variants.
 *  `deriveVariantOptions` guarantees object variants' properties are pairwise distinct, so at most one
 *  can ever match. Drives both the dropdown's selection and which object variant's text input shows. */
export function currentObjectOption(
  options: readonly VariantOption[],
  value: unknown,
): Extract<VariantOption, { kind: 'asset' }> | undefined {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  return objectOptions(options).find((o) => o.property in record);
}

/** The `<select>` tag for the current value: the matching object variant's tag if `value` already
 *  carries one of their shapes, the matching unit value if it's a recognised unit string, or `''` for
 *  anything else (an unmatched value from a schema/value mismatch — the dropdown just shows
 *  unselected, never crashes). */
export function variantTag(options: readonly VariantOption[], value: unknown): string {
  const objectOption = currentObjectOption(options, value);
  if (objectOption !== undefined) {
    return variantObjectTag(objectOption.property);
  }
  if (typeof value === 'string' && options.some((o) => o.kind === 'unit' && o.value === value)) {
    return value;
  }
  return '';
}

/** The full field value to commit when the dropdown's selection changes to `tag`: the bare string for
 *  a unit choice, or `{ [property]: assetValue }` for the object variant `tag` names (defaulting its
 *  text to `''` when not supplied — e.g. right after switching into that choice, before the user
 *  types). An object tag naming a variant not present in `options` (a schema/value mismatch) commits
 *  `''`, same fail-soft rule as an unrecognised unit tag would leave the dropdown unselected. */
export function variantCommitValue(
  options: readonly VariantOption[],
  tag: string,
  assetValue = '',
): unknown {
  const property = propertyFromObjectTag(tag);
  if (property !== null) {
    const option = objectOptions(options).find((o) => o.property === property);
    return option !== undefined ? { [option.property]: assetValue } : '';
  }
  return tag;
}

// --- the widgets ---------------------------------------------------------------------------------

/** A field edit on its way to the wire. `cadence` is the slider drag's whole live-preview mechanism:
 *  omitted (every other widget, and the gesture's end) it commits and reports; `'preview'` writes
 *  without reporting, so a drag of any length is one undo entry. */
export type FieldCommit = (path: FieldPath, value: unknown, cadence?: CommitCadence) => void;

/** Routes one field descriptor to its input. Exhaustive over `FieldDescriptor['kind']`, so a widget
 *  kind added to `inspect-fields.ts` is a compile error here rather than a blank field. */
export function FieldWidget({
  field,
  value,
  commit,
  onError,
}: {
  readonly field: FieldDescriptor;
  readonly value: unknown;
  readonly commit: FieldCommit;
  readonly onError: (diagnostic: DiagnosticView) => void;
}): ReactNode {
  switch (field.kind) {
    case 'number':
      if (field.range !== null && isSliderValue(value)) {
        return (
          <RangeInput
            range={field.range}
            integer={field.integer}
            label={field.label}
            value={value}
            send={(n, cadence) => commit(field.path, n, cadence)}
          />
        );
      }
      return (
        <DraftInput
          className="field-input"
          inputMode={field.integer ? 'numeric' : 'decimal'}
          text={numText(value)}
          onCommit={(draft) => commitNumber(draft, field.integer, value, (n) => commit(field.path, n))}
        />
      );
    case 'boolean':
      // No draft to clobber: a checkbox commits the moment it is toggled.
      return (
        <input
          className="field-checkbox"
          type="checkbox"
          checked={value === true}
          onChange={(event) => commit(field.path, event.target.checked)}
        />
      );
    case 'string':
      return (
        <DraftInput
          className="field-input"
          text={strText(value)}
          onCommit={(draft) => {
            // An `Option<String>` commits an empty draft as `null`; `undefined` (an absent field) and
            // `null` are the same "not set" to this widget, so neither is worth a write.
            const next: string | null = field.nullable && draft === '' ? null : draft;
            if (next === value || (next === null && value === undefined)) {
              return;
            }
            commit(field.path, next);
          }}
        />
      );
    case 'vector': {
      // Each component is its own input, so editing one never freezes its siblings — and each commits
      // to its own index, whatever shape the components are laid out in.
      //
      // A ranged vector STACKS: N tracks sharing one row leave each a sliver of travel in a panel as
      // narrow as the chain's, which is no control at all. So one full-width track per row, tagged
      // with the channel it writes. An unranged vector is N number boxes and stays a single row.
      // Stacking follows the declared range rather than what each element happens to hold, so a
      // component whose value cannot carry a handle drops to a number box without breaking the column.
      const stacked = field.range !== null;
      return (
        <div className={stacked ? 'field-vector field-vector-stack' : 'field-vector'}>
          {field.color && (
            <span
              className="field-swatch"
              style={{ background: rgbaCss(Array.isArray(value) ? value : []) }}
              aria-hidden="true"
            />
          )}
          {Array.from({ length: field.length }, (_, i) => {
            const element = Array.isArray(value) ? (value as readonly unknown[])[i] : undefined;
            const channel = channelName(i, field.length);
            // The range is declared per element, so every component gets its own track over it.
            const input =
              field.range !== null && isSliderValue(element) ? (
                <RangeInput
                  range={field.range}
                  integer={field.integer}
                  // Each component needs its own accessible name; the field label alone would announce
                  // every track of a vector identically. The visible channel tag is that same name
                  // said twice, so it is hidden from the accessibility tree rather than doubled.
                  label={`${field.label} ${channel}`}
                  value={element}
                  send={(n, cadence) => commit([...field.path, i], n, cadence)}
                />
              ) : (
                <DraftInput
                  className="field-input"
                  inputMode={field.integer ? 'numeric' : 'decimal'}
                  text={numText(element)}
                  onCommit={(draft) =>
                    commitNumber(draft, field.integer, element, (n) => commit([...field.path, i], n))
                  }
                />
              );
            return stacked ? (
              <div className="field-vector-row" key={i}>
                <span className="field-channel" aria-hidden="true">
                  {channel}
                </span>
                {input}
              </div>
            ) : (
              <Fragment key={i}>{input}</Fragment>
            );
          })}
        </div>
      );
    }
    case 'json':
      // The fallback for anything the schema doesn't cleanly widget. Invalid JSON reports a diagnostic
      // and reverts; valid JSON that only reformats the value sends nothing.
      return (
        <DraftInput
          className="field-json"
          multiline
          text={jsonText(value)}
          onCommit={(draft) => {
            let parsed: unknown;
            try {
              parsed = JSON.parse(draft);
            } catch (err) {
              onError({
                code: null,
                message: `invalid JSON: ${err instanceof Error ? err.message : String(err)}`,
                help: null,
              });
              return;
            }
            if (jsonText(parsed) === jsonText(value)) {
              return;
            }
            commit(field.path, parsed);
          }}
        />
      );
    case 'variant':
      return <VariantInput field={field} value={value} commit={commit} />;
  }
}

/** A dropdown of the unit-string choices plus one entry per data-carrying object variant the schema
 *  declares (`asset`, `voxel`, …). Selecting a unit choice commits that literal string; selecting an
 *  object choice (or already holding a value shaped like one) reveals a text input for its one
 *  property, under the same draft-and-commit guards as every other text field. The selection and
 *  commit rules themselves are the pure, directly-tested functions above. */
function VariantInput({
  field,
  value,
  commit,
}: {
  readonly field: Extract<FieldDescriptor, { kind: 'variant' }>;
  readonly value: unknown;
  readonly commit: FieldCommit;
}) {
  const activeObjectOption = currentObjectOption(field.options, value);
  const currentTag = variantTag(field.options, value);

  return (
    <div className="field-variant">
      <select
        className="field-select"
        value={currentTag}
        onChange={(event) => commit(field.path, variantCommitValue(field.options, event.target.value))}
      >
        {currentTag === '' && (
          <option value="" disabled>
            choose…
          </option>
        )}
        {field.options.map((option) =>
          option.kind === 'unit' ? (
            <option key={option.value} value={option.value}>
              {option.value}
            </option>
          ) : (
            <option key={option.property} value={variantObjectTag(option.property)}>
              {option.property}…
            </option>
          ),
        )}
      </select>
      {activeObjectOption !== undefined && (
        <DraftInput
          className="field-input"
          text={strText((value as Record<string, unknown>)[activeObjectOption.property])}
          onCommit={(draft) =>
            commit(
              field.path,
              variantCommitValue(field.options, variantObjectTag(activeObjectOption.property), draft),
            )
          }
        />
      )}
    </div>
  );
}

/** A bounded numeric field: a track to drag, and beside it a readout that *is* the input — click it
 *  and it becomes the same `DraftInput` every other number box uses, focused, until the edit commits
 *  or reverts. The two paths' rules are the slider contract at the top of this file.
 *
 *  Exported because it is not only a field's own value that wants this control: the Art Director's
 *  grade bands drive a *derived* number through it (`editor/art/GradeBandControl.tsx`) and must keep
 *  the identical gesture. What varies there is only what `send` writes. */
export function RangeInput({
  range,
  integer,
  label,
  value,
  send,
}: {
  readonly range: FieldRange;
  readonly integer: boolean;
  readonly label: string;
  readonly value: number;
  readonly send: (next: number, cadence?: CommitCadence) => void;
}) {
  // Non-null only while a gesture is in progress, and then it is what the row displays: the ~3 Hz
  // refresh must not pull the handle out from under the pointer.
  const [handle, setHandle] = useState<number | null>(null);
  const [typing, setTyping] = useState(false);
  const drag = useRef(new SliderDrag()).current;

  // Widened off the committed value and never off `handle`, so the track cannot shrink under the
  // pointer as an out-of-range authored value is dragged back inside its declared travel.
  const travel = widen(range, value);
  const shown = handle ?? value;

  // `value` composes forward as previews land, so `begin` is called on the way into every track event
  // rather than only at pointer-down: it is idempotent, and the first call of a gesture is the one
  // that captures the base Escape restores.
  const beginGesture = (): void => drag.begin(value, range, integer);

  const apply = (instruction: SliderInstruction): void => {
    if (instruction.done === true) {
      // The gesture's own value is dropped in favour of the caller's, which composes the committed
      // value in synchronously — so releasing does not flash the pre-drag number for a poll cycle.
      setHandle(null);
    } else if (instruction.handle !== undefined) {
      setHandle(instruction.handle);
    }
    if (instruction.preview !== undefined) {
      send(instruction.preview, 'preview');
    }
    if (instruction.commit !== undefined) {
      send(instruction.commit, 'commit');
    }
  };

  return (
    <div className="field-slider">
      <input
        type="range"
        className="field-range"
        min={travel.min}
        max={travel.max}
        step={niceStep(travel.min, travel.max, integer)}
        value={shown}
        // The one control here with no text of its own; its readout is a sibling, not its label.
        aria-label={label}
        onPointerDown={beginGesture}
        onChange={(event) => {
          beginGesture();
          apply(drag.move(Number(event.target.value), performance.now()));
        }}
        onPointerUp={(event) => apply(drag.end(Number(event.currentTarget.value)))}
        // A pointer that ends anywhere but on the track — cancelled, or focus taken by another control
        // — still ends the gesture, so previews can never outlive the hand that started them.
        onPointerCancel={(event) => apply(drag.end(Number(event.currentTarget.value)))}
        onBlur={(event) => apply(drag.end(Number(event.currentTarget.value)))}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            apply(drag.escape());
          }
        }}
        onKeyUp={(event) => apply(drag.end(Number(event.currentTarget.value)))}
      />
      {typing ? (
        <DraftInput
          className="field-input field-readout num"
          autoFocus
          inputMode={integer ? 'numeric' : 'decimal'}
          text={numText(shown)}
          onCommit={(draft) => commitNumber(draft, integer, value, send)}
          onDone={() => setTyping(false)}
        />
      ) : (
        <button
          type="button"
          className="field-input field-readout num"
          title="Click to type an exact value."
          onClick={() => setTyping(true)}
        >
          {numText(shown)}
        </button>
      )}
    </div>
  );
}

/** The one text-backed input — single-line or multiline — implementing the draft-and-commit contract
 *  at the top of this file. `onCommit` receives the raw draft and decides whether it is worth sending;
 *  it is called only when the draft differs from the displayed text. `onDone` fires whichever way the
 *  edit ended, so a caller that swapped a readout into this input can swap it back. */
function DraftInput({
  text,
  className,
  multiline = false,
  autoFocus = false,
  inputMode,
  onCommit,
  onDone,
}: {
  readonly text: string;
  readonly className: string;
  readonly multiline?: boolean;
  readonly autoFocus?: boolean;
  readonly inputMode?: 'numeric' | 'decimal';
  readonly onCommit: (draft: string) => void;
  readonly onDone?: () => void;
}) {
  const [draft, setDraft] = useState(text);
  const [editing, setEditing] = useState(false);
  const reverting = useRef(false);

  // Also fires when `editing` falls to false, which is what re-syncs the draft after a rejected,
  // reverted, or unchanged edit.
  useEffect(() => {
    if (!editing) {
      setDraft(text);
    }
  }, [text, editing]);

  const commit = (): void => {
    setEditing(false);
    const reverted = reverting.current;
    reverting.current = false;
    if (!reverted && draft !== text) {
      onCommit(draft);
    }
    onDone?.();
  };

  const shared = {
    className,
    spellCheck: false,
    autoFocus,
    value: draft,
    onFocus: () => setEditing(true),
    onBlur: commit,
  };

  if (multiline) {
    return (
      <textarea
        {...shared}
        rows={Math.min(10, Math.max(2, draft.split('\n').length))}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          // Multiline: Enter inserts a newline, so only blur (or Escape) ends the edit.
          if (event.key === 'Escape') {
            reverting.current = true;
            event.currentTarget.blur();
          }
        }}
      />
    );
  }
  return (
    <input
      {...shared}
      type="text"
      inputMode={inputMode}
      onChange={(event) => setDraft(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.currentTarget.blur();
        } else if (event.key === 'Escape') {
          reverting.current = true;
          event.currentTarget.blur();
        }
      }}
    />
  );
}

/** Sends `draft` as a number, or sends nothing: an empty, non-finite, or (for an integer field)
 *  fractional draft is rejected, and a draft that parses to the value already there is a no-op. */
function commitNumber(
  draft: string,
  integer: boolean,
  current: unknown,
  send: (value: number) => void,
): void {
  const trimmed = draft.trim();
  const n = Number(trimmed);
  if (trimmed === '' || !Number.isFinite(n) || (integer && !Number.isInteger(n))) {
    return;
  }
  if (n === current) {
    return;
  }
  send(n);
}

function numText(value: unknown): string {
  return typeof value === 'number' ? String(value) : value === undefined || value === null ? '' : String(value);
}

function strText(value: unknown): string {
  return typeof value === 'string' ? value : value === undefined || value === null ? '' : String(value);
}

function jsonText(value: unknown): string {
  return value === undefined ? '' : JSON.stringify(value, null, 2);
}

/** A CSS `rgba(...)` from the first 3–4 entries of a normalised (0–1) color array, clamped. */
function rgbaCss(array: readonly unknown[]): string {
  const channel = (i: number): number => {
    const v = typeof array[i] === 'number' ? (array[i] as number) : 0;
    return Math.round(Math.max(0, Math.min(1, v)) * 255);
  };
  const alpha = array.length >= 4 && typeof array[3] === 'number' ? Math.max(0, Math.min(1, array[3])) : 1;
  return `rgba(${channel(0)}, ${channel(1)}, ${channel(2)}, ${alpha})`;
}
