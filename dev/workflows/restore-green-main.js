export const meta = {
  name: 'restore-green-main',
  description: 'Diagnose and fix every red CI job on main (Rust CI, SDK+Go, containers/packaging/Windows), then verify in real GitHub Actions; repeat up to 3 rounds',
  phases: [
    { title: 'Fix', detail: 'three fixers in isolated worktrees, each owning a failure group, landing on main' },
    { title: 'Verify', detail: 'a verifier waits for the CI runs of main HEAD and reports per-job conclusions' },
  ],
}

const REPO = 'Paffin/AgentGit'
const COMMON = `FIRST read dev/agent-brief.md completely and follow it (rules, environment, integrating to main, final report) — it is in the repo root of your worktree. Context: main went RED after several agents landed work in parallel. Your job is to make your group of failures green on main, with real fixes (no skipping/ignoring tests, no loosening assertions, no mocks): diagnose from the real CI logs, reproduce locally where possible, fix the root cause, add/keep tests, land on main (rebase first; other fixers are landing too), and confirm in Actions.

How to read CI logs (the inline tail of get_job_logs is useless): load the tool with ToolSearch ("select:mcp__github__get_job_logs,mcp__github__actions_list,mcp__github__actions_get"), call get_job_logs with job_id (WITHOUT return_content) to get a URL, then \`curl -sL "<url>" -o /tmp/job.log\` (use your scratch dir) and grep: \`grep -n -E "error(\\[|:)|FAILED|panicked|failures:|##\\[error\\]" job.log | head -50\` then read around the hits. Public repo API without auth also works for run/job METADATA via curl https://api.github.com/repos/${REPO}/actions/runs?per_page=5&branch=main (60 requests/hour — be frugal; prefer the MCP tools).
Run ids at the red commit c69526d: CI run 39078224139, Windows run 39078224125; at 7335ccd: Containers run 39078196217, Packaging run 39078196181. Disk is tight (~5 GB free): no cargo clean, no new big directories; never leave main red.`

const GROUPS = [
  {
    key: 'rust',
    brief: `YOUR GROUP: "rust" — the Rust jobs of CI (run 39078224139).
 * job "MSRV (1.89)" (id 117529696680): \`cargo check --workspace --all-targets\` on toolchain 1.89.0 fails. Find the error. Fix the code or the dependency choice so it compiles on 1.89 (do NOT raise rust-version unless a dependency truly forces it; if you must, change Cargo.toml rust-version, ci.yml, README badge, docs consistently and justify in the commit).
 * job "fmt + clippy + rustdoc" (id 117529696697): fmt and clippy pass, \`cargo doc --workspace --no-deps\` (RUSTDOCFLAGS=-D warnings) fails: fix every rustdoc warning/error.
 * job "test (macos-latest)" (id 117529696778): \`spawn::tests::the_child_never_touches_the_allocator\` in ag-sandbox fails on macOS (and look for any OTHER failing tests in that log). ag-sandbox's namespace/seccomp parts are Linux-only by design (docs/architecture/sandbox.md); decide correctly whether the test is Linux-specific (then cfg-gate it with a recorded reason) or whether the code is wrong on macOS (then fix the code). Make the whole macOS test job green.
Verify locally with \`cargo check --workspace --all-targets\`, \`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps\`, \`cargo clippy --workspace --all-targets -- -D warnings\`, targeted tests; install a 1.89 toolchain only if disk allows (\`rustup toolchain install 1.89.0 --profile minimal\`, then remove it).`,
  },
  {
    key: 'sdk-go',
    brief: `YOUR GROUP: "sdk-go" — job "client SDKs (TypeScript, Python)" (id 117529696798; the TypeScript step fails after 5 s, most likely generated clients out of date after OpenAPI/proto changes: /v1/auth/config, ui, control plane…) and job "agent orchestrator (Go)" (id 117529696704; step "vet, format, race tests (the integration suite runs a real ag-server)").
Fix the root causes: regenerate the SDKs with their generators (sdk/typescript, sdk/python — see their package scripts/README and sdk/api-policy.json), make \`generate:check\`, types, lint, tests against the real server, packaging, Python tests/ruff/mypy and the docs code-sample check green; for Go find why vet/gofmt/race tests fail (server behaviour changes? gofmt version? Go toolchain 1.27 in the Dockerfile vs go.mod? a real race?) and fix properly. Reproduce locally: build ag-server and ag-cli (shared cargo target), then run the SDK and Go suites against them as CI does (read .github/workflows/ci.yml for the exact commands and env).`,
  },
  {
    key: 'containers',
    brief: `YOUR GROUP: "containers" — (1) CI job "container images build" (run 39078224139, job 117529696666; step "Build the server image" fails) and the Containers workflow (run 39078196217) and Packaging workflow (run 39078196181, failed at 7335ccd) and the Windows workflow (run 39078224125, failed at c69526d).
Probable cause of (1): ag-server now embeds the Web UI (crate crates/ag-ui-assets, feature "ui", scripts/build_ui.sh) but deploy/docker/Dockerfile does not build the UI before \`cargo build --release -p ag-server\` (it needs a Node 22 stage producing apps/web/dist copied to the path the embed script expects; keep the allow-listed build context, cargo-chef layers, non-root, healthcheck, small size). Fix the Dockerfile (and anything else the build/CI assumes), then check every consumer of the image (containers.yml, release.yml, compose, kubernetes overlays, docs/operations/deployment.md) still matches. ci.yml's "images" job duplicates containers.yml — remove the duplicate if containers.yml fully covers it (keep one source of truth).
For Packaging and Windows: read their failing jobs, fix root causes (no ignoring tests without a recorded platform reason). Windows: the last known state is in dev/handoff/windows.md; failures there may be real portability bugs.
You cannot dispatch workflows (403), but pushing to main triggers Containers (paths deploy/**, crates/**, Cargo.*), Packaging and Windows — iterate by pushing small commits and reading the runs. Wait for runs with a Bash until-loop polling the public API, not sleep.`,
  },
]

