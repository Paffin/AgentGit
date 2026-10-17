// Starts a real ag-server on a random loopback port with a throw-away data directory.
// Hermetic: no network except 127.0.0.1; the process is killed (and the directory removed) on
// stop(), and also when the test process exits for any reason.

import { spawn, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { AgentGitClient, type AgentGitClientOptions } from '../../src/index.js';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../..');

export function findServerBinary(): string {
  const candidates = [
    process.env['AG_SERVER_BIN'],
    '/home/user/bin/ag-server',
    path.join(REPO_ROOT, 'target/debug/ag-server'),
    path.join(REPO_ROOT, 'target/release/ag-server'),
  ].filter((c): c is string => Boolean(c));
  for (const c of candidates) if (existsSync(c)) return c;
  throw new Error(
    `ag-server binary not found (looked in: ${candidates.join(', ')}). Build it with \`cargo build -p ag-server\` or set AG_SERVER_BIN.`,
  );
}

const running = new Set<RunningServer>();
process.once('exit', () => {
  for (const s of running) s.killNow();
});

export interface StartOptions {
  /** Extra TOML written to a config file (for example `[limits]\nrequests_per_second = 5`). */
  config?: string;
}

export class RunningServer {
  readonly baseUrl: string;
  readonly bootstrapToken: string;
  readonly dataDir: string;
  #child: ChildProcess;
  #exited: Promise<void>;

  constructor(
    child: ChildProcess,
    baseUrl: string,
    token: string,
    dataDir: string,
    log: () => string,
    exited: Promise<void>,
  ) {
    this.#child = child;
    this.baseUrl = baseUrl;
    this.bootstrapToken = token;
    this.dataDir = dataDir;
    this.#exited = exited;
    this.logText = log;
  }

  logText: () => string;

  /** A client with the bootstrap administrator credential. */
  admin(options: Partial<AgentGitClientOptions> = {}): AgentGitClient {
    return new AgentGitClient({ baseUrl: this.baseUrl, token: this.bootstrapToken, ...options });
  }

  /** A client without credentials. */
  anonymous(options: Partial<AgentGitClientOptions> = {}): AgentGitClient {
    return new AgentGitClient({ baseUrl: this.baseUrl, ...options });
  }

  /**
   * Grants `role` on `repo` to `subject` (idempotent) and returns a client that authenticates as
   * `subject` with a fresh scoped credential.
   */
  async actor(
    subject: string,
    repo: string,
    role: 'reader' | 'writer' | 'reviewer' | 'integrator' | 'runner' | 'admin',
    options: Partial<AgentGitClientOptions> & {
      ttlSecs?: number;
      scope?: { repos?: string[]; streams?: string[]; operations?: string[] };
    } = {},
  ): Promise<AgentGitClient> {
    const { ttlSecs, scope, ...clientOptions } = options;
    const admin = this.admin();
    await admin.roles.grant({ subject, repo, role });
    const minted = await admin.credentials.mint({ subject, ttl_secs: ttlSecs ?? 900, ...(scope ? { scope } : {}) });
    return new AgentGitClient({ baseUrl: this.baseUrl, token: minted.token, ...clientOptions });
  }

  killNow(): void {
    if (this.#child.exitCode === null && this.#child.signalCode === null) this.#child.kill('SIGKILL');
    rmSync(this.dataDir, { recursive: true, force: true });
  }

  async stop(): Promise<void> {
    running.delete(this);
    if (this.#child.exitCode === null && this.#child.signalCode === null) {
      this.#child.kill('SIGTERM');
      const timer = setTimeout(() => this.#child.kill('SIGKILL'), 5000);
      await this.#exited;
      clearTimeout(timer);
    }
    rmSync(this.dataDir, { recursive: true, force: true });
  }
}

// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-9;]*m/g;

export async function startServer(options: StartOptions = {}): Promise<RunningServer> {
  const binary = findServerBinary();
  const dataDir = mkdtempSync(path.join(tmpdir(), 'agentgit-sdk-ts-'));
  const token = randomBytes(24).toString('hex');
  const args = ['serve', '--data-dir', path.join(dataDir, 'data'), '--listen', '127.0.0.1:0'];
  if (options.config) {
    const file = path.join(dataDir, 'server.toml');
    writeFileSync(file, options.config);
    args.push('--config', file);
  }
  const child = spawn(binary, args, {
    env: { ...process.env, AG_BOOTSTRAP_TOKEN: token, NO_COLOR: '1', RUST_BACKTRACE: '0' },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let log = '';
  const exited = new Promise<void>((resolve) => child.once('exit', () => resolve()));
  const address = new Promise<string>((resolve, reject) => {
    const onData = (chunk: Buffer): void => {
      log += chunk.toString();
      const m = /(?:addr=|listening[^\n]*?)(127\.0\.0\.1:\d+)/.exec(log.replace(ANSI, ''));
      if (m) resolve(m[1]!);
    };
    child.stdout?.on('data', onData);
    child.stderr?.on('data', onData);
    child.once('exit', (code) => reject(new Error(`ag-server exited early (code ${code}):\n${log}`)));
    child.once('error', reject);
    setTimeout(() => reject(new Error(`ag-server did not report its address within 20 s:\n${log}`)), 20_000).unref();
  });
  let addr: string;
  try {
    addr = await address;
  } catch (e) {
    child.kill('SIGKILL');
    rmSync(dataDir, { recursive: true, force: true });
    throw e;
  }
  const server = new RunningServer(child, `http://${addr}`, token, dataDir, () => log.replace(ANSI, ''), exited);
  running.add(server);
  // Ready means /healthz answers.
  for (let i = 0; i < 100; i++) {
    try {
      const r = await fetch(`${server.baseUrl}/healthz`);
      if (r.ok) return server;
    } catch {
      /* not yet */
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  await server.stop();
  throw new Error(`ag-server never became healthy:\n${log}`);
}
