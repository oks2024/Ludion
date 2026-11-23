// Parity audit: the mechanical half of Phase 4's exit criterion 2 — "the inspector shows live
// component values using only public protocol commands (parity audit: zero endpoints exist that
// the CLI couldn't also call)" (implementation-guide.md, Phase 4 — Human Eyes) — extended in
// Phase 6 Wave 6 to the editing surface (`Plan/phase-6-editing.md`: "the parity audit still finds
// zero editor-private endpoints").
//
// The server side of this pillar is already structurally parity-safe: `dispatch.rs` is an
// exhaustive match over the one `Command` enum (CLAUDE.md's "the exhaustive match IS the parity
// gate: a command without a handler is a compile error") and there is no server-private route a
// connection can reach. What's left to check is the *viewer*: does it, in fact, only ever speak
// methods from that one public `Command` union, and does it only ever open the wire through the
// one client module that carries them? This test checks both, mechanically:
//
//   (a) extracts every `method: '<name>'` literal from the generated protocol union
//       (`editor/protocol/src/protocol.d.ts`, regenerated from the JSON Schema snapshot — never
//       hand-edited) — the full set of methods that exist to call, and pins the Wave 1 additions
//       (`component.remove`, `render.bounds`) and the settled Phase 6 method count so a stale
//       regen fails here instead of silently auditing against last phase's union;
//   (b) scans every non-test source file under `src/` for `method: '<name>'` / `method: "<name>"`
//       literals and asserts each one used by the app is in that generated set — so a typo'd or
//       made-up method name fails loudly here instead of silently at runtime. This *is* the "grep
//       for any editor-private endpoint = zero" exit-gate check, mechanized: an editor-private
//       endpoint is exactly a method-shaped string sent over the wire that isn't a member of the
//       generated union;
//   (c) asserts the same extracted set actually exercises the Wave 2-4 editing surface
//       (`render.bounds` for picking, `component.remove` for undo of an inserting `component.set`,
//       `component.set` for edits) — so this audit can't silently pass by never calling the
//       methods it's supposed to be gating;
//   (d) asserts `new WebSocket(` appears in exactly one file, `src/protocol/client.ts` — the one
//       place the app is allowed to open the wire;
//   (e) asserts `.send(` on the wire likewise appears only in `src/protocol/client.ts` — closing
//       the gap (d) alone leaves open (a second module could reuse a socket handed to it rather
//       than constructing its own) — so every command in (b) is provably reachable only through
//       the one protocol client, never a second, editor-private transport.
//
// Uses the shared Node shims in `../test-node-shims.d.ts` to read files under vitest's
// `environment: 'node'` (see `vite.config.ts`).

import { readFileSync, readdirSync } from 'node:fs';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_SRC_DIR = fileURLToPath(new URL('../', import.meta.url)); // editor/app/src/
const PROTOCOL_DTS_PATH = fileURLToPath(
  new URL('../../../protocol/src/protocol.d.ts', import.meta.url),
); // editor/protocol/src/protocol.d.ts

