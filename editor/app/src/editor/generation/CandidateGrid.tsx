// The candidate grid: one cell per output of the selected run, and the accept that freezes one into
// the project.
//
// THE LABEL IS THE INDEX, NOT THE POSITION. Cells fill row-major from `#0`, but the label is what a
// human and a model both actually read (`ludion docs generation-runs`), so every cell carries
// `#<index> seed <n>` (`seed ?` when the run could not know it) whatever the layout does at a narrow
// width.
//
// A MESH RUN HAS NO THUMBNAIL. `gen sheet` refuses one with `L1171` and points at the model stage,
// so a mesh candidate is one cell stating what it is and how big the file is — the look happens in
// the Models view once it has been accepted and imported.
//
// ACCEPT IS THE ONE BRIDGE, AND IT NEVER OVERWRITES. The dialog proposes a project-relative
// destination and the verb refuses an occupied one with `L1172`; the refusal is shown verbatim
// rather than being turned into a retry, because choosing another name is the human's call.

import { useCallback, useEffect, useState } from 'react';
import { editorFileSize, readEditorImage } from '../../shell/genHost';
import { DiagnosticList } from './DiagnosticList';
import {
  candidateLabel,
  defaultDestination,
  type AcceptModel,
  type CandidateModel,
  type FailedJobModel,
  type RunModel,
  type VerbMessages,
} from './model';
import { projectFilePath } from './paths';

export interface CandidateGridProps {
  /** The run whose candidates these are, or `null` with none selected. */
  readonly run: RunModel | null;
  /** The run folder, project-relative — where a candidate's file sits. */
  readonly runDir: string | null;
  readonly projectDir: string;
  /** The recipe's run-folder name, for the default destination. */
  readonly recipe: string;
  /** Runs `gen accept <runDir> <index> --out <destination> [--allow-lossy]`. */
  readonly onAccept: (index: number, destination: string, allowLossy: boolean) => void;
  /** The index currently being accepted, or `null`. */
  readonly accepting: number | null;
  /** What the last accept wrote. */
  readonly accepted: AcceptModel | null;
  /** The last accept's refusal (`L1172`, `L1202`, …). */
  readonly acceptError: VerbMessages | null;
}

/** One failed or lost job, shaped as the diagnostic row the verbs' own messages use. */
function FailedJob({ job }: { job: FailedJobModel }) {
  return (
    <div className="vx-diag error">
      <span className="vx-diag-code">job {job.job}</span>
      <span className="vx-diag-msg">{job.message}</span>
      {job.class !== null && (
        <span className="vx-diag-loc">
          {job.class}
          {job.node !== null && ` (node ${job.node})`}
        </span>
      )}
      {job.reason === 'lost' && (
        <span className="vx-diag-help">lost — the server no longer knows this job</span>
      )}
    </div>
  );
}

/** A byte count as a human reads it. Two significant places past a kilobyte, which is where the
 *  difference between two mesh candidates actually shows. */
function bytes(size: number): string {
  if (size < 1024) {
    return `${size} B`;
  }
  if (size < 1024 * 1024) {
    return `${(size / 1024).toFixed(1)} KB`;
  }
  return `${(size / (1024 * 1024)).toFixed(2)} MB`;
}

/** One image candidate's thumbnail, loaded on mount and dropped with the cell. A run folder is
 *  scratch and a candidate is read once, so there is nothing worth caching across runs. */
