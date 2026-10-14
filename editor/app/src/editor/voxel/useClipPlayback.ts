// The clip strip's session: the state machine in `clipPlayback.ts` bound to one stage client, the
// selected clip's document, and the session's own clock.
//
// EVERYTHING ARITHMETIC IS IN THE PURE MODULE. This hook holds three things the machine cannot: the
// wire (one command per intent), the clip file (read through the caller's reader and re-read on the
// model poll's cadence, so a keyframe edited in a text editor re-poses the stage with no click), and
// the frame — read off a reply envelope, never a wall clock, because the stage's sim time is the
// only clock a `started` stamp may come from.
//
// THE FRAME IS READ IMMEDIATELY BEFORE EVERY STAMP. A running session moves between the moment a
// control is clicked and the moment the write lands, so each `set-animation` is re-derived at a
// frame read on the spot (`restamped`) and the value that actually landed is fed back into the
// state. That extra read is the Effects strip's own Restart pattern, and it is what makes the
// scrub's `started = now − k` mean the tick it says.
//
// INTENTS RUN IN ORDER, ONE GESTURE AT A TIME. A scrub is pause → step → stamp, and a second gesture
// starting mid-flight would interleave its own pause with that stamp; the queue below serializes
// them. Scrub writes are latest-wins at the camera throttle's cadence, with the final value always
// sent — the same posture as an orbit drag, for the same reason: the bar must never come to rest
// somewhere the human did not leave it.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { EntityId } from '../../App';
import { ProtocolError, type ProtocolClient } from '../../protocol/client';
import { componentSet } from '../../protocol/commands';
import { toDiagnostic, type DiagnosticView } from '../../viewer/diagnostic';
import { usePolling } from '../../viewer/usePolling';
import { durationTicks, keyframeMarks, parseAnimDoc, type AnimDoc, type KeyframeMark } from './anim';
import { clipFilePath, type ClipBinding } from './assembly';
import {
  ANIMATION,
  animationValue,
  crossfadeTo,
  fadeTicks,
  INITIAL_CLIP_STATE,
  loopRestartDue,
  restamped,
  restart,
  scrubTo,
  selectClip,
  setLoopOneShot,
  setSpeed,
  shownTick,
  toRest,
  togglePlay,
  withPosedTick,
  withStamp,
  type ClipContext,
  type ClipIntent,
  type Crossfade,
  type ClipState,
  type ClipTransition,
} from './clipPlayback';
import { CAMERA_SEND_MS, MODEL_POLL_MS, sendDue } from './stage';

/** The component read for the session's frame: the subject always carries it — it is what the
 *  assembly is staged through — and `component.get` never dirties a frame. */
const MESH = 'Mesh';

/** How often the frame is read while the session runs. At 10 Hz the playhead advances about a sixth
 *  of vigil's walk per step: fine enough to read as motion, coarse enough that a poll costs
 *  nothing. A stopped session has no clock to follow, so it falls back to the model poll. */
const PLAYING_POLL_MS = 100;


/** The staged clip's overlay rate: a character binding's `speed`, 1 for everything else. */
function rateOf(clips: readonly ClipBinding[], clip: string | null): number {
  return clips.find((entry) => entry.name === clip)?.speed ?? 1;
}
const NO_MARKS: readonly KeyframeMark[] = [];

export interface ClipPlaybackOptions {
  /** The stage's client, or `null` while the host is still starting it. */
  readonly client: ProtocolClient | null;
  /** The stage scene's `subject` entity, or `null` before the query answers. */
  readonly subjectId: EntityId | null;
  /** The staged model's clip bindings — an assembly's or a character's. */
  readonly clips: readonly ClipBinding[];
  /** The path of the file that binds them, under the library root — what clip bindings resolve
   *  against — or `null` when what is on the stage binds no clips. */
  readonly ownerRelPath: string | null;
  /** Reads a file at a path relative to the same root as `ownerRelPath`. */
  readonly readFile: (relPath: string) => Promise<string>;
}

