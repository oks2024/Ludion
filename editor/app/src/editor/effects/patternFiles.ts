// Which files on disk are pattern documents, what key the engine knows them by, and what a brand
// new one contains.
//
// NOTHING PEEKS `kind:` FOR US. The host's `list_editor_dir(path, ext)` filters on a filename
// suffix only, and `.yaml` under the content root is every scene, atlas source, map source and data
// doc a project owns. So the picker lists `.yaml`, reads each candidate and keeps the ones whose
// text carries a top-level `kind: pattern` line — the same first-line-of-the-envelope test the
// engine's own content walk makes. `isPatternDoc` is deliberately textual rather than a YAML parse:
// it runs once per file in a directory listing, and a malformed document must read as "not a
// pattern" rather than throw.
//
// A DOC KEY IS CONTENT-ROOT-RELATIVE AND FORWARD-SLASHED (`Plan/projectile-patterns.md` Wave 1
// ruling 1): `patterns/boss.yaml`, never a name and never an absolute path. It is what
// `PatternPlayer.pattern`, a timeline's `play:` and `asset.reload { path }` all take.
//
// Pure: no React, no Tauri, no DOM.

/** Where a project's pattern documents live, relative to the content root. */
export const PATTERN_DIR = 'patterns';

/** The extension the picker lists. */
export const PATTERN_EXT = '.yaml';

/** Every `\` a `/`, collapsed repeats, no trailing separator — a leading one is kept, because a
 *  POSIX content root starts with it. */
function normalizeRoot(path: string): string {
  return path.replace(/\\/g, '/').replace(/\/{2,}/g, '/').replace(/\/+$/, '');
}

/** A relative path: {@link normalizeRoot} plus the leading separator dropped. */
function normalize(path: string): string {
  return normalizeRoot(path).replace(/^\/+/, '');
}

/**
 * Whether `text` is a `kind: pattern` document.
 *
 * A top-level (column-zero) `kind:` key naming `pattern`, quoted or bare, with a trailing comment
 * allowed. An indented `kind:` belongs to something else — a `Mesh`'s `shape`-sibling, an atlas
 * source's region — and never counts.
 */
export function isPatternDoc(text: string): boolean {
  for (const line of text.split('\n')) {
    if (/^kind:[ \t]*("pattern"|'pattern'|pattern)[ \t]*(#.*)?\r?$/.test(line)) {
      return true;
    }
  }
  return false;
}

/**
 * The `DocStore` key for a file under the content root: `relPath` made forward-slashed, with
 * `root`'s own prefix stripped when it carries one (the host lists relative paths, but an absolute
 * one reaching here must not become a key the engine cannot resolve).
 */
export function docKey(root: string, relPath: string): string {
  const normalizedRoot = normalizeRoot(root).replace(/^\/+/, '');
  const normalizedPath = normalize(relPath);
  if (normalizedRoot !== '' && normalizedPath.toLowerCase().startsWith(`${normalizedRoot.toLowerCase()}/`)) {
    return normalizedPath.slice(normalizedRoot.length + 1);
  }
  return normalizedPath;
}

/** The absolute path of a doc key under `root` — what `read_editor_file`/`write_editor_file` take. */
export function docPath(root: string, key: string): string {
  return `${normalizeRoot(root)}/${normalize(key)}`;
}

/** A doc key as a picker row reads: `patterns/boss.yaml` → `boss`. */
export function docLabel(key: string): string {
  const base = key.slice(key.lastIndexOf('/') + 1);
  return base.endsWith(PATTERN_EXT) ? base.slice(0, -PATTERN_EXT.length) : base;
}

/** A typed name as a filename stem: lowercased, every run of anything else a single `-`. */
export function slugify(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

/** The doc key a new pattern named `name` takes, or `null` when the name slugs to nothing. */
export function newPatternKey(name: string): string | null {
  const slug = slugify(name);
  return slug === '' ? null : `${PATTERN_DIR}/${slug}${PATTERN_EXT}`;
}

/**
 * The smallest valid pattern document: one bullet kind, one timeline entry (the first example of
 * `ludion docs patterns`, trimmed to what a designer wants to see before their first edit).
 *
 * Block style throughout — the content walk reports a flow-style data document as `L1005`.
 */
export function newPatternText(name: string): string {
  return `format: 1
kind: pattern
name: ${slugify(name)}
bullets:
  needle:
    size:
      - 0.3
      - 0.3
    color:
      - 1.0
      - 0.4
      - 0.8
      - 1.0
    blend: additive
    hit_radius: 0.12
    life: 240
timeline:
  - at: 0
    shape: ring
    count: 12
    bullet: needle
    speed: 4.0
loop: 120
`;
}

/** The listed key equal to `key` ignoring ASCII case (macOS and Windows fold case on disk), or
 *  `null` when none is. */
export function existingPatternKey(keys: readonly string[], key: string): string | null {
  const folded = key.toLowerCase();
  return keys.find((listed) => listed.toLowerCase() === folded) ?? null;
}
