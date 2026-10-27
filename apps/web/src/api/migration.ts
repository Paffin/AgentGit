import { client } from './instance'
import type { RequestOptions } from './client'

type CredentialOptions = Pick<RequestOptions, 'token'>
/** Pin retries to the initiating session; its caller handles a rejected credential locally. */
function credentials(options: CredentialOptions): RequestOptions {
  return { ...options, quietUnauthenticated: options.token !== undefined }
}

export interface MigrationRef {
  name: string
  destination: string
  git_commit: string
  changeset: string
}
export interface MigrationReport {
  default_branch: string
  commits: number
  blobs: number
  branches: MigrationRef[]
  tags: MigrationRef[]
  skipped_paths: [string, string][]
  skipped_submodules: number
  lossy_commits: number
  lfs_pointers: number
  other_refs: string[]
}
export interface MigrationPlan {
  id: string
  name: string
  source_sha256: string
  report: MigrationReport
  state: 'ready' | 'imported'
}
export function prepareMigration(
  name: string,
  file: File,
  key: string,
  options: CredentialOptions = {},
) {
  return client.json<MigrationPlan>('/v1/migrations/git', {
    ...credentials(options),
    method: 'PUT',
    query: { name },
    binary: file,
    idempotencyKey: key,
    timeoutMs: 300_000,
  })
}
export function readMigration(id: string, options: CredentialOptions = {}) {
  return client.get<MigrationPlan>(
    `/v1/migrations/${encodeURIComponent(id)}`,
    undefined,
    credentials(options),
  )
}
export function finishMigration(id: string, acceptLossy: boolean, options: CredentialOptions = {}) {
  return client.post<MigrationPlan>(
    `/v1/migrations/${encodeURIComponent(id)}/commit`,
    {
      accept_lossy: acceptLossy,
    },
    credentials(options),
  )
}
export function migrationWarnings(report: MigrationReport): string[] {
  const warnings: string[] = []
  if (report.skipped_paths.length)
    warnings.push(`${report.skipped_paths.length} unsupported paths were omitted.`)
  if (report.skipped_submodules)
    warnings.push(
      `${report.skipped_submodules} submodule entries were omitted. Migrate those repositories separately.`,
    )
  if (report.lfs_pointers)
    warnings.push(
      `${report.lfs_pointers} Git LFS pointers were copied without their external file contents. Keep your LFS storage available.`,
    )
  if (report.lossy_commits)
    warnings.push(`${report.lossy_commits} commit messages needed UTF-8 text repair.`)
  if (report.other_refs.length)
    warnings.push(
      `${report.other_refs.length} refs outside commit-based branches and tags are retained only in the source archive.`,
    )
  return warnings
}
/** Accept Git's copied SSH address without changing its remote path semantics. */
export function parseGitRemote(source: string): { remote: string; path: string } {
  const scp = /^([a-zA-Z0-9._-]+@[a-zA-Z0-9][a-zA-Z0-9.-]*):([^\s\p{Cc}]+)$/u.exec(source)
  if (scp) return { remote: source, path: scp[2]! }
  const url = new URL(source)
  if (
    !['https:', 'ssh:'].includes(url.protocol) ||
    (url.protocol === 'https:' && url.username) ||
    url.password
  )
    throw new Error('Use an HTTPS or SSH URL without embedded credentials.')
  return { remote: url.href, path: url.pathname }
}

/** POSIX shell quoting; remotes are never executed by the browser or sent to the server. */
export function bundleCommands(source: string): string {
  const { remote } = parseGitRemote(source)
  const quoted = `'${remote.replaceAll("'", "'\\''")}'`
  return `git clone --mirror -- ${quoted} agentgit-source.git\ngit -C agentgit-source.git bundle create ../repository.bundle --all`
}
