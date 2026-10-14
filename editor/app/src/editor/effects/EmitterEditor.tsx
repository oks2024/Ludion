// The Effects view's emitter session: everything that needs a DIALED stage — the subject, its
// schema, the poll, the time strip's state, and the one `component.set` path they all write
// through.
//
// WHY THIS IS A SEPARATE COMPONENT FROM THE VIEW. `useComponentCommit` takes a connected
// `ProtocolClient`, not a client-or-null: it is the editor's one commit path and it has no idle
// state to represent. The view's client, though, is null for as long as the host is building and
// starting the stage — minutes, on a cold build. Splitting the two is what keeps that
// contract literally true: `views/EffectsView.tsx` owns the stage lifecycle and the layout and
// mounts this component only once the session is dialed, so the hook here never sees a client that
// cannot send. A placeholder client standing in for the missing one would be a lie the next reader
// has to discover.
//
// ONE WRITE PATH, AND IT LIVES HERE. Sliders, the Restart button and the loop toggle all set one
// field of one component, and the loop fires from the poll below — which cannot reach a hook held
// inside the panel. So the hook is this component's: `restart()` is its only `started` writer, the
// panel's button calls it and the poll calls it, and the panel's rows commit through the same hook.
// Two writers over one assembly base would fight.
//
// THE STAGE NEVER HOLDS UNSAVED STATE (the Voxel Artist's contract 6). There is no undo here, no
// dirty flag and no save: a write is reported to nobody — there is nothing to invert and nothing to
// persist. What a human agrees to is carried into the game's scene or `tunables.rs` by hand.

import { useCallback, useEffect, useState } from 'react';
import type { ResponseBody } from '@ludion/protocol';
import type { EntityId } from '../../App';
import type { ProtocolClient } from '../../protocol/client';
import { componentSet, entityRef } from '../../protocol/commands';
import { DiagnosticBanner } from '../../viewer/DiagnosticBanner';
import { toDiagnostic, type DiagnosticView } from '../../viewer/diagnostic';
import type { FieldPath } from '../../viewer/inspect-fields';
import { useComponentCommit, type CommitCadence } from '../../viewer/useComponentCommit';
import { usePolling } from '../../viewer/usePolling';
import { parseHierarchy } from '../hierarchy';
import { EmitterPanel } from './EmitterPanel';
import { PatternEditor } from './PatternEditor';
import {
  EMITTER,
  EMITTER_POLL_MS,
  PATTERN_KIND,
  PATTERN_PLAYER,
  STAGE_SCENE,
  SUBJECT,
  TARGET,
} from './stage';
import { INITIAL_RUN, loopDue, periodFrames, type SimRun } from './time';

/** The stage scene without its extension — the status bar's name for this session. */
const STAGE_LABEL = STAGE_SCENE.replace(/\.yaml$/, '');

/** The commit hook's value before the first poll answers. A stable identity, so the hook's handback
 *  check does not read it as a fresh poll every render. */
const NO_VALUE: Record<string, unknown> = {};

export interface EmitterEditorProps {
  /** The stage's own client, dialed and handshaken — see the header. */
  readonly client: ProtocolClient;
  /** The project the stage was started on; the pattern tab lists its content root. */
  readonly projectDir: string;
  /** The one-line session readout for the status bar; cleared when this component goes away. */
  readonly onStatusNote: (note: string | null) => void;
}

/** The `value` of a `component.get` response, or `null`. */
function componentValue(body: ResponseBody): Record<string, unknown> | null {
  const b = body as { kind?: unknown; value?: unknown };
  if (b.kind !== 'component_value' || typeof b.value !== 'object' || b.value === null) {
    return null;
  }
  return b.value as Record<string, unknown>;
}

/** `ParticleEmitter`'s schema out of an `engine.describe` reply, or `null`. */
function emitterSchema(body: ResponseBody): unknown {
  if (body.kind !== 'engine_description' || typeof body.description !== 'object') {
    return null;
  }
  const components = (body.description as { components?: Record<string, unknown> }).components;
  return components?.[EMITTER] ?? null;
}

/** The `kind: pattern` document schema out of the same reply, or `null` when this host registers no
 *  such kind (plain `ludion serve` does not — the pattern tab says so and falls back). */
function patternDocSchema(body: ResponseBody): unknown {
  if (body.kind !== 'engine_description' || typeof body.description !== 'object') {
    return null;
  }
  const documents = (body.description as { documents?: Record<string, unknown> }).documents;
  return documents?.[PATTERN_KIND] ?? null;
}

