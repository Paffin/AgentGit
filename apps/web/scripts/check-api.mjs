// Fails when src/api/schema.gen.ts is not exactly what `npm run gen:api` would produce from
// docs/protocol/openapi.json. Run in CI, so the UI cannot drift from the protocol document.
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const bin = (name) => join(root, 'node_modules', '.bin', name)
const dir = mkdtempSync(join(tmpdir(), 'agentgit-api-'))
const out = join(dir, 'schema.gen.ts')

function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: root, encoding: 'utf8' })
  if (r.status !== 0) {
    process.stderr.write(r.stdout + r.stderr)
    process.exit(2)
  }
}

try {
  run(bin('openapi-typescript'), ['../../docs/protocol/openapi.json', '-o', out])
  run(bin('prettier'), ['--config', join(root, '.prettierrc.json'), '--write', out])
  const fresh = readFileSync(out, 'utf8')
  const committed = readFileSync(join(root, 'src/api/schema.gen.ts'), 'utf8')
  if (fresh !== committed) {
    process.stderr.write(
      'src/api/schema.gen.ts is out of date with docs/protocol/openapi.json.\n' +
        'Run `npm run gen:api` in apps/web and commit the result.\n',
    )
    process.exit(1)
  }
  process.stdout.write('src/api/schema.gen.ts matches docs/protocol/openapi.json\n')
} finally {
  rmSync(dir, { recursive: true, force: true })
}
