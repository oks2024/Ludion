// The Effects view's pattern tab: pick a `kind: pattern` file, edit it through a schema-driven form,
// and watch the volleys it fires stand up on the same stage the emitter tab tunes.
//
// A DOC EDIT WRITES THE FILE IMMEDIATELY (`Plan/projectile-patterns.md` decision 10). This is the
// opposite of a scene edit, and of the emitter tab beside it: a `ParticleEmitter`'s live component
// value IS the effect, with nothing on disk under it, whereas a pattern document's file is its only
// truth — the engine holds a `DocStore` entry parsed from it, and there is no `doc.save`. So every
// committed field is a write followed by `asset.reload { path }`, which is exactly the two steps an
// agent takes, so the tab adds no wire method. It fires `asset.reload` itself because the stage
// watcher covers only `voxels/` and `sculpts/`. The view says so in one line, because a surface that
// saves on every keystroke has to.
//
// THE WRITE IS `systems/syncedFile.ts`'s, the same session the voxel, model and recipe editors use:
// it polls the open file, and an agent's terminal edit made between two polls or while the form holds
// unsaved changes raises the Reload / Keep mine banner and HOLDS the write, rather than reverting the
// agent's work with the form's older copy.
//
// THE PICKER READS FILES, because the host cannot filter on content. `list_editor_dir(root, ext)`
// matches a filename suffix, and `.yaml` under the content root is every scene, atlas source and
// data doc a project owns — so the listing is read and filtered on a top-level `kind: pattern` line
// (`patternFiles.ts`). One pass per open of the tab, cached until something asks for a re-list.
//
// A SLIDER DRAG DOES NOT WRITE SIXTY FILES A SECOND. A `'preview'` cadence is by definition not a
// commit (`viewer/useComponentCommit.ts`): it updates the open document in memory and writes on the
// shared throttle (a write, and so an `asset.reload`, at most every 150 ms, the last value always
// following); any other cadence writes now. So "every commit writes the file" stays literally true
// and the drag's final value always lands.
//
// THE HOST MAY NOT KNOW WHAT A PATTERN IS. The effects stage is plain `ludion serve` with no game
// crate, so nothing calls `.patterns()` and `engine.describe` reports no `documents.pattern`. The
// tab still lists, edits and saves against the bundled schema (`patternSchema.ts`); what it cannot
// do is play anything, and it says which.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ResponseBody } from '@ludion/protocol';
import type { ProtocolClient } from '../../protocol/client';
import { componentSet, entityRef } from '../../protocol/commands';
import { listEditorDir } from '../../shell/host';
import { readEditorFile, writeEditorFile } from '../../terminal/pty';
import { diagnosticText, toDiagnostic, type DiagnosticView } from '../../viewer/diagnostic';
import type { FieldPath } from '../../viewer/inspect-fields';
import type { EntityId } from '../undo';
import { useSyncedFile } from '../systems/useSyncedFile';
import { DocForm } from './DocForm';
import { deriveDocForm, markFloats, subschemaAt, type DocEdit } from './docFields';
import {
  docKey,
  docLabel,
  docPath,
  isPatternDoc,
  newPatternKey,
  newPatternText,
  PATTERN_EXT,
} from './patternFiles';
import { PATTERN_SCHEMA } from './patternSchema';
import { contentRoot, PATTERN_PLAYER, TRANSFORM } from './stage';
import { docValue, parseDoc, removeAtPath, renderDoc, setAtPath } from './yamlDoc';

/** The line the view owes a human the moment the tab opens. */
const IMMEDIATE_WRITE_NOTE = 'Edits write the file immediately — the file is the pattern’s only truth.';

export interface PatternEditorProps {
  /** The stage's own client, dialed. */
  readonly client: ProtocolClient;
  /** The stage scene's `subject`, which carries the `PatternPlayer`, or `null`. */
  readonly subject: EntityId | null;
  /** The stage scene's `target` aim marker, or `null`. */
  readonly target: EntityId | null;
  /** The project directory the stage was started on; its `scenes/` is the content root. */
  readonly projectDir: string;
  /** `documents.pattern` from `engine.describe`, or `null` on a host registering no pattern kind. */
  readonly schema: unknown | null;
  readonly onError: (diagnostic: DiagnosticView) => void;
}

