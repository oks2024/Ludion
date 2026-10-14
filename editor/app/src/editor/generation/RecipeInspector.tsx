// The recipe inspector: one control per editable value in the open `.gen.yaml`, grouped by the node
// title the override keys on.
//
// IT IS A VIEW OF THE FILE, NOT OF A SCHEMA. `sculpt/SculptInspector.tsx`'s posture: the entity
// inspector renders `FieldDescriptor`s derived from a component's JSON Schema and commits over the
// wire; this renders positions in a text document and commits by splicing that text. Nothing is
// shared but the look, so it reuses those stylesheet classes and none of the code.
//
// A DRAG ENDS HONESTLY. `onNumber` fires through the travel so the readout tracks the handle, and
// the pointer release re-reports the same value as a COMMIT, so the file always ends the gesture
// holding exactly what the human left on screen.
//
// A PROMPT COMMITS ON BLUR, NOT PER KEYSTROKE. A prompt is hundreds of characters and nothing
// watches it change, so the throttled write is pure disk churn; the value is reported live (the
// working document is the textarea's value) and committed when focus leaves.

import { type ChangeEvent, type PointerEvent } from 'react';
import { controlFor, quantize, type SliderRange } from './controls';
import type { RecipeDoc, RecipeNumber, RecipeText } from './doc';

export interface RecipeInspectorProps {
  /** The working document — the inspector redraws from it after every edit. */
  readonly doc: RecipeDoc;
  readonly onNumber: (pointer: string, value: number, commit: boolean) => void;
  readonly onText: (pointer: string, value: string, commit: boolean) => void;
  /** No host, or an unresolved conflict: every control greys out. */
  readonly disabled?: boolean;
}

/** The readout's text. `String` is shortest round-trip for a double, the same rule the file's own
 *  writer follows — so what the row shows is what a commit would write. */
function readout(value: number): string {
  return String(value);
}

function NumberRow({
  field,
  group,
  disabled,
  onNumber,
}: {
  field: RecipeNumber;
  group: string;
  disabled: boolean;
  onNumber: RecipeInspectorProps['onNumber'];
}) {
  const control = controlFor(field);
  if (control.kind === 'number') {
    return (
      <div className="field">
        <span className="field-label" title={field.pointer}>
          {field.key}
        </span>
        <input
          type="number"
          className="field-input num"
          value={field.value}
          step={control.step}
          min={control.min ?? undefined}
          disabled={disabled}
          aria-label={`${group} ${field.key}`}
          onChange={(event: ChangeEvent<HTMLInputElement>) => {
            const next = Number(event.currentTarget.value);
            if (Number.isFinite(next)) {
              onNumber(field.pointer, next, true);
            }
          }}
        />
      </div>
    );
  }
  // The travel must be able to hold the value already there: a hand-authored number outside the
  // authored range would otherwise snap the moment the row rendered.
  const range: SliderRange = {
    ...control.range,
    min: Math.min(control.range.min, field.value),
    max: Math.max(control.range.max, field.value),
  };
  const report = (event: { currentTarget: { value: string } }, commit: boolean): void => {
    onNumber(field.pointer, quantize(Number(event.currentTarget.value), range), commit);
  };
  return (
    <div className="field">
      <span className="field-label" title={field.pointer}>
        {field.key}
      </span>
      <div className="field-slider">
        <input
          type="range"
          className="field-range"
          min={range.min}
          max={range.max}
          step={range.step}
          value={field.value}
          disabled={disabled}
          aria-label={`${group} ${field.key}`}
          onChange={(event: ChangeEvent<HTMLInputElement>) => report(event, false)}
          onPointerUp={(event: PointerEvent<HTMLInputElement>) => report(event, true)}
          // A pointer that ends anywhere but on the track still ends the gesture, so a throttled
          // write can never be the last word.
          onPointerCancel={(event: PointerEvent<HTMLInputElement>) => report(event, true)}
          onBlur={(event) => report(event, true)}
          onKeyUp={(event) => report(event, true)}
        />
        <span className="field-input field-readout num">{readout(field.value)}</span>
      </div>
    </div>
  );
}

function TextRow({
  field,
  group,
  disabled,
  onText,
}: {
  field: RecipeText;
  group: string;
  disabled: boolean;
  onText: RecipeInspectorProps['onText'];
}) {
  return (
    <div className="field-block gen-prompt">
      <span className="field-label" title={field.pointer}>
        {field.key}
        <span className="count">
          <span className="num">{field.value.length}</span> chars
        </span>
      </span>
      <textarea
        className="field-input gen-prompt-input"
        value={field.value}
        rows={6}
        spellCheck={false}
        disabled={disabled}
        aria-label={`${group} ${field.key}`}
        // Enter arrives here as a line break the strict subset cannot hold, so `edit.ts` folds it
        // to a space before it is written — and the textarea then shows the folded text, which is
        // what the file holds. Nothing silently differs between the two.
        onChange={(event: ChangeEvent<HTMLTextAreaElement>) =>
          onText(field.pointer, event.currentTarget.value, false)
        }
        onBlur={(event) => onText(field.pointer, event.currentTarget.value, true)}
      />
    </div>
  );
}

export function RecipeInspector({ doc, onNumber, onText, disabled = false }: RecipeInspectorProps) {
  return (
    <div className="component-fields">
      {(doc.seed !== null || doc.count !== null) && (
        <div className="fx-group">
          <div className="fx-group-head">run</div>
          {[doc.seed, doc.count]
            .filter((field): field is RecipeNumber => field !== null)
            .map((field) => (
              <NumberRow
                key={field.pointer}
                field={field}
                group="run"
                disabled={disabled}
                onNumber={onNumber}
              />
            ))}
        </div>
      )}

      {doc.images.length > 0 && (
        <div className="fx-group">
          <div className="fx-group-head">images</div>
          {doc.images.map((image) => (
            <div key={image.name} className="field">
              <span className="field-label">{image.name}</span>
              <span className="field-input gen-readonly" title={image.path}>
                {image.path}
              </span>
            </div>
          ))}
        </div>
      )}

      {doc.nodes.map((node) => (
        <div key={node.title} className="fx-group">
          <div className="fx-group-head">{node.title}</div>
          {node.numbers.map((field) => (
            <NumberRow
              key={field.pointer}
              field={field}
              group={node.title}
              disabled={disabled}
              onNumber={onNumber}
            />
          ))}
          {node.texts.map((field) => (
            <TextRow
              key={field.pointer}
              field={field}
              group={node.title}
              disabled={disabled}
              onText={onText}
            />
          ))}
          {node.refs.map((ref) => (
            <div key={ref.key} className="field">
              <span className="field-label">{ref.key}</span>
              <span className="field-input gen-readonly">images.{ref.image}</span>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
