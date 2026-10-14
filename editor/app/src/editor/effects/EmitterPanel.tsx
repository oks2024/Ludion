// The Effects view's left column: the tab strip, the forward-only time strip, and the subject's
// emitter fields.
//
// LAYOUT — three stacked blocks under one `panel-head`: the tabs (Emitter and Pattern), the time
// strip, then the open tab's own surface — one row per field of the subject's `ParticleEmitter`, or
// the pattern tab's document form (`PatternEditor.tsx`).
//
// TWO TABS, ONE TIME STRIP. Both surfaces are judged the same way — restart, pause, step one frame,
// slow it down — and a second copy of those five controls would be two things to keep in step for no
// gain. So the strip lives here, above the tabs' bodies, and only its Restart differs: on the
// emitter tab it re-stamps `ParticleEmitter.started`, on the pattern tab `PatternPlayer.started`.
// The loop toggle is the emitter's alone (a pattern document carries its own `loop:` in frames) and
// is disabled on the pattern tab rather than quietly meaning something else.
//
// THE FIELDS ARE THE GENERIC INSPECTOR, not a hand-written form — down to the row: `deriveFields`
// reads the schema `engine.describe` reports and the Level Designer's own `FieldRow` renders each
// descriptor, so a slider's travel is the component's own `x-ui-range` and a colour swatch is its
// own `_color` name. A field added to `ParticleEmitter` in Rust reaches this column with no edit
// here, and a `ParticleEmitter` field renders through the same component as any other — a
// look-alike row would only teach a human that they are different things.
//
// THE WRITE PATH IS THE EDITOR'S, and there is exactly one. A slider, the Restart button and the
// loop toggle all write one `ParticleEmitter` field; the loop fires from the poll in
// `EmitterEditor.tsx`, which cannot reach a hook held here, so `useComponentCommit` lives there
// (over the dialed client it requires) and this panel takes its `commit` and its `onRestart`. One
// hook, one assembly base, one `component.set`.
//
// TIME IS FORWARD-ONLY (`editor/effects/time.ts`): restart re-stamps `started` at the session's
// current frame, and pause/step/speed are the engine's own `sim.*` methods. There is no scrubber and
// no rewind — a particle is a pure function of `(emitter, sim time)`, so an earlier moment is
// reached by restarting and stepping forward again.
//
// The classes this panel introduces are prefixed `fx-`; the shell primitives (`panel-head`, `count`,
// `icon-btn`, `field*`, `num`) are reused as-is.

import { useCallback, useMemo, useState, type ReactNode } from 'react';
import type { Command } from '@ludion/protocol';
import type { ProtocolClient } from '../../protocol/client';
import { toDiagnostic, type DiagnosticView } from '../../viewer/diagnostic';
import type { FieldCommit } from '../../viewer/FieldWidgets';
import { deriveFields, valueAtPath } from '../../viewer/inspect-fields';
import { entityKey, FieldRow } from '../../viewer/Inspector';
import type { EntityId } from '../undo';
import { afterPauseToggle, afterStep, SPEEDS, withSpeed, type SimRun } from './time';

/** Which surface the column is showing. */
export type FxTab = 'emitter' | 'pattern';

/** The Pattern tab's tooltip: the one thing about it a human must know before clicking. */
const PATTERN_TITLE = 'A kind: pattern document — edits write the file immediately';

export interface EmitterPanelProps {
  /** The stage's own client, dialed — this panel is mounted by `EmitterEditor`, which exists only
   *  for a live session. The waiting state is the view's own, above this. */
  readonly client: ProtocolClient;
  /** The stage scene's `subject`, or `null` before the query answers. */
  readonly entity: EntityId | null;
  /** `ParticleEmitter`'s JSON Schema from `engine.describe`, or `null` before it arrives. */
  readonly schema: unknown | null;
  /** What the rows show: the editor's commit hook's effective value (a just-sent edit over the last
   *  polled one), or `null` before the first poll answers. */
  readonly value: Record<string, unknown> | null;
  /** The sim frame the latest poll reply applied at. */
  readonly frame: number | null;
  readonly run: SimRun;
  readonly loop: boolean;
  /** The editor's one write path — the same one Restart goes through. */
  readonly commit: FieldCommit;
  readonly onRestart: () => void;
  /** Restart on the pattern tab: re-stamps `PatternPlayer.started` instead. */
  readonly onRestartPattern: () => void;
  /** The pattern tab's surface, mounted only while that tab is open. */
  readonly pattern: ReactNode;
  readonly onRun: (run: SimRun) => void;
  readonly onLoop: (loop: boolean) => void;
  readonly onError: (diagnostic: DiagnosticView) => void;
}