/** Everything the strip draws and every control it fires. `<ClipStrip {...playback} />`. */
export interface ClipPlayback {
  /** The staged model's bindings, in authored order. */
  readonly clips: readonly ClipBinding[];
  /** The staged clip's name, or `null` at the rest pose. */
  readonly clip: string | null;
  /** The clip's length in seconds, as authored. */
  readonly duration: number;
  readonly clipTicks: number;
  /** The clip's own `loop:` — engine-side wrapping, not the one-shot toggle. */
  readonly loops: boolean;
  readonly marks: readonly KeyframeMark[];
  /** The tick posed right now. */
  readonly tick: number;
  readonly playing: boolean;
  readonly speed: number;
  readonly loopOneShot: boolean;
  /** The crossfade the staged clip was entered through, or `null`. */
  readonly fade: Crossfade | null;
  /** The fade band's length on the bar, in ticks (0 without a fade). */
  readonly fadeTicks: number;
  /** The session's latest frame, or `null` before the first reply. */
  readonly frame: number | null;
  /** Whether a live session with a subject and a staged model is behind the controls. */
  readonly enabled: boolean;
  readonly error: DiagnosticView | null;
  readonly onSelectClip: (clip: string | null) => void;
  readonly onTogglePlay: () => void;
  readonly onRestart: () => void;
  /** Enters `clip` through a blend of `seconds` from the staged one. */
  readonly onCrossfade: (clip: string, seconds: number) => void;
  /** `commit` sends unthrottled — a drag's final value, an arrow key, a click. */
  readonly onScrub: (tick: number, commit?: boolean) => void;
  readonly onSpeed: (speed: number) => void;
  readonly onLoopOneShot: (on: boolean) => void;
  readonly onDismissError: () => void;
}

