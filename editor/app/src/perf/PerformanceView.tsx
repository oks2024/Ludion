// The Performance view — where the running frame goes, live.
//
// SUBSCRIBING IS THE ARMING SWITCH. `Channel::Profile` has no start/stop verb: the engine's live
// profiler is armed by the fact that someone is listening and disarmed when nobody is (a disconnect
// disarms it for free, exactly like the `frames` channel). So this view's cost is visibility-gated by
// construction — mount subscribes, unmount unsubscribes, and there is deliberately no poll anywhere
// in it. Windows arrive as events at roughly 1 Hz because they close on 60 sim FRAMES, not on a
// timer: a paused sim emits none, and the timeline simply stops advancing rather than inventing a
// flat line. Re-subscription after a reconnect is free for the same reason it is everywhere else in
// this app — App swaps the `client` object, and the effect below is keyed on it.
//
// THE CAPTURE RUNS ON ITS OWN CONNECTION, and that is not an optimisation. A `profile.capture` reply
// is deferred until its window closes, and a connection answers its requests in order, so the whole
// connection is blocked for the capture's duration (~1 s at 60 frames, ten times that at 600). This
// view shares App's client with the session strip, the context bridge and whatever tab is mounted
// next, so a capture on it would freeze saving, undo and the inspector for as long as it ran. A
// short-lived second `ProtocolClient` costs one socket for the duration and blocks nothing (the
// Voxel Artist's own stage client is the precedent for a view owning a connection). It also means a
// capture that is abandoned mid-flight dies with its socket instead of with the editor.
//
// THE GAME PREVIEW COSTS NO GPU TIME, which is what lets it sit beside GPU numbers. Three facts,
// each structural rather than a promise: no client can change the render resolution (`stream.subscribe`
// carries a channel and a delivery mode, and the 27-method surface has no resize verb); the frame is
// already rendered because an armed profiler counts as a watcher, so while this view is subscribed the
// engine renders every frame and discards the image when nobody streams — the preview stops the
// discard, it does not add a render; and the client scales for display only, the canvas keeping the
// frame's own pixel size. It is NOT free of TRANSPORT cost: the QOI encode and the socket write are
// real and are this viewer's, which is why the Transport group is absent until something streams and
// why the preview completes the profile rather than distorting it. The frame's true pixel dimensions
// ride the preview's own chip so that claim is readable on screen, not just here.
//
// Classes are prefixed `pf-`; shared shell primitives (`main-row`, `col`, `col-side`, `col-center`,
// `panel-head`, `icon-btn`, `num`) are reused as-is. What each `pf-` class is:
//   pf-panel        the breakdown panel filling the left column (flex column, min-height 0)
//   pf-hero         the frame-cost stat at the top of that panel — the one large number in the view
//   pf-hero-value   that number, mono and tabular; pf-hero-over when the frame missed the budget
//   pf-hero-unit    the `ms` beside it, small and muted
//   pf-hero-note    the budget line under it, muted
//   pf-scroll       the scrolling group list under the hero
//   pf-group        one breakdown group (systems / host spans / GPU passes / transport)
//   pf-group-head   its name and total on one line
//   pf-group-note   the quiet line under that head (coverage, or why the group reads as it does)
//   pf-bar-row      one entry: swatch, name, track, value
//   pf-swatch       the 8px colour chip carrying series identity
//   pf-name         the entry's name, ellipsised rather than wrapped
//   pf-track        the bar's ground, pf-fill the bar itself (rounded at the data end)
//   pf-value        the ms readout, right-aligned and tabular
//   pf-center       the centre column's wrapper
//   pf-toolbar      the strip above the plot: lane switch left, capture controls right
//   pf-lanes        the segmented lane switch, pf-lane one button, pf-lane-on the active one
//   pf-frames       the capture-length select in that strip
//   pf-plot         the positioned canvas box (TimelineCanvas), pf-canvas the canvas itself
//   pf-axis-max     the y-scale's top label, pf-axis-guide the 16.6 ms line's label
//   pf-readout      the fixed-height strip under the plot: hovered frame, or the ring's span
//   pf-readout-entry one series inside that strip (swatch, name, value)
//   pf-legend       the series legend, pf-legend-item one toggle, pf-legend-off a hidden series
//   pf-note         a quiet explanatory line (waiting, paused, no timestamp support)
//   pf-report       the pinned capture card and its save actions
//   pf-preview      the game preview under the breakdown, pf-preview-head its `panel-head` variant
//   pf-preview-stage the 16:9 box the frame fills, pf-preview-off its placeholder when toggled off
//   pf-preview-note the one line saying what streaming here does to the Transport group

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ProfileReport } from '@ludion/protocol';
import { ProtocolClient } from '../protocol/client';
import { openSubscription } from '../protocol/subscription';
import { DiagnosticBanner } from '../viewer/DiagnosticBanner';
import { toDiagnostic, type DiagnosticView } from '../viewer/diagnostic';
import { isTauri } from '../shell/tauri';
import { writeEditorFile } from '../terminal/pty';
import { colorFor, createRegistry, PALETTE_SLOTS, slotFor } from './colors';
import {
  emptyRing,
  extractSeries,
  foldSeries,
  formatMs,
  FRAME_BUDGET_MS,
  laneSeries,
  latestBreakdown,
  OTHER_KEY,
  pushWindow,
  type ColumnReadout,
  type Lane,
  type PerfWindow,
  type WindowRing,
} from './model';
import { traceFromReport, traceFromRing } from './trace';
import { TimelineCanvas } from './TimelineCanvas';
import { PreviewPanel } from './PreviewPanel';
import type { EditorMode } from '../App';
import type { FrameStatus } from '../viewer/useFrameStream';

