// The clip strip: the transport and the tick-quantized scrub bar that sit under the stage viewport.
//
// IT IS THE EFFECTS TIME STRIP PLUS A BAR. The transport row is that strip's own markup — the same
// `fx-time` row, `icon-btn` transport, `fx-speed` select and `fx-loop` checkbox — because the two
// stages are judged the same way and a look-alike row would only teach a human they are different
// things. What is new is the second row, and it is new because a voxel clip can be scrubbed: with
// the sim stopped a pose is a pure function of one tick, so the bar can go backwards where an
// emitter's particles never can.
//
// THE BAR IS IN TICKS, NOT SECONDS. Its span is `[0, durationTicks]`, its marks are the clip's own
// authored keyframe times, and a mark whose time is not a 60 Hz multiple is flagged: that pose never
// renders as authored, and nothing else in the toolchain says so (`ludion docs voxel-animation`).
// The label is the CLI's phase-report wording verbatim, so a human reading the bar and an agent
// reading `render --strip` stdout quote the same instant.
//
// No state of its own beyond the pointer capture: everything here is `useClipPlayback`'s, so what
// the strip shows is what the session holds.

import { useCallback, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { SPEEDS } from '../effects/time';
import { tickLabel, type KeyframeMark } from './anim';
import { barRatio, tickAtRatio } from './clipPlayback';
import type { ClipPlayback } from './useClipPlayback';

/** Everything the strip needs is what the hook returns: `<ClipStrip {...playback} />`. */
export type ClipStripProps = ClipPlayback;

/** The blend length the crossfade row starts at, and its smallest step. */
const DEFAULT_FADE_SECONDS = 0.2;
const FADE_STEP_SECONDS = 0.05;

/** How far one arrow key moves the playhead, and how far a shifted one does. */
const ARROW_TICKS = 1;
const ARROW_TICKS_FAST = 10;

function markTitle(mark: KeyframeMark): string {
  return mark.onTick
    ? `keyframe at ${tickLabel(mark.tick)}`
    : `keyframe authored at t ${mark.t}, which is not a 60 Hz tick multiple: it samples at tick ${mark.tick} and never renders exactly as authored`;
}

export function ClipStrip(props: ClipStripProps) {
  const { clips, clip, clipTicks, enabled, marks, tick, onScrub } = props;
  const staged = enabled && clip !== null;
  const dragRef = useRef<number | null>(null);
  const [fadeTarget, setFadeTarget] = useState<string | null>(null);
  const [fadeSeconds, setFadeSeconds] = useState(DEFAULT_FADE_SECONDS);
  const others = clips.filter((binding) => binding.name !== clip);
  const target = others.find((binding) => binding.name === fadeTarget) ?? others[0];

  const tickAt = useCallback(
    (event: PointerEvent<HTMLDivElement>): number => {
      const rect = event.currentTarget.getBoundingClientRect();
      return tickAtRatio(
        rect.width <= 0 ? 0 : (event.clientX - rect.left) / rect.width,
        clipTicks,
      );
    },
    [clipTicks],
  );

  const onPointerDown = useCallback(
    (event: PointerEvent<HTMLDivElement>): void => {
      if (event.button !== 0 || !staged) {
        return;
      }
      dragRef.current = event.pointerId;
      event.currentTarget.setPointerCapture(event.pointerId);
      event.currentTarget.focus();
      event.preventDefault();
      onScrub(tickAt(event));
    },
    [onScrub, staged, tickAt],
  );

  const onPointerMove = useCallback(
    (event: PointerEvent<HTMLDivElement>): void => {
      if (dragRef.current !== null) {
        onScrub(tickAt(event));
      }
    },
    [onScrub, tickAt],
  );

  // The gesture's final tick is sent unthrottled: the bar must never come to rest somewhere the
  // human did not leave it.
  const endDrag = useCallback(
    (event: PointerEvent<HTMLDivElement>): void => {
      const pointerId = dragRef.current;
      if (pointerId === null) {
        return;
      }
      dragRef.current = null;
      if (event.currentTarget.hasPointerCapture(pointerId)) {
        event.currentTarget.releasePointerCapture(pointerId);
      }
      onScrub(tickAt(event), true);
    },
    [onScrub, tickAt],
  );

  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>): void => {
      if (!staged) {
        return;
      }
      const step = event.shiftKey ? ARROW_TICKS_FAST : ARROW_TICKS;
      const target =
        event.key === 'ArrowLeft'
          ? tick - step
          : event.key === 'ArrowRight'
            ? tick + step
            : event.key === 'Home'
              ? 0
              : event.key === 'End'
                ? clipTicks
                : null;
      if (target === null) {
        return;
      }
      event.preventDefault();
      onScrub(target, true);
    },
    [clipTicks, onScrub, staged, tick],
  );

  if (clips.length === 0) {
    return (
      <section className="vx-clip">
        <p className="vx-clip-empty">
          This model binds no clips — its <code>clips:</code> block is where one is named.
        </p>
      </section>
    );
  }

  return (
    <section className="vx-clip" aria-label="Clip playback">
      <div className="fx-time">
        <div className="vx-clip-chips" role="group" aria-label="Clip">
          <button
            type="button"
            className={clip === null ? 'fx-tab fx-tab-on' : 'fx-tab'}
            disabled={!enabled}
            aria-pressed={clip === null}
            title="The rest pose: the Animation component comes off the subject."
            onClick={() => props.onSelectClip(null)}
          >
            rest
          </button>
          {clips.map((binding) => (
            <button
              key={binding.name}
              type="button"
              className={clip === binding.name ? 'fx-tab fx-tab-on' : 'fx-tab'}
              disabled={!enabled}
              aria-pressed={clip === binding.name}
              title={binding.file}
              onClick={() => props.onSelectClip(binding.name)}
            >
              {binding.name}
            </button>
          ))}
        </div>

        <button type="button" className="icon-btn" disabled={!enabled} onClick={props.onTogglePlay}>
          {props.playing ? 'Pause' : 'Play'}
        </button>
        <button type="button" className="icon-btn" disabled={!staged} onClick={props.onRestart}>
          Restart
        </button>

        <label className="fx-speed">
          <span className="field-label">speed</span>
          <select
            className="fx-select"
            disabled={!enabled}
            value={props.speed}
            onChange={(event) => props.onSpeed(Number(event.target.value))}
          >
            {SPEEDS.map((speed) => (
              <option key={speed} value={speed}>
                ×{speed}
              </option>
            ))}
          </select>
        </label>

        {/* A clip that loops does so in the engine, so its box shows that state fixed rather than
            offering a toggle that would quietly mean something else. */}
        <label
          className="fx-loop"
          title={
            props.loops
              ? 'This clip carries `loop: true` — the engine wraps it.'
              : 'Restart this one-shot when it runs out.'
          }
        >
          <input
            className="field-checkbox"
            type="checkbox"
            checked={props.loops || props.loopOneShot}
            disabled={!staged || props.loops}
            onChange={(event) => props.onLoopOneShot(event.target.checked)}
          />
          loop
        </label>

        <span className="fx-frame">
          frame <span className="num">{props.frame ?? '—'}</span>
        </span>
      </div>

      {clips.length >= 2 && staged && target !== undefined && (
        <div className="fx-time">
          <label className="fx-speed">
            <span className="field-label">crossfade to</span>
            <select
              className="fx-select"
              value={target.name}
              onChange={(event) => setFadeTarget(event.target.value)}
            >
              {others.map((binding) => (
                <option key={binding.name} value={binding.name}>
                  {binding.name}
                </option>
              ))}
            </select>
          </label>
          <label className="fx-speed">
            <span className="field-label">seconds</span>
            <input
              className="fx-select"
              type="number"
              min={FADE_STEP_SECONDS}
              step={FADE_STEP_SECONDS}
              value={fadeSeconds}
              onChange={(event) => setFadeSeconds(Number(event.target.value))}
            />
          </label>
          <button
            type="button"
            className="icon-btn"
            title="Blends from the staged clip into the chosen one over the given length."
            onClick={() => props.onCrossfade(target.name, fadeSeconds)}
          >
            Crossfade
          </button>
          {props.fade !== null && (
            <span className="fx-frame">
              from <span className="num">{props.fade.from}</span>
              <span className="sep">·</span>
              <span className="num">{props.fade.seconds}</span> s
            </span>
          )}
        </div>
      )}

      <div className="vx-clip-bar-row">
        <div
          className={staged ? 'vx-clip-bar' : 'vx-clip-bar vx-clip-bar-off'}
          role="slider"
          tabIndex={staged ? 0 : -1}
          aria-label="Clip time"
          aria-valuemin={0}
          aria-valuemax={clipTicks}
          aria-valuenow={tick}
          aria-valuetext={tickLabel(tick)}
          aria-disabled={!staged}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onKeyDown={onKeyDown}
        >
          <div className="vx-clip-track" />
          {props.fade !== null && (
            <span
              className="vx-clip-fade"
              style={{ width: `${barRatio(props.fadeTicks, clipTicks) * 100}%` }}
              title={`crossfade from ${props.fade.from}: ${props.fade.seconds} s`}
            />
          )}
          {marks.map((mark) => (
            <span
              key={`${mark.t}`}
              className={mark.onTick ? 'vx-clip-mark' : 'vx-clip-mark off'}
              style={{ left: `${barRatio(mark.tick, clipTicks) * 100}%` }}
              title={markTitle(mark)}
            />
          ))}
          <span
            className="vx-clip-head"
            style={{ left: `${barRatio(tick, clipTicks) * 100}%` }}
            aria-hidden="true"
          />
        </div>

        <span className="vx-clip-label">
          <span className="num">{staged ? tickLabel(tick) : 'rest pose'}</span>
          {staged && (
            <>
              <span className="sep">·</span>
              {props.duration} s
              <span className="sep">·</span>
              {props.loops ? 'loops' : 'one-shot'}
            </>
          )}
        </span>
      </div>

      {props.error !== null && (
        <p className="vx-clip-error">
          {props.error.code !== null && <span className="vx-diag-code">{props.error.code}</span>}
          {props.error.message}
          <button type="button" className="icon-btn" onClick={props.onDismissError}>
            Dismiss
          </button>
        </p>
      )}
    </section>
  );
}
