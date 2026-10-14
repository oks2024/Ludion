import { describe, expect, it } from 'vitest';
import {
  SyncedFileSession,
  type FileIo,
  type SyncedFileOptions,
  type SyncedFileSnapshot,
  type Timers,
} from './syncedFile';

/** An in-memory disk: text plus a monotonically increasing mtime, a write log, and an optional gate
 *  that holds `write` open until the test releases it. */
class FakeDisk implements FileIo {
  files = new Map<string, { text: string; mtimeMs: number }>();
  writes: { path: string; text: string }[] = [];
  private clock = 100;
  private gate: Promise<void> | null = null;
  private release: (() => void) | null = null;

  put(path: string, text: string): void {
    this.clock += 1;
    this.files.set(path, { text, mtimeMs: this.clock });
  }
  text(path: string): string | undefined {
    return this.files.get(path)?.text;
  }
  hold(): void {
    this.gate = new Promise((resolve) => {
      this.release = resolve;
    });
  }
  open(): void {
    this.release?.();
    this.gate = null;
  }
  async read(path: string): Promise<string> {
    const file = this.files.get(path);
    if (file === undefined) throw new Error(`no such file ${path}`);
    return file.text;
  }
  async stat(path: string): Promise<{ mtimeMs: number }> {
    const file = this.files.get(path);
    if (file === undefined) throw new Error(`no such file ${path}`);
    return { mtimeMs: file.mtimeMs };
  }
  async write(path: string, text: string): Promise<{ mtimeMs: number }> {
    if (this.gate !== null) await this.gate;
    this.put(path, text);
    this.writes.push({ path, text });
    return { mtimeMs: this.files.get(path)!.mtimeMs };
  }
}

/** Timers fired by hand; `now` advances only when the test says so. */
class ManualTimers implements Timers {
  time = 1000;
  private next = 1;
  pending = new Map<number, { fn: () => void; due: number }>();
  now(): number {
    return this.time;
  }
  set(fn: () => void, ms: number): number {
    const id = this.next++;
    this.pending.set(id, { fn, due: this.time + ms });
    return id;
  }
  clear(id: number): void {
    this.pending.delete(id);
  }
  fireAll(): void {
    const due = [...this.pending.values()];
    this.pending.clear();
    for (const entry of due) entry.fn();
  }
}

const options: SyncedFileOptions<string> = { parse: (text) => text, source: (doc) => doc };

function rig(path = 'a.yaml', text = 'a: 1', extra: Partial<SyncedFileOptions<string>> = {}) {
  const disk = new FakeDisk();
  disk.put(path, text);
  return { disk, ...session(disk, path, extra) };
}

function session(disk: FakeDisk, path: string, extra: Partial<SyncedFileOptions<string>>) {
  const timers = new ManualTimers();
  const changes: SyncedFileSnapshot<string>[] = [];
  const made = new SyncedFileSession(path, disk, timers, { ...options, ...extra }, (s) => changes.push(s));
  return { session: made, timers, changes };
}

