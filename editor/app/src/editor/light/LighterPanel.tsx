// The Lighter's left column: one GI card over the scene's lights.
//
// WHY THIS IS NOT THE INSPECTOR. A property grid over `GiVolume` already ships in the Level
// Designer, and it presents five floats as five equals. Three of them are sample-time uniforms and
// two re-run work measured in hundreds of milliseconds (`giFields.ts` holds that model), so the card
// splits them by gesture: the cheap three are the shared slider, the expensive two are rows of
// discrete stops that write once per press. What a human is told, per row, is what the edit will
// make the engine re-run — because there is no bake verb to press, and the cost model is the only
// honest "bake" UI there can be.
//
// THIS FILE DECIDES NOTHING. Every rule it renders — the order, the cost badges, the ladders, the
// state chip, which A/B is offered, which term a light lives in, whether the moving-light note
// applies, whether a card offers the `GiDynamic` toggle — comes from `giFields.ts` or `lights.ts`
// and is tested there. What lives here is chrome: the cards, the dimming of an unauthored value, the
// two momentary buttons, and the copy.
//
// THE REBAKE LEVER IS SESSION STATE, NOT SCENE STATE, and the card says so. `render.gi_rebake` sets
// a host's caches to follow the world (`auto`) or to keep the baseline they already have (`hold`);
// nothing about it is hashed, snapshotted or recorded, so it is the one control on this card that no
// undo reaches and no save persists. The STALE badge beside it is not this panel's opinion: it is
// bit 0 of the frame header the viewport is decoding, so it describes the image on screen rather
// than what the lever was last set to.
//
// THE LIGHT CARDS ARE THE INSPECTOR'S. `ComponentForm` (`viewer/Inspector.tsx`) is exactly "one
// component's editable card over `useComponentCommit` + `deriveFields` + `FieldWidget`", which is
// what a sun and a selected point light need; a second form here would be a second answer to what an
// edit does. The list beside them exists because a bare light entity has no mesh, so `render.bounds`
// has no AABB for it and the viewport cannot pick it (see `lights.ts`).
//
// The classes this panel introduces are prefixed `lt-`; the shell primitives (`panel-head`, `field`,
// `icon-btn`, `count`) are reused as-is.

import { useMemo } from 'react';
import type { GiRebakeMode } from '@ludion/protocol';
import type { ProtocolClient } from '../../protocol/client';
import { DiagnosticBanner } from '../../viewer/DiagnosticBanner';
import type { DiagnosticView } from '../../viewer/diagnostic';
import { FieldWidget, type FieldCommit } from '../../viewer/FieldWidgets';
import { ComponentForm, entityKey } from '../../viewer/Inspector';
import { deriveFields, type FieldDescriptor } from '../../viewer/inspect-fields';
import type { MutationReport } from '../../viewer/useComponentCommit';
import type { EntityId } from '../undo';
import type { MaterializationTarget } from '../art/target';
import {
  COST_BADGE,
  effectiveGi,
  GI_FIELD_ORDER,
  GI_TERM_HINT,
  GI_VOLUME,
  giLightTerm,
  giState,
  isCurrentStep,
  MARK_COST_BADGE,
  offersBounceAb,
  offersGiAb,
  showsMovingLightNote,
  stepListFor,
  CELL_SIZE_STEPS,
  PROBE_SPACING_STEPS,
  type GiFieldName,
  type GiLightTerm,
} from './giFields';
import {
  DIRECTIONAL_LIGHT,
  lightLabel,
  offersGiDynamicToggle,
  POINT_LIGHT,
  SPOT_LIGHT,
  SPOT_FEEDS_NO_GI,
  SUN_ALWAYS_BAKES,
  type LightEntry,
} from './lights';

/** Which hold-to-compare owns the component, if either. Both write one `GiVolume` field. */
export type GiHold = 'gi' | 'bounce';

