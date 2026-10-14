// The hierarchy panel (Meridian): the top half of the left column — a tree of every live entity,
// indented by parenting, under a scene root. Thin wiring only — the tree/forest shape, dangling/cyclic
// classification, reparent-target list, and reparent-command decision all live in the pure
// `hierarchy.ts` (see hierarchy.test.ts); this component owns the poll, click-to-select, the reparent
// `<select>`'s edit-mode gating, the pointer-driven drag-to-reparent, and reporting the entity count
// up to the status bar.
//
// Drag uses pointer events, not HTML5 drag-and-drop: Tauri on Windows intercepts DnD while the window's
// drag-drop handling is on. No pointer capture — it would starve the other rows' `pointerenter`.
//
// A prefab instance is one collapsible header row over its parts. When a part is selected in edit mode
// an action bar offers Apply / Revert / Revert part / Unpack over `prefab.*` (`prefab-actions.ts` owns
// the undo capture). Revert and unpack are one undo step each; apply writes the prefab file, so it asks
// first and records nothing. Override markers (part flags, header placement/removed/diagnostic flags)
// and the selected part's per-field revert list come from `prefab.overrides` (`prefab-overrides.ts`),
// polled at most ~1 Hz and refreshed right after each panel action; its failure clears the markers
// without touching the banner.
//
// Polls `query.entities` on its own ~3 Hz cadence (the `usePolling` pattern), independently of the
// inspector's poll — the two panels ask for different component sets (`Name`/`Parent`/`PrefabInstance`
// here vs. every registry component there) and neither should block on the other.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ProtocolClient } from '../protocol/client';
import { entityRef } from '../protocol/commands';
import type { EditorMode, EntityId } from '../App';
import { entityKey } from '../viewer/Inspector';
import { usePolling } from '../viewer/usePolling';
import { toDiagnostic, type DiagnosticView } from '../viewer/diagnostic';
import { DiagnosticBanner } from '../viewer/DiagnosticBanner';
import { ask } from '@tauri-apps/plugin-dialog';
import type { Command, ResponseBody } from '@ludion/protocol';
import {
  EMPTY_OVERRIDES,
  buildTree,
  displayName,
  dropCommand,
  parentOptions,
  parseHierarchy,
  reparentCommand,
  type HierarchyRow,
} from './hierarchy';
import { reparentEntries } from './mutations';
import { runInstanceAction, revertPartPointer, type InstanceOp } from './prefab-actions';
import { canRevertPointer, parseOverrides, shouldRefresh } from './prefab-overrides';
import type { UndoEntry } from './undo';

interface HierarchyPanelProps {
  readonly client: ProtocolClient;
  readonly mode: EditorMode;
  readonly selection: EntityId | null;
  readonly onSelect: (entity: EntityId | null) => void;
  /** Reports a successful reparent's undo group (applied order) so `App` records it as one step. */
  readonly onEditGroup: (entries: UndoEntry[]) => void;
  /** Reports the live entity count up to the status bar. */
  readonly onEntityCount: (count: number) => void;
  /** The scene root label (scene file basename), or null before a scene path is known. */
  readonly sceneName: string | null;
}

const POLL_INTERVAL_MS = 300;

/** The value the reparent `<select>` uses for the "none" option — never a valid `entityKey`. */
const NONE_VALUE = '';

/** The drop-target key of the scene-root row (un-parent); entity rows use their `entityKey`. */
const ROOT_KEY = 'root';

/** Pointer travel (px) before a press on a row becomes a drag rather than a click. */
const DRAG_THRESHOLD_PX = 4;

interface DragSession {
  readonly startX: number;
  readonly startY: number;
  active: boolean;
}