export interface PerformanceViewProps {
  readonly client: ProtocolClient;
  /** Gates the preview's input forwarding, exactly as it gates the viewport's: edit mode pauses the
   *  sim, and keystrokes are gameplay input. */
  readonly mode: EditorMode;
  /** The game session's URL — the capture dials its own connection to it (see the header). */
  readonly url: string;
  readonly sceneName: string | null;
  /** Where a saved trace lands, under `.ludion/`. Empty (or plain-browser dev) falls back to a
   *  download. */
  readonly projectDir: string;
  readonly onFrameStatus?: (status: FrameStatus) => void;
}

/** How long without a window before the view says so. Two windows' worth at the default tick rate:
 *  long enough that a slow frame never trips it, short enough that a pause is not a mystery. */
const STALE_AFTER_MS = 2500;

/** The capture lengths on offer. 60 frames is one window; 600 is ten seconds of a run, which is what
 *  a spike hunt wants and what a Perfetto trace is worth opening for. */
const CAPTURE_LENGTHS = [60, 300, 600] as const;

/** localStorage key for the preview toggle. Persisted rather than component state: a human who turned
 *  the preview off to read a viewer-free baseline must not have it back the next time this tab is
 *  opened, and the view remounts on every tab switch. */
const PREVIEW_KEY = 'ludion.editor.perfPreview';

const LANES: readonly { readonly id: Lane; readonly label: string }[] = [
  { id: 'sim', label: 'Sim' },
  { id: 'gpu', label: 'GPU' },
  { id: 'transport', label: 'Transport' },
];

/** A `profile.capture` response body, or `null` for anything else. */
function reportOf(body: { readonly kind?: unknown }): ProfileReport | null {
  return body.kind === 'profile_report' ? ((body as { report: ProfileReport }).report ?? null) : null;
}

