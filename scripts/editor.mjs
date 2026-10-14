#!/usr/bin/env node
// scripts/editor.mjs — launch the Ludion editor, optionally pointed at a project directory.
//
// Usage: node scripts/editor.mjs [<dir>] [--dry-run]
//   <dir>       a Ludion project directory (holds `ludion.toml`); `.` names the current directory.
//               Omitted, the editor reopens whichever project it last had open (localStorage).
//   --dry-run   print the resolved Tauri CLI argv as JSON instead of spawning the editor; this is
//               what makes the wrapper agent-verifiable without a GUI.
//   --help, -h  print this text.
//
// `npm ci` in `editor/app` is required once (the Tauri CLI is a dev dependency there, not
// installed globally).
//
// The Tauri CLI's JS entry is spawned directly with node, no shell:
//   `tauri dev -- -- --project <abs>`
// Two `--` are needed: the first ends `tauri dev`'s own options, and args after the second go to
// the application instead of `tauri dev` itself (`Plan/editor-open-project.md`). Spawning node
// directly, with no shell, keeps a path containing a space as one argument.

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const EDITOR_APP_DIR = path.join(REPO_ROOT, 'editor', 'app');
const TAURI_JS = path.join(EDITOR_APP_DIR, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');

function printHelp() {
  const help = [];
  for (const line of fs.readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n')) {
    if (line.startsWith('#!')) continue;
    if (line.startsWith('//')) help.push(line.slice(3));
    else if (help.length) break;
  }
  console.log(help.join('\n'));
}

function usage(message) {
  console.error(`editor.mjs: ${message}`);
  console.error('usage: node scripts/editor.mjs [<dir>] [--dry-run]');
  process.exit(2);
}

let dryRun = false;
let dir;
for (const arg of process.argv.slice(2)) {
  if (arg === '--help' || arg === '-h') {
    printHelp();
    process.exit(0);
  } else if (arg === '--dry-run') {
    dryRun = true;
  } else if (arg.startsWith('-')) {
    usage(`unknown flag \`${arg}\``);
  } else if (dir !== undefined) {
    usage(`too many arguments: \`${dir}\` and \`${arg}\``);
  } else {
    dir = arg;
  }
}

let abs;
if (dir !== undefined) {
  abs = path.resolve(process.cwd(), dir);
  const manifestStat = fs.statSync(path.join(abs, 'ludion.toml'), { throwIfNoEntry: false });
  if (!manifestStat?.isFile()) {
    console.error(
      `\`${abs}\` has no ludion.toml — a Ludion project is what \`ludion new\` creates; the engine ` +
        'checkout itself is not one.',
    );
    process.exit(1);
  }
}

if (!fs.existsSync(TAURI_JS)) {
  console.error('editor.mjs: editor/app/node_modules/@tauri-apps/cli/tauri.js is missing — run `npm ci` in editor/app');
  process.exit(1);
}

const tauriArgs = abs !== undefined ? ['dev', '--', '--', '--project', abs] : ['dev'];

if (dryRun) {
  console.log(JSON.stringify(tauriArgs));
  process.exit(0);
}

const child = spawn(process.execPath, [TAURI_JS, ...tauriArgs], { cwd: EDITOR_APP_DIR, stdio: 'inherit' });
child.on('exit', (code) => process.exit(code ?? 1));