export function useClipPlayback(options: ClipPlaybackOptions): ClipPlayback {
  const { client, subjectId, clips, ownerRelPath, readFile } = options;

  const [state, setState] = useState<ClipState>(INITIAL_CLIP_STATE);
  const [doc, setDoc] = useState<AnimDoc | null>(null);
  const [frame, setFrame] = useState<number | null>(null);
  const [error, setError] = useState<DiagnosticView | null>(null);

  // Held in refs as well as state: the executor and the poll read them between renders, where a
  // captured value would be a cycle stale — and a stale frame is a stamp on the wrong tick.
  const stateRef = useRef(state);
  const frameRef = useRef(0);
  const docRef = useRef<AnimDoc | null>(doc);
  docRef.current = doc;
  // The reader is the caller's; holding it in a ref keeps a view that rebuilds the closure every
  // render from re-reading the file every render.
  const readFileRef = useRef(readFile);
  readFileRef.current = readFile;

  const live = client !== null && subjectId !== null && ownerRelPath !== null;

  const applyState = useCallback((update: (state: ClipState) => ClipState): void => {
    const next = update(stateRef.current);
    stateRef.current = next;
    setState(next);
  }, []);

  // Always a reply's `gameFrame`: the frame `Animation.started` lives in.
  const noteFrame = useCallback((at: number | null): void => {
    if (typeof at === 'number') {
      frameRef.current = at;
      setFrame(at);
    }
  }, []);

  const clipsRef = useRef(clips);
  clipsRef.current = clips;

  const ctxNow = useCallback(
    (): ClipContext => ({
      frame: frameRef.current,
      clipTicks: docRef.current === null ? 0 : durationTicks(docRef.current.duration),
      loops: docRef.current?.loop ?? false,
      rate: rateOf(clipsRef.current, stateRef.current.clip),
    }),
    [],
  );

  // --- the wire ------------------------------------------------------------------------------- //

  const execute = useCallback(
    async (intents: readonly ClipIntent[]): Promise<void> => {
      if (client === null || subjectId === null) {
        return;
      }
      // The handle is re-spelled rather than passed through: the generated params type carries an
      // index signature the editor's own `EntityId` does not.
      const entity = { index: subjectId.index, generation: subjectId.generation };
      for (const intent of intents) {
        switch (intent.kind) {
          case 'set-animation': {
            const read = await client.callFull({
              method: 'component.get',
              params: { entity, component: MESH },
            });
            noteFrame(read.gameFrame);
            const landed = restamped(
              intent,
              frameRef.current,
              rateOf(clipsRef.current, intent.clip),
            );
            await client.call(componentSet(entity, ANIMATION, animationValue(landed)));
            applyState((current) => withStamp(current, landed));
            break;
          }
          case 'remove-animation':
            try {
              await client.call({
                method: 'component.remove',
                params: { entity, component: ANIMATION },
              });
            } catch (err) {
              // `L3006` is "the entity does not carry that component" — the state asked for. A
              // scene reload drops the component without telling anyone, so this is ordinary.
              if (!(err instanceof ProtocolError) || err.code !== 'L3006') {
                throw err;
              }
            }
            break;
          case 'pause': {
            noteFrame((await client.callFull({ method: 'sim.pause' })).gameFrame);
            applyState((current) => withPosedTick(current, ctxNow()));
            break;
          }
          case 'resume':
            noteFrame((await client.callFull({ method: 'sim.resume' })).gameFrame);
            break;
          case 'step':
            noteFrame(
              (
                await client.callFull({
                  method: 'sim.step',
                  params: { ticks: intent.ticks, hash: false },
                })
              ).gameFrame,
            );
            break;
          case 'set-speed':
            await client.call({
              method: 'sim.set_speed',
              params: { speed: intent.speed },
            });
            break;
        }
      }
    },
    [client, subjectId, applyState, ctxNow, noteFrame],
  );

  const queueRef = useRef<Promise<void>>(Promise.resolve());
  const run = useCallback(
    (intents: readonly ClipIntent[]): void => {
      if (intents.length === 0) {
        return;
      }
      queueRef.current = queueRef.current
        .then(() => execute(intents))
        .catch((err: unknown) => setError(toDiagnostic(err)));
    },
    [execute],
  );

  const dispatch = useCallback(
    (step: (state: ClipState, ctx: ClipContext) => ClipTransition): void => {
      const { state: next, intents } = step(stateRef.current, ctxNow());
      applyState(() => next);
      run(intents);
    },
    [applyState, ctxNow, run],
  );

  // --- the clip file -------------------------------------------------------------------------- //

  const clipPath = useMemo(() => {
    if (ownerRelPath === null || state.clip === null) {
      return null;
    }
    const binding = clips.find((entry) => entry.name === state.clip);
    return binding === undefined ? null : clipFilePath(ownerRelPath, binding.file);
  }, [ownerRelPath, state.clip, clips]);

  const sourceRef = useRef<string | null>(null);

  useEffect(() => {
    sourceRef.current = null;
    setDoc(null);
    if (clipPath === null) {
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const text = await readFileRef.current(clipPath);
        if (!cancelled) {
          sourceRef.current = text;
          setDoc(parseAnimDoc(text));
        }
      } catch (err) {
        if (!cancelled) {
          setError(toDiagnostic(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [clipPath]);

  const reread = useCallback(async (): Promise<void> => {
    if (clipPath === null) {
      return;
    }
    try {
      const text = await readFileRef.current(clipPath);
      if (text !== sourceRef.current) {
        sourceRef.current = text;
        setDoc(parseAnimDoc(text));
      }
    } catch {
      // A file caught mid-save reads as missing for an instant; the next poll sees the new bytes.
      // A clip that is really gone was already reported by the load above.
    }
  }, [clipPath]);

  usePolling(clipPath !== null, MODEL_POLL_MS, reread);

  // --- the session ---------------------------------------------------------------------------- //

  const pollFrame = useCallback(async (): Promise<void> => {
    if (client === null || subjectId === null) {
      return;
    }
    try {
      const { gameFrame: at } = await client.callFull({
        method: 'component.get',
        params: {
          entity: { index: subjectId.index, generation: subjectId.generation },
          component: MESH,
        },
      });
      noteFrame(at);
      if (loopRestartDue(stateRef.current, ctxNow())) {
        dispatch(restart);
      }
    } catch (err) {
      setError(toDiagnostic(err));
    }
  }, [client, subjectId, ctxNow, dispatch, noteFrame]);

  usePolling(live, state.playing ? PLAYING_POLL_MS : MODEL_POLL_MS, pollFrame);

  // A new model is a new set of clip names, so whatever was staged belongs to the model that
  // left. Compared against the path actually staged rather than fired on every identity change: a
  // re-dialed client must not silently clear the human's selection.
  const stagedForRef = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    if (stagedForRef.current === ownerRelPath) {
      return;
    }
    stagedForRef.current = ownerRelPath;
    dispatch(toRest);
  }, [ownerRelPath, dispatch]);

  // A rig switch is a `scene.load`: the world that carried the component is gone and the subject is
  // a fresh entity, so the clip and the tick are re-applied to it rather than silently lost.
  useEffect(() => {
    if (subjectId === null || stateRef.current.clip === null) {
      return;
    }
    dispatch((current, ctx) =>
      current.playing ? restart(current, ctx) : scrubTo(current, current.tick, ctx),
    );
  }, [subjectId, dispatch]);

  // --- the scrub throttle --------------------------------------------------------------------- //

  const pendingScrubRef = useRef<number | null>(null);
  const scrubTimerRef = useRef<number | null>(null);
  const lastScrubMsRef = useRef<number | null>(null);

  const clearScrubTimer = useCallback((): void => {
    if (scrubTimerRef.current !== null) {
      window.clearTimeout(scrubTimerRef.current);
      scrubTimerRef.current = null;
    }
  }, []);

  const sendScrub = useCallback(
    (tick: number): void => {
      lastScrubMsRef.current = performance.now();
      pendingScrubRef.current = null;
      dispatch((current, ctx) => scrubTo(current, tick, ctx));
    },
    [dispatch],
  );

  const onScrub = useCallback(
    (tick: number, commit = false): void => {
      // A scrub that still has to stop the sim is never deferred: the throttled path below applies
      // the transition without its writes, and a dropped `sim.pause` would leave the state claiming
      // a stopped session that is in fact still ticking.
      const due = sendDue(lastScrubMsRef.current, performance.now(), CAMERA_SEND_MS);
      if (commit || due || stateRef.current.playing) {
        clearScrubTimer();
        sendScrub(tick);
        return;
      }
      // Latest-wins: the bar takes the new tick immediately, the write it owes lands at the next
      // cadence slot. The sim is already stopped here, so a state ahead of its write is a pose one
      // throttle window behind, never a lie.
      pendingScrubRef.current = tick;
      applyState((current) => scrubTo(current, tick, ctxNow()).state);
      if (scrubTimerRef.current === null) {
        scrubTimerRef.current = window.setTimeout(() => {
          scrubTimerRef.current = null;
          const pending = pendingScrubRef.current;
          if (pending !== null) {
            sendScrub(pending);
          }
        }, CAMERA_SEND_MS);
      }
    },
    [applyState, clearScrubTimer, ctxNow, sendScrub],
  );

  useEffect(() => clearScrubTimer, [clearScrubTimer]);

  // --- what the strip draws ------------------------------------------------------------------- //

  const marks = useMemo(() => (doc === null ? NO_MARKS : keyframeMarks(doc)), [doc]);
  const ctx: ClipContext = {
    frame: frame ?? 0,
    clipTicks: doc === null ? 0 : durationTicks(doc.duration),
    loops: doc?.loop ?? false,
    rate: rateOf(clips, state.clip),
  };

  const onSelectClip = useCallback(
    (clip: string | null): void =>
      dispatch((current, at) =>
        selectClip(current, clip, { ...at, rate: rateOf(clipsRef.current, clip) }),
      ),
    [dispatch],
  );
  const onTogglePlay = useCallback((): void => dispatch(togglePlay), [dispatch]);
  const onRestart = useCallback((): void => dispatch(restart), [dispatch]);
  const onCrossfade = useCallback(
    (clip: string, seconds: number): void =>
      dispatch((current, at) =>
        crossfadeTo(current, clip, seconds, { ...at, rate: rateOf(clipsRef.current, clip) }),
      ),
    [dispatch],
  );
  const onSpeed = useCallback(
    (speed: number): void => dispatch((current) => setSpeed(current, speed)),
    [dispatch],
  );
  const onLoopOneShot = useCallback(
    (on: boolean): void => dispatch((current) => setLoopOneShot(current, on)),
    [dispatch],
  );
  const onDismissError = useCallback((): void => setError(null), []);

  return {
    clips,
    clip: state.clip,
    duration: doc?.duration ?? 0,
    clipTicks: ctx.clipTicks,
    loops: ctx.loops,
    marks,
    tick: shownTick(state, ctx),
    playing: state.playing,
    speed: state.speed,
    loopOneShot: state.loopOneShot,
    fade: state.fade,
    fadeTicks: state.fade === null ? 0 : fadeTicks(state.fade),
    frame,
    enabled: live,
    error,
    onSelectClip,
    onTogglePlay,
    onRestart,
    onCrossfade,
    onScrub,
    onSpeed,
    onLoopOneShot,
    onDismissError,
  };
}
