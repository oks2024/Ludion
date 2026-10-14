// Pure helpers shared by commit.mjs and mirror-gap.mjs: the privacy boundary and the parsers for
// `lore status` / `lore repository dump` output. No I/O, so scripts/mirror-lib.test.mjs drives them
// with fixture strings.

const rules = (text) =>
  text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith('#'));

// The ignore delta IS the privacy boundary: anchored rules (`/path` or `/dir/`) in .gitignore that
// .loreignore does not carry (Lore excludes `.lore/` itself, so that rule is not a private path). Returned repo-relative, directories without the trailing slash.
export function privatePaths(gitignore, loreignore) {
  const bare = (r) => r.replace(/^\/+|\/+$/g, '');
  const lore = new Set(rules(loreignore ?? '').map(bare));
  return rules(gitignore)
    .filter((r) => r.startsWith('/') && !r.startsWith('!') && !/[*?[]/.test(r) && !lore.has(bare(r)) && r !== '/.lore/')
    .map((r) => r.slice(1).replace(/\/+$/, ''));
}

// `.`, `./` and the empty string all name the whole tree: `''`.
const norm = (p) => p.replaceAll('\\', '/').replace(/^\.(?:\/|$)/, '').replace(/\/+$/, '');

// True when `p` is one of the private paths or lies under one.
export function isPrivate(p, priv) {
  const n = norm(p);
  return priv.some((q) => n === q || n.startsWith(`${q}/`));
}

// `lore status` change lines: `M path`, `A path`, `D path`; directories end in `/`.
export function parseLoreStatus(text) {
  const changes = [];
  for (const line of text.split(/\r?\n/)) {
    const m = line.match(/^([MADRCTLOU]) (.+)$/);
    if (m) changes.push({ kind: m[1], path: m[2] });
  }
  return changes;
}

// `lore repository dump` lines: `<path> id N parent N ... [child N | addr ...]`. Files carry `addr`;
// directories end in `/` and carry `child`. Returns the file paths.
export function parseLoreDumpFiles(text) {
  const files = [];
  for (const line of text.split(/\r?\n/)) {
    const m = line.match(/^(.+?) id \d+ parent \d+ /);
    if (m && !m[1].endsWith('/') && / addr /.test(line)) files.push(m[1]);
  }
  return files;
}

// Does `p` (a file or a directory spec, as `lore status` prints them) overlap any named path?
export function overlaps(p, named) {
  const a = norm(p);
  return named.some((q) => {
    const b = norm(q);
    return a === '' || b === '' || a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`);
  });
}
