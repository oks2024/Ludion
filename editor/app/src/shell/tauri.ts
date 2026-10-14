// Tauri is the product; the Vite browser page is dev-only. Native window
// chrome (custom titlebar controls, drag region) must degrade to nothing in plain-browser dev, so
// every native-only affordance keys off this one runtime probe rather than a build flag.

/** True when running inside the Tauri webview (the internals bridge is injected by the host). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
