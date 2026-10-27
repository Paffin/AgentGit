import { readFileSync } from 'node:fs'
import { availableParallelism } from 'node:os'
import type { Plugin } from 'vite'
import { fileURLToPath, URL } from 'node:url'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vitest/config'

// `npm run dev` proxies the API to a local ag-server. Override with AG_SERVER_URL.
const target = process.env.AG_SERVER_URL ?? 'http://127.0.0.1:8080'

// The version of this UI build, written next to the app as `ui-version.json`: the page compares it with the
// `server_version` the server reports and warns when they do not belong together (shell/VersionBanner.vue).
// It is the version in package.json, which a Rust test (crates/ag-server/tests/ui.rs) keeps equal to the server's.
const uiVersion = (
  JSON.parse(readFileSync(fileURLToPath(new URL('./package.json', import.meta.url)), 'utf8')) as {
    version: string
  }
).version
const versionFile: Plugin = {
  name: 'ag-ui-version',
  generateBundle() {
    this.emitFile({
      type: 'asset',
      fileName: 'ui-version.json',
      source: JSON.stringify({ version: uiVersion }) + '\n',
    })
  },
}

export default defineConfig({
  plugins: [vue(), versionFile],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  server: {
    port: 5173,
    proxy: {
      '/v1': { target, changeOrigin: false },
      '/openapi.json': { target, changeOrigin: false },
      '/healthz': { target, changeOrigin: false },
    },
  },
  build: {
    outDir: 'dist',
    target: 'es2022',
    sourcemap: false,
    // The server's CSP forbids inline scripts; keep every asset a real file.
    assetsInlineLimit: 0,
    cssCodeSplit: true,
  },
  test: {
    // Each file gets an isolated VM; reuse bounded workers instead of starting a process per file.
    pool: 'vmThreads',
    isolate: true,
    maxWorkers: Math.min(4, availableParallelism()),
    vmMemoryLimit: '512MB',
    environment: 'happy-dom',
    include: ['src/**/*.spec.ts', 'tests/**/*.spec.ts'],
    setupFiles: ['./tests/setup.ts'],
    restoreMocks: true,
    clearMocks: true,
  },
})