const METHOD_LITERAL = /method:\s*['"]([A-Za-z0-9_.]+)['"]/g;

/** Every `method: '<name>'` literal appearing in `text`, deduplicated. */
function extractMethodLiterals(text: string): Set<string> {
  const methods = new Set<string>();
  for (const match of text.matchAll(METHOD_LITERAL)) {
    methods.add(match[1] as string);
  }
  return methods;
}

/** All files under `dir`, as paths relative to `dir`, using forward slashes on every platform. */
function listFilesRelative(dir: string): string[] {
  return readdirSync(dir, { recursive: true })
    .filter((entry) => extname(entry) !== '') // readdirSync's recursive listing includes directories too; files have an extension here
    .map((entry) => entry.split('\\').join('/'));
}

/** True for a `.ts`/`.tsx` app source file that isn't a test file or an ambient `.d.ts`. */
function isAuditableSource(relativePath: string): boolean {
  if (!relativePath.endsWith('.ts') && !relativePath.endsWith('.tsx')) return false;
  if (relativePath.endsWith('.d.ts')) return false;
  if (relativePath.includes('.test.')) return false;
  return true;
}

/** Every `method: '<name>'` literal across the auditable (non-test) source tree, deduplicated. */
function extractUsedMethods(sourceFiles: string[]): Set<string> {
  const used = new Set<string>();
  for (const relativePath of sourceFiles) {
    if (!isAuditableSource(relativePath)) continue;
    const text = readFileSync(join(APP_SRC_DIR, relativePath)).toString();
    for (const method of extractMethodLiterals(text)) used.add(method);
  }
  return used;
}

describe('protocol parity audit', () => {
  const generatedMethods = extractMethodLiterals(readFileSync(PROTOCOL_DTS_PATH).toString());
  const sourceFiles = listFilesRelative(APP_SRC_DIR);
  const usedMethods = extractUsedMethods(sourceFiles);

  it('finds a non-empty set of generated protocol methods to audit against', () => {
    // A sanity floor, not a magic number: if this ever drops to zero, the regex or the path above
    // broke silently and every other assertion in this file would trivially pass for the wrong
    // reason.
    expect(generatedMethods.size).toBeGreaterThan(10);
  });

  it('includes the Phase 6 Wave 1 protocol additions: component.remove, render.bounds', () => {
    // Guards against a stale TS regen specifically: both of these are new as of Phase 6 Wave 1
    // (Plan/phase-6-editing.md); if `editor/protocol` wasn't regenerated after the Rust-side
    // schema snapshot grew them, this fails here instead of the audit silently running against
    // last phase's union (where every check below would trivially still pass).
    expect(generatedMethods.has('component.remove')).toBe(true);
    expect(generatedMethods.has('render.bounds')).toBe(true);
  });

  it('includes the Phase 7 Wave 1 protocol addition: engine.shutdown', () => {
    // New as of Phase 7 Wave 1 (Plan/phase-7-designer-flow.md, decision 2): the wire command that
    // stops a hosted session cleanly. If `editor/protocol` wasn't regenerated after the Rust-side
    // schema snapshot grew it, this fails here instead of the count assertion below going stale.
    expect(generatedMethods.has('engine.shutdown')).toBe(true);
  });

  it('includes the GI live-session arc addition: render.gi_rebake', () => {
    // New with the GI rebake lever: the one verb that sets, triggers and queries a host's
    // global-illumination rebake mode. Same job as the two guards above -- a stale TS regen fails
    // here by name, before the count assertion below has to guess what moved.
    expect(generatedMethods.has('render.gi_rebake')).toBe(true);
  });

  it('includes the viewport-latency arc addition: render.size', () => {
    // New with the stream-resolution knob (Plan/editor-viewport-latency.md Phase 1b): the one verb
    // that sets and reads a session's render extent -- and the reason the knob below this bar is a
    // legitimate editor feature rather than an editor-private endpoint. Same job as the guards
    // above: a stale TS regen fails here by name, before the count assertion has to guess.
    expect(generatedMethods.has('render.size')).toBe(true);
  });

  it('includes the view-camera arc addition: render.view', () => {
    // New with the editor's free camera (Plan/editor-stage-camera.md decision 3): the host-state
    // override every editor viewport flies through -- and the reason a view camera is a legitimate
    // editor feature rather than an editor-private one. Named here for the same reason render.size
    // is: a stale TS regen fails by name, before the count assertion has to guess.
    expect(generatedMethods.has('render.view')).toBe(true);
  });

  it('includes the debug-draw addition: render.debug', () => {
    // New with the character controller's debug draw (Plan/character-controller.md decision 19):
    // the host-state lever for which debug-draw categories render. Named here so a stale TS regen
    // fails by name, before the count assertion has to guess.
    expect(generatedMethods.has('render.debug')).toBe(true);
  });

  it('includes the hierarchy addition: entity.reparent', () => {
    // New with the transform hierarchy (Plan/transform-hierarchy-and-prefabs.md): re-points an
    // entity's Parent, keeping its world pose or its local Transform.
    expect(generatedMethods.has('entity.reparent')).toBe(true);
  });

  it('includes the prefab addition: prefab.spawn', () => {
    // New with runtime prefabs (Plan/transform-hierarchy-and-prefabs.md): instantiates a
    // `kind: prefab` file and replies with the new entity ids.
    expect(generatedMethods.has('prefab.spawn')).toBe(true);
  });

  it('includes the prefab edits: prefab.create/apply/revert/unpack', () => {
    // New with prefab editing (Plan/transform-hierarchy-and-prefabs.md): write live entities out as
    // a prefab, write an instance's edits back, restore it from its prefab, or unpack it.
    expect(generatedMethods.has('prefab.create')).toBe(true);
    expect(generatedMethods.has('prefab.apply')).toBe(true);
    expect(generatedMethods.has('prefab.revert')).toBe(true);
    expect(generatedMethods.has('prefab.unpack')).toBe(true);
  });

  it('includes the override read: prefab.overrides', () => {
    // New with W4 override markers (Plan/transform-hierarchy-and-prefabs.md): what `scene.save`
    // would write for each prefab instance, as a pure read.
    expect(generatedMethods.has('prefab.overrides')).toBe(true);
  });

  it('includes the 0022 additions: map.spawn, prefab.despawn', () => {
    // The wire faces of `spawn_map` and `despawn_instance` (engine request 0022).
    expect(generatedMethods.has('map.spawn')).toBe(true);
    expect(generatedMethods.has('prefab.despawn')).toBe(true);
  });

  it('has exactly the settled protocol surface: 48 methods', () => {
    // Not a magic number: this is the full method count of the generated union (Phase 6's 25 +
    // engine.shutdown + profile.capture + render.gi_rebake + render.size + render.view + render.debug + entity.reparent + prefab.spawn + prefab.create + prefab.apply + prefab.revert + prefab.unpack + prefab.overrides + map.spawn + prefab.despawn). A silent surface change
    // (a method added or dropped without anyone noticing) would slip past every other assertion in
    // this file, since they all key off `generatedMethods` itself -- this makes that drift loud.
    // Update this count *deliberately* the next time the protocol grows or shrinks a method.
    expect(generatedMethods.size).toBe(48);
  });

  it('uses only methods that exist in the generated protocol union', () => {
    const usedByFile = new Map<string, string[]>();
    for (const relativePath of sourceFiles) {
      if (!isAuditableSource(relativePath)) continue;
      const text = readFileSync(join(APP_SRC_DIR, relativePath)).toString();
      const used = extractMethodLiterals(text);
      if (used.size === 0) continue;
      const unknown = [...used].filter((method) => !generatedMethods.has(method));
      if (unknown.length > 0) {
        usedByFile.set(relativePath, unknown);
      }
    }
    expect(Object.fromEntries(usedByFile)).toEqual({});
  });

  it('reports the set of methods the viewer actually calls (informational)', () => {
    // No fixed membership assertion here on purpose: pinning the exact list would make this test
    // fail every time the viewer grows a new (still valid) command. Its only job is to make the
    // set visible in the test output; the previous test is what actually gates unknown methods.
    expect(usedMethods.size).toBeGreaterThan(0);
  });

  it('exercises the Phase 6 editing surface: render.bounds, component.remove, component.set', () => {
    // Picking (render.bounds, Wave 2), undo of an inserting set (component.remove, Wave 2-4), and
    // field/gizmo edits (component.set, Waves 2-3) are the load-bearing new-surface calls this
    // phase added. Auditing an editor that has stopped calling the thing it's supposed to be
    // exercising is a false green, so this pins presence, not just "some set is non-empty". If a
    // real call site ever fails to show up here, fix `extractMethodLiterals`/`isAuditableSource`
    // above -- not this assertion.
    for (const method of ['render.bounds', 'component.remove', 'component.set']) {
      expect(usedMethods.has(method)).toBe(true);
    }
  });

  it('constructs WebSocket in exactly one module: src/protocol/client.ts', () => {
    const NEW_WEBSOCKET = /new\s+WebSocket\s*\(/;
    const offenders: string[] = [];
    for (const relativePath of sourceFiles) {
      // Scoped to app source, like the method-literal scan above -- this audit's own doc comment
      // above names the pattern it looks for, which would otherwise flag itself.
      if (!isAuditableSource(relativePath)) continue;
      const text = readFileSync(join(APP_SRC_DIR, relativePath)).toString();
      if (NEW_WEBSOCKET.test(text)) {
        offenders.push(relativePath);
      }
    }
    expect(offenders).toEqual(['protocol/client.ts']);
  });

  it('sends over the wire from exactly one module: src/protocol/client.ts', () => {
    // Closes the gap `new WebSocket(` alone leaves open: a second module could receive a socket
    // handed to it (or reach `ws` via a closure) and call `.send()` without ever constructing one
    // itself. Scoped to a literal `.send(` -- not a bare `send(` -- so this doesn't fire on
    // unrelated prose (e.g. a comment mentioning "send") the way a looser word-boundary match
    // would.
    const DOT_SEND = /\.send\(/;
    const offenders: string[] = [];
    for (const relativePath of sourceFiles) {
      if (!isAuditableSource(relativePath)) continue;
      const text = readFileSync(join(APP_SRC_DIR, relativePath)).toString();
      if (DOT_SEND.test(text)) {
        offenders.push(relativePath);
      }
    }
    expect(offenders).toEqual(['protocol/client.ts']);
  });
});