const RESULT = {
  type: 'object',
  properties: {
    group: { type: 'string' },
    fixed: { type: 'boolean' },
    commits: { type: 'array', items: { type: 'string' } },
    root_causes: { type: 'string' },
    still_red: { type: 'string' },
    evidence: { type: 'string' },
  },
  required: ['group', 'fixed', 'commits', 'root_causes', 'still_red', 'evidence'],
}
const VERIFY = {
  type: 'object',
  properties: {
    head_sha: { type: 'string' },
    all_green: { type: 'boolean' },
    workflows: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          name: { type: 'string' },
          conclusion: { type: 'string' },
          failing_jobs: { type: 'array', items: { type: 'string' } },
        },
        required: ['name', 'conclusion', 'failing_jobs'],
      },
    },
    failing_groups: { type: 'array', items: { type: 'string', enum: ['rust', 'sdk-go', 'containers'] } },
    notes: { type: 'string' },
  },
  required: ['head_sha', 'all_green', 'workflows', 'failing_groups', 'notes'],
}

const history = []
let pending = GROUPS
let verdict = null
for (let round = 1; round <= 3 && pending.length > 0; round++) {
  log(`round ${round}: fixing ${pending.map(g => g.key).join(', ')}`)
  const extra = verdict
    ? `\n\nThis is round ${round}. The previous round's verification says (head ${verdict.head_sha}): ${JSON.stringify(verdict.workflows.filter(w => w.conclusion !== 'success'))}\nNotes: ${verdict.notes}`
    : ''
  const res = await parallel(pending.map(g => () => agent(
    `${COMMON}\n\n${g.brief}${extra}`,
    { label: `fix:${g.key}:r${round}`, phase: 'Fix', isolation: 'worktree', schema: RESULT },
  )))
  history.push({ round, results: res.filter(Boolean) })

  verdict = await agent(
    `You are the CI verifier for ${REPO} (read-only; no code changes). Fixers just landed commits on main. 1) \`git fetch origin main\` in your worktree and take origin/main HEAD sha. 2) Wait (Bash until-loop polling \`curl -s "https://api.github.com/repos/${REPO}/actions/runs?head_sha=<sha>&per_page=30"\`, sleeping inside the loop, up to ~50 minutes; if a newer commit lands on main meanwhile, restart with the new HEAD) until ALL workflow runs for that sha are completed: CI, Web UI, Windows, Containers, Packaging (others may exist; some are path-filtered and legitimately absent — a workflow absent for a sha is fine). Runs 'cancelled' because a newer push superseded them are not failures: follow the newest HEAD. 3) For every run not 'success' list the failing job names (jobs endpoint /actions/runs/<id>/jobs). 4) Map failing jobs to groups: rust = MSRV / fmt+clippy+rustdoc / test (ubuntu|macos) / backends / docs+demo / deployment files; sdk-go = client SDKs / agent orchestrator (Go); containers = container images / Containers / Packaging / Windows workflow. Web UI failures: report in notes (no group). Return all_green only if every completed run for the final HEAD is success (continue-on-error steps are fine).`,
    { label: `verify:r${round}`, phase: 'Verify', schema: VERIFY },
  )
  pending = GROUPS.filter(g => verdict && verdict.failing_groups.includes(g.key))
  if (verdict && verdict.all_green) pending = []
}
return { rounds: history, final: verdict }
