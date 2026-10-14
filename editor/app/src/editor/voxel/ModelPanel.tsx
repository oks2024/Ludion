// The MODEL panel: what the engine says about the selected model, and its palette as swatches.
//
// Everything measured here comes from `check-scene` through `readout.ts` — one checker, the
// engine's. The parsed document (`doc.ts`) contributes the palette strip and nothing else: it never
// validates, so the two halves can never disagree about whether a model is good.
//
// The diagnostics list is the point of the panel on a PASSING model: `L1129` (a stray component) and
// `L1115` (the cell budget) are warnings on an exit-0 run, and they are what a human would otherwise
// never see. On a failing one it explains a blank subject — a rejected file leaves the asset cache
// entry dropped, so the stage shows nothing until the next good save.
//
// THE SWATCHES DO TRIPLE DUTY (the plan says so): they display the palette, they select the glyph
// the slice painter paints with, and the selected one opens the palette editor below the strip. The
// erase chip (`.`) is part of the strip for the same reason — it is a paint choice, not a mode.

import { EMPTY_GLYPH, type VoxelDoc } from './doc';
import type { MaterialField } from './edit';
import { PaletteEditor } from './PaletteEditor';
import type { ModelReadout } from './readout';
import { swatchCss, swatchIsDark, swatchTitle } from './swatch';

interface ModelPanelProps {
  /** The selected model's `voxels/`-relative path, or `null` when nothing is selected. */
  readonly relPath: string | null;
  readonly readout: ModelReadout | null;
  readonly doc: VoxelDoc | null;
  /** True while a `check-scene` run is in flight (the first one, or a re-run after a disk change). */
  readonly checking: boolean;
  /** A host-side failure (the file could not be read or the checker could not be run). */
  readonly error: string | null;
  /** The glyph the painter paints with — `.` erases. Owned by the view, because the painter shares it. */
  readonly paintGlyph: string;
  readonly onPaintGlyph: (glyph: string) => void;
  /** False with no host, no document, or an unresolved conflict: the editor greys out. */
  readonly editable: boolean;
  /** Omitted together to make the palette read-only STRUCTURALLY: with no write callbacks there is
   *  no editor to render and no handler behind a greyed control. */
  readonly onColor?: (glyph: string, rgb: readonly [number, number, number], commit: boolean) => void;
  readonly onMaterial?: (glyph: string, field: MaterialField, value: number, commit: boolean) => void;
  /** Switches the stage to the night rig, or `null` when it is already there. */
  readonly onJudgeUnderNight: (() => void) | null;
  /** Why this document cannot be edited here, said once, or `null` for an editable model. */
  readonly readOnlyNote?: string | null;
  /** The uniform scale the subject is actually stamped with, for a document that declares none.
   *  Shown rather than assumed: the number decides whether a size judgment means anything. */
  readonly stagedScale?: number | null;
}

