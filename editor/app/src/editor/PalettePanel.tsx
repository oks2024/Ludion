// The kit shelf (Meridian): the archetype spawn buttons reframed as tiles in a horizontal strip under
// the viewport, plus the selection actions (duplicate / destroy) in the shelf header. Thin wiring only
// — the archetype defaults live in `palette.ts`, the spawn/duplicate/destroy flows in `entity-ops.ts`,
// and all state (selection, undo, error banner) is lifted to `App`. Everything is gated to edit mode,
// matching how the session strip disables undo/redo in play mode.

import type { ReactNode } from 'react';
import type { Command } from '@ludion/protocol';
import type { EditorMode, EntityId } from '../App';
import { ARCHETYPES, spawnCommand } from './palette';

interface PalettePanelProps {
  readonly mode: EditorMode;
  readonly selection: EntityId | null;
  /** Spawn a prebuilt archetype command (App runs it, records undo, selects the new entity). */
  readonly onSpawn: (command: Command) => void;
  /** Duplicate the current selection. */
  readonly onDuplicate: () => void;
  /** Destroy the current selection (and un-parent its children). */
  readonly onDestroy: () => void;
}

/** A generic mark per archetype id (the kit is hand-maintained, so a small icon map is fine). Missing
 *  ids fall back to a plain box — a new archetype still renders a tile without a code change here. */
const ICONS: Record<string, ReactNode> = {
  sprite: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <rect x="4" y="4" width="16" height="16" rx="1" />
      <path d="M4 15l4-4 4 3 3-3 5 5" />
    </svg>
  ),
  mesh: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z" />
      <path d="M12 3v18M4 7.5l8 4.5 8-4.5" />
    </svg>
  ),
  light: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <path d="M9 18h6M10 21h4M12 3a6 6 0 0 1 4 10.5V16H8v-2.5A6 6 0 0 1 12 3z" />
    </svg>
  ),
  sun: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M19 5l-2 2M7 17l-2 2" />
    </svg>
  ),
  camera: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <rect x="3" y="7" width="14" height="10" rx="1" />
      <path d="M17 10l4-2v8l-4-2z" />
    </svg>
  ),
  text: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <path d="M5 6h14M12 6v12M9 18h6" />
    </svg>
  ),
  'hud-panel': (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <rect x="3" y="4" width="18" height="16" rx="1" />
      <path d="M3 9h18M7 4v5" />
    </svg>
  ),
};

const FALLBACK_ICON: ReactNode = (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
    <rect x="5" y="5" width="14" height="14" rx="1" />
  </svg>
);

export function PalettePanel({ mode, selection, onSpawn, onDuplicate, onDestroy }: PalettePanelProps) {
  const editing = mode === 'edit';
  const hasSelection = selection !== null;
  return (
    <section className="kitshelf">
      <div className="kit-head">
        Kit library
        <span className="count">{ARCHETYPES.length} parts</span>
        <div className="head-actions">
          <button type="button" className="icon-btn" disabled={!editing || !hasSelection} onClick={onDuplicate}>
            Duplicate
          </button>
          <button type="button" className="icon-btn" disabled={!editing || !hasSelection} onClick={onDestroy}>
            Destroy
          </button>
        </div>
      </div>
      <div className="kit-strip">
        {ARCHETYPES.map((archetype) => (
          <button
            key={archetype.id}
            type="button"
            className="kit-tile"
            disabled={!editing}
            title={editing ? `Spawn ${archetype.label}` : 'switch to Edit mode to spawn'}
            onClick={() => onSpawn(spawnCommand(archetype))}
          >
            {ICONS[archetype.id] ?? FALLBACK_ICON}
            <span>{archetype.label}</span>
          </button>
        ))}
      </div>
    </section>
  );
}
