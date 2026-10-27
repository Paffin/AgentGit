// Vitest's normal config is linked from the source tree by the mutation runner. Make the
// worker's aliases explicit so a mutation cannot be credited for a failed test bootstrap.
import { spawnSync } from 'node:child_process'
import { realpathSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { stripVTControlCharacters } from 'node:util'

const source = realpathSync(resolve(dirname(fileURLToPath(import.meta.url)), '..'))
const working = realpathSync(process.cwd())
const worker = working !== source
let config = join(source, 'vite.config.ts')
if (worker) {
  const workerRoot = join(source, 'target', 'mutation-workers')
  const inside = relative(workerRoot, working)
  if (!inside || inside.startsWith('..')) throw new Error('Unexpected mutation worker directory')
  config = join(working, '.mutation-vite.config.mjs')
  writeFileSync(
    config,
    `import original from ${JSON.stringify(pathToFileURL(join(source, 'vite.config.ts')).href)};
export default {...original,
  resolve: {...original.resolve, alias: {'@': ${JSON.stringify(join(working, 'src'))}}},
  server: {...original.server, fs: {allow: ${JSON.stringify([source, working])}}},
  test: {...original.test, setupFiles: ${JSON.stringify([join(source, 'tests', 'setup.ts')])}}
};\n`,
  )
}
const result = spawnSync(
  process.execPath,
  [
    join(source, 'node_modules/vitest/vitest.mjs'),
    'run',
    '--config',
    config,
    ...process.argv.slice(2),
  ],
  { encoding: 'utf8' },
)
const output = `${result.stdout ?? ''}${result.stderr ?? ''}`
process.stdout.write(output)
const plain = stripVTControlCharacters(output)
if (!/Tests\s+[^\n]*\d+ (?:passed|failed)/.test(plain)) {
  process.stderr.write(
    'UNVERIFIED: no actual test results; do not credit this as a killed mutation.\n',
  )
  // The external runner treats every nonzero exit as a kill. Conservatively leave a worker
  // mutation alive (which fails its gate), while a broken baseline exits unsuccessfully.
  process.exit(worker ? 0 : 2)
}
process.exit(result.status ?? 2)
