// The Generation view (`Plan/comfy-generation.md`): browse the project's `kind: generation`
// recipes, edit one, run it against a ComfyUI server, and freeze a candidate into the project.
//
// IT DRIVES THE CLI, NOT A SERVER. Every capability here is `ludion gen <verb> … --json` run as a
// subprocess by the host (`src-tauri/src/generation.rs`), which is the parity ruling this view
// stands on (`Plan/comfy-generation.md` decision 15): a pipeline verb is a CLI command, so nothing
// in the editor speaks HTTP to ComfyUI and nothing here is a capability a shell does not have. The
// view adds no wire method — the schema hash does not move.
//
// THE FILE IS THE SOURCE OF TRUTH, AND THE VERB READS IT OFF THE DISK. There is no stage, no
// watcher and no live reload in this view: `gen check` and `gen run` are subprocesses that open the
// recipe themselves. So every control splices the file in place (`generation/edit.ts`) and every
// verb run flushes the pending write first — the tool must never read a recipe the human is not
// looking at. A `gen run` overrides nothing from the UI (no `--count`, no `--seed`): the document's
// own values are the run's, which is what keeps a run reproducible from the committed text.
//
// THE SERVER ADDRESS IS PER-SESSION. The field is `useState`, never `localStorage` and never a file:
// the address is a per-machine property that belongs in `LUDION_COMFY_URL`, and no artifact this
// pipeline writes records it (`ludion docs generation`). Left empty — the ordinary case — the child
// inherits the host's own environment.
//
// LAYOUT: the recipe library on the LEFT, the run bar over the recipe form in the CENTRE, the
// candidate grid under it. A prompt is the widest thing edited in this view, which is why the form
// takes the centre column rather than the 320px side one.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listEditorDir } from '../shell/host';
import { genCancel, genInvoke, nextGenTicket, type GenResult, type GenVerb } from '../shell/genHost';
import { isTauri } from '../shell/tauri';
import { readEditorFile } from '../terminal/pty';
import { AssetBrowser } from '../editor/voxel/AssetBrowser';
import { CandidateGrid } from '../editor/generation/CandidateGrid';
import { DiagnosticList } from '../editor/generation/DiagnosticList';
import {
  beginVerb,
  endVerb,
  IDLE,
  isBusy,
  isCurrent,
  type BusyCounts,
  type VerbScope,
} from '../editor/generation/inflight';
import { RecipeInspector } from '../editor/generation/RecipeInspector';
import { useRecipeEdit } from '../editor/generation/useRecipeEdit';
import {
  acceptModel,
  checkModel,
  runFolders,
  runModel,
  statusModel,
  timeoutArgs,
  verbMessages,
  type AcceptModel,
  type CheckModel,
  type RunModel,
  type StatusModel,
  type VerbMessages,
} from '../editor/generation/model';
import {
  RECIPE_EXT,
  recipeFilePath,
  recipeName,
  recipeProjectPath,
  recipesRoot,
  runProjectPath,
  runsRoot,
} from '../editor/generation/paths';

/** The last-selected recipe, so switching views (which unmounts this one) does not lose the seat. */
const RECIPE_KEY = 'ludion.editor.generationRecipe';

/** How often the elapsed readout ticks while a verb is running. A `gen run` is minutes long and the
 *  only honest progress signal there is: the server reports no percentage. */
const TICK_MS = 1000;

export interface GenerationViewProps {
  readonly projectDir: string;
  /** The one-line readout for the status bar. */
  readonly onStatusNote: (note: string | null) => void;
}

/** A verb that could not be run at all (the host rejected the call): no object, only the reason. */
function failure(err: unknown): VerbMessages {
  return { diagnostics: [], stderr: String(err) };
}

