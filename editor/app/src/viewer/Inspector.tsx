// The entity inspector: a live list of entities and, for the selected one, its component values.
//
// It reads only public protocol commands (the Phase 4 parity requirement): `engine.describe` once to
// learn the registry's component schemas, then `query.entities` on a modest poll — filtered to
// nothing (every live entity) and including every component name, so each match carries exactly the
// components it actually has, with their current values. The poll is visibility-aware and
// non-overlapping (see `usePolling`); it stops when the tab is hidden and never stacks requests.
//
// Two faces, chosen by `mode` (Plan/phase-6-editing.md Wave 2):
//   - **play** — the selected entity's components pretty-print read-only (`JSON.stringify`).
//   - **edit** — each component renders as a widget form derived from its JSON Schema (numbers,
//     vectors, colors, strings, booleans; a raw-JSON textarea for anything the schema doesn't cleanly
//     describe). Editing a field draft-and-commits: on commit the *full* component value is written
//     back with `component.set`, and every successful mutation is reported through `onMutation` with
//     the exact command sent and the body it resolved with (the body carries the `previous` value the
//     undo stack needs — reporting it is load-bearing).
//
// Selection is lifted to `App` (props + local `useState`, no context) so the viewport's selection
// overlay reads the same handle: this component renders the current `selection` and reports clicks via
// `onSelect`. Widget derivation and value assembly live in the pure `inspect-fields` module, the commit
// round trip in `useComponentCommit`, and the inputs themselves in `FieldWidgets.tsx` — all three
// shared with the Art Director's chain rows. This file owns the fetch and the row layout.

import { useCallback, useMemo, useRef, useState } from 'react';
import type { ProtocolClient } from '../protocol/client';
import type { EntityMatch } from '@ludion/protocol';
import type { EditorMode, EntityId } from '../App';
import { toDiagnostic, type DiagnosticView } from './diagnostic';
import { DiagnosticBanner } from './DiagnosticBanner';
import { usePolling } from './usePolling';
import { FieldWidget, type FieldCommit } from './FieldWidgets';
import { useComponentCommit, type MutationReport } from './useComponentCommit';
import { deriveFields, valueAtPath, type FieldDescriptor } from './inspect-fields';

interface InspectorProps {
  readonly client: ProtocolClient;
  readonly selection: EntityId | null;
  readonly onMutation: (report: MutationReport) => void;
  /** Play = read-only pretty-print; edit = widget forms wired to `component.set`. */
  readonly mode: EditorMode;
}

/** How often the entity list refreshes (~3 Hz) — the low-Hz poll the architecture doc prescribes in
 *  place of change-detection ticks. */
const POLL_INTERVAL_MS = 300;

/** A stable string key for an entity handle (`index:generation`), used for selection and list keys.
 *  Exported so other panels (e.g. the viewport's selection data-attribute) format the same handle
 *  identically without duplicating the rule. */
export function entityKey(entity: { index: number; generation: number }): string {
  return `${entity.index}:${entity.generation}`;
}

