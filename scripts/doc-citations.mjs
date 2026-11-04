#!/usr/bin/env node
// Topic citations resolve against the right root. `ludion docs <t>` serves `docs/<t>.md` and `ludion
// devdocs <t>` serves `dev-docs/<t>.md`; a citation naming a topic in the other root (or in none) sends
// the reader to an L0009. Scans `docs/*.md`, `dev-docs/*.md` and `crates/*/src/**/*.rs`, matching across
// a line wrap (a Rust `//!` or `///` continuation included). A `<topic>` placeholder does not match.
//
// Usage:  node scripts/doc-citations.mjs
// `scripts/lists-in-step.mjs` imports `checkDocCitations`, so `check.mjs` and CI run the same check.
// Zero npm dependencies — Node built-ins only.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const CITATION = /\bludion\s+(?:\/\/[!\/]\s*)?(docs|devdocs)\s*(?:\\\s*)?(?:\/\/[!\/]\s*)?\s([a-z][a-z0-9-]*)/g;
const ROOTS = { docs: 'docs', devdocs: 'dev-docs' };
const OTHER = { docs: 'devdocs', devdocs: 'docs' };

function* walk(dir, ext) {
  for (const n of readdirSync(dir)) {
    const p = path.join(dir, n);
    if (statSync(p).isDirectory()) yield* walk(p, ext);
    else if (p.endsWith(ext)) yield p;
  }
}

function sources(root) {
  const files = [];
  for (const d of ['docs', 'dev-docs']) {
    const dir = path.join(root, d);
    if (existsSync(dir)) files.push(...readdirSync(dir).filter((n) => n.endsWith('.md')).map((n) => path.join(dir, n)));
  }
  const crates = path.join(root, 'crates');
  if (existsSync(crates)) {
    for (const c of readdirSync(crates)) {
      const src = path.join(crates, c, 'src');
      if (existsSync(src)) files.push(...walk(src, '.rs'));
    }
  }
  return files;
}

export function checkDocCitations(root) {
  const errors = [];
  let count = 0;
  const has = (verb, topic) => existsSync(path.join(root, ROOTS[verb], `${topic}.md`));
  for (const file of sources(root)) {
    const text = readFileSync(file, 'utf8').replace(/\r\n/g, '\n');
    for (const m of text.matchAll(CITATION)) {
      count += 1;
      const [, verb, topic] = m;
      if (has(verb, topic)) continue;
      const line = text.slice(0, m.index).split('\n').length;
      const where = `${path.relative(root, file).split(path.sep).join('/')}:${line}`;
      const hint = has(OTHER[verb], topic) ? ` — use \`ludion ${OTHER[verb]} ${topic}\`` : '';
      errors.push(`doc-citations: ${where} cites \`ludion ${verb} ${topic}\`, which is no topic in ${ROOTS[verb]}/${hint}`);
    }
  }
  return { errors, count };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const { errors, count } = checkDocCitations(root);
  if (errors.length) {
    for (const e of errors) console.error(`  - ${e}`);
    process.exit(1);
  }
  console.log(`doc-citations: OK (${count} citations)`);
}
