import path from 'node:path';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      // Examples and docs import the published name; tests run them against the sources.
      '@agentgit/client': path.resolve(import.meta.dirname, 'src/index.ts'),
    },
  },
  test: {
    include: ['test/**/*.test.ts'],
    // Each test file starts its own real server process, so keep files in separate processes.
    pool: 'forks',
    testTimeout: 60_000,
    hookTimeout: 60_000,
    teardownTimeout: 15_000,
    reporters: ['default'],
  },
});
