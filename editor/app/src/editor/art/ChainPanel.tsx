// The chain: one row per post-process effect, in the pipeline's pinned order, with its fields nested
// underneath.
//
// WHY THIS IS NOT THE INSPECTOR. A property grid over `PostProcess` already ships in the Level
// Designer, and it answers the wrong question. What a human directing a look needs to see first is
// whether an effect is *contributing* — `shafts: 0.02` with no `DirectionalLight` in the scene renders
// nothing and reports no error, and a zero gate removes its passes from the frame graph entirely. So
// the row, not the field, is the unit here: its state chip is `deriveLiveEffects`' verdict, its A/B
// toggle writes the effect's neutral while held, and only an opened row spends space on values.
//
// THE WIDGETS ARE NOT THIS PANEL'S. `deriveFields` (`viewer/inspect-fields.ts`) derives the fields and
// `FieldWidget` (`viewer/FieldWidgets.tsx`) renders them — the same layer the entity inspector edits
// through, so the two can never disagree about what an edit does. This file owns only the chrome the
// chain adds around a field: the row it belongs to, the dimming of a value the scene does not author,
// and the tag naming it as shipped.
//
// The three graded triples are the one exception, and they are not a private widget either:
// `GradeBandControl` is the same `RangeInput`, over a reversible view of the same value (`grade.ts`),
// writing the whole triple through the same `commit`. The two layers still cannot disagree about what
// an edit does — only about what a colourist is shown.

import { useMemo, type ReactNode } from 'react';
import { DiagnosticBanner } from '../../viewer/DiagnosticBanner';
import type { DiagnosticView } from '../../viewer/diagnostic';
import { FieldWidget, type FieldCommit } from '../../viewer/FieldWidgets';
import { deriveFields, valueAtPath, type FieldDescriptor } from '../../viewer/inspect-fields';
import {
  effectFields,
  effectRow,
  isComparable,
  type EffectId,
  type EffectReason,
  type EffectState,
  type FieldBuckets,
  type GraphDisagreement,
} from './chain';
import { GradeBandControl } from './GradeBandControl';
import { gradeControl } from './gradeControl';
import type { MaterializationTarget } from './target';

/** One accordion row. The chain's effects plus `other` — the bucket that catches a `PostProcess` field
 *  the engine grew and `chain.ts`'s table does not know, which must be visible rather than dropped. */
export type ChainRowId = EffectId | 'other';

export interface ChainPanelProps {
  /** `PostProcess`'s JSON Schema from `engine.describe`, or `null` before it arrives. */
  readonly schema: unknown;
  /** One state per effect, already in pinned pipeline order. */
  readonly effects: readonly EffectState[];
  /** Where the derived model and the real frame graph disagree — always shown, never swallowed. */
  readonly disagreements: readonly GraphDisagreement[];
  /** The runtime schema's fields bucketed across the chain, or `null` before the schema arrives. */
  readonly buckets: FieldBuckets | null;
  /** The values the engine actually renders: what the scene authors over the shipped defaults. */
  readonly values: Readonly<Record<string, unknown>>;
  /** The field names the scene authors. Everything else is dimmed and labelled as shipped. */
  readonly authoredFields: ReadonlySet<string>;
  readonly target: MaterializationTarget | null;
  /** Whether the target carries a `PostProcess` today. */
  readonly authored: boolean;
  readonly open: ChainRowId | null;
  readonly onOpen: (row: ChainRowId | null) => void;
  /** The effect whose A/B is held down, or `null`. */
  readonly holding: EffectId | null;
  readonly onHoldStart: (effect: EffectId) => void;
  readonly onHoldEnd: () => void;
  readonly onCommit: FieldCommit;
  readonly error: DiagnosticView | null;
  readonly onDismissError: () => void;
  readonly onError: (diagnostic: DiagnosticView) => void;
}

/** What a row's state chip says, per `EffectReason`. The two blocked chips are the whole reason this
 *  panel exists: authored, non-neutral, and contributing nothing because something outside the
 *  effect's own fields withholds what the shaft march needs — the scene's sun, or a medium to scatter
 *  in. */
const CHIP: Readonly<Record<EffectReason, { readonly text: string; readonly tone: string }>> = {
  live: { text: 'live', tone: 'ad-chip-live' },
  neutral: { text: 'off', tone: 'ad-chip-off' },
  'missing-sun': { text: 'no sun', tone: 'ad-chip-blocked' },
  'missing-medium': { text: 'no medium', tone: 'ad-chip-blocked' },
};