export function PerformanceView({ client, mode, url, sceneName, projectDir, onFrameStatus }: PerformanceViewProps) {
  const [ring, setRing] = useState<WindowRing>(emptyRing);
  const [lane, setLane] = useState<Lane>('sim');
  const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
  const [hover, setHover] = useState<ColumnReadout | null>(null);
  const [stale, setStale] = useState(false);
  const [subscribed, setSubscribed] = useState(false);
  const [frames, setFrames] = useState<number>(CAPTURE_LENGTHS[0]);
  const [capturing, setCapturing] = useState(false);
  const [report, setReport] = useState<ProfileReport | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<DiagnosticView | null>(null);
  const [preview, setPreview] = useState<boolean>(() => {
    try {
      return localStorage.getItem(PREVIEW_KEY) !== 'off';
    } catch {
      return true;
    }
  });

  const togglePreview = useCallback((): void => {
    const next = !preview;
    setPreview(next);
    try {
      localStorage.setItem(PREVIEW_KEY, next ? 'on' : 'off');
    } catch {
      // Private-mode / storage-disabled: keep it in memory for this session.
    }
  }, [preview]);

  // One registry for the whole session: a name keeps its colour while its rank moves under it.
  const registryRef = useRef(createRegistry());

  // --- the subscription (and, with it, the engine's arming) ---------------------------------------
  useEffect(() => {
    setRing(emptyRing());
    setHover(null);

    const off = client.onEvent((event) => {
      if (event.event !== 'profile' || !sub.accepts(event.sub)) {
        return;
      }
      setRing((current) => pushWindow(current, event.data as unknown as PerfWindow));
    });

    const sub = openSubscription(client, 'profile', {
      onSubscribed: () => setSubscribed(true),
      onError: (err) => setError(toDiagnostic(err)),
    });

    return () => {
      setSubscribed(false);
      off();
      // Closing also drops the engine's arming; a reply that races this cleanup is unsubscribed on arrival.
      sub.close();
    };
  }, [client]);

  // "No new windows" is a real state with a real cause (windows close on frames, so a paused sim
  // closes none) and it needs saying. Armed off each arrival rather than polled.
  useEffect(() => {
    setStale(false);
    const timer = window.setTimeout(() => setStale(true), STALE_AFTER_MS);
    return () => window.clearTimeout(timer);
  }, [ring]);

  // --- what the chart draws ----------------------------------------------------------------------
  const set = useMemo(() => extractSeries(ring), [ring]);
  const ranked = useMemo(() => laneSeries(set, lane), [set, lane]);
  const bands = useMemo(
    () => foldSeries(ranked.filter((s) => !hidden.has(s.key)), PALETTE_SLOTS),
    [ranked, hidden],
  );
  const breakdown = useMemo(() => latestBreakdown(ring, PALETTE_SLOTS), [ring]);
  const laneHasData = useMemo(
    () => new Map(LANES.map(({ id }) => [id, laneSeries(set, id).length > 0])),
    [set],
  );

  const slotOf = useCallback(
    (key: string): number | null => (key === OTHER_KEY ? null : slotFor(registryRef.current, key)),
    [],
  );
  const swatch = useCallback((key: string): string => colorFor(registryRef.current, key), []);

  const toggle = useCallback((key: string): void => {
    setHidden((current) => {
      const next = new Set(current);
      if (!next.delete(key)) {
        next.add(key);
      }
      return next;
    });
  }, []);

  // --- the capture, on its own connection --------------------------------------------------------
  const probeRef = useRef<ProtocolClient | null>(null);
  useEffect(
    () => () => {
      probeRef.current?.close();
      probeRef.current = null;
    },
    [],
  );

  const capture = useCallback((): void => {
    if (probeRef.current !== null) {
      return;
    }
    const probe = new ProtocolClient();
    probeRef.current = probe;
    setCapturing(true);
    setSaved(null);
    void (async () => {
      try {
        await probe.connect(url);
        const body = await probe.call({
          method: 'profile.capture',
          params: { frames, include_samples: true },
        });
        const captured = reportOf(body);
        if (captured !== null) {
          setReport(captured);
          setError(null);
        }
      } catch (err) {
        setError(toDiagnostic(err));
      } finally {
        probe.close();
        if (probeRef.current === probe) {
          probeRef.current = null;
        }
        setCapturing(false);
      }
    })();
  }, [url, frames]);

  // --- saving ------------------------------------------------------------------------------------
  const save = useCallback(
    (name: string, text: string): void => {
      if (isTauri() && projectDir !== '') {
        const path = `${projectDir}/.ludion/${name}`;
        void writeEditorFile(path, text)
          .then(() => setSaved(path))
          .catch((err: unknown) => setError(toDiagnostic(err)));
        return;
      }
      // Plain-browser dev has no host to write through; hand the bytes to the browser instead.
      const href = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
      const anchor = document.createElement('a');
      anchor.href = href;
      anchor.download = name;
      anchor.click();
      // Deferred: revoking in the same task can cancel a download that has only just been handed to
      // the browser.
      window.setTimeout(() => URL.revokeObjectURL(href), 0);
      setSaved(name);
    },
    [projectDir],
  );

  const captureStem = useMemo(() => {
    const first = report?.samples?.frames[0];
    const last = report?.samples?.frames.at(-1);
    const span = first === undefined || last === undefined ? `${report?.measured_frames ?? 0}f` : `f${first}-${last}`;
    return `profile-${(sceneName ?? 'scene').replace(/[^\w.-]+/g, '-')}-${span}`;
  }, [report, sceneName]);

  const savePerfetto = useCallback((): void => {
    if (report === null) {
      return;
    }
    save(`${captureStem}.trace.json`, JSON.stringify(traceFromReport({ ...report, samples: report.samples ?? null })));
  }, [report, captureStem, save]);

  const saveJson = useCallback((): void => {
    if (report === null) {
      return;
    }
    save(`${captureStem}.json`, JSON.stringify(report, null, 2));
  }, [report, captureStem, save]);

  const saveStreamTrace = useCallback((): void => {
    save(`profile-stream-f${set.frames[0] ?? 0}-${set.frames.at(-1) ?? 0}.trace.json`, JSON.stringify(traceFromRing(ring, sceneName)));
  }, [ring, set, sceneName, save]);

  // --- the panel ---------------------------------------------------------------------------------
  const overBudget = breakdown !== null && breakdown.cpuMs > FRAME_BUDGET_MS;

  return (
    <div className="main-row">
      <div className="col col-side">
        <div className="pf-panel">
          <div className="panel-head">
            Frame cost
            {breakdown !== null && (
              <span className="count">
                {breakdown.frames} frames · f{breakdown.firstFrame}–{breakdown.lastFrame}
              </span>
            )}
          </div>

          <div className="pf-hero">
            <div className={overBudget ? 'pf-hero-value pf-hero-over num' : 'pf-hero-value num'}>
              {formatMs(breakdown?.cpuMs)} <span className="pf-hero-unit">ms</span>
            </div>
            <div className="pf-hero-note">
              CPU per frame · tick {formatMs(breakdown?.frameTotalMs)} + host spans · budget 16.6 ms
              {breakdown !== null && breakdown.threads !== null && (
                <>
                  {' · '}
                  {breakdown.threads} sim {breakdown.threads === 1 ? 'worker' : 'workers'}
                </>
              )}
            </div>
          </div>

          <div className="pf-scroll">
            {breakdown === null ? (
              <p className="pf-note">
                {subscribed
                  ? 'Waiting for the first window. One closes every 60 sim frames — about a second at the default tick rate.'
                  : 'Subscribing to the profile channel…'}
              </p>
            ) : (
              breakdown.groups.map((group) => (
                <div className="pf-group" key={group.id}>
                  <div className="pf-group-head">
                    <span>{group.label}</span>
                    <span className="pf-value num">{formatMs(group.totalMs)}</span>
                  </div>
                  {group.note !== null && <div className="pf-group-note">{group.note}</div>}
                  {group.entries.map((entry) => (
                    <div className="pf-bar-row" key={entry.key}>
                      <span className="pf-swatch" style={{ background: swatch(entry.key) }} />
                      <span className="pf-name" title={entry.name}>
                        {entry.name}
                      </span>
                      <span className="pf-track">
                        <span
                          className="pf-fill"
                          style={{ width: `${Math.max(entry.share * 100, entry.ms > 0 ? 1.5 : 0)}%`, background: swatch(entry.key) }}
                        />
                      </span>
                      <span className="pf-value num">{formatMs(entry.ms)}</span>
                    </div>
                  ))}
                </div>
              ))
            )}

            {breakdown !== null && !breakdown.timestampsSupported && (
              <p className="pf-note">
                This adapter reports no <code>TIMESTAMP_QUERY</code> support, so per-pass GPU time is
                unavailable. Everything else is measured normally.
              </p>
            )}
            {stale && (
              <p className="pf-note">
                No new windows. They close on sim frames, so a paused sim closes none — resume the sim
                or leave edit mode to keep the timeline moving.
              </p>
            )}
          </div>
        </div>

        <div className="pf-preview">
          <div className="panel-head pf-preview-head">
            Game preview
            <button type="button" className="icon-btn" aria-pressed={preview} onClick={togglePreview}>
              {preview ? 'Hide' : 'Show'}
            </button>
          </div>
          <PreviewPanel client={preview ? client : null} mode={mode} onFrameStatus={onFrameStatus} />
          <p className="pf-preview-note">
            {preview
              ? 'Streaming is what fills the Transport group: the encode and send it measures are this viewer’s own cost. The frame itself is rendered full size either way.'
              : 'Transport stays absent until something streams — the engine still renders every frame while the profiler is armed, it just drops the image.'}
          </p>
        </div>
      </div>

      <div className="col-center pf-center">
        <div className="pf-toolbar">
          <div className="pf-lanes" role="group" aria-label="Timeline lane">
            {LANES.map(({ id, label }) => (
              <button
                type="button"
                key={id}
                className={id === lane ? 'pf-lane pf-lane-on' : 'pf-lane'}
                onClick={() => setLane(id)}
                disabled={laneHasData.get(id) !== true}
                title={laneHasData.get(id) === true ? undefined : 'nothing measured on this lane yet'}
              >
                {label}
              </button>
            ))}
          </div>

          <div className="head-actions">
            <select
              className="pf-frames"
              aria-label="Capture length"
              value={frames}
              onChange={(event) => setFrames(Number(event.target.value))}
              disabled={capturing}
            >
              {CAPTURE_LENGTHS.map((n) => (
                <option key={n} value={n}>
                  {n} frames
                </option>
              ))}
            </select>
            <button type="button" className="icon-btn" onClick={capture} disabled={capturing}>
              {capturing ? 'Capturing…' : 'Capture'}
            </button>
            <button type="button" className="icon-btn" onClick={saveStreamTrace} disabled={ring.windows.length === 0}>
              Save timeline trace
            </button>
          </div>
        </div>

        <div className="app-banner">
          <DiagnosticBanner diagnostic={error} onDismiss={() => setError(null)} />
        </div>

        <TimelineCanvas set={set} series={bands} slotOf={slotOf} showTotal={lane === 'sim'} onHover={setHover} />

        <div className="pf-readout">
          {hover === null ? (
            <span>
              {set.frames.length === 0
                ? 'no frames yet'
                : `frames ${set.frames[0]}–${set.frames.at(-1)} · ${ring.windows.length} window${ring.windows.length === 1 ? '' : 's'} · hover the plot to read one`}
            </span>
          ) : (
            <>
              <span className="num">frame {hover.frame}</span>
              <span className="sep">·</span>
              <span>
                stacked <span className="num">{formatMs(hover.stackedMs)} ms</span>
              </span>
              {hover.totalMs !== null && (
                <>
                  <span className="sep">·</span>
                  <span>
                    tick <span className="num">{formatMs(hover.totalMs)} ms</span>
                  </span>
                </>
              )}
              {hover.entries.slice(0, 3).map((entry) => (
                <span className="pf-readout-entry" key={entry.key}>
                  <span className="pf-swatch" style={{ background: swatch(entry.key) }} />
                  {entry.name} <span className="num">{formatMs(entry.ms)}</span>
                </span>
              ))}
            </>
          )}
        </div>

        <div className="pf-legend">
          {bands.map((series) =>
            // `Other` is a sum, not a series: there is nothing to hide and nothing to name, so it is
            // a chip rather than a toggle.
            series.key === OTHER_KEY ? (
              <span className="pf-legend-item pf-legend-static" key={series.key} title="everything past the eighth series, summed">
                <span className="pf-swatch" style={{ background: swatch(series.key) }} />
                {series.name}
                <span className="num">{formatMs(series.medianMs)}</span>
              </span>
            ) : (
              <button type="button" className="pf-legend-item" key={series.key} onClick={() => toggle(series.key)}>
                <span className="pf-swatch" style={{ background: swatch(series.key) }} />
                {series.name}
                <span className="num">{formatMs(series.medianMs)}</span>
              </button>
            ),
          )}
          {ranked
            .filter((series) => hidden.has(series.key))
            .map((series) => (
              <button
                type="button"
                className="pf-legend-item pf-legend-off"
                key={series.key}
                onClick={() => toggle(series.key)}
              >
                <span className="pf-swatch" style={{ background: swatch(series.key) }} />
                {series.name}
              </button>
            ))}
        </div>

        {report !== null && (
          <div className="pf-report">
            <div className="pf-group-head">
              <span>
                Capture · <span className="num">{report.measured_frames}</span> frames · {report.scene}
              </span>
              <div className="head-actions">
                <button type="button" className="icon-btn" onClick={savePerfetto}>
                  Save Perfetto trace
                </button>
                <button type="button" className="icon-btn" onClick={saveJson}>
                  Save JSON
                </button>
                <button type="button" className="icon-btn" onClick={() => setReport(null)}>
                  clear
                </button>
              </div>
            </div>
            <div className="pf-group-note">
              tick <span className="num">{formatMs(report.sim.frame_total_median_ms)} ms</span> ·{' '}
              {report.sim.systems.length} systems ·{' '}
              {report.gpu.timestamps_supported ? `${report.gpu.passes.length} GPU passes` : 'no GPU timestamps'}
              {saved !== null && <> · saved to <code>{saved}</code></>}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
