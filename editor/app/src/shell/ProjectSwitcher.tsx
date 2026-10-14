// The titlebar's project switcher (`Plan/editor-open-project.md` decision 3): the button naming
// the current project opens a popover offering up to 8 recents, a native folder picker, and a
// pasted path — every choice is validated through `project_info` before it is accepted, with the
// refusal shown inline (the popover stays open) rather than silently discarded.

import { useEffect, useRef, useState } from 'react';
import { open as openFolderDialog } from '@tauri-apps/plugin-dialog';
import { normalizePath, resolveProjectDir, sameProjectPath } from './projectDir';
import { projectInfo } from '../terminal/pty';

export interface ProjectSwitcherProps {
  /** The current project's resolved absolute directory. */
  readonly projectDir: string;
  /** The current project, validated through `project_info`; `null` before the first answer. */
  readonly project: { name: string } | { error: string } | null;
  readonly repoRoot: string;
  /** Up to 8 project directories, most-recent first, in their persisted (stored) form. */
  readonly recents: readonly string[];
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  /** Runs the switch (stops old-project stage sessions, commits the new one). Resolves once done. */
  readonly onSwitch: (absDir: string) => Promise<void>;
}

/** A recent's resolved name, once its own `project_info` has answered. */
type RecentInfo = { readonly name: string } | { readonly error: string };

export function ProjectSwitcher({ projectDir, project, repoRoot, recents, open, onOpenChange, onSwitch }: ProjectSwitcherProps) {
  const [recentInfo, setRecentInfo] = useState<Readonly<Record<string, RecentInfo>>>({});
  const [pathDraft, setPathDraft] = useState('');
  const [switching, setSwitching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);

  // Resolve every recent's name in parallel whenever the popover opens; a stale answer from a
  // since-closed popover is dropped.
  useEffect(() => {
    if (!open) {
      return;
    }
    setError(null);
    setPathDraft('');
    let cancelled = false;
    for (const entry of recents) {
      projectInfo(resolveProjectDir(entry, repoRoot))
        .then((info) => {
          if (!cancelled) {
            setRecentInfo((prev) => ({ ...prev, [entry]: { name: info.name } }));
          }
        })
        .catch((e: unknown) => {
          if (!cancelled) {
            setRecentInfo((prev) => ({ ...prev, [entry]: { error: String(e) } }));
          }
        });
    }
    return () => {
      cancelled = true;
    };
  }, [open, recents, repoRoot]);

  // Escape and an outside pointerdown both close the popover.
  useEffect(() => {
    if (!open) {
      return;
    }
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        onOpenChange(false);
      }
    };
    const onPointerDown = (e: PointerEvent): void => {
      if (rootRef.current !== null && !rootRef.current.contains(e.target as Node)) {
        onOpenChange(false);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('pointerdown', onPointerDown);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('pointerdown', onPointerDown);
    };
  }, [open, onOpenChange]);

  const choose = async (abs: string): Promise<void> => {
    setSwitching(true);
    setError(null);
    try {
      await projectInfo(abs);
      await onSwitch(abs);
      onOpenChange(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setSwitching(false);
    }
  };

  const openFolder = async (): Promise<void> => {
    let picked: string | null;
    try {
      picked = await openFolderDialog({ directory: true, multiple: false });
    } catch (e) {
      setError(String(e));
      return;
    }
    if (picked === null) {
      return; // cancelled
    }
    await choose(normalizePath(picked));
  };

  const submitPath = (): void => {
    const trimmed = pathDraft.trim();
    if (trimmed === '') {
      return;
    }
    void choose(resolveProjectDir(normalizePath(trimmed), repoRoot));
  };

  // `null` (nothing resolved yet — `projectDir` is '' or `project_info` is still in flight) reads the
  // same as an error for the label, but carries no error message of its own to show as a title.
  const errored = project !== null && 'error' in project;
  const label = project !== null && 'name' in project ? project.name : 'no project';
  const title = project !== null && 'error' in project ? project.error : project === null ? undefined : projectDir;

  return (
    <div className="project-switcher" ref={rootRef}>
      <button
        type="button"
        className={`project-name no-drag${errored ? ' project-name-error' : ''}`}
        title={title}
        onClick={() => onOpenChange(!open)}
      >
        {label}
      </button>
      {open && (
        <div className="project-switcher-popover no-drag">
          <div className="project-switcher-recents">
            {recents.length === 0 ? (
              <div className="project-switcher-empty">No recent projects</div>
            ) : (
              recents.map((entry) => {
                const abs = resolveProjectDir(entry, repoRoot);
                const info = recentInfo[entry];
                const failed = info !== undefined && 'error' in info;
                const current = sameProjectPath(abs, projectDir);
                return (
                  <button
                    key={entry}
                    type="button"
                    className={`project-switcher-recent${current ? ' current' : ''}${failed ? ' not-found' : ''}`}
                    disabled={failed || switching}
                    title={failed ? info.error : abs}
                    onClick={() => void choose(abs)}
                  >
                    <span className="psr-name">{failed ? 'not found' : (info?.name ?? entry)}</span>
                    <span className="psr-path">{abs}</span>
                  </button>
                );
              })
            )}
          </div>
          <div className="project-switcher-divider" />
          <button type="button" className="project-switcher-action" disabled={switching} onClick={() => void openFolder()}>
            Open folder…
          </button>
          <div className="project-switcher-path">
            <input
              type="text"
              className="project-switcher-input"
              placeholder="path to the game project"
              spellCheck={false}
              value={pathDraft}
              disabled={switching}
              onChange={(e) => setPathDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  submitPath();
                }
              }}
            />
            <button
              type="button"
              className="project-switcher-action"
              disabled={switching || pathDraft.trim() === ''}
              onClick={submitPath}
            >
              Open
            </button>
          </div>
          {error !== null && <div className="project-switcher-error">{error}</div>}
        </div>
      )}
    </div>
  );
}