export interface LighterPanelProps {
  readonly client: ProtocolClient;
  /** The registry's component schemas from `engine.describe`, or `null` before they arrive. */
  readonly schemas: Readonly<Record<string, unknown>> | null;
  readonly target: MaterializationTarget | null;
  /** Whether the target carries a `GiVolume` today. */
  readonly authored: boolean;
  /** The live `GiVolume`, or `null` when the target carries none. */
  readonly gi: Record<string, unknown> | null;
  readonly onEnableGi: () => void;
  readonly onCommit: FieldCommit;
  /** The session's rebake lever, or `null` until the query form of `render.gi_rebake` answers. */
  readonly rebakeMode: GiRebakeMode | null;
  readonly onRebakeMode: (mode: GiRebakeMode) => void;
  readonly onRebakeNow: () => void;
  /** Bit 0 of the newest frame header: the image on screen is lit by a held baseline. */
  readonly giStale: boolean;
  readonly holding: GiHold | null;
  readonly onHoldStart: (hold: GiHold) => void;
  readonly onHoldEnd: () => void;
  /** The scene's sun, or `null` when it authors no `DirectionalLight`. */
  readonly sun: EntityId | null;
  /** The sun's live `DirectionalLight`, or `null` before the first poll answers. */
  readonly sunValue: Record<string, unknown> | null;
  readonly lights: readonly LightEntry[];
  readonly selection: EntityId | null;
  readonly onSelect: (entity: EntityId) => void;
  /** The selection's live `PointLight`, or `null` when it carries none. */
  readonly pointLight: Record<string, unknown> | null;
  /** The selection's live `SpotLight`, or `null` when it carries none. */
  readonly spotLight: Record<string, unknown> | null;
  /** Whether the selection carries `GiDynamic` — polled beside its `PointLight`, so an undo of a
   *  mark is reflected here without the panel being told about it. */
  readonly selectionDynamic: boolean;
  readonly onToggleDynamic: (entity: EntityId, dynamic: boolean) => void;
  readonly onReport: (report: MutationReport) => void;
  readonly error: DiagnosticView | null;
  readonly onDismissError: () => void;
  readonly onError: (diagnostic: DiagnosticView) => void;
}

/** What the GI card's chip says. `bounce` wears the warm fill rather than a second accent tone: it is
 *  the state that makes every light move re-bake the volume, so it reads as a cost and not as one
 *  more shade of on. */
const CHIP: Readonly<Record<'off' | 'sky' | 'bounce', { readonly text: string; readonly tone: string }>> = {
  off: { text: 'off', tone: 'lt-chip-off' },
  sky: { text: 'sky', tone: 'lt-chip-live' },
  bounce: { text: 'sky + bounce', tone: 'lt-chip-cost' },
};

/** The stops each expensive field offers. */
const STEPS: Readonly<Record<'probe_spacing' | 'cell_size', readonly number[]>> = {
  probe_spacing: PROBE_SPACING_STEPS,
  cell_size: CELL_SIZE_STEPS,
};

/** The lever's two settings, and what each one does to this session's caches. Titles rather than
 *  visible prose: the row is two words wide, and the sentence belongs on hover. */
const LEVER: readonly { readonly mode: GiRebakeMode; readonly label: string; readonly title: string }[] =
  [
    {
      mode: 'auto',
      label: 'Auto',
      title:
        'Voxelize and bake follow the world: every change to their inputs rebuilds them on the next frame. The shipped behaviour, and what every one-shot render, golden and replay always uses.',
    },
    {
      mode: 'hold',
      label: 'Hold',
      title:
        'Keep the baseline already built: a change to the inputs serves the retained caches instead of rebuilding them, and the frame is marked stale until you press Rebake. Session state only — nothing is hashed, snapshotted or saved.',
    },
  ];

/** Why the Rebake button is disabled, said rather than implied (the {@link HOLD_HINT} discipline). */
const REBAKE_HINT: Readonly<Record<'auto' | 'hold' | 'unknown', string>> = {
  auto: 'Nothing is held at Auto — the caches already follow the world, so a rebake would change nothing.',
  hold: 'Rebuild both caches once against the world as it is now, and make that the new held baseline.',
  unknown: 'Reading the session lever…',
};

