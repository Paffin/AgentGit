import { defineConfig } from 'tsup';

// Ships ESM + CJS + .d.ts. The two noble libraries are ESM-only, so they are bundled into the
// output (their MIT notices are in THIRD_PARTY_NOTICES.md): the package has no runtime dependencies.
export default defineConfig({
  entry: ['src/index.ts'],
  format: ['esm', 'cjs'],
  dts: true,
  tsconfig: 'tsconfig.build.json',
  target: 'es2022',
  platform: 'neutral',
  sourcemap: true,
  clean: true,
  splitting: false,
  treeshake: true,
  noExternal: [/^@noble\//],
  outExtension: ({ format }) => ({ js: format === 'cjs' ? '.cjs' : '.js' }),
});
