// Typed wrappers for the HTTP/JSON API (docs/protocol/openapi.json). One function per operation the UI uses.
// Request bodies are the generated request types with every field optional; responses are the generated
// response types. Free-form names (streams, revisions, paths) always travel in the query string or body.
import { filePath, repoPath } from './client'
import type { ApiClient, RequestOptions } from './client'
import type {
  Agent,
  Issue,
  IssueActivity,
  AuditEntry,
  AuthConfig,
  Capabilities,
  Changeset,
  Credential,
  DiffResponse,
  SearchTreeResponse,
  DirEntry,
  FsckResponse,
  GcResponse,
  IntegrationResult,
  Lease,
  Operation,
  Overlap,
  Pin,
  PolicyVersion,
  Preparation,
  Principal,
  Proposal,
  ProposalDetail,
  ProvenanceEntry,
  QueueEntry,
  ReflogEntry,
  Repo,
  Req,
  Review,
  RoleBinding,
  Run,
  ServerEvent,
  Stream,
  Task,
  TaskScheduling,
  TreeEntry,
  Attestation,
  FileChange,
  SyncWorkspaceResponse,
  Workspace,
  BlameResponse,
  Job,
  Page,
  Quota,
  CredentialScope,
} from './types'

type Opts = Pick<RequestOptions, 'signal' | 'timeoutMs' | 'runId' | 'idempotencyKey'>
// Creation spans two requests; keep both tied to the initiating session.
type SessionOpts = Opts & Pick<RequestOptions, 'token' | 'quietUnauthenticated'>

