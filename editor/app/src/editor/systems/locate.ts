// fn_path → source file resolution, pure enough to unit-test (the probing is injected as a `read`
// callback). `engine.describe` gives a system's Rust module path (`crate::mod1::mod2::fn_name`) but no
// filesystem path — no on-disk layout leaks over the wire — so the editor reconstructs candidate
// files from Rust's module layout rules and probes them via the host's `read_editor_file`. Once a
// file loads, the target `fn` line is found textually for scroll-to-highlight. `registered_at.file`
// is the fallback when the fn body can't be located from the module path (registration is one lib.rs
// chain, so registered_at alone lands on the wrong file — hence it's last, not first).

import { fnNameOf } from './match';

/** A resolved source location: the file that was read, its full text, and the 1-based line of the
 *  target fn (or `0` when the fn couldn't be located textually but the file was still readable). */
export interface ResolvedSource {
  readonly path: string;
  readonly text: string;
  readonly line: number;
}

/** True for an absolute path — POSIX (`/…`) or Windows (`C:\…` / `C:/…` / UNC `\\…`). */
export function isAbsolutePath(path: string): boolean {
  return /^([/\\]|[A-Za-z]:[/\\]|\\\\)/.test(path);
}

/** Joins a directory and a relative path with a single `/`, trimming any trailing separator on the
 *  directory. The host's `read_editor_file` accepts forward slashes on every platform. */
export function joinPath(dir: string, rel: string): string {
  return `${dir.replace(/[/\\]+$/, '')}/${rel}`;
}

/** Candidate source files for a `crate::mod1::…::fn_name` path, in probe order, RELATIVE to the crate
 *  root (the project dir). Rust layout: a `mod1::mod2` module lives in `src/mod1/mod2.rs` or
 *  `src/mod1/mod2/mod.rs`; but the fn may also be in an *inline* submodule of a parent file, so the
 *  walk climbs the module chain (`src/mod1.rs`, `src/mod1/mod.rs`, …) before falling back to the crate
 *  root (`src/lib.rs`, `src/main.rs`). A single-segment `crate::fn_name` resolves to the crate root
 *  directly. Deduped, order-preserving. */
export function candidatePaths(fnPath: string): string[] {
  const base = fnPath.split('<')[0];
  const segments = base.split('::').filter((s) => s.length > 0 && !s.startsWith('{{'));
  const out: string[] = [];
  // segments = [crate, ...modules, fnName]; modules are everything between.
  const modules = segments.length >= 2 ? segments.slice(1, -1) : [];
  for (let depth = modules.length; depth >= 1; depth -= 1) {
    const sub = modules.slice(0, depth).join('/');
    out.push(`src/${sub}.rs`);
    out.push(`src/${sub}/mod.rs`);
  }
  out.push('src/lib.rs');
  out.push('src/main.rs');
  return [...new Set(out)];
}

/** The 1-based line of `fn <fnName>` in `text`, or `null` if not found. Tolerant of visibility
 *  (`pub`, `pub(crate)`), qualifiers (`async`/`const`/`unsafe`/`extern "C"`), and leading indentation
 *  or attributes on their own lines (attributes precede the `fn` line, so they don't interfere). Takes
 *  the first match. */
export function findFnLine(text: string, fnName: string): number | null {
  const pattern = new RegExp(
    `^\\s*(?:pub(?:\\([^)]*\\))?\\s+)?(?:(?:async|const|unsafe|extern(?:\\s+"[^"]*")?)\\s+)*fn\\s+${escapeRegExp(fnName)}\\b`,
  );
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    if (pattern.test(lines[i])) {
      return i + 1;
    }
  }
  return null;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Resolves a system's source. Probes the module-layout candidates (under `projectDir`) in order,
 *  reading each via `read`; the first that both loads AND contains the fn definition wins (scrolled to
 *  that line). If none contains the fn, falls back to `registeredAtFile` (tried as-is when absolute,
 *  else under `projectDir`), then to the first candidate that merely read. Returns `null` when nothing
 *  could be read at all. `read` rejects for a missing file — that's a normal "try the next candidate". */
export async function resolveSystemSource(input: {
  readonly fnPath: string;
  readonly registeredAtFile: string | null;
  readonly projectDir: string;
  readonly read: (path: string) => Promise<string>;
}): Promise<ResolvedSource | null> {
  const { fnPath, registeredAtFile, projectDir, read } = input;
  const fnName = fnNameOf(fnPath);
  let firstReadable: { path: string; text: string } | null = null;

  for (const rel of candidatePaths(fnPath)) {
    const path = joinPath(projectDir, rel);
    let text: string;
    try {
      text = await read(path);
    } catch {
      continue;
    }
    if (firstReadable === null) {
      firstReadable = { path, text };
    }
    const line = findFnLine(text, fnName);
    if (line !== null) {
      return { path, text, line };
    }
  }

  if (registeredAtFile !== null && registeredAtFile !== '') {
    const path = isAbsolutePath(registeredAtFile) ? registeredAtFile : joinPath(projectDir, registeredAtFile);
    try {
      const text = await read(path);
      return { path, text, line: findFnLine(text, fnName) ?? 0 };
    } catch {
      // fall through to the first readable candidate
    }
  }

  if (firstReadable !== null) {
    return { path: firstReadable.path, text: firstReadable.text, line: findFnLine(firstReadable.text, fnName) ?? 0 };
  }
  return null;
}
