// The Game Designer view's main row (settled design). A navigator over
// the engine's schedule: select an entity → see the systems that drive it → read their Rust. Layout:
// LEFT column = hierarchy over the Systems|Data tabs; CENTER = a vertical split, viewport over a
// read-only code panel (~45%, a drag handle, a maximize toggle that collapses the viewport to a slim
// strip); RIGHT = the terminal drawer column, unchanged. No kit shelf in this view. The code panel is
// a CodeMirror editor wired to the `ludion dev` loop (save → rebuild, inline diagnostics, file watch).

import { useCallback, useEffect, useState } from 'react';
import type { ContextCamera } from '../terminal/context';
import type { ProtocolClient } from '../protocol/client';
import type { EditorMode, EntityId } from '../App';
import { HierarchyPanel } from '../editor/HierarchyPanel';
import { Viewport } from '../viewer/Viewport';
import type { FrameStatus } from '../viewer/useFrameStream';
import type { MutationReport } from '../viewer/useComponentCommit';
import type { UndoEntry } from '../editor/undo';
import { TerminalDrawer } from '../terminal/TerminalDrawer';
import { readEditorFile } from '../terminal/pty';
import { isTauri } from '../shell/tauri';
import { SystemsDataTabs } from '../editor/systems/SystemsDataTabs';
import { CodePanel, type CodePanelStatus } from '../editor/systems/CodePanel';
import { resolveSystemSource, type ResolvedSource } from '../editor/systems/locate';
import type { SystemDesc } from '../editor/systems/match';
import { VerticalSplit } from './VerticalSplit';

interface GameDesignerViewProps {
  readonly client: ProtocolClient;
  readonly mode: EditorMode;
  readonly selection: EntityId | null;
  readonly onSelect: (entity: EntityId | null) => void;
  readonly onMutation: (report: MutationReport) => void;
  readonly onEntityCount: (count: number) => void;
  readonly sceneName: string | null;
  /** The selected entity's component names (App-owned snapshot), for the systems match rule. */
  readonly selectionComponents: readonly string[] | null;
  // --- viewport plumbing (shared with the Level Designer view) ---
  readonly onEditEntry: (entry: UndoEntry) => void;
  readonly onEditGroup: (entries: UndoEntry[]) => void;
  readonly onDirty: () => void;
  readonly connection: 'live' | 'reconnecting' | 'failed';
  readonly onFrameStatus: (status: FrameStatus) => void;
  /** The view camera, for `.ludion/editor-context.json` so a persona knows there is a "here". */
  readonly onCamera: (camera: ContextCamera | null) => void;
  readonly pointer: readonly [number, number, number] | null;
  readonly onPointerChange: (position: readonly [number, number, number] | null) => void;
  // --- terminal drawer ---
  readonly url: string;
  readonly viewId: string;
  readonly projectDir: string;
  /** The current project's name, validated through `project_info`; `null` before it resolves. */
  readonly projectName: string | null;
  readonly onOpenProjectSwitcher: () => void;
}

