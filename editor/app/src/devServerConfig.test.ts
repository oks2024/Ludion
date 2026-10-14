// The dev port and the webview's content security policy, pinned: the editor loads only this
// checkout's frontend (a busy port fails instead of hopping), and the page can reach nothing but
// loopback sockets and the Tauri IPC.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import config from '../vite.config';

const tauri = JSON.parse(readFileSync(fileURLToPath(new URL('../src-tauri/tauri.conf.json', import.meta.url)), 'utf8'));

function parse(csp: string): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const part of csp.split(';')) {
    const [name, ...sources] = part.trim().split(/\s+/);
    if (name) out.set(name, sources);
  }
  return out;
}

const devHeader = (config.server?.headers as Record<string, string> | undefined)?.['Content-Security-Policy'];
const policies: [string, string | null | undefined][] = [
  ['production', tauri.app.security.csp],
  ['dev', devHeader],
];

describe('dev server', () => {
  it('pins port 1420 and refuses to hop', () => {
    expect(config.server?.port).toBe(1420);
    expect(config.server?.strictPort).toBe(true);
  });

  it('is the port Tauri loads', () => {
    expect(new URL(tauri.build.devUrl).port).toBe(String(config.server?.port));
  });

  it('leaves devCsp unset: Tauri never injects it into a page served from devUrl', () => {
    expect(tauri.app.security.devCsp).toBeUndefined();
  });
});

describe.each(policies)('%s CSP', (_name, csp) => {
  it('exists and defaults to self', () => {
    expect(csp).toBeTruthy();
    expect(parse(csp as string).get('default-src')).toEqual(["'self'"]);
  });

  it('allows no wildcard and no http(s) host beyond the Tauri IPC', () => {
    const allowedHosts = new Set(['ipc.localhost']);
    for (const [directive, sources] of parse(csp as string)) {
      for (const source of sources) {
        expect(source, directive).not.toBe('*');
        if (/^https?:\/\//.test(source)) {
          expect(allowedHosts.has(new URL(source.replace(':*', '')).hostname), `${directive} ${source}`).toBe(true);
        }
      }
    }
  });

  it('connects to the Tauri IPC and WebSockets on any host', () => {
    const sources = parse(csp as string).get('connect-src') ?? [];
    for (const source of sources) {
      expect(source).toMatch(/^('self'|ipc:|http:\/\/ipc\.localhost|wss?:)$/);
    }
  });

  it('forbids plugins and foreign bases', () => {
    const parsed = parse(csp as string);
    expect(parsed.get('object-src')).toEqual(["'none'"]);
    expect(parsed.get('base-uri')).toEqual(["'self'"]);
  });
});

describe('production CSP script-src', () => {
  it('runs bundled scripts only', () => {
    const sources = parse(tauri.app.security.csp).get('script-src') ?? [];
    expect(sources).toEqual(["'self'"]);
  });
});
