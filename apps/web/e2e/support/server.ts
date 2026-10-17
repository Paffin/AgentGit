// Starts a REAL ag-server (never a mock) serving the built UI, for the end-to-end tests.
import { spawn } from 'node:child_process'
import type { ChildProcess } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

export interface RunningServer {
  baseURL: string
  adminToken: string
  dataDir: string
  stop(): Promise<void>
}

/** The prebuilt server binary: AG_SERVER_BIN, else the dev machine's, else a local cargo build. */
export function serverBinary(): string {
  const candidates = [
    process.env.AG_SERVER_BIN,
    '/home/user/bin/ag-server',
    resolve(import.meta.dirname, '../../../../target/debug/ag-server'),
    resolve(import.meta.dirname, '../../../../target/release/ag-server'),
  ].filter((p): p is string => !!p)
  const found = candidates.find((p) => existsSync(p))
  if (!found) {
    throw new Error(
      `No ag-server binary found. Set AG_SERVER_BIN. Looked in:\n  ${candidates.join('\n  ')}`,
    )
  }
  return found
}

async function freePort(): Promise<number> {
  return new Promise((res, rej) => {
    const s = createServer()
    s.once('error', rej)
    s.listen(0, '127.0.0.1', () => {
      const addr = s.address()
      const port = typeof addr === 'object' && addr ? addr.port : 0
      s.close(() => res(port))
    })
  })
}

async function waitHealthy(baseURL: string, child: ChildProcess, log: () => string): Promise<void> {
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (child.exitCode !== null)
      throw new Error(`ag-server exited early (${child.exitCode}):\n${log()}`)
    try {
      const r = await fetch(`${baseURL}/healthz`)
      if (r.ok) return
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 100))
  }
  throw new Error(`ag-server did not become healthy:\n${log()}`)
}

export interface ServerOptions {
  /**
   * Where the UI comes from. A directory (`[ui] dir`, the override), or `'embedded'`: no UI configuration at all,
   * so the server answers with the build compiled into its own binary (the production path).
   */
  ui: string | 'embedded'
  /** Extra TOML appended to the configuration (after the sections the suite always writes). */
  extraConfig?: string[]
}

/** The UI source the suite runs against: AG_E2E_UI=embedded (CI) or the freshly built directory (default). */
export function defaultUi(webDir: string): string {
  return process.env.AG_E2E_UI === 'embedded' ? 'embedded' : webDir
}

/**
 * Configuration that makes the server trust an identity provider and the UI offer redirect sign-in with it.
 * Every OIDC user belongs to `staff` (a reader of acme/*); `platform` members are administrators.
 */
export function oidcConfig(issuer: string, clientId: string): string[] {
  return [
    '[[auth.oidc]]',
    `issuer = ${JSON.stringify(issuer)}`,
    `audience = ${JSON.stringify(clientId)}`,
    `jwks_uri = ${JSON.stringify(`${issuer}/jwks`)}`,
    'algorithms = ["EdDSA"]',
    'group_roles = [',
    '  { group = "staff", role = "reader", repo = "acme/*" },',
    '  { group = "platform", role = "admin", repo = "acme/*" },',
    ']',
    '',
    '[[ui.login]]',
    `issuer = ${JSON.stringify(issuer)}`,
    `client_id = ${JSON.stringify(clientId)}`,
    'name = "Fake SSO"',
    '',
  ]
}

export async function startServer(opts: ServerOptions): Promise<RunningServer> {
  const webDir = opts.ui
  if (webDir !== 'embedded' && !existsSync(join(webDir, 'index.html'))) {
    throw new Error(
      `${webDir}/index.html is missing: run \`npm run build\` first (npm run test:e2e does it for you).`,
    )
  }
  const dir = mkdtempSync(join(tmpdir(), 'agentgit-e2e-'))
  const dataDir = join(dir, 'data')
  mkdirSync(dataDir, { recursive: true })
  const port = await freePort()
  const baseURL = `http://127.0.0.1:${port}`
  const adminToken = `e2e-bootstrap-${'x'.repeat(8)}-${Math.random().toString(36).slice(2)}-0123456789abcdef`
  const config = join(dir, 'ag-server.toml')
  writeFileSync(
    config,
    [
      '[server]',
      `listen = "127.0.0.1:${port}"`,
      `data_dir = ${JSON.stringify(dataDir)}`,
      '',
      ...(webDir === 'embedded' ? [] : ['[ui]', `dir = ${JSON.stringify(webDir)}`, '']),
      '[limits]',
      // The suite is not a load test, so the limiter is off unless asked for (AG_E2E_RPS / AG_E2E_BURST), e.g. to
      // watch the UI ride out real 429 answers. The client's retry rules are unit-tested separately.
      `requests_per_second = ${Number(process.env.AG_E2E_RPS ?? 0)}`,
      ...(process.env.AG_E2E_BURST ? [`burst = ${Number(process.env.AG_E2E_BURST)}`] : []),
      '',
      '[metrics]',
      'enabled = false',
      '',
      ...(opts.extraConfig ?? []),
    ].join('\n'),
  )
  let output = ''
  const child = spawn(serverBinary(), ['serve', '--config', config], {
    env: { ...process.env, AG_BOOTSTRAP_TOKEN: adminToken },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  child.stdout?.on('data', (d: Buffer) => (output = (output + d.toString()).slice(-8000)))
  child.stderr?.on('data', (d: Buffer) => (output = (output + d.toString()).slice(-8000)))
  await waitHealthy(baseURL, child, () => output)
  return {
    baseURL,
    adminToken,
    dataDir,
    async stop() {
      child.kill('SIGTERM')
      await new Promise<void>((res) => {
        const t = setTimeout(() => {
          child.kill('SIGKILL')
          res()
        }, 5000)
        child.once('exit', () => {
          clearTimeout(t)
          res()
        })
      })
      rmSync(dir, { recursive: true, force: true })
    },
  }
}
