// Minimal ambient module shims for the couple of Node built-ins this package's vitest suites read
// files or scan sources with (see `protocol/qoi-fixture.test.ts` and `protocol/parity.test.ts`).
//
// The app's `tsconfig.app.json` intentionally scopes `types` to `["vite/client"]` only — pulling
// in all of `@types/node`'s globals would collide with DOM globals (`Blob`, `ReadableStream`,
// etc.) that the browser-facing app code under `src/` relies on. vitest itself runs test files
// under `environment: 'node'` (see `vite.config.ts`), so Node built-ins are safe to use at
// runtime from `*.test.ts` files; this file just gives them types without widening the shared
// config. Ambient `.d.ts` files under `src/` are picked up automatically by `tsconfig.app.json`'s
// `include: ["src"]` — no explicit import needed.

declare module 'node:fs' {
  export function readFileSync(path: string): Uint8Array;
  export function readFileSync(path: string, encoding: 'utf8'): string;
  export function readdirSync(path: string): string[];
  export function readdirSync(path: string, options: { recursive: true }): string[];
  export function statSync(path: string): { isDirectory(): boolean };
}
declare module 'node:url' {
  export function fileURLToPath(url: string | URL): string;
}
declare module 'node:path' {
  export function join(...segments: string[]): string;
  export function relative(from: string, to: string): string;
  export function extname(path: string): string;
}