/** The `value` of a `component.get` response, or `null`. */
function componentValue(body: ResponseBody): Record<string, unknown> | null {
  const reply = body as { kind?: unknown; value?: unknown };
  if (reply.kind !== 'component_value' || typeof reply.value !== 'object' || reply.value === null) {
    return null;
  }
  return reply.value as Record<string, unknown>;
}

/** An `asset.reload` reply as one line, verbatim enough to be worth reading. */
function reloadNote(body: ResponseBody): string {
  const reply = body as { kind?: unknown; paths?: unknown };
  if (reply.kind === 'assets_reloaded' && Array.isArray(reply.paths)) {
    return reply.paths.length === 0 ? 'saved — nothing reloaded' : `saved and reloaded ${reply.paths.join(', ')}`;
  }
  return `saved — ${JSON.stringify(body)}`;
}

/** The two horizontal components of a translation the target fields move. The stage's visible plane
 *  is its camera's, and its `target` sits three units along world X at the floor's height, so X and Z
 *  are the two a human drags it in; Y is the marker's own height and is not a pattern's business. */
const TARGET_AXES: readonly { readonly label: string; readonly index: number }[] = [
  { label: 'x', index: 0 },
  { label: 'z', index: 2 },
];

export function PatternEditor(props: PatternEditorProps) {
  const { client, subject, target, projectDir, schema, onError } = props;
  const root = useMemo(() => contentRoot(projectDir), [projectDir]);

  const [keys, setKeys] = useState<readonly string[]>([]);
  const [listing, setListing] = useState(true);
  const [openKey, setOpenKey] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [fresh, setFresh] = useState('');
  const [translation, setTranslation] = useState<readonly number[] | null>(null);

  /** The doc key behind each opened path, so a write that lands after a switch reloads ITS document. */
  const keyByPath = useRef(new Map<string, string>());
  const pointedKey = useRef<string | null>(null);
  /** The target's whole `Transform`: `component.set` replaces a component, never patches it. */
  const targetTransform = useRef<Record<string, unknown> | null>(null);

  const {
    current,
    edit: writeEdit,
    doc,
    loadError,
    writeError,
    phase,
    reload,
    keepMine,
  } = useSyncedFile(openKey === null ? null : docPath(root, openKey), {
    parse: parseDoc,
    source: renderDoc,
    afterWrite: async (written) => {
      const key = keyByPath.current.get(written);
      if (key === undefined) {
        return;
      }
      try {
        setNote(reloadNote(await client.call({ method: 'asset.reload', params: { path: key } })));
      } catch (err) {
        setNote(diagnosticText(toDiagnostic(err)));
      }
    },
  });
  const value = useMemo(() => (doc === null ? null : docValue(doc)), [doc]);

  // The live schema when the host knows the kind, the bundled copy otherwise — one form either way.
  const formSchema = schema ?? PATTERN_SCHEMA;
  const nodes = useMemo(
    () => (value === null ? [] : deriveDocForm(formSchema, value)),
    [formSchema, value],
  );

  // ---------------------------------------------------------------------------------------------
  // The listing: every `.yaml` under the content root whose text carries a top-level `kind: pattern`.
  const relist = useCallback(async (): Promise<void> => {
    if (projectDir === '') {
      setKeys([]);
      setListing(false);
      return;
    }
    setListing(true);
    try {
      const entries = await listEditorDir(root, PATTERN_EXT);
      const found: string[] = [];
      for (const entry of entries) {
        if (entry.isDir) {
          continue;
        }
        const key = docKey(root, entry.relPath);
        try {
          if (isPatternDoc(await readEditorFile(docPath(root, key)))) {
            found.push(key);
          }
        } catch {
          // An unreadable candidate is simply not a pattern document; the picker is not the place
          // to report a file nobody asked to open.
        }
      }
      setKeys(found);
    } catch (err) {
      setKeys([]);
      setNote(String(err));
    } finally {
      setListing(false);
    }
  }, [projectDir, root]);

  useEffect(() => {
    void relist();
  }, [relist]);

  // ---------------------------------------------------------------------------------------------
  // The stage subject's `PatternPlayer`: which document it plays, from which frame, aimed at what.
  const pointPlayer = useCallback(
    async (key: string): Promise<void> => {
      if (subject === null) {
        return;
      }
      const { gameFrame: frame } = await client.callFull({
        method: 'component.get',
        params: { entity: subject, component: PATTERN_PLAYER },
      });
      await client.call(
        componentSet(subject, PATTERN_PLAYER, {
          pattern: key,
          started: typeof frame === 'number' ? frame : 0,
          target: target === null ? null : entityRef(target),
        }),
      );
    },
    [client, subject, target],
  );

  const open = useCallback(
    (key: string): void => {
      keyByPath.current.set(docPath(root, key), key);
      setOpenKey(key);
      setNote(null);
    },
    [root],
  );

  // The session loads the file; once it has, the stage subject plays it. A file that cannot be read
  // closes the tab's document and surfaces the error.
  useEffect(() => {
    if (loadError !== null) {
      setOpenKey(null);
      onError(toDiagnostic(loadError));
      return;
    }
    if (openKey === null) {
      pointedKey.current = null;
    } else if (doc !== null && pointedKey.current !== openKey) {
      pointedKey.current = openKey;
      void pointPlayer(openKey).catch((err: unknown) => onError(toDiagnostic(err)));
    }
  }, [openKey, doc, loadError, pointPlayer, onError]);

  // Leaving the tab (or the session) stops the stage firing a document nobody is looking at.
  useEffect(
    () => () => {
      if (subject !== null) {
        void client
          .call(componentSet(subject, PATTERN_PLAYER, { pattern: '', started: 0, target: null }))
          .catch(() => {
            // The session is going away with the tab; a refused clear has nobody left to tell.
          });
      }
    },
    [client, subject],
  );

  /**
   * One form edit: into a copy of the document, then onto disk through the session. Whatever the
   * engine answers to the follow-up reload — a reload or a diagnostic — is the note, verbatim; a host
   * that does not register the pattern kind answers there and must be read.
   *
   * A timeline entry created by the list widget carries the schema's defaults and no `bullet`, which
   * is `L1149` the moment it is validated. The one pattern-specific default in this file fills it
   * with the document's first bullet kind, so "Add" produces a document the engine accepts.
   */
  const applyEdit = useCallback(
    (change: DocEdit): void => {
      const file = current();
      if (file === null) {
        return;
      }
      // The session's document is never mutated: the `yaml` document API edits in place.
      const next = file.clone();
      if (change.op === 'remove') {
        removeAtPath(next, change.path);
      } else {
        const filled = fillNewTimelineEntry(change.path, change.value, value);
        setAtPath(next, change.path, markFloats(filled, subschemaAt(formSchema, change.path), formSchema));
      }
      writeEdit(next, change.op === 'set' && change.cadence === 'preview' ? 'throttle' : 'commit');
    },
    [current, formSchema, value, writeEdit],
  );

  // ---------------------------------------------------------------------------------------------
  // The aim marker. The gizmo the plan wanted lives in a viewport this stage deliberately does not
  // make interactive, so the two horizontal axes are plain fields — the same `component.set` a drag
  // would have sent.
  useEffect(() => {
    if (target === null) {
      setTranslation(null);
      return;
    }
    let cancelled = false;
    void client
      .call({ method: 'component.get', params: { entity: target, component: TRANSFORM } })
      .then((body) => {
        const transform = componentValue(body);
        if (cancelled || transform === null) {
          return;
        }
        targetTransform.current = transform;
        setTranslation(Array.isArray(transform.translation) ? (transform.translation as number[]) : null);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          onError(toDiagnostic(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [client, target, onError]);

  const moveTarget = useCallback(
    (index: number, next: number): void => {
      const transform = targetTransform.current;
      if (target === null || transform === null || translation === null) {
        return;
      }
      const moved = [...translation];
      moved[index] = next;
      targetTransform.current = { ...transform, translation: moved };
      setTranslation(moved);
      void client
        .call(componentSet(target, TRANSFORM, targetTransform.current))
        .catch((err: unknown) => onError(toDiagnostic(err)));
    },
    [client, target, translation, onError],
  );

  const create = useCallback((): void => {
    const key = newPatternKey(fresh);
    if (key === null) {
      return;
    }
    setFresh('');
    void (async () => {
      try {
        await writeEditorFile(docPath(root, key), newPatternText(fresh));
        await relist();
        open(key);
      } catch (err) {
        onError(toDiagnostic(err));
      }
    })();
  }, [fresh, root, relist, open, onError]);

  return (
    <div className="fx-pattern">
      <div className="fx-doc-head">
        <select
          className="fx-select fx-doc-pick"
          value={openKey ?? ''}
          onChange={(event) => {
            const key = event.target.value;
            if (key !== '') {
              open(key);
            }
          }}
        >
          <option value="">{listing ? 'reading…' : 'choose a pattern…'}</option>
          {keys.map((key) => (
            <option key={key} value={key}>
              {docLabel(key)}
            </option>
          ))}
        </select>
        <button type="button" className="icon-btn fx-mini" onClick={() => void relist()}>
          Rescan
        </button>
      </div>

      <p className="fx-hint fx-doc-warning">{IMMEDIATE_WRITE_NOTE}</p>
      {openKey !== null && <p className="fx-hint fx-doc-key">{openKey}</p>}
      {schema === null && (
        <p className="fx-hint fx-doc-warning">
          This stage’s engine registers no <code>pattern</code> kind, so nothing plays here — the form
          is the bundled schema and saving still writes the file.
        </p>
      )}
      {phase === 'conflict' && (
        <div className="code-banner" role="alert">
          <span className="code-banner-text">
            This pattern changed on disk while you have unsaved edits. Writes are held until you choose.
          </span>
          <button type="button" className="icon-btn" onClick={reload}>
            Reload
          </button>
          <button type="button" className="icon-btn" onClick={keepMine}>
            Keep mine
          </button>
        </div>
      )}
      {(writeError ?? note) !== null && <p className="fx-hint fx-doc-note">{writeError ?? note}</p>}

      <div className="fx-add">
        <input
          className="field-input"
          placeholder="new pattern name"
          value={fresh}
          onChange={(event) => setFresh(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              create();
            }
          }}
        />
        <button type="button" className="icon-btn fx-mini" disabled={newPatternKey(fresh) === null} onClick={create}>
          New
        </button>
      </div>

      {translation !== null && (
        <div className="fx-target">
          <span className="field-label">aim target</span>
          {TARGET_AXES.map((axis) => (
            <label className="fx-target-axis" key={axis.label}>
              <span className="field-channel">{axis.label}</span>
              <input
                className="field-input"
                inputMode="decimal"
                value={String(translation[axis.index] ?? 0)}
                onChange={(event) => {
                  const next = Number(event.target.value);
                  if (Number.isFinite(next)) {
                    moveTarget(axis.index, next);
                  }
                }}
              />
            </label>
          ))}
        </div>
      )}

      {value === null ? (
        <p className="fx-hint">
          {keys.length === 0 && !listing
            ? 'No kind: pattern document under the project’s content root yet — name one above.'
            : 'Choose a pattern document to edit.'}
        </p>
      ) : (
        <DocForm nodes={nodes} value={value} onEdit={applyEdit} onError={onError} />
      )}
    </div>
  );
}

/** The one pattern-specific default: a timeline entry the list widget just created gets the
 *  document's first bullet kind, so "Add" produces something the validator accepts rather than an
 *  `L1149` a human has to read before their first edit. */
function fillNewTimelineEntry(path: FieldPath, value: unknown, doc: unknown): unknown {
  if (path.length !== 2 || path[0] !== 'timeline' || typeof path[1] !== 'number') {
    return value;
  }
  if (typeof value !== 'object' || value === null || Array.isArray(value) || 'bullet' in value) {
    return value;
  }
  const bullets = (doc as { bullets?: unknown } | null)?.bullets;
  const first =
    typeof bullets === 'object' && bullets !== null && !Array.isArray(bullets)
      ? Object.keys(bullets)[0]
      : undefined;
  return first === undefined ? value : { ...value, bullet: first };
}
