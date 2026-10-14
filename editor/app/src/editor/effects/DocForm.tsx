// The renderer for a `docFields.ts` tree: nested groups, keyed maps, ordered lists, optional blocks,
// and — at every leaf — the editor's one field row.
//
// THE LEAVES ARE THE LEVEL DESIGNER'S ROWS, verbatim (`viewer/Inspector.tsx`'s `FieldRow`). A
// `speed` in a pattern document must look and behave like a `speed` anywhere else in the editor, or
// a human learns that two identical-looking things are different. What this file adds is only what
// a document has and a component does not: the containers.
//
// EVERY CONTROL EMITS A `DocEdit`, never a file write. This component holds no state beyond the
// text a human is part-way through typing into a key or a new-entry name; the open document, the
// write and the reload all belong to `PatternEditor`. That is what keeps the whole derivation
// testable without rendering anything.

import { useCallback, useState, type ReactNode } from 'react';
import type { DiagnosticView } from '../../viewer/diagnostic';
import type { FieldCommit } from '../../viewer/FieldWidgets';
import { valueAtPath, type FieldPath } from '../../viewer/inspect-fields';
import { FieldRow } from '../../viewer/Inspector';
import type { DocEdit, DocListNode, DocMapNode, DocNode } from './docFields';

export interface DocFormProps {
  /** The derived tree (`deriveDocForm`). */
  readonly nodes: readonly DocNode[];
  /** The document value the tree was derived from — what each row displays. */
  readonly value: unknown;
  readonly onEdit: (edit: DocEdit) => void;
  readonly onError: (diagnostic: DiagnosticView) => void;
}

/** A path as a stable React key. */
function pathKey(path: FieldPath): string {
  return path.join('.');
}

export function DocForm({ nodes, value, onEdit, onError }: DocFormProps) {
  const commit: FieldCommit = useCallback(
    (path, next, cadence) => onEdit({ op: 'set', path, value: next, cadence }),
    [onEdit],
  );

  const render = (node: DocNode): ReactNode => {
    switch (node.kind) {
      case 'field':
        return (
          <FieldRow
            key={pathKey(node.path)}
            field={node.field}
            current={valueAtPath(value, node.field.path)}
            commit={commit}
            onError={onError}
          />
        );

      case 'group':
        return (
          <section className="fx-group" key={pathKey(node.path)}>
            <h4 className="fx-group-head" title={node.help ?? undefined}>
              {node.label}
            </h4>
            <div className="component-fields">{node.children.map(render)}</div>
          </section>
        );

      case 'optional':
        return (
          <section className="fx-group fx-optional" key={pathKey(node.path)}>
            <label className="fx-group-head" title={node.help ?? undefined}>
              <input
                className="field-checkbox"
                type="checkbox"
                checked={node.present}
                onChange={(event) =>
                  onEdit(
                    event.target.checked
                      ? { op: 'set', path: node.path, value: node.enabled }
                      : { op: 'remove', path: node.path },
                  )
                }
              />
              {node.label}
            </label>
            {node.present && <div className="component-fields">{render(node.inner)}</div>}
          </section>
        );

      case 'map':
        return <MapBlock key={pathKey(node.path)} node={node} onEdit={onEdit} render={render} />;

      case 'list':
        return <ListBlock key={pathKey(node.path)} node={node} onEdit={onEdit} render={render} />;
    }
  };

  return <div className="fx-doc-form">{nodes.map(render)}</div>;
}

/** A keyed map: rename, remove, add. A rename rewrites the whole map so the surviving entries keep
 *  their order — the one edit here that cannot be a single-path write. */
