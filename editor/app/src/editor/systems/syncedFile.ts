// The synced-file session: one open document bound to one path, from the disk read to the disk write —
// pure (no React, no DOM, no Tauri), so the whole policy is unit-tested (syncedFile.test.ts) and
// `useSyncedFile.ts` only wires a timer, an interval and the host's file calls to it.
//
// It drives the settled save/watch reducer (`fileSync.ts`) and the write throttle (`voxel/save.ts`)
// for the four editors that own a parsed document (voxel, sculpt, recipe, pattern); the code panel's
// document lives in CodeMirror and keeps using the reducer on its own.
//
// FOUR RULES, each of which was once a bug in a hand-copied hook:
//
//   1. THE PATH IS BOUND AT CONSTRUCTION. A document switch disposes this session and builds another,
//      so a pending write can only ever reach the file it was edited for.
//   2. DISPOSE FLUSHES. A view switch or a document switch writes a pending dirty edit instead of
//      stranding it on a timer that is about to be cleared.
//   3. A WRITE STATS FIRST. The poll runs once a second; a human can nudge a field in between an
//      agent's terminal edit and the next poll. When the disk's mtime is not the acknowledged baseline
//      the write is not made: the disk text is read and fed to the reducer as an external change,
//      which raises the conflict banner while the local edits are dirty.
//   4. A CONFLICT HOLDS THE WRITE, and a dispose under a conflict writes nothing. Auto-writing through
//      the banner would silently drop one side's edit, the one outcome the reducer exists to prevent;
//      `keepMine` (adopt the incoming mtime, keep the local text) then writes, `reload` discards.
//
// Writes are serialised: a flush waits for the one in flight, then re-checks the phase. An edit that
// lands while a write is in flight keeps the document dirty once the write's `save` is applied.

import { initialSyncState, reduce, type SyncEvent, type SyncPhase, type SyncState } from './fileSync';
import { writeAction } from '../voxel/save';

/** How a caller wants its edit to reach the disk. See `voxel/save.ts` for why the painter and the
 *  sliders answer this differently. */
export type WriteMode =
  /** Update the working document only — the caller commits later (a stroke in progress). */
  | 'defer'
  /** Write if the throttle allows, otherwise schedule a trailing write (a slider or palette drag). */
  | 'throttle'
  /** Write now: a pointer release, an undo, a single click. */
  | 'commit';

export interface FileIo {
  read(path: string): Promise<string>;
  stat(path: string): Promise<{ readonly mtimeMs: number }>;
  /** Resolves with the written file's mtime — adopted as the new baseline, never stat-ed after. */
  write(path: string, text: string): Promise<{ readonly mtimeMs: number }>;
}

/** The clock and timers, injected so the session never touches `window` or `performance`. */
export interface Timers {
  now(): number;
  set(fn: () => void, ms: number): number;
  clear(id: number): void;
}

export interface SyncedFileOptions<D> {
  parse(text: string): D;
  /** The document as the file text. */
  source(doc: D): string;
  /** Runs after each successful write, awaited by `flush`; a throw is the caller's to report and
   *  never marks the write failed. */
  afterWrite?(path: string): Promise<void> | void;
  /** The working document was replaced from the disk (a load is not one): the caller's undo history
   *  and gesture state no longer describe it. */
  onReplaced?(): void;
}

export interface SyncedFileSnapshot<D> {
  readonly doc: D | null;
  readonly phase: SyncPhase;
  readonly loadError: string | null;
  readonly writeError: string | null;
  /** Bumped whenever the DISK content changed (a load, an adopted external change, our own save). */
  readonly savedRevision: number;
}

export const EMPTY_SNAPSHOT: SyncedFileSnapshot<never> = {
  doc: null,
  phase: 'clean',
  loadError: null,
  writeError: null,
  savedRevision: 0,
};

export class SyncedFileSession<D> {
  private doc: D | null = null;
  private sync: SyncState = initialSyncState;
  private loadError: string | null = null;
  private writeError: string | null = null;
  private savedRevision = 0;
  private lastWrite: number | null = null;
  private trailing: number | null = null;
  private writing = 0;
  private tail: Promise<void> = Promise.resolve();
  private editRevision = 0;
  private disposed = false;

  private readonly path: string;
  private readonly io: FileIo;
  private readonly timers: Timers;
  private readonly options: SyncedFileOptions<D>;
  private readonly onChange: (snapshot: SyncedFileSnapshot<D>) => void;

  constructor(
    path: string,
    io: FileIo,
    timers: Timers,
    options: SyncedFileOptions<D>,
    onChange: (snapshot: SyncedFileSnapshot<D>) => void,
  ) {
    this.path = path;
    this.io = io;
    this.timers = timers;
    this.options = options;
    this.onChange = onChange;
  }

  /** The working document, synchronously — several pointer events can arrive before a re-render, and
   *  each edit must build on the last. */
  current(): D | null {
    return this.doc;
  }

  snapshot(): SyncedFileSnapshot<D> {
    return {
      doc: this.doc,
      phase: this.sync.phase,
      loadError: this.loadError,
      writeError: this.writeError,
      savedRevision: this.savedRevision,
    };
  }