export function createApi(c: ApiClient) {
  const r = repoPath
  return {
    client: c,

    // ---- session --------------------------------------------------------------------------------
    capabilities: (o?: Opts) =>
      c.get<Capabilities>('/v1/capabilities', undefined, { ...o, token: null }),
    /** How a browser can sign in (public): `token`, or the identity providers to redirect to. */
    authConfig: (o?: Opts) =>
      c.get<AuthConfig>('/v1/auth/config', undefined, { ...o, token: null }),
    /** Checks a pasted token. A 401 here means "wrong token", not "your session ended". */
    whoami: (token?: string | null, o?: Opts) =>
      c.get<Principal>('/v1/whoami', undefined, {
        ...o,
        token,
        quietUnauthenticated: token !== undefined,
        retries: token !== undefined ? 1 : undefined,
      }),

    // ---- repositories ---------------------------------------------------------------------------
    repos: {
      list: async (
        q: { prefix?: string; page_size?: number; page_token?: string } = {},
        o?: Opts,
      ): Promise<Page<Repo>> => {
        const res = await c.get<{ repos: Repo[]; next_page_token: string }>('/v1/repos', q, o)
        return { items: res.repos ?? [], nextPageToken: res.next_page_token ?? '' }
      },
      get: (repo: string, o?: Opts) => c.get<Repo>(r(repo), undefined, o),
      create: (
        body: { name: string; description?: string; default_stream?: string },
        o?: SessionOpts,
      ) => c.post<Repo>('/v1/repos', body, o),
      configure: (repo: string, body: { description?: string; quota?: Partial<Quota> }, o?: Opts) =>
        c.patch<Repo>(r(repo), body, o),
    },

    // ---- streams --------------------------------------------------------------------------------
    streams: {
      list: async (repo: string, o?: Opts) =>
        (await c.get<{ streams: Stream[] }>(r(repo, '/streams'), undefined, o)).streams ?? [],
      head: (repo: string, stream: string, o?: Opts) =>
        c.get<Stream>(r(repo, '/streams/head'), { stream }, o),
      create: (repo: string, body: { name: string; from_rev?: string }, o?: Opts) =>
        c.post<Stream>(r(repo, '/streams'), body, o),
      reflog: async (repo: string, stream: string, limit = 100, o?: Opts) =>
        (await c.get<{ entries: ReflogEntry[] }>(r(repo, '/reflog'), { stream, limit }, o))
          .entries ?? [],
    },

    // ---- history & trees ------------------------------------------------------------------------
    history: {
      log: async (
        repo: string,
        q: { rev: string; path?: string; page_size?: number; page_token?: string },
        o?: Opts,
      ): Promise<Page<Changeset>> => {
        const res = await c.get<{ changesets: Changeset[]; next_page_token: string }>(
          r(repo, '/log'),
          q,
          o,
        )
        return { items: res.changesets ?? [], nextPageToken: res.next_page_token ?? '' }
      },
      changeset: (repo: string, rev: string, includeOps = false, o?: Opts) =>
        c.get<Changeset>(r(repo, '/changeset'), { rev, include_ops: includeOps }, o),
      resolve: async (repo: string, rev: string, o?: Opts) =>
        (await c.get<{ changeset_id: string }>(r(repo, '/history/resolve'), { rev }, o))
          .changeset_id,
      /** Who last changed each line. Follows renames; lines whose origin is older than the walk are `boundary`. */
      blame: (
        repo: string,
        q: {
          path: string
          rev?: string
          first_line?: number
          last_line?: number
          max_commits?: number
        },
        o?: Opts,
      ) => c.get<BlameResponse>(r(repo, '/blame'), q, o),
      mergeBase: async (repo: string, a: string, b: string, o?: Opts) =>
        (await c.get<{ bases: string[] }>(r(repo, '/history/merge-base'), { a, b }, o)).bases ?? [],
    },
    tree: {
      search: (
        repo: string,
        q: {
          rev: string
          query: string
          case_sensitive?: boolean
          path_prefix?: string
          page_size?: number
          page_token?: string
        },
        o?: SessionOpts,
      ) => c.get<SearchTreeResponse>(r(repo, '/search'), q, o),
      manifest: async (
        repo: string,
        q: { rev: string; page_size?: number; page_token?: string },
        o?: Opts,
      ) => {
        const res = await c.get<{
          entries: TreeEntry[]
          next_page_token: string
          total_entries: number
        }>(r(repo, '/tree'), q, o)
        return {
          entries: res.entries ?? [],
          nextPageToken: res.next_page_token ?? '',
          total: res.total_entries ?? 0,
        }
      },
      list: async (
        repo: string,
        q: { rev: string; path?: string; page_size?: number; page_token?: string },
        o?: Opts,
      ) => {
        const res = await c.get<{
          entries: DirEntry[]
          next_page_token: string
          tree_id: string
        }>(r(repo, '/tree/list'), q, o)
        return {
          entries: res.entries ?? [],
          nextPageToken: res.next_page_token ?? '',
          treeId: res.tree_id,
        }
      },
      resolve: (repo: string, rev: string, path: string, o?: Opts) =>
        c.get<{ kind: string; entry: TreeEntry | null }>(
          r(repo, '/tree/resolve'),
          { rev, path },
          o,
        ),
      /** File bytes, refusing to download more than `limit`. */
      file: (repo: string, rev: string, path: string, limit: number, o?: Opts) =>
        c.bytes(filePath(repo, path), { ...o, query: { rev }, limit }),
      diff: (
        repo: string,
        body: { from?: string; to: string; paths?: string[]; context?: number },
        o?: Opts,
      ) => c.post<DiffResponse>(r(repo, '/diff'), body, { ...o, readOnly: true }),
    },

    issues: {
      list: (
        repo: string,
        q: { state?: string; query?: string; page_size?: number; page_token?: string } = {},
        o?: SessionOpts,
      ) => c.get<{ issues: Issue[]; next_page_token: string }>(r(repo, '/issues'), q, o),
      get: (repo: string, number: number, o?: SessionOpts) =>
        c.get<Issue>(r(repo, `/issues/${number}`), undefined, o),
      create: (
        repo: string,
        body: { title: string; body: string; task_id?: string },
        o?: SessionOpts,
      ) => c.post<Issue>(r(repo, '/issues'), body, o),
      update: (
        repo: string,
        number: number,
        body: {
          title: string
          body: string
          state: string
          task_id: string
          expected_version: number
        },
        o?: SessionOpts,
      ) => c.json<Issue>(r(repo, `/issues/${number}`), { ...o, method: 'PUT', body }),
      comment: (repo: string, number: number, body: string, o?: SessionOpts) =>
        c.post<IssueActivity>(r(repo, `/issues/${number}/comments`), { body }, o),
      activity: (
        repo: string,
        number: number,
        q: { page_size?: number; page_token?: string } = {},
        o?: SessionOpts,
      ) =>
        c.get<{ activity: IssueActivity[]; next_page_token: string }>(
          r(repo, `/issues/${number}/activity`),
          q,
          o,
        ),
    },

    // ---- proposals ------------------------------------------------------------------------------
    proposals: {
      list: async (
        repo: string,
        q: {
          state?: string
          stream?: string
          author?: string
          page_size?: number
          page_token?: string
        } = {},
        o?: Opts,
      ): Promise<Page<Proposal>> => {
        const res = await c.get<{ proposals: Proposal[]; next_page_token: string }>(
          r(repo, '/proposals'),
          q,
          o,
        )
        return { items: res.proposals ?? [], nextPageToken: res.next_page_token ?? '' }
      },
      get: (repo: string, id: string, o?: Opts) =>
        c.get<ProposalDetail>(r(repo, `/proposals/${encodeURIComponent(id)}`), undefined, o),
      review: (
        repo: string,
        id: string,
        body: { decision: string; body?: string; expected_head?: string },
        o?: SessionOpts,
      ) => c.post<Review>(r(repo, `/proposals/${encodeURIComponent(id)}/reviews`), body, o),
      close: (repo: string, id: string, o?: Opts) =>
        c.post<Proposal>(r(repo, `/proposals/${encodeURIComponent(id)}/close`), {}, o),
      /** Moves a proposal to a newer head (only its author may). */
      update: (repo: string, id: string, newHead: string, o?: Opts) =>
        c.patch<Proposal>(
          r(repo, `/proposals/${encodeURIComponent(id)}`),
          { new_head: newHead },
          o,
        ),
      create: (
        repo: string,
        body: {
          stream: string
          head: string
          title: string
          description?: string
          task_id?: string
        },
        o?: SessionOpts,
      ) => c.post<Proposal>(r(repo, '/proposals'), body, o),
    },

    // ---- virtual workspaces: edit files without a clone -----------------------------------------
    workspaces: {
      create: (
        repo: string,
        body: { name: string; stream?: string; from_rev?: string },
        o?: SessionOpts,
      ) => c.post<Workspace>(r(repo, '/workspaces'), body, o),
      get: (repo: string, name: string, o?: Opts) =>
        c.get<Workspace>(r(repo, `/workspaces/${encodeURIComponent(name)}`), undefined, o),
      /** Applies the changes as ONE new changeset on top of the workspace head. */
      modify: (
        repo: string,
        name: string,
        body: { changes: FileChange[]; message?: string; expected_head?: string },
        o?: SessionOpts,
      ) => c.post<Workspace>(r(repo, `/workspaces/${encodeURIComponent(name)}/changes`), body, o),
      /** Brings the stream head in. Conflicts change nothing until every one has a resolution. */
      sync: (repo: string, name: string, resolutions: FileChange[] = [], o?: Opts) =>
        c.post<SyncWorkspaceResponse>(
          r(repo, `/workspaces/${encodeURIComponent(name)}/sync`),
          resolutions.length ? { resolutions } : {},
          o,
        ),
      remove: (repo: string, name: string, o?: SessionOpts) =>
        c.del<Record<string, never>>(
          r(repo, `/workspaces/${encodeURIComponent(name)}`),
          undefined,
          o,
        ),
    },

    // ---- integration ----------------------------------------------------------------------------
    integration: {
      /** Side-effect free unless `enqueue_validations` is set. */
      prepare: (repo: string, proposal: string, enqueueValidations = false, o?: Opts) =>
        c.post<Preparation>(
          r(repo, `/proposals/${encodeURIComponent(proposal)}/prepare`),
          enqueueValidations ? { enqueue_validations: true } : {},
          { ...o, readOnly: !enqueueValidations },
        ),
      integrate: (repo: string, proposal: string, fencingTokens: number[] = [], o?: Opts) =>
        c.post<IntegrationResult>(
          r(repo, `/proposals/${encodeURIComponent(proposal)}/integrate`),
          fencingTokens.length > 0 ? { fencing_tokens: fencingTokens } : {},
          o,
        ),
      validate: async (repo: string, proposal: string, checks: string[] = [], o?: Opts) =>
        (
          await c.post<{ jobs: Job[] }>(
            r(repo, `/proposals/${encodeURIComponent(proposal)}/validate`),
            checks.length > 0 ? { checks } : {},
            o,
          )
        ).jobs ?? [],
      attestations: async (repo: string, treeId: string, o?: Opts) =>
        (
          await c.get<{ attestations: Attestation[] }>(
            r(repo, '/attestations'),
            { tree_id: treeId },
            o,
          )
        ).attestations ?? [],
      queue: async (repo: string, stream: string, o?: Opts) =>
        (await c.get<{ entries: QueueEntry[] }>(r(repo, '/queue'), { stream }, o)).entries ?? [],
      enqueue: (repo: string, proposal: string, o?: Opts) =>
        c.post<QueueEntry>(r(repo, '/queue'), { proposal }, o),
      runQueue: (repo: string, stream: string, o?: Opts) =>
        c.post<{ processed: QueueEntry[]; stopped_at: string }>(
          r(repo, '/queue/run'),
          { stream },
          o,
        ),
      operation: (repo: string, q: { idempotency_key?: string; operation_id?: string }, o?: Opts) =>
        c.get<Operation>(r(repo, '/operations'), q, o),
    },

    // ---- agents ---------------------------------------------------------------------------------
    agents: {
      list: async (repo: string, o?: Opts) =>
        (await c.get<{ agents: Agent[] }>(r(repo, '/agents'), undefined, o)).agents ?? [],
      register: (
        repo: string,
        body: { name: string; public_key: string; role?: string },
        o?: Opts,
      ) => c.post<Agent>(r(repo, '/agents'), body, o),
      run: (repo: string, id: string, o?: SessionOpts) =>
        c.get<Run>(r(repo, `/runs/${encodeURIComponent(id)}`), undefined, o),
      runs: async (
        repo: string,
        q: { agent?: string; active_only?: boolean; limit?: number } = {},
        o?: Opts,
      ) => (await c.get<{ runs: Run[] }>(r(repo, '/runs'), q, o)).runs ?? [],
      scheduling: (repo: string, o?: SessionOpts) =>
        c.get<TaskScheduling>(r(repo, '/tasks/scheduling'), undefined, o),
      setScheduling: (
        repo: string,
        body: { paused: boolean; reason: string; expected_version: number },
        o?: SessionOpts,
      ) => c.json<TaskScheduling>(r(repo, '/tasks/scheduling'), { ...o, method: 'PUT', body }),
      tasks: async (repo: string, q: { state?: string; ready_only?: boolean } = {}, o?: Opts) =>
        (await c.get<{ tasks: Task[] }>(r(repo, '/tasks'), q, o)).tasks ?? [],
      task: (repo: string, id: string, o?: SessionOpts) =>
        c.get<Task>(r(repo, `/tasks/${encodeURIComponent(id)}`), undefined, o),
      addDependency: (repo: string, id: string, dependency: string, o?: SessionOpts) =>
        c.post<Task>(
          r(repo, `/tasks/${encodeURIComponent(id)}/dependencies`),
          { depends_on: dependency },
          o,
        ),
      createTask: (
        repo: string,
        body: {
          title: string
          description?: string
          scope?: string[]
          depends_on?: string[]
          source_issue?: number
          expected_issue_version?: number
        },
        o?: SessionOpts,
      ) => c.post<Task>(r(repo, '/tasks'), body, o),
      finishTask: (repo: string, id: string, state: 'done' | 'cancelled', o?: SessionOpts) =>
        c.post<Task>(r(repo, `/tasks/${encodeURIComponent(id)}/finish`), { state }, o),
      leases: async (repo: string, activeOnly = true, o?: SessionOpts) =>
        (await c.get<{ leases: Lease[] }>(r(repo, '/leases'), { active_only: activeOnly }, o))
          .leases ?? [],
      releaseLease: (repo: string, id: string, o?: Opts) =>
        c.del<Record<string, never>>(r(repo, `/leases/${encodeURIComponent(id)}`), undefined, o),
      extendLease: (repo: string, id: string, ttlMs: number, o?: Opts) =>
        c.post<Lease>(r(repo, `/leases/${encodeURIComponent(id)}/extend`), { ttl_ms: ttlMs }, o),
      provenance: async (repo: string, changeset: string, o?: Opts) =>
        (await c.get<{ entries: ProvenanceEntry[] }>(r(repo, '/provenance'), { changeset }, o))
          .entries ?? [],
      predict: async (repo: string, stream: string, o?: Opts) =>
        (await c.get<{ overlaps: Overlap[] }>(r(repo, '/predict'), { stream }, o)).overlaps ?? [],
    },

    // ---- events ---------------------------------------------------------------------------------
    events: {
      replay: async (
        repo: string,
        q: { after_seq?: number; limit?: number; topics?: string[] },
        o?: Opts,
      ) => {
        const res = await c.get<{ events: ServerEvent[]; latest_seq: number }>(
          r(repo, '/events/replay'),
          { after_seq: q.after_seq ?? 0, limit: q.limit, topics: q.topics },
          o,
        )
        return { events: res.events ?? [], latestSeq: res.latest_seq ?? 0 }
      },
    },

    // ---- administration -------------------------------------------------------------------------
    admin: {
      policy: (repo: string, stream: string, o?: Opts) =>
        c.get<PolicyVersion>(r(repo, '/policy'), { stream }, o),
      setPolicy: (repo: string, stream: string, document: string, o?: SessionOpts) =>
        c.put<PolicyVersion>(r(repo, '/policy'), { stream, document }, o),
      policyHistory: async (repo: string, stream: string, o?: Opts) =>
        (await c.get<{ versions: PolicyVersion[] }>(r(repo, '/policy/history'), { stream }, o))
          .versions ?? [],
      pins: async (repo: string, o?: Opts) =>
        (await c.get<{ pins: Pin[] }>(r(repo, '/pins'), undefined, o)).pins ?? [],
      pin: (repo: string, body: { name: string; rev: string; reason?: string }, o?: Opts) =>
        c.post<Pin>(r(repo, '/pins'), body, o),
      unpin: (repo: string, name: string, o?: Opts) =>
        c.del<Record<string, never>>(r(repo, '/pins'), { name }, o),
      audit: async (repo: string, q: { contains?: string; limit?: number } = {}, o?: Opts) =>
        (await c.get<{ entries: AuditEntry[] }>(r(repo, '/audit'), q, o)).entries ?? [],
      verifyAudit: (repo: string, o?: Opts) =>
        c.get<{ ok: boolean; entries: number; detail: string }>(
          r(repo, '/audit/verify'),
          undefined,
          o,
        ),
      gc: (
        repo: string,
        body: { dry_run: boolean; grace_ms?: number; reflog_retention_ms?: number },
        o?: Opts,
      ) => c.post<GcResponse>(r(repo, '/gc'), body, { timeoutMs: 120_000, ...o }),
      fsck: (repo: string, o?: Opts) =>
        c.post<FsckResponse>(r(repo, '/fsck'), {}, { timeoutMs: 120_000, ...o }),
      roles: async (q: { subject?: string; repo?: string } = {}, o?: Opts) =>
        (await c.get<{ bindings: RoleBinding[] }>('/v1/roles', q, o)).bindings ?? [],
      grantRole: (body: { subject: string; repo: string; role: string }, o?: SessionOpts) =>
        c.post<RoleBinding>('/v1/roles', body, o),
      revokeRole: (q: { subject: string; repo: string; role: string }, o?: SessionOpts) =>
        c.del<Record<string, never>>('/v1/roles', q, o),
      credentials: async (q: { subject?: string; include_revoked?: boolean } = {}, o?: Opts) =>
        (await c.get<{ credentials: Credential[] }>('/v1/credentials', q, o)).credentials ?? [],
      mintCredential: (
        body: {
          subject?: string
          description?: string
          scope?: Partial<CredentialScope>
          ttl_secs: number
        },
        o?: SessionOpts,
      ) => c.post<{ credential: Credential; token: string }>('/v1/credentials', body, o),
      revokeCredential: (id: string, o?: Opts) =>
        c.del<{ revoked: boolean }>(`/v1/credentials/${encodeURIComponent(id)}`, undefined, o),
    },
  }
}

export type Api = ReturnType<typeof createApi>
export type { Req }
