// Starts everything the end-to-end tests (and `npm run demo:server`) stand on: a real ag-server serving
// the built UI, the demo world, the CI stand-in and the heartbeat keeper.
import { mkdirSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { seedDemo, seedEmpty, seedHostile } from './seed.ts'
import type { Demo } from './seed.ts'
import { startIdp } from './idp.ts'
import { defaultUi, oidcConfig, startServer } from './server.ts'
import type { RunningServer } from './server.ts'
import { RUNTIME_DIR, WEB_ROOT, writeState } from './state.ts'
import type { State } from './state.ts'
import { bootstrapCast, startCiWorker, startHeartbeats } from './world.ts'
import type { Cast, HeartbeatKeeper, Worker } from './world.ts'

export interface World {
  server: RunningServer
  cast: Cast
  demo: Demo
  hostile: { proposal: string }
  worker: Worker
  beats: HeartbeatKeeper
  state: State
  stop(): Promise<void>
}

export async function bootWorld(webDir = join(WEB_ROOT, 'dist')): Promise<World> {
  rmSync(RUNTIME_DIR, { recursive: true, force: true })
  mkdirSync(RUNTIME_DIR, { recursive: true })
  // The identity provider comes first: the server's configuration names it (it fetches its keys from it).
  const idp = await startIdp()
  const server = await startServer({
    ui: defaultUi(webDir),
    extraConfig: oidcConfig(idp.issuer, idp.clientId),
  })
  let worker: Worker | undefined
  let beats: HeartbeatKeeper | undefined
  const stop = async () => {
    await beats?.stop()
    await worker?.stop()
    await server.stop()
    await idp.stop()
  }
  try {
    const cast = await bootstrapCast(server.baseURL, server.adminToken)
    worker = startCiWorker({
      baseURL: server.baseURL,
      token: cast.token('ci'),
      runtimeDir: RUNTIME_DIR,
    })
    beats = startHeartbeats(server.baseURL)
    const demo = await seedDemo(cast, beats)
    const hostile = await seedHostile(cast)
    await seedEmpty(cast)
    const state: State = {
      baseURL: server.baseURL,
      tokens: cast.tokens,
      demo,
      hostile,
      idp: { issuer: idp.issuer, clientId: idp.clientId },
    }
    writeState(state)
    return { server, cast, demo, hostile, worker, beats, state, stop }
  } catch (e) {
    await stop()
    throw e
  }
}