export function EmitterEditor({ client, projectDir, onStatusNote }: EmitterEditorProps) {
  const [entity, setEntity] = useState<EntityId | null>(null);
  const [target, setTarget] = useState<EntityId | null>(null);
  const [schema, setSchema] = useState<unknown>(null);
  const [docSchema, setDocSchema] = useState<unknown>(null);
  const [polled, setPolled] = useState<Record<string, unknown> | null>(null);
  /** The sim frame the latest poll reply applied at — the session's own clock, and the only clock a
   *  `started` stamp may come from. */
  const [frame, setFrame] = useState<number | null>(null);
  const [run, setRun] = useState<SimRun>(INITIAL_RUN);
  const [looping, setLooping] = useState(false);
  const [error, setError] = useState<DiagnosticView | null>(null);

  // Reading the stage, once per client: the `subject` entity by name, and the one component schema
  // the field rows are derived from. A fresh session is running at ×1 — the host spawns it without
  // `--paused` — so the strip's state resets with it rather than describing the session before.
  useEffect(() => {
    let cancelled = false;
    setEntity(null);
    setTarget(null);
    setSchema(null);
    setDocSchema(null);
    setPolled(null);
    setFrame(null);
    setRun(INITIAL_RUN);
    void (async () => {
      try {
        const rows = parseHierarchy(
          await client.call({ method: 'query.entities', params: { filter: [], include: ['Name'] } }),
        );
        if (cancelled) {
          return;
        }
        setEntity(rows.find((row) => row.name === SUBJECT)?.entity ?? null);
        setTarget(rows.find((row) => row.name === TARGET)?.entity ?? null);
        const described = await client.call({ method: 'engine.describe' });
        if (!cancelled) {
          setSchema(emitterSchema(described));
          setDocSchema(patternDocSchema(described));
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
  }, [client]);

  // Every field of the component assembles onto the last polled value, whatever the edited path.
  const initialBase = useCallback(() => polled ?? NO_VALUE, [polled]);
  // The stage has no undo and no dirty flag (contract 6), so a write is reported to nobody.
  const onReport = useCallback((): void => {}, []);
  const { effective, commit } = useComponentCommit<Record<string, unknown>>({
    client,
    entity,
    component: EMITTER,
    value: polled ?? NO_VALUE,
    initialBase,
    onReport,
    onError: setError,
  });

  const commitField = useCallback(
    (path: FieldPath, value: unknown, cadence?: CommitCadence): void => {
      void commit(path, value, cadence);
    },
    [commit],
  );

  /**
   * The only `started` writer: the Restart button's and the loop's.
   *
   * The frame it stamps is the reply envelope's `gameFrame` rather than the poll's last one — the
   * session's gameplay clock at the moment of the write, so a restart lands on the frame it was asked
   * for even when the poll is a cycle behind or the game has held frames.
   */
  const restart = useCallback(async (): Promise<void> => {
    if (entity === null) {
      return;
    }
    try {
      const { gameFrame: at } = await client.callFull({
        method: 'component.get',
        params: { entity: entityRef(entity), component: EMITTER },
      });
      if (typeof at === 'number') {
        await commit(['started'], at, 'commit');
      }
    } catch (err) {
      setError(toDiagnostic(err));
    }
  }, [client, entity, commit]);

  const handleRestart = useCallback((): void => {
    void restart();
  }, [restart]);

  /**
   * The pattern tab's Restart: the same re-stamp, on the other component.
   *
   * It lives here rather than in `PatternEditor` because the time strip that fires it lives in the
   * panel, and because it needs no state of the tab's — a whole-value read-modify-write of
   * `PatternPlayer` with the reply envelope's own gameplay frame is the entire operation, and the
   * same two calls an agent would make.
   */
  const restartPattern = useCallback((): void => {
    if (entity === null) {
      return;
    }
    void (async () => {
      try {
        const { body, gameFrame: at } = await client.callFull({
          method: 'component.get',
          params: { entity: entityRef(entity), component: PATTERN_PLAYER },
        });
        const value = componentValue(body);
        if (value === null || typeof at !== 'number') {
          return;
        }
        await client.call(componentSet(entity, PATTERN_PLAYER, { ...value, started: at }));
      } catch (err) {
        setError(toDiagnostic(err));
      }
    })();
  }, [client, entity]);

  // The poll is also where the loop lives: one reply carries both the component and the gameplay
  // frame it was read at, which is what `loopDue` compares against `started` (a gameplay frame; the
  // status line keeps the sim frame). It reads the commit hook's `effective` value
  // so a restart's own stamp is seen immediately — the polled value is a cycle behind, and a loop
  // that judged by it would re-fire the restart it just made.
  const poll = useCallback(async (): Promise<void> => {
    if (entity === null) {
      return;
    }
    try {
      const { body, frame: at, gameFrame } = await client.callFull({
        method: 'component.get',
        params: { entity: entityRef(entity), component: EMITTER },
      });
      setFrame(at);
      const value = componentValue(body);
      if (value !== null) {
        setPolled(value);
      }
      if (
        looping &&
        !run.paused &&
        typeof gameFrame === 'number' &&
        loopDue(gameFrame, effective.started, periodFrames(effective))
      ) {
        await restart();
      }
    } catch (err) {
      setError(toDiagnostic(err));
    }
  }, [client, entity, looping, run.paused, effective, restart]);

  usePolling(entity !== null, EMITTER_POLL_MS, poll);

  // The status bar's line: which session, where its clock is, and how it is running. Cleared on
  // unmount, so a stale frame count cannot outlive the session it was read from.
  useEffect(() => {
    onStatusNote(
      frame === null
        ? `${STAGE_LABEL} · reading the emitter…`
        : `${STAGE_LABEL} · frame ${frame} · ${run.paused ? 'paused' : 'running'} ×${run.speed}`,
    );
  }, [frame, run, onStatusNote]);
  useEffect(() => () => onStatusNote(null), [onStatusNote]);

  return (
    <>
      <DiagnosticBanner diagnostic={error} onDismiss={() => setError(null)} />
      <EmitterPanel
        client={client}
        entity={entity}
        schema={schema}
        value={polled === null ? null : effective}
        frame={frame}
        run={run}
        loop={looping}
        commit={commitField}
        onRestart={handleRestart}
        onRestartPattern={restartPattern}
        pattern={
          <PatternEditor
            client={client}
            subject={entity}
            target={target}
            projectDir={projectDir}
            schema={docSchema}
            onError={setError}
          />
        }
        onRun={setRun}
        onLoop={setLooping}
        onError={setError}
      />
    </>
  );
}