function entityLabel(target: MaterializationTarget): string {
  return `#${target.entity.index}:${target.entity.generation}`;
}

/** Why the A/B is offered, or why it is not. A disabled control that does not say what would enable
 *  it is indistinguishable from a broken one — which is exactly how an always-enabled A/B over an
 *  effect that is already off reads. */
function compareHint(state: EffectState): string {
  if (!isComparable(state.id)) {
    return 'The tonemap has no off — every operator is a curve, so there is nothing to compare against.';
  }
  if (state.reason === 'missing-sun') {
    return 'Authored, but contributing nothing: the shaft march reads a shadow map, and this scene has no directional light.';
  }
  if (state.reason === 'missing-medium') {
    return 'Authored, but contributing nothing: the beams are one term of the fog medium, and this scene has no fog density. Raise it and they appear.';
  }
  if (!state.live) {
    return 'Nothing to compare — this effect is already off. Raise its gate field and the comparison becomes live.';
  }
  return 'Hold to see the frame without this effect; release restores it exactly.';
}

export function ChainPanel(props: ChainPanelProps) {
  const { schema, values, buckets, open, onOpen, onCommit, onError } = props;

  // The Inspector's derivation, unchanged: schema in, one descriptor per field out. Fed the
  // *effective* values so an unauthored scene still shows the numbers it renders through.
  const fields = useMemo(() => (schema === null ? [] : deriveFields(schema, values)), [schema, values]);

  // Rows own their fields by name, so the flat descriptor list is indexed by its top-level key once.
  const byKey = useMemo(() => {
    const map = new Map<string, FieldDescriptor[]>();
    for (const field of fields) {
      const key = typeof field.path[0] === 'string' ? field.path[0] : '';
      const bucket = map.get(key);
      if (bucket === undefined) {
        map.set(key, [field]);
      } else {
        bucket.push(field);
      }
    }
    return map;
  }, [fields]);

  const renderFields = (names: readonly string[]): ReactNode => {
    if (schema === null) {
      return <p className="ad-hint">Waiting for the component schema…</p>;
    }
    return (
      <div className="ad-fields">
        {names.flatMap((name) =>
          (byKey.get(name) ?? []).map((field, index) => (
            <FieldLine
              key={`${name}:${index}`}
              field={field}
              value={valueAtPath(values, field.path)}
              authored={props.authoredFields.has(name)}
              commit={onCommit}
              onError={onError}
            />
          )),
        )}
      </div>
    );
  };

  return (
    <section className="ad-panel">
      <div className="panel-head">
        The chain
        <span className="count">{props.target === null ? 'no target' : entityLabel(props.target)}</span>
      </div>

      {/* Which entity holds the look, why that one, and whether it is authored — beside the handle the
          head names, because that is where the human is already looking. */}
      {props.target !== null && (
        <div className="ad-target">
          <span className="ad-target-rule">
            <code>PostProcess</code>{' '}
            {props.target.reason === 'environment'
              ? 'on the Environment carrier'
              : 'on the lowest entity id'}
          </span>
          <span className={props.authored ? 'ad-chip ad-chip-live' : 'ad-chip ad-chip-off'}>
            {props.authored ? 'authored' : 'not authored — shipped defaults'}
          </span>
        </div>
      )}

      <DiagnosticBanner diagnostic={props.error} onDismiss={props.onDismissError} />

      <div className="ad-scroll">
        {props.target === null && (
          <p className="ad-hint">
            No entity to hold the look yet — the chain shows the shipped defaults once the scene has one.
          </p>
        )}

        {props.disagreements.length > 0 && (
          <div className="ad-warn" role="alert">
            <b>The chain disagrees with the frame graph.</b>
            {props.disagreements.map((d) => (
              <span key={`${d.effect}:${d.pass}`}>
                {d.effect}: pass <code>{d.pass}</code>{' '}
                {d.expected ? 'should be in the graph and is not' : 'is in the graph and should not be'}
              </span>
            ))}
          </div>
        )}

        {buckets !== null && buckets.missing.length > 0 && (
          <div className="ad-warn" role="alert">
            <b>Fields this chain knows that the engine no longer reports:</b>
            <span>{buckets.missing.join(', ')}</span>
          </div>
        )}

        {props.effects.map((state) => {
          const row = effectRow(state.id);
          const isOpen = open === state.id;
          const chip = CHIP[state.reason];
          const held = props.holding === state.id;
          const names = buckets?.byEffect[state.id] ?? effectFields(state.id);
          return (
            <div key={state.id} className={isOpen ? 'ad-row ad-row-open' : 'ad-row'}>
              <div className="ad-row-head">
                <button
                  type="button"
                  className="ad-row-toggle"
                  aria-expanded={isOpen}
                  onClick={() => onOpen(isOpen ? null : state.id)}
                >
                  <span className="ad-caret" aria-hidden="true">
                    {isOpen ? '▾' : '▸'}
                  </span>
                  <span className="ad-label">{row.label}</span>
                  <span className={`ad-chip ${chip.tone}`}>{chip.text}</span>
                </button>
                {row.gatedPasses.length > 0 && (
                  <span className="ad-passes" title={`gates the frame-graph passes ${row.gatedPasses.join(', ')}`}>
                    passes
                  </span>
                )}
                <button
                  type="button"
                  className={held ? 'ad-compare ad-compare-held' : 'ad-compare'}
                  // An effect that is not contributing has nothing to take away, so the comparison
                  // would write the value it already holds and show an unchanged frame — a button
                  // that looks broken rather than one that says there is nothing here.
                  disabled={!isComparable(state.id) || !state.live || props.target === null}
                  title={compareHint(state)}
                  // Momentary, not a toggle: the press writes the neutral and the release puts the
                  // captured value back. `preventDefault` keeps the pointer-down from focusing the
                  // button and swallowing the release as a click.
                  onPointerDown={(event) => {
                    event.preventDefault();
                    props.onHoldStart(state.id);
                  }}
                  onPointerUp={props.onHoldEnd}
                  onPointerLeave={props.onHoldEnd}
                >
                  A/B
                </button>
              </div>
              {isOpen && renderFields(names)}
            </div>
          );
        })}

        {buckets !== null && buckets.other.length > 0 && (
          <div className={open === 'other' ? 'ad-row ad-row-open ad-other' : 'ad-row ad-other'}>
            <div className="ad-row-head">
              <button
                type="button"
                className="ad-row-toggle"
                aria-expanded={open === 'other'}
                onClick={() => onOpen(open === 'other' ? null : 'other')}
              >
                <span className="ad-caret" aria-hidden="true">
                  {open === 'other' ? '▾' : '▸'}
                </span>
                <span className="ad-label">Other</span>
                <span className="ad-chip ad-chip-off">{buckets.other.length} unassigned</span>
              </button>
            </div>
            {open === 'other' && renderFields(buckets.other)}
          </div>
        )}
      </div>
    </section>
  );
}

