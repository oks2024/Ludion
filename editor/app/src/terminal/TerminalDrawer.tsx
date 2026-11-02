// The right-column terminal drawer: xterm.js tabs over the Tauri PTY host, each tab a persona
// instance — `claude`, or `opencode` against a local llama.cpp server — driving the same live
// engine session as a peer client. Thin on purpose — the PTY/xterm lifecycle lives in the
// module-level registry (`sessions.ts`, survives remount) and all derivations are pure
// (`status.ts`, `cache.ts`, `spawnConfig.ts`, `opencode.ts`); this component wires them to the DOM
// and the mockup's chrome.

import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import { isTauri } from '../shell/tauri';
import { editorPaths, readEditorFile, writeEditorFile, type EditorPaths, type PtySpawnOpts } from './pty';
import {
  disposeSession,
  fitSession,
  listSessions,
  nextTermId,
  spawnSession,
  type TermSession,
} from './sessions';
import { buildSpawnConfig, systemPromptText } from './spawnConfig';
import {
  buildOpencodeSpawnConfig,
  DEFAULT_LLAMACPP,
  opencodePersonaPath,
  resolveLlamaSettings,
  validateLlamaSettings,
  type LlamaCppSettings,
} from './opencode';
import { modelKind, modelLabel, resolveModelId, TERMINAL_MODELS } from './models';
import { isConfinementRefusal } from '../shell/hostRefusal';
import { statusSinkPath } from './context';
import { parseStatusText } from './cache';
import { terminalStatus } from './status';
import {
  cacheCountdownMs,
  contextFraction,
  formatContextTokens,
  formatCountdown,
  updateCacheState,
} from './cache';

/** Last-used model, so the picker opens on the human's previous choice. */
const MODEL_KEY = 'ludion.editor.terminalModel';

/** Last-used llama.cpp server settings, for the opencode spawn path. */
const LLAMA_URL_KEY = 'ludion.editor.opencodeBaseUrl';
const LLAMA_MODEL_KEY = 'ludion.editor.opencodeModel';
const LLAMA_CONTEXT_KEY = 'ludion.editor.opencodeContext';

/** The stored llama.cpp settings — the panel seeds its drafts from these, and the spawn re-reads
 *  them, so a restart adopts whatever the last spawn saved. Storage-disabled falls to defaults. */
function storedLlamaSettings(): LlamaCppSettings {
  try {
    return resolveLlamaSettings({
      baseUrl: localStorage.getItem(LLAMA_URL_KEY),
      modelId: localStorage.getItem(LLAMA_MODEL_KEY),
      contextTokens: localStorage.getItem(LLAMA_CONTEXT_KEY),
    });
  } catch {
    return resolveLlamaSettings({ baseUrl: null, modelId: null, contextTokens: null });
  }
}

interface TerminalDrawerProps {
  /** The connected engine url (peer clients dial exactly this). */
  readonly url: string;
  /** Active view id — selects the tab set (only `level-designer` today). */
  readonly viewId: string;
  /** Named in the spawned instance's appended system prompt. */
  readonly scenePath: string;
  /** The game project dir (cwd for spawned instances); '' until the human sets it. */
  readonly projectDir: string;
  /** The current project's name, validated through `project_info`; `null` before it resolves. */
  readonly projectName: string | null;
  /** Opens the titlebar's project switcher (the only way to change the project now). */
  readonly onOpenProjectSwitcher: () => void;
}

export function TerminalDrawer(props: TerminalDrawerProps) {
  if (!isTauri()) {
    return (
      <div className="terminal">
        <div className="term-tabs">
          <div className="term-tab active">
            <div className="tt-top">
              <span className="tt-name">Terminal</span>
            </div>
          </div>
        </div>
        <div className="term-drawer-empty">
          <span className="term-empty-title">Terminals need the desktop editor</span>
          <span className="term-empty-sub">
            The persona terminals run over the Tauri host&apos;s PTY. Open the editor in the desktop
            shell (not the browser dev page) to spawn one.
          </span>
        </div>
      </div>
    );
  }
  return <TauriDrawer {...props} />;
}