function MapBlock({
  node,
  onEdit,
  render,
}: {
  readonly node: DocMapNode;
  readonly onEdit: (edit: DocEdit) => void;
  readonly render: (node: DocNode) => ReactNode;
}) {
  const [fresh, setFresh] = useState('');

  const add = (): void => {
    const key = fresh.trim();
    if (key === '' || node.entries.some((entry) => entry.key === key)) {
      return;
    }
    setFresh('');
    onEdit({ op: 'set', path: [...node.path, key], value: node.entryDefault ?? {} });
  };

  const rename = (from: string, to: string): void => {
    const next = to.trim();
    if (next === '' || next === from || node.entries.some((entry) => entry.key === next)) {
      return;
    }
    onEdit({ op: 'set', path: node.path, value: renameIn(node, from, next) });
  };

  return (
    <section className="fx-block">
      <div className="fx-block-head" title={node.help ?? undefined}>
        {node.label}
        <span className="count">{node.entries.length}</span>
      </div>
      {node.entries.map((entry) => (
        <div className="fx-entry" key={entry.key}>
          <div className="fx-entry-bar">
            <NameInput value={entry.key} onCommit={(next) => rename(entry.key, next)} />
            <button
              type="button"
              className="icon-btn fx-mini"
              title={`remove ${entry.key}`}
              onClick={() => onEdit({ op: 'remove', path: [...node.path, entry.key] })}
            >
              ✕
            </button>
          </div>
          {render(entry.node)}
        </div>
      ))}
      <div className="fx-add">
        <input
          className="field-input"
          placeholder="new name"
          value={fresh}
          onChange={(event) => setFresh(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              add();
            }
          }}
        />
        <button type="button" className="icon-btn fx-mini" onClick={add}>
          Add
        </button>
      </div>
    </section>
  );
}

/** The map's value with one key renamed in place, order preserved. */
function renameIn(node: DocMapNode, from: string, to: string): Record<string, unknown> {
  const renamed: Record<string, unknown> = {};
  for (const entry of node.entries) {
    renamed[entry.key === from ? to : entry.key] = entry.value;
  }
  return renamed;
}

/** An ordered list: append, remove, move. A move rewrites the whole list, for the map's reason. */
function ListBlock({
  node,
  onEdit,
  render,
}: {
  readonly node: DocListNode;
  readonly onEdit: (edit: DocEdit) => void;
  readonly render: (node: DocNode) => ReactNode;
}) {
  const move = (from: number, to: number): void => {
    if (to < 0 || to >= node.values.length) {
      return;
    }
    const next = [...node.values];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    onEdit({ op: 'set', path: node.path, value: next });
  };

  return (
    <section className="fx-block">
      <div className="fx-block-head" title={node.help ?? undefined}>
        {node.label}
        <span className="count">{node.items.length}</span>
      </div>
      {node.items.map((item, i) => (
        <div className="fx-entry" key={pathKey(item.path)}>
          <div className="fx-entry-bar">
            <span className="fx-entry-name">{i + 1}</span>
            <button
              type="button"
              className="icon-btn fx-mini"
              title="move up"
              disabled={i === 0}
              onClick={() => move(i, i - 1)}
            >
              ↑
            </button>
            <button
              type="button"
              className="icon-btn fx-mini"
              title="move down"
              disabled={i === node.items.length - 1}
              onClick={() => move(i, i + 1)}
            >
              ↓
            </button>
            <button
              type="button"
              className="icon-btn fx-mini"
              title="remove"
              onClick={() => onEdit({ op: 'remove', path: [...node.path, i] })}
            >
              ✕
            </button>
          </div>
          {render(item)}
        </div>
      ))}
      <div className="fx-add">
        <button
          type="button"
          className="icon-btn fx-mini"
          onClick={() =>
            onEdit({ op: 'set', path: [...node.path, node.items.length], value: node.itemDefault ?? {} })
          }
        >
          Add
        </button>
      </div>
    </section>
  );
}

/** A map key's own input: a local draft committed on Enter or blur, reverted on Escape — the same
 *  contract every text field in the editor keeps, minus the refresh race (a key only changes when
 *  this input changes it). */
function NameInput({
  value,
  onCommit,
}: {
  readonly value: string;
  readonly onCommit: (next: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  const [editing, setEditing] = useState(false);
  const shown = editing ? draft : value;
  return (
    <input
      className="field-input fx-entry-name"
      value={shown}
      onFocus={() => {
        setDraft(value);
        setEditing(true);
      }}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={() => {
        setEditing(false);
        onCommit(draft);
      }}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.currentTarget.blur();
        } else if (event.key === 'Escape') {
          setDraft(value);
          setEditing(false);
        }
      }}
    />
  );
}