/** One field: its label, its widget, and — when the scene does not author it — the shipped default it
 *  is rendering through. The dimming is not decoration: three of the shipped defaults are live
 *  (`bloom_strength`, `exposure`, `dither`), so "unauthored" never means "off".
 *
 *  `gradeControl` claims the three graded triples for the band control; every other field — the Grade
 *  row's `contrast`, `saturation`, `temperature` and `tint` included — goes through `FieldWidget`. */
function FieldLine({
  field,
  value,
  authored,
  commit,
  onError,
}: {
  readonly field: FieldDescriptor;
  readonly value: unknown;
  readonly authored: boolean;
  readonly commit: FieldCommit;
  readonly onError: (diagnostic: DiagnosticView) => void;
}) {
  const blockKind = field.kind === 'json';
  const classes = ['ad-field', 'field'];
  if (blockKind) {
    classes.push('field-block');
  }
  if (!authored) {
    classes.push('ad-field-default');
  }
  const grade = gradeControl(field);
  return (
    <div className={classes.join(' ')}>
      {field.label !== '' && <span className="field-label">{field.label}</span>}
      {grade === null ? (
        <FieldWidget field={field} value={value} commit={commit} onError={onError} />
      ) : (
        <GradeBandControl
          band={grade.band}
          rows={grade.rows}
          label={field.label}
          path={field.path}
          value={value}
          commit={commit}
        />
      )}
      {!authored && (
        <span className="ad-default-tag" title="The engine's shipped value; the scene does not author it.">
          shipped default
        </span>
      )}
    </div>
  );
}
