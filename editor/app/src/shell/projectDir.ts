// The game-project directory the editor works in — the spawned personas' cwd, the `.ludion/`
// context file's home, the Voxel Artist stage's `<project>/scenes` — and how that choice survives a
// reload AND a checkout move.
//
// WHY RELATIVE. The host resolves the engine checkout's root at build time (`pty::repo_root`), so a
// project inside the checkout — e.g. `games/breakout` — has one name that is stable
// across machines: its path relative to that root. The first shape persisted was the absolute path,
// and it broke the moment the checkout changed drives: every view kept asking for the old machine's
// path, and the model stage refused to start on a scene that did not exist there. Persisting the
// checkout-relative form whenever the project lies inside the checkout, and resolving it against
// the host's own root at boot, makes the setting a property of the repo instead of the machine.
//
// A project OUTSIDE the checkout (a `ludion new` project) has no such anchor and stays absolute.
//
// Pure and storage-free: the stored string and the repo root arrive as arguments, so the policy is
// testable without a browser, a host or a `localStorage`.

/** localStorage key holding the project directory in its persisted form ({@link toStoredProjectDir}). */
export const PROJECT_DIR_KEY = 'ludion.editor.projectDir';

/** `/`-separated, backslashes folded, no trailing separator, no leading `./`. `''` stays `''`. A
 *  bare root (`/`, `C:/`) keeps its one separator. */
export function normalizePath(path: string): string {
  let out = path.replace(/\\/g, '/');
  while (out.startsWith('./')) {
    out = out.slice(2);
  }
  if (out === '.') {
    return '';
  }
  const trimmed = out.replace(/\/+$/, '');
  if (trimmed === '' || /^[A-Za-z]:$/.test(trimmed)) {
    return out === trimmed ? trimmed : `${trimmed}/`;
  }
  return trimmed;
}

/** A rooted path: `/…` (POSIX) or `X:/…` (a Windows drive), separators already folded or not. */
export function isAbsolutePath(path: string): boolean {
  return /^(?:[A-Za-z]:)?[\\/]/.test(path);
}

function isDrivePath(path: string): boolean {
  return /^[A-Za-z]:/.test(path);
}

/** The comparison key of a normalized path: Windows drive paths fold case (the file system does;
 *  `d:/Repos` and `D:/Repos` are one directory), POSIX paths compare exactly. */
function foldCase(path: string): string {
  return isDrivePath(path) ? path.toLowerCase() : path;
}

/**
 * The form to persist for `dir`: checkout-relative when it lies inside `repoRoot` (`games/breakout`),
 * otherwise the normalized path as given — an outside project stays absolute, a relative input stays
 * relative. `repoRoot` may be `''` (no host yet), in which case nothing can be made relative.
 */
export function toStoredProjectDir(dir: string, repoRoot: string): string {
  const path = normalizePath(dir);
  const root = normalizePath(repoRoot);
  if (path === '' || root === '' || !isAbsolutePath(path) || !isAbsolutePath(root)) {
    return path;
  }
  if (foldCase(path) === foldCase(root)) {
    return '';
  }
  const rootPrefix = root.endsWith('/') ? root : `${root}/`;
  return foldCase(path).startsWith(foldCase(rootPrefix)) ? path.slice(rootPrefix.length) : path;
}

/** Whether `a` and `b` name the same project directory: normalized, and compared via {@link
 *  foldCase} — a drive-letter path folds case, a POSIX path does not. Callers deduping stored or
 *  recent project paths must use this rather than a plain string comparison. */
export function sameProjectPath(a: string, b: string): boolean {
  return foldCase(normalizePath(a)) === foldCase(normalizePath(b));
}

/**
 * The absolute directory a stored value names against this host's checkout root, or `''` when
 * nothing is stored — or when a relative value cannot be resolved yet (`repoRoot` is `''`: the
 * browser dev page, or the host has not answered). An absolute value resolves with no root at all.
 */
export function resolveProjectDir(stored: string | null, repoRoot: string): string {
  const path = normalizePath(stored ?? '');
  if (path === '') {
    return '';
  }
  if (isAbsolutePath(path)) {
    return path;
  }
  const root = normalizePath(repoRoot);
  if (root === '') {
    return '';
  }
  return root.endsWith('/') ? `${root}${path}` : `${root}/${path}`;
}

/**
 * The stored form to use for this session: a `--project` launch argument (non-empty) wins over
 * whatever is stored, persisted the same way any project directory is (`toStoredProjectDir`);
 * otherwise the stored value, re-derived through `toStoredProjectDir` too (an absolute path stored
 * from before the editor knew to keep it relative still collapses onto the checkout; a relative
 * value passes through unchanged); otherwise `''` — no project: the editor opens on the project
 * picker. The launch argument winning every boot is what makes "launch persists" true — the caller
 * writes the returned value back to storage.
 */
export function pickProjectDir(stored: string | null, launch: string | null, repoRoot: string): string {
  if (launch !== null && launch !== '') {
    return toStoredProjectDir(launch, repoRoot);
  }
  const normalizedStored = normalizePath(stored ?? '');
  return normalizedStored !== '' ? toStoredProjectDir(normalizedStored, repoRoot) : '';
}
