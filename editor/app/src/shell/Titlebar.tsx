// The custom titlebar (Tauri decorations are off): brand mark + doc path, and the window controls.
// The window-control cluster and the drag region only make sense inside Tauri, so both are hidden in
// plain-browser dev (`isTauri()`), where the OS still draws its own frame. Controls call the
// `@tauri-apps/api` window methods (permissions declared in src-tauri/capabilities/default.json).

import { getCurrentWindow } from '@tauri-apps/api/window';
import { docParts } from './docpath';
import { isTauri } from './tauri';
import { ProjectSwitcher } from './ProjectSwitcher';

interface TitlebarProps {
  /** The served scene's on-disk path (may be empty before the first `scene.changed`). */
  readonly scenePath: string;
  /** The current project, validated through `project_info`; `null` before the first answer. */
  readonly project: { name: string } | { error: string } | null;
  /** The current project's resolved absolute directory. */
  readonly projectDir: string;
  readonly repoRoot: string;
  readonly recents: readonly string[];
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onSwitch: (absDir: string) => Promise<void>;
}

export function Titlebar({ scenePath, project, projectDir, repoRoot, recents, open, onOpenChange, onSwitch }: TitlebarProps) {
  const tauri = isTauri();
  const { project: sceneProject, scene } = docParts(scenePath);

  return (
    <div className="titlebar">
      <div className="brand">
        <div className="brand-mark" />
        <span className="brand-name">LUDION</span>
      </div>
      {tauri ? (
        <ProjectSwitcher
          projectDir={projectDir}
          project={project}
          repoRoot={repoRoot}
          recents={recents}
          open={open}
          onOpenChange={onOpenChange}
          onSwitch={onSwitch}
        />
      ) : (
        project !== null &&
        ('error' in project ? (
          <span className="project-name project-name-error" title={project.error}>
            no project
          </span>
        ) : (
          <span className="project-name">{project.name}</span>
        ))
      )}
      {scene !== null && (
        <div className="doc-path">
          {sceneProject !== null && <>{sceneProject} — </>}
          <b>{scene}</b>
        </div>
      )}
      <div className="titlebar-spacer" />
      {tauri && (
        <div className="winctl">
          <button type="button" title="Minimize" onClick={() => void getCurrentWindow().minimize()}>
            <svg viewBox="0 0 10 10">
              <rect x="0" y="4.5" width="10" height="1" fill="currentColor" />
            </svg>
          </button>
          <button type="button" title="Maximize" onClick={() => void getCurrentWindow().toggleMaximize()}>
            <svg viewBox="0 0 10 10">
              <rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" />
            </svg>
          </button>
          <button type="button" className="close" title="Close" onClick={() => void getCurrentWindow().close()}>
            <svg viewBox="0 0 10 10">
              <path d="M0 0 L10 10 M10 0 L0 10" stroke="currentColor" strokeWidth="1" />
            </svg>
          </button>
        </div>
      )}
    </div>
  );
}