export function GameDesignerView(props: GameDesignerViewProps) {
  const { client, mode, selection, onSelect, onMutation, onEntityCount, sceneName, selectionComponents } = props;

  const [openSystem, setOpenSystem] = useState<SystemDesc | null>(null);
  const [source, setSource] = useState<ResolvedSource | null>(null);
  const [codeStatus, setCodeStatus] = useState<CodePanelStatus>('idle');
  const [codeError, setCodeError] = useState<string | null>(null);
  // An out-of-band file open (a diagnostics-list click for a file that isn't the selected system's).
  // Takes precedence over the system-resolved source until the next system is opened.
  const [override, setOverride] = useState<ResolvedSource | null>(null);
  const [maximized, setMaximized] = useState(false);

  // Resolve a newly-opened system's source (module-path candidates → file, then the target fn line).
  // Tauri-only (the host serves the file read); a missing project dir or a failed resolution surfaces
  // as the code panel's error state rather than a silent blank.
  useEffect(() => {
    if (openSystem === null) {
      setSource(null);
      setCodeStatus('idle');
      setCodeError(null);
      return;
    }
    if (!isTauri()) {
      setSource(null);
      setCodeStatus('error');
      setCodeError('Source browsing needs the desktop editor (the host serves the file read).');
      return;
    }
    if (props.projectDir === '') {
      setSource(null);
      setCodeStatus('error');
      setCodeError('Set a project directory (in the terminal drawer) to resolve system source.');
      return;
    }
    let cancelled = false;
    setCodeStatus('loading');
    setCodeError(null);
    void (async () => {
      try {
        const resolved = await resolveSystemSource({
          fnPath: openSystem.fnPath,
          registeredAtFile: openSystem.registeredAt?.file ?? null,
          projectDir: props.projectDir,
          read: readEditorFile,
        });
        if (cancelled) {
          return;
        }
        if (resolved === null) {
          setSource(null);
          setCodeStatus('error');
          setCodeError(`Could not locate source for ${openSystem.fnPath}.`);
        } else {
          setSource(resolved);
          setCodeStatus('ready');
        }
      } catch (e) {
        if (!cancelled) {
          setSource(null);
          setCodeStatus('error');
          setCodeError(String(e));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [openSystem, props.projectDir]);

  // Opening a system clears any diagnostics-driven override so its source wins again.
  const handleOpenSystem = useCallback((system: SystemDesc) => {
    setOverride(null);
    setOpenSystem(system);
  }, []);

  // Open an arbitrary file at a line (a diagnostics row for another file). Tauri-only; a failed read
  // surfaces in the code panel's error state via the override path.
  const handleOpenPath = useCallback(
    async (filePath: string, line: number) => {
      if (!isTauri()) {
        return;
      }
      try {
        const text = await readEditorFile(filePath);
        setOverride({ path: filePath, text, line });
      } catch {
        setOverride(null);
      }
    },
    [],
  );

  const effectiveSource = override ?? source;
  const effectiveStatus: CodePanelStatus = override !== null ? 'ready' : codeStatus;
  const effectiveSystemName = override !== null ? null : openSystem?.name ?? null;

  return (
    <div className="main-row">
      {/* Left column: hierarchy over the Systems | Data tabs. */}
      <div className="col col-side">
        <HierarchyPanel
          client={client}
          mode={mode}
          selection={selection}
          onSelect={onSelect}
          onEditGroup={props.onEditGroup}
          onEntityCount={onEntityCount}
          sceneName={sceneName}
        />
        <SystemsDataTabs
          client={client}
          mode={mode}
          selection={selection}
          onMutation={onMutation}
          selectionComponents={selectionComponents}
          openSystemName={override !== null ? null : openSystem?.name ?? null}
          onOpenSystem={handleOpenSystem}
        />
      </div>

      {/* Center column: a vertical split, viewport over the code panel. */}
      <VerticalSplit
        className="col-center"
        maximized={maximized}
        onRestore={() => setMaximized(false)}
        hiddenLabel="Viewport hidden"
        restoreLabel="Restore viewport"
        top={
          <Viewport
            client={client}
            mode={mode}
            selection={selection}
            onSelect={onSelect}
            onEditEntry={props.onEditEntry}
            onDirty={props.onDirty}
            connection={props.connection}
            scenePath={sceneName}
            onFrameStatus={props.onFrameStatus}
            onCamera={props.onCamera}
            pointer={props.pointer}
            onPointerChange={props.onPointerChange}
          />
        }
        bottom={
          <CodePanel
            systemName={effectiveSystemName}
            source={effectiveSource}
            status={effectiveStatus}
            errorText={codeError}
            maximized={maximized}
            onToggleMaximize={() => setMaximized((m) => !m)}
            projectDir={props.projectDir}
            onOpenPath={handleOpenPath}
          />
        }
      />

      {/* Right column: the persona terminal drawer (unchanged). */}
      <div className="col col-term">
        <TerminalDrawer
          url={props.url}
          viewId={props.viewId}
          scenePath={sceneName ?? ''}
          projectDir={props.projectDir}
          projectName={props.projectName}
          onOpenProjectSwitcher={props.onOpenProjectSwitcher}
        />
      </div>
    </div>
  );
}