export function GenerationView({ projectDir, onStatusNote }: GenerationViewProps) {
  // --- the recipe -------------------------------------------------------------------------------
  const [selected, setSelected] = useState<string | null>(() => {
    try {
      return localStorage.getItem(RECIPE_KEY);
    } catch {
      return null;
    }
  });
  const recipes = useMemo(() => recipesRoot(projectDir), [projectDir]);
  const recipePath = useMemo(
    () => (projectDir === '' || selected === null ? null : recipeFilePath(projectDir, selected)),
    [projectDir, selected],
  );
  const name = selected === null ? null : recipeName(selected);
  const session = useRecipeEdit(recipePath);

  // --- the server address (this session only; see the module doc) --------------------------------
  const [url, setUrl] = useState('');
  // Session-only like the address: the seconds `gen run` waits; empty is the verb's default.
  const [timeout, setTimeoutText] = useState('');
  const timeoutFlag = timeoutArgs(timeout);

  // --- the verbs --------------------------------------------------------------------------------
  const [busy, setBusy] = useState<BusyCounts>(IDLE);
  const [elapsed, setElapsed] = useState(0);
  const [check, setCheck] = useState<CheckModel | null>(null);
  const [checkError, setCheckError] = useState<VerbMessages | null>(null);
  // The shown run's last verb: run, fetch or status.
  const [runMessages, setRunMessages] = useState<VerbMessages | null>(null);
  const [runStatus, setRunStatus] = useState<StatusModel | null>(null);
  const [accepting, setAccepting] = useState<number | null>(null);
  const [accepted, setAccepted] = useState<AcceptModel | null>(null);
  const [acceptError, setAcceptError] = useState<VerbMessages | null>(null);

  // --- the runs ---------------------------------------------------------------------------------
  const [runIds, setRunIds] = useState<readonly string[]>([]);
  const [runId, setRunId] = useState<string | null>(null);
  const [run, setRun] = useState<RunModel | null>(null);
  // Bumped after a run lands, to re-list the run folder.
  const [runsRevision, setRunsRevision] = useState(0);

  const flushRef = useRef(session.flush);
  flushRef.current = session.flush;
  // Read at completion: a verb's result is dropped when the recipe, project or run moved on since it began.
  const scopeRef = useRef<VerbScope>({ projectDir, recipe: selected, runId });
  scopeRef.current = { projectDir, recipe: selected, runId };
  // Mirrors `busy` so two fast clicks in one frame both see the first one's count.
  const busyRef = useRef<BusyCounts>(IDLE);
  const setBusyCounts = useCallback((next: BusyCounts): void => {
    busyRef.current = next;
    setBusy(next);
  }, []);
  /** The run id whose record has been read, so a failed read is not retried forever. */
  const loadedRunRef = useRef<string | null>(null);

  /** The cancel tickets of the run and fetch calls in flight. */
  const ticketsRef = useRef<Set<string>>(new Set());

  /** One verb, with the busy/elapsed bookkeeping and the pending write flushed first. */
  const runVerb = useCallback(
    async (verb: GenVerb, args: readonly string[]): Promise<GenResult | null> => {
      if (!isTauri() || projectDir === '') {
        return null;
      }
      setBusyCounts(beginVerb(busyRef.current, verb));
      const ticket = verb === 'run' || verb === 'fetch' ? nextGenTicket() : undefined;
      if (ticket !== undefined) {
        ticketsRef.current.add(ticket);
      }
      try {
        await flushRef.current();
        return await genInvoke(verb, args, projectDir, url, ticket);
      } finally {
        if (ticket !== undefined) {
          ticketsRef.current.delete(ticket);
        }
        setBusyCounts(endVerb(busyRef.current, verb));
      }
    },
    [projectDir, url, setBusyCounts],
  );

  const running = isBusy(busy, 'run') || isBusy(busy, 'fetch');
  useEffect(() => {
    if (!running) {
      return;
    }
    setElapsed(0);
    const started = performance.now();
    const timer = window.setInterval(() => setElapsed((performance.now() - started) / 1000), TICK_MS);
    return () => window.clearInterval(timer);
  }, [running]);

  const cancelRun = useCallback((): void => {
    for (const ticket of ticketsRef.current) {
      genCancel(ticket).catch(() => {});
    }
  }, []);

  const runCheck = useCallback((): void => {
    if (selected === null) {
      return;
    }
    setCheckError(null);
    const captured: VerbScope = { projectDir, recipe: selected };
    void runVerb('check', [recipeProjectPath(selected)])
      .then((result) => {
        if (result === null || !isCurrent(captured, scopeRef.current)) {
          return;
        }
        const model = checkModel(result.json);
        setCheck(model);
        // The offline half decides the exit code, so a non-zero exit with a model is an ordinary
        // recipe error the status line already renders; stderr is only needed when there is no
        // model at all.
        setCheckError(model === null ? verbMessages(result) : null);
      })
      .catch((err: unknown) => {
        if (isCurrent(captured, scopeRef.current)) {
          setCheckError(failure(err));
        }
      });
  }, [runVerb, selected, projectDir]);

  const generate = useCallback((): void => {
    if (selected === null || timeoutFlag === null || isBusy(busyRef.current, 'run')) {
      return;
    }
    setRunMessages(null);
    setRunStatus(null);
    setAccepted(null);
    setAcceptError(null);
    const captured: VerbScope = { projectDir, recipe: selected };
    void runVerb('run', [recipeProjectPath(selected), ...timeoutFlag])
      .then((result) => {
        if (result === null || !isCurrent(captured, scopeRef.current)) {
          return;
        }
        setRunsRevision((n) => n + 1);
        const model = runModel(result.json);
        if (model !== null) {
          setRun(model);
          setRunId(model.runId);
          // The verb handed us the record, so the disk read below has nothing left to do for it.
          loadedRunRef.current = model.runId;
        }
        // A timed-out, partial or failed run still prints its record, so the diagnostics ride beside
        // its grid rather than replacing it.
        setRunMessages(verbMessages(result));
      })
      .catch((err: unknown) => {
        if (isCurrent(captured, scopeRef.current)) {
          setRunMessages(failure(err));
        }
      });
  }, [runVerb, selected, projectDir, timeoutFlag]);

  /** `gen fetch` on the shown run: one look at the server, then download what finished. */
  const fetchRun = useCallback((): void => {
    if (name === null || runId === null) {
      return;
    }
    setRunMessages(null);
    setRunStatus(null);
    const captured: VerbScope = { projectDir, recipe: selected, runId };
    void runVerb('fetch', [runProjectPath(name, runId)])
      .then((result) => {
        if (result === null || !isCurrent(captured, scopeRef.current)) {
          return;
        }
        const model = runModel(result.json);
        if (model !== null) {
          setRun(model);
          loadedRunRef.current = model.runId;
        }
        setRunMessages(verbMessages(result));
      })
      .catch((err: unknown) => {
        if (isCurrent(captured, scopeRef.current)) {
          setRunMessages(failure(err));
        }
      });
  }, [runVerb, name, runId, projectDir, selected]);

  /** `gen status` on the shown run: where its jobs are, without downloading anything. */
  const checkRunStatus = useCallback((): void => {
    if (name === null || runId === null) {
      return;
    }
    setRunMessages(null);
    setRunStatus(null);
    const captured: VerbScope = { projectDir, recipe: selected, runId };
    void runVerb('status', [runProjectPath(name, runId)])
      .then((result) => {
        if (result === null || !isCurrent(captured, scopeRef.current)) {
          return;
        }
        setRunStatus(statusModel(result.json));
        setRunMessages(verbMessages(result));
      })
      .catch((err: unknown) => {
        if (isCurrent(captured, scopeRef.current)) {
          setRunMessages(failure(err));
        }
      });
  }, [runVerb, name, runId, projectDir, selected]);

  const accept = useCallback(
    (index: number, destination: string, allowLossy: boolean): void => {
      if (name === null || runId === null) {
        return;
      }
      setAccepting(index);
      setAccepted(null);
      setAcceptError(null);
      const args = [runProjectPath(name, runId), String(index), '--out', destination];
      if (allowLossy) {
        args.push('--allow-lossy');
      }
      const captured: VerbScope = { projectDir, recipe: selected, runId };
      void runVerb('accept', args)
        .then((result) => {
          if (result === null || !isCurrent(captured, scopeRef.current)) {
            return;
          }
          const model = acceptModel(result.json);
          if (model === null || !result.ok) {
            setAcceptError(verbMessages(result));
            return;
          }
          setAccepted(model);
        })
        .catch((err: unknown) => {
          if (isCurrent(captured, scopeRef.current)) {
            setAcceptError(failure(err));
          }
        })
        .finally(() => setAccepting(null));
    },
    [runVerb, name, runId, projectDir, selected],
  );

  // ---------------------------------------------------------------------------------------------
  // A fresh recipe: clear every verb's last word, then check it. `gen check` first, always — a bad
  // graph is a local diagnostic instead of a 400 from a machine minutes later.
  useEffect(() => {
    setCheck(null);
    setCheckError(null);
    setRunMessages(null);
    setRunStatus(null);
    setAccepted(null);
    setAcceptError(null);
    setRun(null);
    setRunId(null);
    // The record is cleared with it, so the disk read must be allowed to happen again — the same
    // run can be selected twice (leave a recipe and come back to it).
    loadedRunRef.current = null;
  }, [selected]);

  // Once per recipe, never per keystroke: `runCheck` is rebuilt whenever the address field changes,
  // and a process per typed character is not a status line.
  const checkedRef = useRef<string | null>(null);
  useEffect(() => {
    if (selected === null || checkedRef.current === selected) {
      return;
    }
    checkedRef.current = selected;
    runCheck();
  }, [selected, runCheck]);

  // The run folders under this recipe, newest first.
  useEffect(() => {
    if (projectDir === '' || name === null || !isTauri()) {
      setRunIds([]);
      return;
    }
    let cancelled = false;
    // `.json` rather than no filter: directories are listed whatever the filter says, so this asks
    // for the folders plus each one's `run.json` instead of every candidate PNG in every run.
    void listEditorDir(runsRoot(projectDir, name), '.json')
      .then((entries) => {
        if (cancelled) {
          return;
        }
        const ids = runFolders(entries);
        setRunIds(ids);
        setRunId((current) => current ?? ids[0] ?? null);
      })
      .catch(() => {
        // A recipe that has never run has no folder to list; that is not an error to report.
        if (!cancelled) {
          setRunIds([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [projectDir, name, runsRevision]);

  // The selected run's record, read off disk — so an older run the view never submitted still
  // fills the grid. `run.json` carries no folder of its own, which is why the path is the caller's.
  useEffect(() => {
    if (projectDir === '' || name === null || runId === null || !isTauri()) {
      return;
    }
    // Keyed on a ref rather than on `run`: a read that fails sets `run` to null, and `run` in the
    // deps would make that failure retry itself forever.
    if (loadedRunRef.current === runId) {
      return;
    }
    loadedRunRef.current = runId;
    let cancelled = false;
    void readEditorFile(`${runsRoot(projectDir, name)}/${runId}/run.json`)
      .then((text) => {
        if (!cancelled) {
          setRun(runModel(JSON.parse(text)));
        }
      })
      .catch(() => {
        if (!cancelled) {
          setRun(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [projectDir, name, runId]);

  const handleSelect = useCallback((relPath: string): void => {
    setSelected(relPath);
    try {
      localStorage.setItem(RECIPE_KEY, relPath);
    } catch {
      // Private-mode / storage-disabled: keep it for this session only.
    }
  }, []);

  useEffect(() => {
    onStatusNote(selected === null ? null : recipeProjectPath(selected));
  }, [selected, onStatusNote]);
  useEffect(() => () => onStatusNote(null), [onStatusNote]);

  const errors = check?.diagnostics.filter((diag) => diag.severity === 'error').length ?? 0;

  return (
    <div className="main-row">
      {/* Left column: the recipe library. */}
      <div className="col col-side">
        <AssetBrowser
          root={recipes}
          ext={RECIPE_EXT}
          selected={selected}
          onSelect={handleSelect}
          reloadKey={session.savedRevision}
          head="Recipes"
          counted="recipes"
        />
      </div>

      {/* Centre column: the run bar over the recipe form, with the candidate grid under both. */}
      <div className="col-center gen-center">
        <div className="gen-bar">
          <button
            type="button"
            className="btn"
            disabled={
              selected === null || isBusy(busy) || !session.editable || timeoutFlag === null
            }
            onClick={generate}
          >
            {isBusy(busy, 'run') ? `Generating… ${elapsed.toFixed(0)}s` : 'Generate'}
          </button>
          <button
            type="button"
            className="icon-btn"
            disabled={selected === null || isBusy(busy, 'check')}
            onClick={runCheck}
          >
            {isBusy(busy, 'check') ? 'Checking…' : 'Check'}
          </button>
          <button
            type="button"
            className="icon-btn"
            disabled={run?.status !== 'submitted' || isBusy(busy)}
            onClick={fetchRun}
          >
            {isBusy(busy, 'fetch') ? `Fetching… ${elapsed.toFixed(0)}s` : 'Fetch'}
          </button>
          {running && (
            <button type="button" className="icon-btn" onClick={cancelRun}>
              Cancel
            </button>
          )}
          <button
            type="button"
            className="icon-btn"
            disabled={run?.status !== 'submitted' || isBusy(busy)}
            onClick={checkRunStatus}
          >
            {isBusy(busy, 'status') ? 'Asking…' : 'Status'}
          </button>

          <label className="gen-url">
            <span className="field-label">server</span>
            <input
              type="text"
              className="field-input"
              value={url}
              placeholder="LUDION_COMFY_URL"
              spellCheck={false}
              aria-label="ComfyUI server address for this session"
              onChange={(event) => setUrl(event.target.value)}
            />
          </label>

          <label className="gen-url">
            <span className="field-label">timeout (s)</span>
            <input
              type="text"
              inputMode="numeric"
              className="field-input"
              value={timeout}
              placeholder="600"
              aria-label="Generate timeout in seconds for this session"
              aria-invalid={timeoutFlag === null}
              onChange={(event) => setTimeoutText(event.target.value)}
            />
          </label>

          <label className="gen-runpick">
            <span className="field-label">run</span>
            <select
              className="field-select"
              value={runId ?? ''}
              aria-label="Run"
              disabled={runIds.length === 0}
              onChange={(event) => {
                setRun(null);
                setRunMessages(null);
                setRunStatus(null);
                setAccepted(null);
                setAcceptError(null);
                setRunId(event.target.value === '' ? null : event.target.value);
              }}
            >
              {runIds.length === 0 && <option value="">no runs yet</option>}
              {runIds.map((id, index) => (
                <option key={id} value={id}>
                  {index === 0 ? `${id} (newest)` : id}
                </option>
              ))}
            </select>
          </label>
        </div>

        <section className="gen-doc">
          <div className="panel-head">
            Recipe
            {check !== null && (
              <span className="count">
                {check.kind ?? '?'} · offline {check.offline} · live {check.live}
                {errors > 0 && (
                  <>
                    {' · '}
                    <span className="num">{errors}</span> errors
                  </>
                )}
              </span>
            )}
          </div>

          <div className="insp-scroll">
            {selected === null ? (
              <div className="insp-hint">
                Select a recipe to edit it. A recipe names a workflow and the values to override in
                it; the text file is the source of truth, and <code>ludion gen</code> reads it off
                the disk.
              </div>
            ) : (
              <div className="vx-model-body">
                <div className="vx-path" title={recipePath ?? selected}>
                  {recipeProjectPath(selected)}
                  {session.doc !== null && session.doc.workflow !== null && (
                    <>
                      {' → '}
                      {session.doc.workflow}
                    </>
                  )}
                </div>

                {session.conflict && (
                  <div className="code-banner" role="alert">
                    <span className="code-banner-text">
                      This recipe changed on disk while you have unsaved edits. Writes are held
                      until you choose.
                    </span>
                    <button type="button" className="icon-btn" onClick={session.reload}>
                      Reload
                    </button>
                    <button type="button" className="icon-btn" onClick={session.keepMine}>
                      Keep mine
                    </button>
                  </div>
                )}

                {session.loadError !== null && <div className="vx-error">{session.loadError}</div>}
                {session.writeError !== null && <div className="vx-error">{session.writeError}</div>}
                {checkError !== null && <DiagnosticList messages={checkError} />}
                {runMessages !== null && <DiagnosticList messages={runMessages} />}
                {runStatus !== null && (
                  <div className="vx-note">
                    {runStatus.ready
                      ? 'every job has a verdict — Fetch completes the run'
                      : `${runStatus.pending.length} still queued: ${runStatus.pending.join(', ')}`}
                    {runStatus.failed.length > 0 && ` · ${runStatus.failed.length} failed or lost`}
                  </div>
                )}

                {check !== null && check.diagnostics.length > 0 && (
                  <DiagnosticList messages={{ diagnostics: check.diagnostics, stderr: null }} />
                )}

                {check !== null && check.live === 'skipped' && (
                  <div className="vx-note">
                    Nothing answered at the resolved address, so only the offline pass ran. Set{' '}
                    <code>LUDION_COMFY_URL</code> in the environment, or type an address above for
                    this session.
                  </div>
                )}

                {session.doc !== null && (
                  <RecipeInspector
                    doc={session.doc}
                    onNumber={session.setNumberAt}
                    onText={session.setTextAt}
                    disabled={!session.editable || session.conflict || isBusy(busy)}
                  />
                )}
              </div>
            )}
          </div>
        </section>

        <CandidateGrid
          run={run}
          runDir={name === null || runId === null ? null : runProjectPath(name, runId)}
          projectDir={projectDir}
          recipe={name ?? ''}
          onAccept={accept}
          accepting={accepting}
          accepted={accepted}
          acceptError={acceptError}
        />
      </div>
    </div>
  );
}
