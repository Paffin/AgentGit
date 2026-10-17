import { bootWorld } from './support/boot.ts'

/** Starts the real server with the demo world; the returned function stops everything. */
export default async function globalSetup(): Promise<() => Promise<void>> {
  const started = Date.now()
  const world = await bootWorld()
  process.stdout.write(
    `e2e: ag-server ${world.server.baseURL} seeded in ${((Date.now() - started) / 1000).toFixed(1)}s\n`,
  )
  return () => world.stop()
}
