// `npm run demo:server`: a real ag-server with the demo world and the built UI, kept running so you can
// click through the same data the end-to-end tests use. Needs `npm run build` first.
import { bootWorld } from './support/boot.ts'
import { PEOPLE } from './support/world.ts'

process.env.AG_E2E_VERBOSE = '1'
const world = await bootWorld()
const out = process.stdout
out.write(`\nAgentGit demo: ${world.server.baseURL}\n\nSign in with one of these tokens:\n`)
for (const name of ['admin', ...Object.keys(PEOPLE)]) {
  out.write(`  ${name.padEnd(8)} ${world.cast.token(name)}\n`)
}
out.write('\nCtrl-C stops the server and deletes its data.\n')
const shutdown = () => void world.stop().finally(() => process.exit(0))
process.on('SIGINT', shutdown)
process.on('SIGTERM', shutdown)
setInterval(() => undefined, 1 << 30)