export function ModelPanel({
  relPath,
  readout,
  doc,
  checking,
  error,
  paintGlyph,
  onPaintGlyph,
  editable,
  onColor,
  onMaterial,
  onJudgeUnderNight,
  readOnlyNote = null,
  stagedScale = null,
}: ModelPanelProps) {
  const palette = doc?.palette ?? [];
  const selected = palette.find((entry) => entry.glyph === paintGlyph) ?? null;
  const writable = onColor !== undefined && onMaterial !== undefined;

  return (
    <section className="inspector vx-model">
      <div className="panel-head">
        Model
        {checking && <span className="count">checking…</span>}
      </div>

      <div className="insp-scroll">
        {relPath === null ? (
          <div className="insp-hint">Select a model to put it on the stage.</div>
        ) : (
          <div className="vx-model-body">
            <div className="vx-path" title={relPath}>
              {relPath}
            </div>

            {error !== null && <div className="vx-error">{error}</div>}

            {readout !== null && (
              <>
                <div className={readout.ok ? 'vx-readout' : 'vx-readout bad'}>
                  {readout.kind === 'assembly' && readout.size !== null ? (
                    <>
                      <span className="num">
                        {readout.size[0]}×{readout.size[1]}×{readout.size[2]}
                      </span>
                      <span className="sep">·</span>
                      <span>
                        <span className="num">{readout.parts}</span>{' '}
                        {readout.parts === 1 ? 'part' : 'parts'}
                      </span>
                      <span className="sep">·</span>
                      <span>
                        <span className="num">{readout.clips.length}</span>{' '}
                        {readout.clips.length === 1 ? 'clip' : 'clips'}
                      </span>
                    </>
                  ) : readout.size !== null ? (
                    <>
                      <span className="num">
                        {readout.size[0]}×{readout.size[1]}×{readout.size[2]}
                      </span>
                      <span className="sep">·</span>
                      <span>
                        <span className="num">{readout.filled}</span> voxels
                      </span>
                      <span className="sep">·</span>
                      <span>
                        <span className="num">{readout.components}</span>{' '}
                        {readout.components === 1 ? 'component' : 'components'}
                      </span>
                      {readout.scale !== null && (
                        <>
                          <span className="sep">·</span>
                          <span>
                            scale <span className="num">{readout.scale}</span>
                          </span>
                        </>
                      )}
                    </>
                  ) : (
                    <span>{readout.summary}</span>
                  )}
                </div>

                {/* The declared scale is intent, not fact: the stage stamps it at placement time and
                    a scene's own Transform always wins. Say so where the number is shown. */}
                {readout.scale !== null && (
                  <div className="vx-note">declared scale — stamped on placement, a scene’s Transform wins</div>
                )}

                {readout.scale === null && stagedScale !== null && (
                  <div className="vx-note">
                    staged at <span className="num">{stagedScale}</span> — the 16-per-unit content
                    convention; an assembly declares no scale, and a scene’s own Transform is what places it
                  </div>
                )}

                {readout.diagnostics.length > 0 && (
                  <ul className="vx-diags">
                    {readout.diagnostics.map((diagnostic, index) => (
                      <li key={`${diagnostic.code}-${index}`} className={`vx-diag ${diagnostic.severity}`}>
                        <span className="vx-diag-code">{diagnostic.code}</span>
                        <span className="vx-diag-msg">
                          {diagnostic.message}
                          {diagnostic.path !== null && <span className="vx-diag-loc"> {diagnostic.path}</span>}
                          {diagnostic.help !== null && <span className="vx-diag-help">{diagnostic.help}</span>}
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
              </>
            )}

            <div className="vx-palette-head">
              Palette
              <span className="count">
                <span className="num">{palette.length}</span>
              </span>
            </div>
            {palette.length === 0 ? (
              <div className="insp-hint">no palette entries read</div>
            ) : (
              <div className="vx-swatches">
                {palette.map((entry) => {
                  const classes = ['vx-swatch'];
                  if (swatchIsDark(entry.color)) {
                    classes.push('dark');
                  }
                  if (entry.glyph === paintGlyph) {
                    classes.push('selected');
                  }
                  return (
                    <button
                      type="button"
                      key={entry.glyph}
                      className={classes.join(' ')}
                      style={{ background: swatchCss(entry.color) }}
                      title={swatchTitle(entry)}
                      aria-pressed={entry.glyph === paintGlyph}
                      onClick={() => onPaintGlyph(entry.glyph)}
                    >
                      <span className="vx-glyph">{entry.glyph}</span>
                      {entry.emissive !== null && <span className="vx-emis" aria-label="emissive" />}
                    </button>
                  );
                })}
                {/* The erase chip is a paint choice, so it exists only where painting does. */}
                {writable && (
                  <button
                    type="button"
                    className={paintGlyph === EMPTY_GLYPH ? 'vx-swatch erase selected' : 'vx-swatch erase'}
                    title="Erase — paints `.`, the format’s empty cell"
                    aria-pressed={paintGlyph === EMPTY_GLYPH}
                    onClick={() => onPaintGlyph(EMPTY_GLYPH)}
                  >
                    <span className="vx-glyph">{EMPTY_GLYPH}</span>
                  </button>
                )}
              </div>
            )}

            {readOnlyNote !== null && <div className="vx-note">{readOnlyNote}</div>}

            {selected !== null && onColor !== undefined && onMaterial !== undefined && (
              <PaletteEditor
                entry={selected}
                disabled={!editable}
                onColor={(rgb, commit) => onColor(selected.glyph, rgb, commit)}
                onMaterial={(field, value, commit) => onMaterial(selected.glyph, field, value, commit)}
                onJudgeUnderNight={onJudgeUnderNight}
              />
            )}
            {writable && palette.length > 0 && selected === null && (
              <div className="vx-note">Pick a swatch to edit its value — and to paint with it below.</div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
