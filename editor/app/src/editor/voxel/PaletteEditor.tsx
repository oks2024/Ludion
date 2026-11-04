// The palette entry editor — the swatch strip's second duty.
//
// This is the control that removes that calibration loop directly: drag the color while watching
// the model under the game's own `env-night.yaml`. So the panel carries a one-click shortcut to the night rig
// when you are not on it — the workflow is worth exactly as much as it is short.
//
// Every control is a live drag: `onChange` streams (throttled to the disk by `save.ts`) and the
// release commits. A field the entry does not declare renders DISABLED WITH ITS REASON rather than
// failing when clicked — `edit.ts` refuses to insert a missing material key, and `palette.ts` turns
// that refusal into the same sentence the edit would have used.
//
// Thin: every value it produces comes from `palette.ts` and every write goes through the session's
// `apply`. It holds no state of its own.

import { formatVoxelFloat, type MaterialField } from './edit';
import type { PaletteEntry } from './doc';
import { quantize } from '../../viewer/slider';
import { colorControl, hexToLinear, materialControl, MATERIAL_FIELDS } from './palette';

interface PaletteEditorProps {
  readonly entry: PaletteEntry;
  /** No host, no document, or an unresolved conflict — every control greys out. */
  readonly disabled: boolean;
  /** Linear r, g, b, already quantized. `commit` is true on release. */
  readonly onColor: (rgb: readonly [number, number, number], commit: boolean) => void;
  readonly onMaterial: (field: MaterialField, value: number, commit: boolean) => void;
  /** Switches the stage to the night rig, or `null` when it is already there. */
  readonly onJudgeUnderNight: (() => void) | null;
}

export function PaletteEditor({ entry, disabled, onColor, onMaterial, onJudgeUnderNight }: PaletteEditorProps) {
  const color = colorControl(entry);

  return (
    <div className="vx-editor">
      <div className="vx-editor-head">
        <span className="vx-editor-glyph">{entry.glyph}</span>
        <span className="vx-editor-nums">{entry.color.map((c) => formatVoxelFloat(c.value)).join(' ')}</span>
        {onJudgeUnderNight !== null && (
          <button
            type="button"
            className="icon-btn"
            title="Put the stage under the game’s own env-night.yaml — the light these values are actually judged in."
            onClick={onJudgeUnderNight}
          >
            judge at night
          </button>
        )}
      </div>

      <label className="vx-ctl">
        <span className="vx-ctl-name">color</span>
        {color.editable ? (
          <>
            <input
              type="color"
              className="vx-color"
              value={color.hex}
              disabled={disabled}
              aria-label={`Albedo of glyph ${entry.glyph}`}
              onChange={(event) => emitColor(event.target.value, false, onColor)}
              onBlur={(event) => emitColor(event.target.value, true, onColor)}
            />
            <span className="vx-ctl-value">{color.hex}</span>
          </>
        ) : (
          <span className="vx-ctl-why">{color.reason}</span>
        )}
      </label>

      {MATERIAL_FIELDS.map((field) => {
        const control = materialControl(entry, field);
        return (
          <label className="vx-ctl" key={field}>
            <span className="vx-ctl-name">{field}</span>
            {control.editable ? (
              <>
                <input
                  type="range"
                  className="vx-range"
                  min={control.range.min}
                  max={control.range.max}
                  step={control.range.step}
                  value={control.value}
                  disabled={disabled}
                  aria-label={`${field} of glyph ${entry.glyph}`}
                  onChange={(event) => emitMaterial(field, event.target.value, false, onMaterial)}
                  onPointerUp={(event) => emitMaterial(field, event.currentTarget.value, true, onMaterial)}
                  onKeyUp={(event) => emitMaterial(field, event.currentTarget.value, true, onMaterial)}
                />
                <span className="vx-ctl-value num">{formatVoxelFloat(control.value)}</span>
              </>
            ) : (
              <span className="vx-ctl-why">{control.reason}</span>
            )}
          </label>
        );
      })}

      {/* The albedo/night relationship is the whole reason this panel exists — say it once, here. */}
      <p className="vx-note">
        Albedo is linear: the swatch is the sRGB read of it, not the number in the file. Night
        readability comes from <span className="num">emissive</span>, not from a brighter albedo.
      </p>
    </div>
  );
}

/** Decodes the picker's hex to quantized linear and forwards it; an unparsable value is dropped
 *  rather than guessed at. */
function emitColor(
  hex: string,
  commit: boolean,
  onColor: (rgb: readonly [number, number, number], commit: boolean) => void,
): void {
  const rgb = hexToLinear(hex);
  if (rgb !== null) {
    onColor(rgb, commit);
  }
}

function emitMaterial(
  field: MaterialField,
  raw: string,
  commit: boolean,
  onMaterial: (field: MaterialField, value: number, commit: boolean) => void,
): void {
  const value = Number(raw);
  if (Number.isFinite(value)) {
    onMaterial(field, quantize(value), commit);
  }
}