function Thumbnail({ path, label }: { path: string; label: string }) {
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    setError(null);
    void readEditorImage(path)
      .then((url) => {
        if (!cancelled) {
          setSrc(url);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(String(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (error !== null) {
    return <div className="gen-cell-empty">{error}</div>;
  }
  if (src === null) {
    return <div className="gen-cell-empty">loading…</div>;
  }
  return <img className="gen-cell-img" src={src} alt={label} />;
}

/** A mesh candidate's one stated fact: it is a mesh, and this is how big it is. */
function MeshFacts({ path }: { path: string }) {
  const [size, setSize] = useState<number | null>(null);
  useEffect(() => {
    let cancelled = false;
    setSize(null);
    void editorFileSize(path)
      .then((value) => {
        if (!cancelled) {
          setSize(value);
        }
      })
      .catch(() => {
        // A size the host could not read is not worth a banner: the cell just says less.
      });
    return () => {
      cancelled = true;
    };
  }, [path]);
  return (
    <div className="gen-cell-empty gen-cell-mesh">
      <span>mesh</span>
      {size !== null && <span className="num">{bytes(size)}</span>}
      <span className="gen-cell-note">look at it in the Models view once accepted</span>
    </div>
  );
}

export function CandidateGrid(props: CandidateGridProps) {
  const { run, runDir, projectDir, recipe, onAccept, accepting, accepted, acceptError } = props;
  const [target, setTarget] = useState<CandidateModel | null>(null);
  const [destination, setDestination] = useState('');
  const [allowLossy, setAllowLossy] = useState(false);

  const openAccept = useCallback(
    (candidate: CandidateModel): void => {
      setTarget(candidate);
      setDestination(defaultDestination(recipe, candidate.index, run?.kind ?? null));
      // Strict is the default on the CLI and stays the default here: a candidate that needs the
      // flag should say so out loud once (`ludion docs generation-runs`).
      setAllowLossy(false);
    },
    [recipe, run],
  );

  if (run === null || runDir === null) {
    return (
      <section className="gen-grid">
        <div className="panel-head">Candidates</div>
        <div className="insp-hint">
          No run selected. Generate one, or pick an earlier run from the header.
        </div>
      </section>
    );
  }

  const isMesh = run.kind === 'mesh';

  return (
    <section className="gen-grid">
      <div className="panel-head">
        Candidates
        <span className="count">
          <span className="num">{run.candidates.length}</span> of {run.count}
        </span>
      </div>

      {run.status === 'submitted' && (
        <div className="vx-note">
          This run is <code>submitted</code> — its jobs are still on the server. <b>Status</b> says
          which are still queued; <b>Fetch</b> completes the run once every job has finished.
        </div>
      )}
      {run.status === 'partial' && (
        <div className="vx-note">
          This run is <code>partial</code> — some jobs did not produce a candidate.
        </div>
      )}
      {run.status === 'failed' && (
        <div className="vx-note">
          This run is <code>failed</code> — no job produced a candidate.
        </div>
      )}
      {run.failed.length > 0 && (
        <div className="vx-diags">
          {run.failed.map((job) => (
            <FailedJob key={job.job} job={job} />
          ))}
        </div>
      )}

      {acceptError !== null && <DiagnosticList messages={acceptError} />}

      {accepted !== null && (
        <div className="vx-note gen-accepted">
          <div>
            wrote <code>{accepted.destination}</code>
          </div>
          {accepted.sidecar !== null && (
            <div>
              provenance <code>{accepted.sidecar}</code>
            </div>
          )}
          {accepted.meshes.map((mesh) => (
            <div key={mesh.path}>
              <code>{mesh.path}</code> — <span className="num">{mesh.vertices}</span> vertices,{' '}
              <span className="num">{mesh.triangles}</span> triangles
            </div>
          ))}
          {accepted.textures.length > 0 && (
            <div>
              <span className="num">{accepted.textures.length}</span> textures
            </div>
          )}
        </div>
      )}

      <div className="gen-cells">
        {run.candidates.map((candidate) => {
          const path = projectFilePath(projectDir, `${runDir}/${candidate.file}`);
          const label = candidateLabel(candidate);
          return (
            <div key={candidate.index} className="gen-cell">
              <div className="gen-cell-body">
                {isMesh ? <MeshFacts path={path} /> : <Thumbnail path={path} label={label} />}
              </div>
              <div className="gen-cell-foot">
                <span className="gen-cell-label num">{label}</span>
                <button
                  type="button"
                  className="icon-btn"
                  disabled={accepting !== null}
                  onClick={() => openAccept(candidate)}
                >
                  {accepting === candidate.index ? 'Accepting…' : 'Accept'}
                </button>
              </div>
            </div>
          );
        })}
        {run.candidates.length === 0 && (
          <div className="insp-hint">No candidate downloaded.</div>
        )}
      </div>

      {target !== null && (
        <div className="gen-accept">
          <div className="gen-accept-head">
            Accept <span className="num">#{target.index}</span>
          </div>
          <label className="gen-accept-field">
            <span className="field-label">destination (project-relative)</span>
            <input
              type="text"
              className="field-input"
              value={destination}
              spellCheck={false}
              aria-label="Accept destination"
              onChange={(event) => setDestination(event.target.value)}
            />
          </label>
          {isMesh && (
            <label className="gen-accept-lossy">
              <input
                type="checkbox"
                checked={allowLossy}
                onChange={(event) => setAllowLossy(event.target.checked)}
              />
              <span>
                allow lossy — a generator&apos;s <code>SaveGLB</code> writes{' '}
                <code>doubleSided</code>, which strict import refuses with <code>L1202</code>
              </span>
            </label>
          )}
          <div className="gen-accept-actions">
            <button
              type="button"
              className="btn"
              disabled={destination.trim() === '' || accepting !== null}
              onClick={() => {
                onAccept(target.index, destination.trim(), allowLossy);
                setTarget(null);
              }}
            >
              Accept
            </button>
            <button type="button" className="icon-btn" onClick={() => setTarget(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
