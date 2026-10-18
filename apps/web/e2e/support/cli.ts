// Runs shell commands the way a person would paste them: `ag` is the REAL command-line client, `curl` the real curl.
import { spawn } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, symlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

/** The CLI binary: AG_CLI_BIN, else the dev machine's, else a local cargo build. */
export function cliBinary(): string {
  const candidates = [
    process.env.AG_CLI_BIN,
    '/home/user/bin/ag',
    resolve(import.meta.dirname, '../../../../target/debug/ag'),
    resolve(import.meta.dirname, '../../../../target/release/ag'),
  ].filter((p): p is string => !!p)
  const found = candidates.find((p) => existsSync(p))
  if (!found) {
    throw new Error(`No ag binary found. Set AG_CLI_BIN. Looked in:\n  ${candidates.join('\n  ')}`)
  }
  return found
}

export interface Shell {
  /** A scratch directory (home of the commands, and their working directory). */
  dir: string
  /** Runs `script` with bash -e; resolves to its output and fails the test on a non-zero exit. */
  run(script: string, env?: Record<string, string>): Promise<string>
}

/** A shell whose PATH has `ag` first and whose HOME is private, so nothing touches the machine's own settings. */
export function newShell(): Shell {
  const dir = mkdtempSync(join(tmpdir(), 'agentgit-shell-'))
  const bin = join(dir, 'bin')
  mkdirSync(bin)
  symlinkSync(cliBinary(), join(bin, 'ag'))
  const home = join(dir, 'home')
  mkdirSync(home)
  const work = join(dir, 'work')
  mkdirSync(work)
  return {
    dir,
    run(script, env = {}) {
      return new Promise((res, rej) => {
        const child = spawn('bash', ['-e', '-o', 'pipefail', '-c', script], {
          cwd: work,
          env: {
            PATH: `${bin}:${process.env.PATH ?? ''}`,
            HOME: home,
            ...env,
          },
          stdio: ['ignore', 'pipe', 'pipe'],
        })
        let out = ''
        let err = ''
        child.stdout.on('data', (d: Buffer) => (out += d.toString()))
        child.stderr.on('data', (d: Buffer) => (err += d.toString()))
        child.once('error', rej)
        child.once('exit', (code) =>
          code === 0
            ? res(out)
            : rej(
                new Error(`exit ${code} for:\n${script}\n--- stdout\n${out}\n--- stderr\n${err}`),
              ),
        )
      })
    },
  }
}