/** What the STALE badge means. It is the frame header speaking, not this panel. */
const STALE_TITLE =
  'The frame on screen was lit by a held baseline: its voxelization or probe bake was built under inputs the world no longer has.';

/** The `GiDynamic` toggle's own sentence. One press is one write — no preview cadence — and it costs
 *  exactly one bake because the light changes which term it contributes through. */
const MARK_TITLE: Readonly<Record<GiLightTerm, string>> = {
  baked:
    'Mark this light GiDynamic: it leaves the bake and is injected per frame instead, so moving or recolouring it stops invalidating the volume. One re-bake to switch.',
  injected:
    'Unmark this light: it goes back into the bake, where moving or recolouring it re-bakes the volume. One re-bake to switch.',
};

function entityLabel(entity: EntityId): string {
  return `#${entityKey(entity)}`;
}

function sameEntity(a: EntityId, b: EntityId): boolean {
  return a.index === b.index && a.generation === b.generation;
}

export function LighterPanel(props: LighterPanelProps) {
  const { gi, authored, target, schemas, onCommit, onError } = props;

  // What the engine actually bakes with: the scene's values over the serde defaults, so an
  // unauthored component still shows the numbers the frame renders through.
  const effective = useMemo(() => effectiveGi(gi), [gi]);

  // The Inspector's derivation over the same effective values, indexed by field name — the widgets a
  // slider row needs (its declared travel, its integer-ness) are the schema's, not this panel's.
  const byName = useMemo(() => {
    const schema = schemas?.[GI_VOLUME];
    const map = new Map<string, FieldDescriptor>();
    if (schema === undefined) {
      return map;
    }
    for (const field of deriveFields(schema, effective)) {
      const key = field.path[0];
      if (typeof key === 'string' && !map.has(key)) {
        map.set(key, field);
      }
    }
    return map;
  }, [schemas, effective]);

  const state = giState(gi);
  const chip = CHIP[state];

  return (
    <section className="lt-panel">
      <div className="panel-head">
        Lighting
        <span className="count">{target === null ? 'no target' : entityLabel(target.entity)}</span>
      </div>

      {/* Which entity holds the volume, why that one, and whether it is authored — the same line the
          Art Director puts under its head, because it answers the same question. */}
      {target !== null && (
        <div className="lt-target">
          <span className="lt-target-rule">
            <code>{GI_VOLUME}</code>{' '}
            {target.reason === 'environment' ? 'on the Environment carrier' : 'on the lowest entity id'}
          </span>
          <span className={authored ? 'lt-chip lt-chip-live' : 'lt-chip lt-chip-off'}>
            {authored ? 'authored' : 'not authored — shipped defaults'}
          </span>
        </div>
      )}

      <DiagnosticBanner diagnostic={props.error} onDismiss={props.onDismissError} />

      <div className="lt-scroll">
        {target === null && (
          <p className="lt-hint">
            No entity to hold the volume yet — the card shows the shipped defaults once the scene has one.
          </p>
        )}

        {/* --- the GI card --- */}
        <div className="lt-card">
          <div className="lt-card-head">
            Global illumination
            <span className={`lt-chip ${chip.tone}`}>{chip.text}</span>
            {props.giStale && (
              <span className="lt-chip lt-chip-cost" title={STALE_TITLE}>
                stale
              </span>
            )}
          </div>

          {!authored && target !== null && (
            <div className="lt-enable">
              <button type="button" className="icon-btn" onClick={props.onEnableGi}>
                Enable GI
              </button>
              <span className="lt-enable-note">writes the shipped defaults; one undo removes it again</span>
            </div>
          )}

          <LeverRow
            mode={props.rebakeMode}
            onMode={props.onRebakeMode}
            onRebakeNow={props.onRebakeNow}
          />

          <div className="lt-fields">
            {GI_FIELD_ORDER.map((row) =>
              row.name === 'probe_spacing' || row.name === 'cell_size' ? (
                <StepRow
                  key={row.name}
                  name={row.name}
                  label={row.label}
                  badge={COST_BADGE[row.cost]}
                  warm={row.cost === 'revoxelize'}
                  current={effective[row.name]}
                  authored={authored}
                  disabled={target === null}
                  commit={onCommit}
                />
              ) : (
                <SliderRow
                  key={row.name}
                  label={row.label}
                  field={byName.get(row.name)}
                  value={effective[row.name]}
                  authored={authored}
                  commit={onCommit}
                  onError={onError}
                  compare={compareFor(row.name, gi)}
                  holding={props.holding}
                  onHoldStart={props.onHoldStart}
                  onHoldEnd={props.onHoldEnd}
                  disabled={target === null}
                />
              ),
            )}
          </div>

          <p className="lt-foot">
            At Auto an edit applies on the next frame — voxelize and bake re-run on their own when
            their inputs change. At Hold this session keeps the baseline it already built and marks
            the frame stale until Rebake refreshes it.
          </p>
          {showsMovingLightNote(gi, null) && (
            <p className="lt-foot lt-foot-warn">
              While bounce is on, moving or recolouring an <em>unmarked</em> light re-bakes the volume
              every frame. A light marked <code>GiDynamic</code> is injected instead and is free to
              move.
            </p>
          )}
        </div>

        {/* --- the sun --- */}
        <div className="lt-section">
          <div className="lt-section-head">Sun</div>
          {/* A resolved sun whose value has not arrived yet is NOT "no sun": that hint tells a human
              to place a light the scene already has. */}
          {props.sun === null ? (
            <p className="lt-hint">
              No sun in this scene — place a <code>DirectionalLight</code> from the Level Designer.
            </p>
          ) : props.sunValue === null ? (
            <p className="lt-hint">Reading the sun…</p>
          ) : (
            <>
              <ComponentForm
                key={`${entityKey(props.sun)}:${DIRECTIONAL_LIGHT}`}
                client={props.client}
                entity={props.sun}
                name={DIRECTIONAL_LIGHT}
                schema={schemas?.[DIRECTIONAL_LIGHT]}
                value={props.sunValue}
                onReport={props.onReport}
                onError={onError}
              />
              {/* Each light card asks the same question, and the one that answers no says why —
                  a control that is merely missing is indistinguishable from a broken one. */}
              {!offersGiDynamicToggle(DIRECTIONAL_LIGHT) && <p className="lt-foot">{SUN_ALWAYS_BAKES}</p>}
            </>
          )}
        </div>

        {/* --- the lights, and the selected one --- */}
        <div className="lt-section">
          <div className="lt-section-head">
            Lights
            <span className="count">{props.lights.length}</span>
          </div>
          {props.lights.length === 0 ? (
            <p className="lt-hint">This scene places no point or spot lights.</p>
          ) : (
            <div className="lt-lights">
              {props.lights.map((entry) => {
                const on = props.selection !== null && sameEntity(props.selection, entry.entity);
                const term = giLightTerm(entry.dynamic);
                return (
                  <button
                    key={entityKey(entry.entity)}
                    type="button"
                    className={on ? 'lt-light lt-light-on' : 'lt-light'}
                    onClick={() => props.onSelect(entry.entity)}
                  >
                    <span className="lt-light-name">{lightLabel(entry)}</span>
                    {entry.spot ? (
                      <span className="lt-chip lt-chip-off" title={SPOT_FEEDS_NO_GI}>
                        spot
                      </span>
                    ) : (
                      <span className={termChip(term)} title={GI_TERM_HINT[term]}>
                        {term}
                      </span>
                    )}
                    <span className="lt-light-id">{entityLabel(entry.entity)}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        <div className="lt-section">
          <div className="lt-section-head">Selected light</div>
          {props.selection !== null && props.pointLight !== null ? (
            <>
              <ComponentForm
                key={`${entityKey(props.selection)}:${POINT_LIGHT}`}
                client={props.client}
                entity={props.selection}
                name={POINT_LIGHT}
                schema={schemas?.[POINT_LIGHT]}
                value={props.pointLight}
                onReport={props.onReport}
                onError={onError}
              />
              {offersGiDynamicToggle(POINT_LIGHT) && (
                <MarkRow
                  entity={props.selection}
                  dynamic={props.selectionDynamic}
                  onToggle={props.onToggleDynamic}
                />
              )}
              {showsMovingLightNote(gi, giLightTerm(props.selectionDynamic)) && (
                <p className="lt-foot lt-foot-warn">
                  Baked, with bounce on: every move or colour change of this light re-bakes the
                  volume.
                </p>
              )}
            </>
          ) : props.selection !== null && props.spotLight !== null ? (
            <>
              <ComponentForm
                key={`${entityKey(props.selection)}:${SPOT_LIGHT}`}
                client={props.client}
                entity={props.selection}
                name={SPOT_LIGHT}
                schema={schemas?.[SPOT_LIGHT]}
                value={props.spotLight}
                onReport={props.onReport}
                onError={onError}
              />
              {!offersGiDynamicToggle(SPOT_LIGHT) && <p className="lt-foot">{SPOT_FEEDS_NO_GI}</p>}
            </>
          ) : (
            <p className="lt-hint">Select a light — click it in the viewport or the list above.</p>
          )}
        </div>
      </div>
    </section>
  );
}

/** The chip a term wears. `baked` is the state that makes a light move cost a bake, so it takes the
 *  warm fill this column reserves for cost; `injected` recedes, because it is the free one. */
function termChip(term: GiLightTerm): string {
  return term === 'baked' ? 'lt-chip lt-chip-cost' : 'lt-chip lt-chip-live';
}

/**
 * The session's rebake lever: a two-stop switch and the one-shot button behind it.
 *
 * Rendered as stops rather than a checkbox for the reason `StepRow` is: one press, one write, no
 * preview cadence — and pressing the stop the session already holds sends nothing.
 */
function LeverRow({
  mode,
  onMode,
  onRebakeNow,
}: {
  readonly mode: GiRebakeMode | null;
  readonly onMode: (mode: GiRebakeMode) => void;
  readonly onRebakeNow: () => void;
}) {
  const held = mode === 'hold';
  return (
    <div className="lt-lever">
      <span className="field-label">Rebake</span>
      <span className="lt-chip lt-chip-off">session only</span>
      <div className="lt-steps">
        {LEVER.map((stop) => {
          const on = mode === stop.mode;
          return (
            <button
              key={stop.mode}
              type="button"
              className={on ? 'lt-step lt-step-on' : 'lt-step'}
              disabled={mode === null}
              aria-pressed={on}
              title={stop.title}
              onClick={() => {
                if (!on) {
                  onMode(stop.mode);
                }
              }}
            >
              {stop.label}
            </button>
          );
        })}
        <button
          type="button"
          className="lt-step lt-step-act"
          disabled={!held}
          title={REBAKE_HINT[mode === null ? 'unknown' : mode]}
          onClick={onRebakeNow}
        >
          Rebake
        </button>
      </div>
    </div>
  );
}

/** The selected light's term, and the one press that changes it. */
function MarkRow({
  entity,
  dynamic,
  onToggle,
}: {
  readonly entity: EntityId;
  readonly dynamic: boolean;
  readonly onToggle: (entity: EntityId, dynamic: boolean) => void;
}) {
  const term = giLightTerm(dynamic);
  return (
    <div className="lt-field lt-mark-row">
      <span className="field-label">GI term</span>
      <span className={termChip(term)} title={GI_TERM_HINT[term]}>
        {term}
      </span>
      <span className="lt-chip lt-chip-off">{MARK_COST_BADGE}</span>
      <button
        type="button"
        className={dynamic ? 'lt-step lt-step-on' : 'lt-step'}
        aria-pressed={dynamic}
        title={MARK_TITLE[term]}
        onClick={() => onToggle(entity, !dynamic)}
      >
        GiDynamic
      </button>
    </div>
  );
}

/** The hold a row offers, or `null`. Both availability rules are `giFields.ts`'; this only routes
 *  them to the two rows that can carry a button. */
function compareFor(name: GiFieldName, gi: Record<string, unknown> | null): GiHold | null {
  if (name === 'sky_visibility') {
    return offersGiAb(gi) ? 'gi' : null;
  }
  return name === 'bounce' && offersBounceAb(gi) ? 'bounce' : null;
}

/** A hold's button title, which has to say why a disabled one is disabled: a control that does not
 *  name what would enable it is indistinguishable from a broken one. */
const HOLD_HINT: Readonly<Record<GiHold, string>> = {
  gi: 'Hold to see the frame with sky visibility at 0 — exactly the no-volume look. Free both ways.',
  bounce: 'Hold to see the frame without the bounce term; one re-bake at each end of the hold.',
};

/** One cheap field: the shared slider, plus the hold-to-compare where one is offered. */
function SliderRow({
  label,
  field,
  value,
  authored,
  commit,
  onError,
  compare,
  holding,
  onHoldStart,
  onHoldEnd,
  disabled,
}: {
  readonly label: string;
  readonly field: FieldDescriptor | undefined;
  readonly value: number;
  readonly authored: boolean;
  readonly commit: FieldCommit;
  readonly onError: (diagnostic: DiagnosticView) => void;
  readonly compare: GiHold | null;
  readonly holding: GiHold | null;
  readonly onHoldStart: (hold: GiHold) => void;
  readonly onHoldEnd: () => void;
  readonly disabled: boolean;
}) {
  const classes = ['lt-field', 'field'];
  if (!authored) {
    classes.push('lt-field-default');
  }
  return (
    <div className={classes.join(' ')}>
      <span className="field-label">{label}</span>
      {field === undefined ? (
        <span className="lt-hint">waiting for the component schema…</span>
      ) : (
        <FieldWidget field={field} value={value} commit={commit} onError={onError} />
      )}
      {compare !== null && (
        <button
          type="button"
          className={holding === compare ? 'lt-compare lt-compare-held' : 'lt-compare'}
          disabled={disabled}
          title={HOLD_HINT[compare]}
          // Momentary, not a toggle: the press writes the neutral and the release puts the captured
          // value back. `preventDefault` keeps the pointer-down from focusing the button and
          // swallowing the release as a click.
          onPointerDown={(event) => {
            event.preventDefault();
            onHoldStart(compare);
          }}
          onPointerUp={onHoldEnd}
          onPointerLeave={onHoldEnd}
        >
          A/B
        </button>
      )}
      {!authored && (
        <span className="lt-default-tag" title="The engine's shipped value; the scene does not author it.">
          shipped default
        </span>
      )}
    </div>
  );
}

/** One expensive field: discrete stops, one write per press, no previews ever. Pressing the stop the
 *  component already holds sends nothing — that write would buy a re-bake for a value the engine has. */
function StepRow({
  name,
  label,
  badge,
  warm,
  current,
  authored,
  disabled,
  commit,
}: {
  readonly name: 'probe_spacing' | 'cell_size';
  readonly label: string;
  readonly badge: string;
  readonly warm: boolean;
  readonly current: number;
  readonly authored: boolean;
  readonly disabled: boolean;
  readonly commit: FieldCommit;
}) {
  const classes = ['lt-field', 'lt-step-row'];
  if (!authored) {
    classes.push('lt-field-default');
  }
  return (
    <div className={classes.join(' ')}>
      <span className="field-label">{label}</span>
      <span className={warm ? 'lt-chip lt-chip-cost' : 'lt-chip lt-chip-off'}>{badge}</span>
      <div className="lt-steps">
        {stepListFor(STEPS[name], current).map((stop) => {
          const on = isCurrentStep(stop.value, current);
          const tags = ['lt-step'];
          if (on) {
            tags.push('lt-step-on');
          }
          if (stop.offList) {
            tags.push('lt-step-off-list');
          }
          return (
            <button
              key={stop.value}
              type="button"
              className={tags.join(' ')}
              disabled={disabled}
              aria-pressed={on}
              title={stop.offList ? 'Authored by the scene; not one of the offered stops.' : undefined}
              onClick={() => {
                if (!on) {
                  commit([name], stop.value, 'commit');
                }
              }}
            >
              {stop.value}
            </button>
          );
        })}
      </div>
    </div>
  );
}