export function Inspector({ client, selection, onMutation, mode }: InspectorProps) {
  // The registry's component schemas (name → JSON Schema), fetched once. Their keys are the component
  // names the poll asks `query.entities` to include.
  const [componentSchemas, setComponentSchemas] = useState<Record<string, unknown> | null>(null);
  const [entities, setEntities] = useState<readonly EntityMatch[]>([]);
  const [error, setError] = useState<DiagnosticView | null>(null);
  const selectedKey = selection === null ? null : entityKey(selection);
  // Kept as a ref so field handlers report through the latest `onMutation` without threading it into
  // the commit callback's dependency chain.
  const onMutationRef = useRef(onMutation);
  onMutationRef.current = onMutation;
  const report = useCallback((r: MutationReport) => onMutationRef.current(r), []);

  // Discover the registry's component schemas once; retried by remount if the client changes.
  const ensureSchemas = useCallback(async (): Promise<Record<string, unknown>> => {
    if (componentSchemas !== null) {
      return componentSchemas;
    }
    const body = await client.call({ method: 'engine.describe' });
    const schemas =
      body.kind === 'engine_description' && typeof body.description === 'object'
        ? ((body.description as { components?: Record<string, unknown> }).components ?? {})
        : {};
    setComponentSchemas(schemas);
    return schemas;
  }, [client, componentSchemas]);

  const poll = useCallback(async (): Promise<void> => {
    try {
      const schemas = await ensureSchemas();
      const body = await client.call({
        method: 'query.entities',
        params: { filter: [], include: Object.keys(schemas) },
      });
      if (body.kind === 'query_result') {
        setEntities(body.entities);
        setError(null);
      }
    } catch (err) {
      setError(toDiagnostic(err));
    }
  }, [client, ensureSchemas]);

  usePolling(true, POLL_INTERVAL_MS, poll);

  const selected = useMemo(
    () => entities.find((e) => entityKey(e.entity) === selectedKey) ?? null,
    [entities, selectedKey],
  );

  return (
    <section className="inspector">
      <div className="panel-head">
        Inspector
        <span className="count">{selected === null ? 'no selection' : `#${entityKey(selected.entity)}`}</span>
      </div>

      <DiagnosticBanner diagnostic={error} onDismiss={() => setError(null)} />

      <div className="insp-scroll">
        {selected === null ? (
          <p className="insp-hint">Select an entity to inspect its components.</p>
        ) : (
          <div className="insp-body">
            {mode === 'edit' ? (
              <EditableComponents
                client={client}
                entity={selected.entity}
                components={selected.components ?? {}}
                schemas={componentSchemas}
                onReport={report}
                onError={setError}
              />
            ) : (
              <ComponentValues match={selected} />
            )}
            {selectedKey !== null && !entities.some((e) => entityKey(e.entity) === selectedKey) && (
              <p className="insp-hint">entity {selectedKey} is gone</p>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

/** Pretty-prints one entity's live component values, read-only (play mode), one card per component. */
function ComponentValues({ match }: { readonly match: EntityMatch }) {
  const components = match.components ?? {};
  const names = Object.keys(components);
  if (names.length === 0) {
    return <p className="insp-hint">this entity carries no components</p>;
  }
  return (
    <>
      {names.map((name) => (
        <div key={name} className="card">
          <div className="card-title">{name}</div>
          <div className="card-body">
            <pre className="card-json">{JSON.stringify(components[name], null, 2)}</pre>
          </div>
        </div>
      ))}
    </>
  );
}

// --- Edit mode ----------------------------------------------------------------------------------

/** Renders the selected entity's components as editable widget forms (edit mode). */
function EditableComponents({
  client,
  entity,
  components,
  schemas,
  onReport,
  onError,
}: {
  readonly client: ProtocolClient;
  readonly entity: EntityId;
  readonly components: Readonly<Record<string, unknown>>;
  readonly schemas: Readonly<Record<string, unknown>> | null;
  readonly onReport: (report: MutationReport) => void;
  readonly onError: (diagnostic: DiagnosticView) => void;
}) {
  const names = Object.keys(components);
  if (names.length === 0) {
    return <p className="insp-hint">this entity carries no components</p>;
  }
  return (
    <>
      {names.map((name) => (
        // Keyed by entity AND name: the same-name form must remount (dropping its pending
        // assembly base and any drafts) when the selection moves to a different entity that
        // happens to carry the same component.
        <ComponentForm
          key={`${entityKey(entity)}:${name}`}
          client={client}
          entity={entity}
          name={name}
          schema={schemas?.[name]}
          value={components[name]}
          onReport={onReport}
          onError={onError}
        />
      ))}
    </>
  );
}

/** One component's editable form. The commit path — assemble the full value from a single field edit
 *  and write it back with `component.set` — is `useComponentCommit`. The poll refreshes `value` on its
 *  own cadence: the individual inputs guard their focused draft against that (see the widget comment
 *  below), and the hook guards the *assembly base* against it (`pendingBase.ts`). A selection change
 *  to another entity remounts the form entirely (keyed by entity+name above), dropping both. */
export function ComponentForm({
  client,
  entity,
  name,
  schema,
  value,
  onReport,
  onError,
}: {
  readonly client: ProtocolClient;
  readonly entity: EntityId;
  readonly name: string;
  readonly schema: unknown;
  readonly value: unknown;
  readonly onReport: (report: MutationReport) => void;
  readonly onError: (diagnostic: DiagnosticView) => void;
}) {
  // Every field of this component assembles onto the last polled value, whatever the edited path.
  const initialBase = useCallback(() => value, [value]);
  const { effective, commit } = useComponentCommit<unknown>({
    client,
    entity,
    component: name,
    value,
    initialBase,
    onReport,
    onError,
  });

  const fields = useMemo(() => deriveFields(schema, effective), [schema, effective]);

  return (
    <div className="card">
      <div className="card-title">{name}</div>
      <div className="card-body">
        <div className="component-fields">
          {fields.map((field, i) => (
            <FieldRow
              key={`${field.kind}:${field.label}:${i}`}
              field={field}
              current={valueAtPath(effective, field.path)}
              commit={commit}
              onError={onError}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

/** One field's row: its label, and the shared widget for its kind (`FieldWidgets.tsx`). Exported
 *  because it is the row EVERY schema-derived field is drawn as — the Effects view's emitter fields
 *  included; a second surface rendering a look-alike would drift from this one. */
export function FieldRow({
  field,
  current,
  commit,
  onError,
}: {
  readonly field: FieldDescriptor;
  readonly current: unknown;
  readonly commit: FieldCommit;
  readonly onError: (diagnostic: DiagnosticView) => void;
}) {
  return (
    <div className={field.kind === 'json' ? 'field field-block' : 'field'}>
      {field.label !== '' && <span className="field-label">{field.label}</span>}
      <FieldWidget field={field} value={current} commit={commit} onError={onError} />
    </div>
  );
}