/** Lets every pending microtask (an awaited stat, read, write) settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 20; i += 1) await Promise.resolve();
}

describe('SyncedFileSession', () => {
  it('holds a write behind a conflict when the disk moved since the last poll', async () => {
    const { disk, session: s } = rig();
    await s.load();
    disk.put('a.yaml', 'a: 2'); // the agent's terminal edit, before any poll
    s.edit('a: 9', 'commit');
    await settle();
    expect(disk.writes).toEqual([]);
    expect(s.snapshot().phase).toBe('conflict');
    expect(disk.text('a.yaml')).toBe('a: 2');

    s.keepMine();
    await settle();
    expect(disk.writes).toEqual([{ path: 'a.yaml', text: 'a: 9' }]);
    expect(s.snapshot().phase).toBe('clean');
  });

  it('reload under a conflict adopts the disk and writes nothing', async () => {
    const { disk, session: s } = rig();
    await s.load();
    disk.put('a.yaml', 'a: 2');
    s.edit('a: 9', 'commit');
    await settle();
    s.reload();
    await settle();
    expect(s.current()).toBe('a: 2');
    expect(disk.writes).toEqual([]);
    expect(s.snapshot().phase).toBe('clean');
  });

  it('a clean poll adopts the agent edit and the next edit writes on top of it', async () => {
    const { disk, session: s } = rig();
    await s.load();
    disk.put('a.yaml', 'a: 2');
    await s.poll();
    expect(s.current()).toBe('a: 2');
    s.edit('a: 2\nb: 3', 'commit');
    await settle();
    expect(disk.writes).toEqual([{ path: 'a.yaml', text: 'a: 2\nb: 3' }]);
  });

  it('throttles: one immediate write, then a trailing write with the latest text', async () => {
    const { disk, timers, session: s } = rig();
    await s.load();
    s.edit('a: 3', 'throttle');
    await settle();
    s.edit('a: 4', 'throttle');
    s.edit('a: 5', 'throttle');
    await settle();
    expect(disk.writes.map((w) => w.text)).toEqual(['a: 3']);
    expect(timers.pending.size).toBe(1);
    timers.time += 150;
    timers.fireAll();
    await settle();
    expect(disk.writes.map((w) => w.text)).toEqual(['a: 3', 'a: 5']);
  });

  it('dispose writes a pending trailing edit to its own path, never the next document', async () => {
    const disk = new FakeDisk();
    disk.put('a.yaml', 'a: 1');
    disk.put('b.yaml', 'b: 1');
    const a = session(disk, 'a.yaml', {});
    await a.session.load();
    a.session.edit('a: 3', 'throttle');
    await settle();
    a.session.edit('a: 4', 'throttle');
    expect(a.timers.pending.size).toBe(1);

    const b = session(disk, 'b.yaml', {});
    await a.session.dispose();
    await b.session.load();
    await settle();
    expect(disk.writes).toEqual([
      { path: 'a.yaml', text: 'a: 3' },
      { path: 'a.yaml', text: 'a: 4' },
    ]);
    expect(disk.text('b.yaml')).toBe('b: 1');
    expect(b.session.current()).toBe('b: 1');
    expect(a.timers.pending.size).toBe(0);
  });

  it('dispose under a conflict writes nothing', async () => {
    const { disk, session: s } = rig();
    await s.load();
    disk.put('a.yaml', 'a: 2');
    s.edit('a: 9', 'defer');
    await s.poll();
    expect(s.snapshot().phase).toBe('conflict');
    await s.dispose();
    expect(disk.writes).toEqual([]);
    expect(disk.text('a.yaml')).toBe('a: 2');
  });

  it('a disposed session ignores its late load and never calls onChange', async () => {
    const { session: s, changes } = rig();
    const loading = s.load();
    const disposing = s.dispose();
    await Promise.all([loading, disposing]);
    expect(changes).toEqual([]);
    expect(s.current()).toBeNull();
  });

  it('skips the poll while a write is in flight and while a trailing write is scheduled', async () => {
    const { disk, timers, session: s } = rig();
    await s.load();
    disk.hold();
    s.edit('a: 3', 'commit');
    await settle();
    disk.put('a.yaml', 'a: 7'); // the agent writes while ours is in flight
    await s.poll();
    expect(s.current()).toBe('a: 3');
    expect(s.snapshot().phase).toBe('dirty');
    disk.open();
    await settle();

    s.edit('a: 4', 'throttle');
    expect(timers.pending.size).toBe(1);
    disk.put('a.yaml', 'a: 8');
    await s.poll();
    expect(s.current()).toBe('a: 4');
    expect(s.snapshot().phase).toBe('dirty');
  });

  it('an edit that lands during a write is written next, not marked clean', async () => {
    const { disk, session: s } = rig();
    await s.load();
    disk.hold();
    s.edit('a: 3', 'commit');
    await settle();
    s.edit('a: 4', 'commit');
    disk.open();
    await settle();
    expect(disk.writes.map((w) => w.text)).toEqual(['a: 3', 'a: 4']);
    expect(s.snapshot().phase).toBe('clean');
  });

  it('runs afterWrite once per write with the path, and a throw leaves writeError null', async () => {
    const calls: string[] = [];
    const { disk, session: s } = rig('a.yaml', 'a: 1', {
      afterWrite: (path) => {
        calls.push(path);
        throw new Error('reload refused');
      },
    });
    await s.load();
    s.edit('a: 3', 'commit');
    await settle();
    expect(disk.writes).toHaveLength(1);
    expect(calls).toEqual(['a.yaml']);
    expect(s.snapshot().writeError).toBeNull();
  });

  it('a failed write sets writeError and keeps the document dirty', async () => {
    const { disk, session: s } = rig();
    await s.load();
    disk.write = async () => {
      throw new Error('disk full');
    };
    s.edit('a: 3', 'commit');
    await settle();
    expect(s.snapshot().writeError).toBe('could not write a.yaml: Error: disk full');
    expect(s.snapshot().phase).toBe('dirty');
  });

  it('onReplaced fires for an adopted disk change, not for a load', async () => {
    let replaced = 0;
    const { disk, session: s } = rig('a.yaml', 'a: 1', { onReplaced: () => (replaced += 1) });
    await s.load();
    expect(replaced).toBe(0);
    disk.put('a.yaml', 'a: 2');
    await s.poll();
    expect(replaced).toBe(1);
  });
});
