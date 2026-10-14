// Derives the titlebar/viewport document label from the served scene path (the `scene.save` path,
// prefilled by the `scene.changed` event). Pure so the titlebar and the viewport overlay format the
// same handle identically. The path is the engine's on-disk path (either separator); the label is
// "<project> — <scene>", where the project is the segment after a `games/` root when present, else the
// scene file's parent directory.

export interface DocParts {
  readonly project: string | null;
  readonly scene: string | null;
}

export function docParts(path: string): DocParts {
  const segments = path.split(/[/\\]+/).filter((s) => s !== '');
  if (segments.length === 0) {
    return { project: null, scene: null };
  }
  const scene = segments[segments.length - 1];
  const gamesAt = segments.indexOf('games');
  let project: string | null = null;
  if (gamesAt >= 0 && gamesAt + 1 < segments.length - 1) {
    project = segments[gamesAt + 1];
  } else if (segments.length >= 2) {
    project = segments[segments.length - 2];
  }
  return { project, scene };
}
