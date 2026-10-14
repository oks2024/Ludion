import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

// WebSocket sources: engine hosts on any machine (LAN engines, a device build) and Vite's own HMR
// socket. Scheme sources only, so no http(s) host and no `*` widens the policy.
const WS_SOURCES = 'ws: wss:'

// Tauri injects `devCsp` only into assets it embeds, never into a page served from `devUrl`, so
// the dev policy rides the dev server's response headers. The inline script is React Refresh's
// preamble. The production policy is `app.security.csp` in `src-tauri/tauri.conf.json`.
const DEV_CSP = [
  "default-src 'self'",
  "script-src 'self' 'unsafe-inline'",
  "style-src 'self' 'unsafe-inline'",
  `connect-src 'self' ipc: http://ipc.localhost ${WS_SOURCES}`,
  "img-src 'self' data: blob:",
  "font-src 'self' data:",
  "worker-src 'self' blob:",
  "object-src 'none'",
  "base-uri 'self'",
].join('; ')

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  // The port is the one `devUrl` names; a busy port fails instead of hopping to one Tauri never loads.
  server: {
    port: 1420,
    strictPort: true,
    headers: { 'Content-Security-Policy': DEV_CSP },
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
})