export function HierarchyPanel({ client, mode, selection, onSelect, onEditGroup, onEntityCount, sceneName }: HierarchyPanelProps) {
  const [rows, setRows] = useState<readonly HierarchyRow[]>([]);
  const [error, setError] = useState<DiagnosticView | null>(null);

  // Override markers are advisory: `prefab.overrides` runs a full collapse engine-side, so it is
  // throttled, never overlaps, and its failure only clears the markers (never the banner or the rows).
  const [overridesBody, setOverridesBody] = useState<ResponseBody | null>(null);
  const hasPrefabRef = useRef(false);
  const overridesAtRef = useRef<number | null>(null);
  const overridesInFlightRef = useRef(false);
  const overridesPendingRef = useRef(false);

  const refreshOverrides = useCallback(
    async (forced: boolean): Promise<void> => {
      if (overridesInFlightRef.current) {
        overridesPendingRef.current ||= forced;
        return;
      }
      if (!shouldRefresh(Date.now(), overridesAtRef.current, false, forced)) {
        return;
      }
      overridesInFlightRef.current = true;
      try {
        do {
          overridesPendingRef.current = false;
          overridesAtRef.current = Date.now();
          if (!hasPrefabRef.current) {
            setOverridesBody(null);
            continue;
          }
          try {
            setOverridesBody(await client.call({ method: 'prefab.overrides', params: {} }));
          } catch {
            setOverridesBody(null);
          }
        } while (overridesPendingRef.current);
      } finally {
        overridesInFlightRef.current = false;
      }
    },
    [client],
  );

  const poll = useCallback(async (): Promise<void> => {
    try {
      const body = await client.call({ method: 'query.entities', params: { filter: [], include: ['Name', 'Parent', 'PrefabInstance'] } });
      const read = parseHierarchy(body);
      hasPrefabRef.current = read.some((r) => r.prefab !== null);
      setRows(read);
      setError(null);
      void refreshOverrides(false);
    } catch (err) {
      setError(toDiagnostic(err));
    }
  }, [client, refreshOverrides]);

  usePolling(true, POLL_INTERVAL_MS, poll);

  useEffect(() => {
    onEntityCount(rows.length);
  }, [rows.length, onEntityCount]);

  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const toggleCollapsed = useCallback((key: string): void => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (!next.delete(key)) {
        next.add(key);
      }
      return next;
    });
  }, []);

  const overrides = useMemo(
    () => (overridesBody === null ? EMPTY_OVERRIDES : parseOverrides(overridesBody, rows)),
    [rows, overridesBody],
  );
  const tree = useMemo(() => buildTree(rows, overrides, collapsed), [rows, overrides, collapsed]);
  const selectedKey = selection === null ? null : entityKey(selection);
  const selectedRow = useMemo(() => rows.find((r) => entityKey(r.entity) === selectedKey) ?? null, [rows, selectedKey]);
  const options = useMemo(
    () => (selectedRow === null ? [] : parentOptions(rows, selectedRow.entity)),
    [rows, selectedRow],
  );

  // One reparent path for the `<select>` and the drop: send `entity.reparent`, read back the local
  // `Transform` the engine recomputed (a Transform-less entity has none), and record one undo group.
  const commitReparent = useCallback(
    async (command: Command): Promise<void> => {
      try {
        const body = await client.call(command);
        const previous = (body as { previous_transform?: unknown }).previous_transform;
        let after: unknown | null = null;
        if (previous !== undefined && previous !== null) {
          const entity = (command as { params: { entity: EntityId } }).params.entity;
          const read = await client.call({ method: 'component.get', params: { entity: entityRef(entity), component: 'Transform' } });
          after = (read as { value?: unknown }).value ?? null;
        }
        onEditGroup(reparentEntries({ command, body }, after));
        setError(null);
        await poll();
        await refreshOverrides(true);
      } catch (err) {
        setError(toDiagnostic(err));
      }
    },
    [client, onEditGroup, poll, refreshOverrides],
  );

  const handleReparent = useCallback(
    (value: string): void => {
      if (selectedRow === null) {
        return;
      }
      const chosen = value === NONE_VALUE ? null : (options.find((o) => o.entity !== null && entityKey(o.entity) === value)?.entity ?? null);
      const command = reparentCommand(selectedRow.entity, selectedRow.parent, chosen);
      if (command !== null) {
        void commitReparent(command);
      }
    },
    [selectedRow, options, commitReparent],
  );

  // --- prefab instance actions --------------------------------------------------------------------
  const [busy, setBusy] = useState(false);
  const selectedLink = selectedRow?.prefab ?? null;
  const selectedPointers = (selectedKey === null ? undefined : overrides.parts.get(selectedKey)) ?? [];
  const instanceName = useMemo(() => {
    if (selectedLink === null) {
      return '';
    }
    const header = tree.find((r) => r.kind === 'instance' && entityKey(r.root) === entityKey(selectedLink.root));
    return header?.kind === 'instance' ? header.name : (selectedLink.source.split('/').pop() ?? selectedLink.source);
  }, [tree, selectedLink]);

  const runAction = useCallback(
    async (op: InstanceOp, pointer?: string): Promise<void> => {
      if (selectedRow === null) {
        return;
      }
      if (op === 'apply') {
        const proceed = await ask(`Write ${instanceName}'s changes into ${selectedLink?.source ?? 'the prefab file'}? This cannot be undone.`, {
          title: 'Apply to prefab',
          kind: 'warning',
        });
        if (!proceed) {
          return;
        }
      }
      setBusy(true);
      try {
        const { entries } = await runInstanceAction(op, selectedRow.entity, (cmd) => client.call(cmd), pointer);
        if (entries.length > 0) {
          onEditGroup(entries);
        }
        setError(null);
        await poll();
        await refreshOverrides(true);
      } catch (err) {
        setError(toDiagnostic(err));
      } finally {
        setBusy(false);
      }
    },
    [client, selectedRow, selectedLink, instanceName, onEditGroup, poll, refreshOverrides],
  );

  // --- drag-to-reparent ---------------------------------------------------------------------------
  const hoverRef = useRef<string | null>(null);
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const [drag, setDrag] = useState<{ subject: EntityId; key: string | null } | null>(null);

  const setHover = useCallback((key: string | null): void => {
    hoverRef.current = key;
    setDrag((d) => (d === null ? d : { ...d, key }));
  }, []);

  // The drop target for a hover key: `null` = the scene root, `undefined` = nothing hovered.
  const targetOf = (key: string | null): EntityId | null | undefined => {
    if (key === null) {
      return undefined;
    }
    if (key === ROOT_KEY) {
      return null;
    }
    return rowsRef.current.find((r) => entityKey(r.entity) === key)?.entity;
  };

  const startPress = (e: React.PointerEvent, subject: EntityId): void => {
    if (mode !== 'edit' || e.button !== 0) {
      return;
    }
    const s: DragSession = { startX: e.clientX, startY: e.clientY, active: false };
    const onMove = (ev: PointerEvent): void => {
      if (!s.active && Math.hypot(ev.clientX - s.startX, ev.clientY - s.startY) >= DRAG_THRESHOLD_PX) {
        s.active = true;
        setDrag({ subject, key: hoverRef.current });
      }
    };
    const cleanup = (): void => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('keydown', onKey);
      hoverRef.current = null;
      setDrag(null);
    };
    const onUp = (): void => {
      const target = targetOf(hoverRef.current);
      cleanup();
      if (!s.active || target === undefined) {
        return;
      }
      const command = dropCommand(rowsRef.current, subject, target);
      if (command !== null) {
        void commitReparent(command);
      }
    };
    const onKey = (ev: KeyboardEvent): void => {
      if (ev.key === 'Escape') {
        cleanup();
      }
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('keydown', onKey);
  };

  const dropClass = (key: string, target: EntityId | null): string => {
    if (drag === null || drag.key !== key) {
      return '';
    }
    return dropCommand(rows, drag.subject, target) === null ? ' drop-refused' : ' drop-target';
  };

  return (
    <section className={drag === null ? 'hierarchy' : 'hierarchy dragging'}>
      <div className="panel-head">
        Hierarchy
        <span className="count">
          <span className="num">{rows.length}</span> entities
        </span>
      </div>

      <DiagnosticBanner diagnostic={error} onDismiss={() => setError(null)} />

      <div className="tree-scroll">
        <div
          className={'tree-root' + dropClass(ROOT_KEY, null)}
          onPointerEnter={() => setHover(ROOT_KEY)}
          onPointerLeave={() => setHover(null)}
        >
          <span className="caret">▾</span>
          {sceneName ?? 'scene'}
        </div>
        <ul className="tree">
          {tree.map((outline) => {
            if (outline.kind === 'instance') {
              const isCollapsed = collapsed.has(outline.key);
              return (
                <li key={outline.key}>
                  <button
                    type="button"
                    className={'tree-row instance-row' + (selectedKey === entityKey(outline.root) ? ' selected' : '')}
                    style={{ paddingLeft: 8 + outline.depth * 14 }}
                    title={outline.source}
                    onClick={() => onSelect(outline.root)}
                    onPointerEnter={() => setHover(null)}
                  >
                    <span
                      className="caret"
                      role="button"
                      aria-label={isCollapsed ? 'expand instance' : 'collapse instance'}
                      onClick={(e) => {
                        e.stopPropagation();
                        toggleCollapsed(outline.key);
                      }}
                    >
                      {isCollapsed ? '▸' : '▾'}
                    </span>
                    <span className="glyph">◆</span>
                    <span className="name">{outline.name}</span>
                    <span className="source">{outline.source}</span>
                    <span className="parts">{outline.parts}</span>
                    {outline.status?.placementDiffers === true && (
                      <span className="flag placement" title="placement differs from the prefab's">
                        ◆
                      </span>
                    )}
                    {outline.status !== undefined && outline.status.removed.length > 0 && (
                      <span className="flag removed" title={`removed parts: ${outline.status.removed.join(', ')}`}>
                        −{outline.status.removed.length}
                      </span>
                    )}
                    {outline.status?.diagnostic != null && (
                      <span className="flag instance-error" title={`${outline.status.diagnostic.code}: ${outline.status.diagnostic.message}`}>
                        !
                      </span>
                    )}
                  </button>
                </li>
              );
            }
            const { row, depth, dangling, cyclic, overridden } = outline;
            const key = entityKey(row.entity);
            const title = dangling
              ? 'parent is missing -- scene.save will refuse this dangling reference'
              : cyclic
                ? 'part of a parent cycle'
                : undefined;
            return (
              <li key={key}>
                <button
                  type="button"
                  className={(key === selectedKey ? 'tree-row selected' : 'tree-row') + dropClass(key, row.entity)}
                  style={{ paddingLeft: 8 + depth * 14 }}
                  title={title}
                  onClick={() => onSelect(row.entity)}
                  onPointerDown={(e) => startPress(e, row.entity)}
                  onPointerEnter={() => setHover(key)}
                  onPointerLeave={() => setHover(null)}
                >
                  <span className="node-dot" />
                  <span className="name">{displayName(row)}</span>
                  {dangling === true && <span className="flag dangling">!</span>}
                  {cyclic === true && <span className="flag cyclic">↻</span>}
                  {overridden === true && (
                    <span className="flag override" title={(overrides.parts.get(key) ?? []).join('\n')}>
                      ●
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
        {tree.length === 0 && <div className="tree-empty">no entities</div>}
      </div>

      {mode === 'edit' && selectedLink !== null && (
        <div className="instance-actions">
          <span className="instance-name">{instanceName}</span>
          <button type="button" disabled={busy} title="writes the prefab file; not undoable" onClick={() => void runAction('apply')}>
            Apply
          </button>
          <button type="button" disabled={busy} title="restore every part to the prefab" onClick={() => void runAction('revert')}>
            Revert
          </button>
          <button
            type="button"
            disabled={busy}
            title="restore the selected part to the prefab"
            onClick={() => void runAction('revert', revertPartPointer(selectedLink))}
          >
            Revert part
          </button>
          <button type="button" disabled={busy} title="detach the parts from the prefab" onClick={() => void runAction('unpack')}>
            Unpack
          </button>
          {selectedPointers.length > 0 && (
            <ul className="override-list">
              {selectedPointers.map((p) => (
                <li key={p}>
                  <code>{p}</code>
                  {selectedRow !== null && !canRevertPointer(p, selectedRow, overrides) ? (
                    <span title="a root's placement is not reverted — edit it or apply">—</span>
                  ) : (
                    <button
                      type="button"
                      disabled={busy}
                      title="revert this field to the prefab"
                      onClick={() => void runAction('revert', revertPartPointer(selectedLink) + p)}
                    >
                      ↺
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      {mode === 'edit' && selectedRow !== null && (
        <div className="reparent">
          <span>parent</span>
          <select
            value={selectedRow.parent === null ? NONE_VALUE : entityKey(selectedRow.parent)}
            onChange={(e) => handleReparent(e.target.value)}
          >
            {options.map((o) => (
              <option key={o.entity === null ? NONE_VALUE : entityKey(o.entity)} value={o.entity === null ? NONE_VALUE : entityKey(o.entity)}>
                {o.label}
              </option>
            ))}
          </select>
        </div>
      )}
    </section>
  );
}
