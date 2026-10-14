#!/usr/bin/env node
// Claude Code statusline relay for the editor's terminal drawer.
//
// Claude Code invokes a statusLine command on every assistant message plus a refresh interval,
// piping a JSON payload to stdin (fields include `context_window`: total_input_tokens,
// context_window_size, used_percentage, current_usage.{cache_creation,cache_read}_input_tokens).
// This relay is wired per-instance at spawn via `--settings` (see editor/app/src/terminal/
// spawnConfig.ts); it writes the payload verbatim to the sink file named by $LUDION_STATUS_SINK
// (which the drawer tails for the per-tab context bar + cache countdown) and echoes a one-line
// statusline for the terminal itself.
//
// Plain Node, no dependencies. If the payload shape shifts, this is the single point of repair.

import { readFileSync, writeFileSync, renameSync } from 'node:fs';

function readStdin() {
  try {
    return readFileSync(0, 'utf8');
  } catch {
    return '';
  }
}

const raw = readStdin();

// Persist the raw payload atomically (tmp + rename) so a tailing reader never sees a half-write.
const sink = process.env.LUDION_STATUS_SINK;
if (sink && raw.trim() !== '') {
  try {
    const tmp = `${sink}.tmp`;
    writeFileSync(tmp, raw);
    renameSync(tmp, sink);
  } catch {
    // Sink unwritable (dir gone, permissions): the statusline still renders; drop silently.
  }
}

// Echo a compact one-line status. Best-effort parse — any shape change degrades to a bare line.
function line() {
  let payload;
  try {
    payload = JSON.parse(raw);
  } catch {
    return 'ludion · level-designer';
  }
  const cw = payload && typeof payload.context_window === 'object' ? payload.context_window : {};
  const used = typeof cw.total_input_tokens === 'number' ? cw.total_input_tokens : null;
  const size = typeof cw.context_window_size === 'number' ? cw.context_window_size : null;
  const pct = typeof cw.used_percentage === 'number' ? Math.round(cw.used_percentage) : null;
  const k = (n) => (n >= 1000 ? `${Math.round(n / 1000)}k` : String(Math.round(n)));
  const ctx = used === null ? '' : ` · ${k(used)}${size === null ? '' : `/${k(size)}`}${pct === null ? '' : ` (${pct}%)`}`;
  return `ludion · level-designer${ctx}`;
}

process.stdout.write(line());