  /** Reads the file and adopts its mtime as the baseline. */
  async load(): Promise<void> {
    try {
      const [text, stat] = await Promise.all([this.io.read(this.path), this.io.stat(this.path)]);
      if (this.disposed) {
        return;
      }
      this.doc = this.options.parse(text);
      this.apply({ type: 'loaded', mtime: stat.mtimeMs });
      this.savedRevision += 1;
    } catch (err) {
      if (this.disposed) {
        return;
      }
      this.loadError = String(err);
    }
    this.notify();
  }

  /** The working document becomes `next`; the disk follows per `mode`. */
  edit(next: D, mode: WriteMode): void {
    if (this.disposed || this.doc === null) {
      return;
    }
    this.doc = next;
    this.editRevision += 1;
    this.apply({ type: 'edit' });
    this.schedule(mode);
    this.notify();
  }

  /** An edit that changed nothing: a `commit` still flushes, because the gesture it ends may have
   *  deferred earlier changes. */
  request(mode: WriteMode): void {
    if (!this.disposed) {
      this.schedule(mode);
    }
  }

  /** Writes the working text now and resolves once it has landed (or been held or refused). */
  flush(): Promise<void> {
    this.cancelTrailing();
    this.writing += 1;
    const run = this.tail.then(() => this.writeOnce()).finally(() => {
      this.writing -= 1;
    });
    this.tail = run.catch(() => undefined);
    return run;
  }

  /** One change-poll tick: an external mtime is read and handed to the reducer. Skipped while a write
   *  is in flight or scheduled — reading mid-write would hand the reducer an mtime we are about to
   *  supersede, and the working text is already ahead of the disk by construction. */
  async poll(): Promise<void> {
    if (!this.pollable()) {
      return;
    }
    try {
      const stat = await this.io.stat(this.path);
      if (stat.mtimeMs === this.sync.baselineMtime) {
        return;
      }
      const content = await this.io.read(this.path);
      if (!this.pollable()) {
        return;
      }
      this.apply({ type: 'externalChange', mtime: stat.mtimeMs, content });
      this.notify();
    } catch {
      // A file that vanished mid-edit is the readout's story to tell, not the poll's.
    }
  }

  /** Banner "Reload": discard local edits, take the disk. */
  reload(): void {
    this.apply({ type: 'reload' });
    this.notify();
  }

  /** Banner "Keep mine": keep local edits and write them (last-writer-wins, the settled policy). */
  keepMine(): void {
    this.apply({ type: 'keepMine' });
    this.schedule('commit');
    this.notify();
  }

  /** Ends the session: no further `onChange`, late results ignored — but a pending dirty edit is
   *  still written, to this session's own path. */
  dispose(): Promise<void> {
    this.disposed = true;
    return this.flush();
  }

  private pollable(): boolean {
    return !this.disposed && this.doc !== null && this.writing === 0 && this.trailing === null;
  }

  private apply(event: SyncEvent): void {
    const { state, effect } = reduce(this.sync, event);
    this.sync = state;
    if (effect.type === 'setContent') {
      this.doc = this.options.parse(effect.content);
      this.savedRevision += 1;
      this.options.onReplaced?.();
    }
  }

  private cancelTrailing(): void {
    if (this.trailing !== null) {
      this.timers.clear(this.trailing);
      this.trailing = null;
    }
  }

  private schedule(mode: WriteMode): void {
    if (mode === 'defer') {
      return;
    }
    const action = writeAction(this.lastWrite, this.timers.now(), mode === 'commit');
    if (action.kind === 'write') {
      void this.flush();
      return;
    }
    if (this.trailing === null) {
      this.trailing = this.timers.set(() => {
        this.trailing = null;
        void this.flush();
      }, action.afterMs);
    }
  }

  private notify(): void {
    if (!this.disposed) {
      this.onChange(this.snapshot());
    }
  }

  private async writeOnce(): Promise<void> {
    if (this.doc === null || this.sync.phase !== 'dirty') {
      return;
    }
    const path = this.path;
    try {
      const disk = await this.io.stat(path);
      if (disk.mtimeMs !== this.sync.baselineMtime) {
        // Somebody wrote the file since we last saw it: hold the write behind the banner.
        const content = await this.io.read(path);
        this.apply({ type: 'externalChange', mtime: disk.mtimeMs, content });
        this.notify();
        return;
      }
      // Re-read after the awaits: an edit may have landed while the stat was in flight.
      const doc = this.doc;
      if (doc === null || this.sync.phase !== 'dirty') {
        return;
      }
      const revision = this.editRevision;
      this.lastWrite = this.timers.now();
      const written = await this.io.write(path, this.options.source(doc));
      this.apply({ type: 'save', mtime: written.mtimeMs });
      if (this.editRevision !== revision) {
        // An edit landed during the write: the disk is already behind it.
        this.apply({ type: 'edit' });
      }
      this.savedRevision += 1;
      this.writeError = null;
    } catch (err) {
      this.writeError = `could not write ${path}: ${String(err)}`;
      this.notify();
      return;
    }
    this.notify();
    try {
      await this.options.afterWrite?.(path);
    } catch {
      // The caller reports its own follow-up's failure; the write itself landed.
    }
  }
}
