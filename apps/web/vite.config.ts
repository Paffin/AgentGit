import { fileURLToPath, URL } from 'node:url'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vitest/config'

// `npm run dev` proxies the API to a local ag-server. Override with AG_SERVER_URL.
const target = process.env.AG_SERVER_URL ?? 'http://127.0.0.1:8080'

export default defineConfig({
  plugins: [vue()],
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
    environment: 'happy-dom',
    include: ['src/**/*.spec.ts', 'tests/**/*.spec.ts'],
    setupFiles: ['./tests/setup.ts'],
    restoreMocks: true,
    clearMocks: true,
  },
})
