// Class gate for stream subscriptions: a late `stream.subscribe` reply that arrives after its owner's
// cleanup must be unsubscribed, which only `openSubscription` (`subscription.ts`) does. Every other
// module therefore subscribes through it, never through `.subscribe(` or a raw
// `'stream.subscribe'` call.

import { readFileSync, readdirSync } from 'node:fs';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const APP_SRC_DIR = fileURLToPath(new URL('../', import.meta.url));
const ALLOWED = new Set(['protocol/client.ts', 'protocol/subscription.ts']);
const DIRECT_SUBSCRIBE = /\.subscribe\(|["']stream\.subscribe["']/;

function isAuditableSource(path: string): boolean {
  if (!path.endsWith('.ts') && !path.endsWith('.tsx')) return false;
  return !path.endsWith('.d.ts') && !path.includes('.test.');
}

describe('stream subscription sites', () => {
  it('only protocol/client.ts and protocol/subscription.ts subscribe directly', () => {
    const offenders = readdirSync(APP_SRC_DIR, { recursive: true })
      .filter((entry) => extname(entry) !== '')
      .map((entry) => entry.split('\\').join('/'))
      .filter((path) => isAuditableSource(path) && !ALLOWED.has(path))
      .filter((path) => DIRECT_SUBSCRIBE.test(readFileSync(join(APP_SRC_DIR, path)).toString()));
    expect(offenders).toEqual([]);
  });
});