export function EmitterPanel(props: EmitterPanelProps) {
  const { client, entity, schema, value, run, onRun, onError } = props;
  const [tab, setTab] = useState<FxTab>('emitter');

  const fields = useMemo(
    () => (schema === null || value === null ? [] : deriveFields(schema, value)),
    [schema, value],
  );

  /** One `sim.*` write. The strip's state moves only once the engine has taken the command — a
   *  refused one must not leave the buttons describing a session state that never happened. */
  const send = useCallback(
    (command: Command, applied: () => void): void => {
      void client
        .call(command)
        .then(applied)
        .catch((err: unknown) => onError(toDiagnostic(err)));
    },
    [client, onError],
  );

  const togglePause = useCallback((): void => {
    send({ method: run.paused ? 'sim.resume' : 'sim.pause' }, () => onRun(afterPauseToggle(run)));
  }, [send, run, onRun]);

  // One tick, and the engine pauses after it — which is what makes a burst readable frame by frame.
  const step = useCallback((): void => {
    send({ method: 'sim.step', params: { ticks: 1 } }, () => onRun(afterStep(run)));
  }, [send, run, onRun]);

  const setSpeed = useCallback(
    (speed: number): void => {
      send({ method: 'sim.set_speed', params: { speed } }, () => onRun(withSpeed(run, speed)));
    },
    [send, run, onRun],
  );

  // Restart and the loop stamp `started` with a frame, so both need the component the stamp lands on
  // and the schema the row is assembled from.
  const editable = entity !== null && schema !== null;

  return (
    <section className="fx-panel">
      <div className="panel-head">
        Effects
        <span className="count">{entity === null ? 'no subject' : `#${entityKey(entity)}`}</span>
      </div>

      <div className="fx-tabs" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'emitter'}
          className={tab === 'emitter' ? 'fx-tab fx-tab-on' : 'fx-tab'}
          onClick={() => setTab('emitter')}
        >
          Emitter
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'pattern'}
          className={tab === 'pattern' ? 'fx-tab fx-tab-on' : 'fx-tab'}
          title={PATTERN_TITLE}
          onClick={() => setTab('pattern')}
        >
          Pattern
        </button>
      </div>

      <div className="fx-time">
        <button
          type="button"
          className="icon-btn"
          disabled={!editable}
          onClick={tab === 'emitter' ? props.onRestart : props.onRestartPattern}
        >
          Restart
        </button>
        <button type="button" className="icon-btn" onClick={togglePause}>
          {run.paused ? 'Resume' : 'Pause'}
        </button>
        <button type="button" className="icon-btn" onClick={step}>
          Step
        </button>
        <label className="fx-speed">
          <span className="field-label">speed</span>
          <select
            className="fx-select"
            value={run.speed}
            onChange={(event) => setSpeed(Number(event.target.value))}
          >
            {SPEEDS.map((speed) => (
              <option key={speed} value={speed}>
                ×{speed}
              </option>
            ))}
          </select>
        </label>
        <label className="fx-loop">
          <input
            className="field-checkbox"
            type="checkbox"
            checked={props.loop}
            disabled={!editable || tab === 'pattern'}
            onChange={(event) => props.onLoop(event.target.checked)}
          />
          loop
        </label>
        <span className="fx-frame">
          frame <span className="num">{props.frame ?? '—'}</span>
        </span>
      </div>

      <div className="fx-scroll">
        {tab === 'pattern' ? (
          props.pattern
        ) : fields.length === 0 ? (
          <p className="fx-hint">
            {entity === null
              ? 'The stage scene carries no `subject` entity.'
              : 'Reading the emitter…'}
          </p>
        ) : (
          <div className="component-fields">
            {fields.map((field, i) => (
              <FieldRow
                key={`${field.kind}:${field.label}:${i}`}
                field={field}
                current={valueAtPath(value, field.path)}
                commit={props.commit}
                onError={onError}
              />
            ))}
          </div>
        )}
      </div>
    </section>
  );
}