function TauriDrawer({ url, viewId, scenePath, projectDir, projectName, onOpenProjectSwitcher }: TerminalDrawerProps) {
  const [paths, setPaths] = useState<EditorPaths | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Non-null while the human is choosing a model for a spawn that hasn't happened yet.
  const [pending, setPending] = useState<{ replaceId?: string } | null>(null);
  // Forces a re-render to refresh status chips / context bars off the mutated registry sessions.
  const [, tick] = useReducer((n: number) => n + 1, 0);
  const bodyRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    editorPaths().then(setPaths).catch((e: unknown) => setError(String(e)));
  }, []);

  const sessions = listSessions(viewId);

  // Keep `activeId` valid as tabs come and go: default to the last tab, clear when empty.
  useEffect(() => {
    if (sessions.length === 0) {
      if (activeId !== null) setActiveId(null);
      return;
    }
    if (activeId === null || !sessions.some((s) => s.id === activeId)) {
      setActiveId(sessions[sessions.length - 1].id);
    }
  }, [sessions, activeId]);

  // Re-parent the active tab's xterm element into the body container (moving it out of any prior
  // container) and fit it. `replaceChildren` detaches the previously-shown host back to the registry.
  // `pending` is a dep because it replaces the body element: without it, dismissing the chooser
  // would leave a fresh, empty container behind.
  useEffect(() => {
    const container = bodyRef.current;
    const active = sessions.find((s) => s.id === activeId);
    if (container === null || active === undefined) {
      return;
    }
    container.replaceChildren(active.host);
    fitSession(active);
  }, [activeId, sessions, pending]);

  // Refit the active terminal when the column resizes.
  useEffect(() => {
    const container = bodyRef.current;
    if (container === null) {
      return;
    }
    const observer = new ResizeObserver(() => {
      const active = listSessions(viewId).find((s) => s.id === activeId);
      if (active !== undefined) {
        fitSession(active);
      }
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, [viewId, activeId]);

  // Tail each live session's statusline sink (~1 Hz): parse the latest payload and fold it into the
  // per-session cache-countdown state, then re-render the chips/bars. Cheap host file reads only.
  // Each session's sink lives under ITS OWN spawning project — after a switch, an old tab's sink is
  // still in that project, never the current one.
  useEffect(() => {
    const poll = async (): Promise<void> => {
      for (const session of listSessions(viewId)) {
        if (session.exited) {
          continue;
        }
        // The sink is the claude statusline relay's channel; opencode has no statusline hook, so
        // an opencode tab never has one to read.
        if (modelKind(session.modelId) !== 'claude') {
          continue;
        }
        try {
          const payload = parseStatusText(await readEditorFile(statusSinkPath(session.projectDir, session.id)));
          if (payload !== null) {
            session.status = payload;
            session.cache = updateCacheState(session.cache, payload, Date.now());
          }
        } catch (err) {
          // No sink yet (before the first assistant message) — leave the "—" readouts. A
          // confinement refusal is not "no sink yet": show it.
          if (isConfinementRefusal(err)) {
            const message = String(err);
            setError((current) => (current === message ? current : message));
          }
        }
      }
      tick();
    };
    const timer = window.setInterval(() => void poll(), 1000);
    return () => window.clearInterval(timer);
  }, [viewId]);

  const spawn = useCallback(
    async (
      modelId: string,
      replaceId?: string,
      target?: { readonly projectDir: string; readonly projectName: string | null },
    ): Promise<void> => {
      const { projectDir: targetDir, projectName: targetName } = target ?? { projectDir, projectName };
      if (paths === null || targetDir === '') {
        return;
      }
      // The persona is read per spawn so edits to the versioned file take effect on the next one.
      // The claude path composes it into one prompt flag rather than passing the path, because that
      // CLI refuses the by-path and by-text system-prompt flags together and the editor-session
      // section must ride along.
      let personaText: string;
      try {
        personaText = await readEditorFile(`${paths.personasDir}/${viewId}.md`);
      } catch (e) {
        setError(`persona read failed: ${String(e)}`);
        return;
      }
      const id = nextTermId(viewId);
      let opts: PtySpawnOpts;
      if (modelKind(modelId) === 'opencode') {
        // opencode takes no system-prompt flag, so the persona rides a file its config lists under
        // `instructions`. One file per view, rewritten on every spawn — a restart therefore picks
        // up persona edits, the same freshness rule as the claude path's per-spawn read; the llama
        // settings are re-read here for the same reason.
        const personaPath = opencodePersonaPath(targetDir, viewId);
        try {
          await writeEditorFile(
            personaPath,
            systemPromptText({ personaText, view: viewId, projectDir: targetDir, scenePath, url }),
          );
        } catch (e) {
          setError(`persona write failed: ${String(e)}`);
          return;
        }
        opts = buildOpencodeSpawnConfig({
          termId: id,
          url,
          projectDir: targetDir,
          repoRoot: paths.repoRoot,
          personaPath,
          llama: storedLlamaSettings(),
          windows: navigator.userAgent.includes('Windows'),
          cols: 80,
          rows: 24,
        });
      } else {
        opts = buildSpawnConfig({
          termId: id,
          url,
          projectDir: targetDir,
          personaText,
          modelId,
          view: viewId,
          scenePath,
          relayScript: paths.relayScript,
          repoRoot: paths.repoRoot,
          sinkPath: statusSinkPath(targetDir, id),
          cols: 80,
          rows: 24,
        });
      }
      const replaced = sessions.findIndex((s) => s.id === replaceId);
      const ordinal = replaced >= 0 ? replaced + 1 : sessions.length + 1;
      try {
        await spawnSession({
          viewId,
          label: `${modelLabel(modelId)} · ${ordinal}`,
          modelId,
          opts,
          projectDir: targetDir,
          projectName: targetName,
          replaceId,
        });
        setActiveId(id);
        setError(null);
        setPending(null);
        tick();
      } catch (e) {
        setError(`spawn failed: ${String(e)}`);
      }
    },
    [paths, projectDir, projectName, url, viewId, scenePath, sessions],
  );

  // Restart respawns in the TAB'S recorded project, never the current one — the tab label must stay
  // true even after the human has switched projects underneath it. The model is kept too.
  const restart = useCallback(
    (session: TermSession) => {
      void spawn(session.modelId, session.id, { projectDir: session.projectDir, projectName: session.projectName });
    },
    [spawn],
  );

  const now = Date.now();
  const activeSession = sessions.find((s) => s.id === activeId);

  if (paths === null && error === null) {
    return (
      <div className="terminal">
        <div className="term-drawer-empty">
          <span className="term-empty-sub">Resolving editor paths…</span>
        </div>
      </div>
    );
  }

  return (
    <div className="terminal">
      <div className="term-tabs">
        {sessions.map((session) => (
          <TermTab
            key={session.id}
            session={session}
            now={now}
            active={session.id === activeId}
            onSelect={() => setActiveId(session.id)}
            onClose={() => {
              disposeSession(session.id);
              tick();
            }}
          />
        ))}
        <button
          type="button"
          className="term-newtab"
          title={projectDir === '' ? 'Set a project directory first' : 'New terminal'}
          disabled={projectDir === '' || paths === null}
          onClick={() => setPending({})}
        >
          +
        </button>
      </div>

      {pending !== null || sessions.length === 0 ? (
        <NewTerminalPanel
          viewId={viewId}
          projectDir={projectDir}
          projectName={projectName}
          error={error}
          onOpenProjectSwitcher={onOpenProjectSwitcher}
          onStart={(modelId) => void spawn(modelId, pending?.replaceId)}
          onCancel={sessions.length > 0 ? () => setPending(null) : null}
        />
      ) : (
        <>
          <div className="term-body" ref={bodyRef} />
          <div className="term-footer">
            <button type="button" className="term-projline" onClick={onOpenProjectSwitcher} title={projectDir}>
              project: <span className="num">{projectName ?? projectDir}</span>
            </button>
            {activeSession !== undefined && (
              <button
                type="button"
                className="term-restart"
                onClick={() => restart(activeSession)}
                title={`Restart this terminal fresh (${modelLabel(activeSession.modelId)})`}
              >
                Restart
              </button>
            )}
          </div>
          {error !== null && <div className="term-error term-error-bar">{error}</div>}
        </>
      )}
    </div>
  );
}

/** The spawn screen — also the drawer's empty state, so the first spawn is one screen, not two.
 *  Every spawn goes through it, so the model is never inherited silently from a previous tab. */
function NewTerminalPanel({
  viewId,
  projectDir,
  projectName,
  error,
  onOpenProjectSwitcher,
  onStart,
  onCancel,
}: {
  readonly viewId: string;
  readonly projectDir: string;
  readonly projectName: string | null;
  readonly error: string | null;
  readonly onOpenProjectSwitcher: () => void;
  readonly onStart: (modelId: string) => void;
  readonly onCancel: (() => void) | null;
}) {
  const [modelId, setModelId] = useState(() => {
    try {
      return resolveModelId(localStorage.getItem(MODEL_KEY));
    } catch {
      return resolveModelId(null);
    }
  });
  // Seeded once: flipping between model cards keeps whatever the human typed here.
  const [llamaDraft, setLlamaDraft] = useState(() => {
    const stored = storedLlamaSettings();
    return {
      baseUrl: stored.baseUrl,
      modelId: stored.modelId,
      contextTokens: String(stored.contextTokens),
    };
  });
  const isOpencode = modelKind(modelId) === 'opencode';
  const llama: LlamaCppSettings = {
    baseUrl: llamaDraft.baseUrl,
    modelId: llamaDraft.modelId,
    contextTokens: Number(llamaDraft.contextTokens),
  };
  const llamaError = isOpencode ? validateLlamaSettings(llama) : null;
  const start = (): void => {
    try {
      localStorage.setItem(MODEL_KEY, modelId);
      if (isOpencode) {
        localStorage.setItem(LLAMA_URL_KEY, llama.baseUrl);
        localStorage.setItem(LLAMA_MODEL_KEY, llama.modelId);
        localStorage.setItem(LLAMA_CONTEXT_KEY, llamaDraft.contextTokens);
      }
    } catch {
      // Private-mode / storage-disabled: the model choice still applies to this spawn, but the
      // spawn reads the llama settings back from storage, so those fall to their defaults.
    }
    onStart(modelId);
  };
  return (
    <div className="term-drawer-empty">
      <span className="term-empty-title">Spawn a terminal</span>
      <span className="term-empty-sub">
        A {viewId} persona instance on the model you pick — it drives this live session as a peer
        client.
      </span>
      <div className="term-models">
        {TERMINAL_MODELS.map((model) => (
          <button
            key={model.id}
            type="button"
            className={`term-model${model.id === modelId ? ' active' : ''}`}
            onClick={() => setModelId(model.id)}
          >
            <span className="tm-label">{model.label}</span>
            <span className="tm-sub">{model.sub}</span>
          </button>
        ))}
      </div>
      {isOpencode && (
        <div className="term-llama-fields">
          <label>
            <span>server</span>
            <input
              className="term-proj-input num"
              type="text"
              spellCheck={false}
              value={llamaDraft.baseUrl}
              placeholder={DEFAULT_LLAMACPP.baseUrl}
              onChange={(e) => setLlamaDraft({ ...llamaDraft, baseUrl: e.target.value })}
            />
          </label>
          <label>
            <span>model id</span>
            <input
              className="term-proj-input num"
              type="text"
              spellCheck={false}
              value={llamaDraft.modelId}
              placeholder={DEFAULT_LLAMACPP.modelId}
              onChange={(e) => setLlamaDraft({ ...llamaDraft, modelId: e.target.value })}
            />
          </label>
          <label>
            <span>context</span>
            <input
              className="term-proj-input num"
              type="text"
              inputMode="numeric"
              spellCheck={false}
              value={llamaDraft.contextTokens}
              placeholder={String(DEFAULT_LLAMACPP.contextTokens)}
              onChange={(e) => setLlamaDraft({ ...llamaDraft, contextTokens: e.target.value })}
            />
          </label>
        </div>
      )}
      <button type="button" className="term-projline" onClick={onOpenProjectSwitcher} title={projectDir}>
        {projectDir === '' ? (
          'no project — choose one'
        ) : (
          <>
            project: <span className="num">{projectName ?? projectDir}</span>
          </>
        )}
      </button>
      {error !== null && <span className="term-error">{error}</span>}
      {llamaError !== null && <span className="term-error">{llamaError}</span>}
      <div className="term-proj-actions">
        <button
          type="button"
          className="term-spawn-btn"
          disabled={llamaError !== null || projectDir === ''}
          onClick={start}
        >
          Spawn terminal
        </button>
        {onCancel !== null && (
          <button type="button" className="term-projline" onClick={onCancel}>
            cancel
          </button>
        )}
      </div>
    </div>
  );
}

/** One tab head: name + status chip on top, and — for the models that report telemetry — a
 *  context-usage bar + token count + cache countdown below. */
function TermTab({
  session,
  now,
  active,
  onSelect,
  onClose,
}: {
  readonly session: TermSession;
  readonly now: number;
  readonly active: boolean;
  readonly onSelect: () => void;
  readonly onClose: () => void;
}) {
  const status = terminalStatus({ lastOutputAt: session.lastOutputAt, exited: session.exited, now });
  // Only the claude path feeds the statusline sink, so only its tabs have context/cache readouts.
  const hasTelemetry = modelKind(session.modelId) === 'claude';
  const fraction = contextFraction(session.status);
  const closeSession = (event: React.MouseEvent): void => {
    event.stopPropagation();
    onClose();
  };
  return (
    <button type="button" className={`term-tab${active ? ' active' : ''}`} onClick={onSelect}>
      <div className="tt-top">
        <span className="tt-name">{session.label}</span>
        <span className="tt-proj" title={session.projectDir}>
          {session.projectName ?? session.projectDir.split('/').pop()}
        </span>
        <span className={`status-chip ${status}`}>
          <span className="sdot" />
          {status}
        </span>
        <span className="tt-close" title="Close terminal" onClick={closeSession}>
          ×
        </span>
      </div>
      {hasTelemetry && (
        <div className="tt-bot">
          <span className="ctx-track">
            <span className="ctx-fill" style={{ width: `${(fraction ?? 0) * 100}%` }} />
          </span>
          <span className="ctx-num">{formatContextTokens(session.status)}</span>
          <span className="cache-cd">
            cache <span className="num">{formatCountdown(cacheCountdownMs(session.cache, now))}</span>
          </span>
        </div>
      )}
    </button>
  );
}
