export const meta = {
  name: 'product-wave-2',
  description: 'Fix every confirmed audit finding (no mocks/stubs/hardcodes), finish the tails of the seven tracks, prove the rest in real CI, then fold docs; three sequential lanes, CI verified at the end',
  phases: [
    { title: 'Lanes', detail: 'A server (audit→hardening→HA→perf), B core/policy/clients/orchestrator, C docs/security/MCP/web' },
    { title: 'Verify', detail: 'CI verifier + generic fixers until main is green' },
    { title: 'Fold', detail: 'README/CHANGELOG/ROADMAP/traceability folded from handoffs; final verification' },
  ],
}

const REPO = 'Paffin/AgentGit'

const COMMON = `FIRST read, in your worktree, dev/agent-brief.md (rules, environment, integrating to main, final report) and dev/STATUS.md (state of the project). The owner's bar for this phase is "200% ready, with NO mocks, NO stubs, NO simulated behaviour and NO hardcoded values in product code": real components everywhere, configuration instead of literals, claims backed by tests/CI evidence or removed. Test doubles are acceptable ONLY for things that cannot be real in a test (a third-party service you cannot run), only in test code, and must be named as such in the docs; if the real thing can run (a real ag-server, a real PostgreSQL, a real S3 implementation, a real container runtime, a real OIDC IdP) the test must use it.
The audit lives in dev/audit/: CONFIRMED.md is the work list (both adversarial verifiers agreed) — fix every confirmed item of your areas with the verifier-refined fix, each with a test that fails without it; audit-final.json also holds CONTESTED items (status "contested", one verifier agreed): triage each in your areas — fix it, or record in your handoff why it stays (cite the code/doc that makes it legitimate). Items are verified at commit 5d35ee2; the code has moved — re-read before editing and drop items that no longer apply (say so in the handoff).
Rules specific to this phase: (1) main is GREEN now and must stay green: land small slices, rebase first, run the gates, push, then WAIT for the CI runs of your last commit (poll the public API https://api.github.com/repos/${REPO}/actions/runs?head_sha=<sha> in a Bash until-loop — foreground sleep is blocked; runs cancelled by a newer push are not failures — follow the newest HEAD) and fix anything red before you finish. Path-filtered workflows (Containers, Packaging, Web UI, Windows) run only when their paths change. You cannot dispatch workflows (403). (2) Write dev/handoff/<your-package>.md (state, evidence with run links, what is NOT verified, next steps) and land it. (3) Format/hash/merge-semantics/protocol changes need an ADR (docs/adr/) — avoid them unless a confirmed finding requires one. (4) Disk is tight: before finishing delete your worktree's node_modules/target leftovers and stale test executables: \`find /home/user/ag-target/debug/deps -maxdepth 1 -type f -perm -u+x ! -name '*.so' ! -name '*.d' ! -name '*.rlib' ! -name '*.rmeta' -size +2M -mmin +30 -delete\`. Never cargo clean. (5) Your final structured report must be honest: ci_green only if you saw the runs of your last pushed commit succeed.`

const WPS = {
  server_audit: {
    areas: 'server-security, server-api',
    brief: `Fix the confirmed audit items in crates/ag-server (areas server-security, server-api): credentials whose scope cannot be parsed must FAIL CLOSED (control.rs); object quota must be enforced wherever bytes enter a repository (virtual workspaces, all writers) not only on upload; SSE topic filtering must not skip events (filter in SQL / loop until limit) and the stream must end on credential revocation/expiry by re-validating each batch; every other confirmed/contested item (hardcoded limits advertised by the server must come from the engine's single source of truth, hardcoded host/port/timeouts → config, error-swallowing fallbacks, claims in comments/docs that code does not honour). Also from the release track: repo.create / repo list responses hardcode sqlite/fs backends — they must report the repository's real storage configuration. Add focused tests (use the real engine; the test harnesses in crates/ag-server/tests start real servers). Regenerate the OpenAPI golden if the API changes (AG_UPDATE_GOLDEN=1 cargo test -p ag-server --test observability) and the SDK clients (sdk/typescript, sdk/python generators) — the CI "client SDKs" job checks both.`,
  },
  server_hardening: {
    areas: 'server hardening from ROADMAP',
    brief: `Implement the ROADMAP "server hardening that the integration work exposed", for real, with tests and docs: (1) bind credentials to a RUN (today: to an agent name) so concurrent runs of one agent cannot act for each other — design in a short ADR (docs/adr/0009-run-bound-credentials.md): how a run-bound credential is minted, how it is enforced on every agent-scoped operation, what happens on run finish/expiry; keep name-bound credentials working but make the safer path the default for the orchestrator and MCP server; (2) a DELEGATOR role that may mint credentials for agent:* subjects only (the orchestrator then does not need admin) — enforced and tested (it must NOT be able to mint human/admin/service credentials or escalate); (3) server-side finish/reap of tasks and runs by an operator (stale runs, abandoned leases) with audit entries; (4) integration.prepare read-consistent with a validation job completing at the same moment (a test that interleaves them deterministically); (5) repository listings with real stream/changeset counts. Update the Go orchestrator (services/agent-orchestrator) and ag-mcp to use the delegator/run-bound credentials where they now apply and remove its "credentials bound to an agent name" warning once true; update docs (docs/guides/agents.md, security.md, protocol docs), regenerate OpenAPI golden + SDKs. Coordinate with the preceding server work already on main (rebase).`,
  },
  ha_tail: {
    areas: 'server HA tail',
    brief: `Finish the multi-replica server (ADR-0007, dev/handoff/ha.md): bring the multi-process tests from branch origin/wip/ha-track-rebased (crates/ag-server/tests/multi_replica.rs, scripts/ha-test.sh) onto main, rebased over the current server, and make them PASS for real: no lost update across replicas (reflog stays one contiguous chain), kill -9 of a replica mid-flight, exactly-once job claim across replicas, SSE event seen across replicas, bounded credential-revocation visibility, rolling restart; wire them into the CI "backends" job (additive) and watch them pass in Actions. Then the missing deliverables: docs/operations/high-availability.md (topology, guarantees, failure modes, upgrade procedure, honest limits incl. per-replica rate limits and PostgreSQL itself being a single point unless HA Postgres), deploy/kubernetes/overlays/ha (replicas 3, PDB, anti-affinity, readiness; validated by the deployment-files job) and a Compose profile if cheap, a CLI/admin test for migrate-control with real data (credentials, roles, repos, agent keys), update deployment.md/adr-traceability.md/security.md where facts change. ADR-0007 must stop saying "design goals" for invariants the tests now prove (and keep saying it for the rest).`,
  },
  perf: {
    areas: 'performance',
    brief: `Make performance claims real: (1) a benchmark suite (criterion or the repo's existing scale tests) for the hot paths — publish, integrate (CAS), three-way merge incl. rename detection, tree diff, blame, manifest decode — at 1k/10k/100k files with the numbers recorded in docs/architecture/performance.md (machine, commit, method; ADR-0006 already has some); (2) an end-to-end load test of ag-server (a small Rust or Go load generator in the repo, no external SaaS) driving real clients through HTTP/JSON and gRPC: concurrent integrations on different streams, same-stream contention, SSE fan-out, rate-limit behaviour; record throughput/latency percentiles and failure modes in docs/operations/performance.md, with a quick smoke variant run in CI (non-gating, artifacts uploaded); (3) fix any regression/bottleneck the measurements expose if the fix is local and safe (with a before/after number), otherwise list it in the ROADMAP-notes of your handoff. Do not invent SLOs: state measured numbers with their conditions only.`,
  },
  core_engine_storage: {
    areas: 'core-formats, core-merge, engine, storage',
    brief: `Fix the confirmed audit items in crates ag-types, ag-object, ag-changeset, ag-crypto, ag-tree, ag-dag, ag-diff, ag-merge, ag-integrate, ag-storage and migrations/ (areas core-formats, core-merge, engine, storage): e.g. merge_full swallowing TreeError::TooManyEntries, silent clock fallback, advertised limits duplicated as literals, unnamed magic constants, error-swallowing, claims in comments the code does not honour. Respect determinism: no behaviour change in canonical formats/hashes/merge results without an ADR (limits that decide changeset validity stay fixed — the verifiers narrowed that item). Storage: the S3 suite currently runs against moto, a Python re-implementation — add a REAL S3-compatible server leg (SeaweedFS is what the Compose stack uses; or Garage/MinIO binary if you can pin one that exists) to scripts/s3-test.sh and the CI "backends" job so tests/s3.rs runs against both, keeping signature verification on; fix whatever it finds. Update docs/operations/postgres-and-s3.md to say what is verified against what.`,
  },
  policy_cli_execution: {
    areas: 'policy-agent-protocol, cli-client-git, execution-plane',
    brief: `Fix the confirmed audit items in crates ag-policy, ag-agent, ag-protocol, proto/, ag-cli, ag-client, ag-git, ag-worker, ag-sandbox (areas policy-agent-protocol, cli-client-git, execution-plane). Execution plane proof: the OCI sandbox backend is verified only against a canned shell-script "runtime" — add tests against REAL runtimes in CI (docker on ubuntu runners; podman as a second leg), with a small digest-pinned image pulled in CI, asserting from inside the container the limits/isolation the docs claim (no network, read-only root, memory/pids/cpu limits, no new privileges, key/credential invisibility); verify cgroup v2 limits on the runner and make the namespace-sandbox tests exercise real memory/cpu limits where delegation exists (skip with an explicit reason only where the runner truly cannot). Whatever fails is a bug in the code or the docs — fix it. Update docs/architecture/sandbox.md and docs/guides/workers.md so every claim says what is verified where (Linux CI, macOS CI, real docker/podman) and what is not (microVM, Windows).`,
  },
  clients_ops: {
    areas: 'web-ui, sdks, ops-docs-claims',
    brief: `Fix the confirmed audit items in apps/web (src only), sdk/typescript, sdk/python, deploy/, scripts/, .github/workflows, examples/, docs/ and the README claims (areas web-ui, sdks, ops-docs-claims): e.g. the default image tag "latest" that nothing publishes (make every default resolvable and consistent with the release workflow: pin to the version/tag the workflow really produces, document how to override), hardcoded URLs/hosts/ports/versions in the UI/SDKs/scripts → configuration or derived values, placeholder/demo data in product code paths, UI/CLI/doc text promising features that do not exist (implement or remove the claim), stale tables in docs (docs/operations/deployment.md verified table, adr-traceability). Cross-check every row of the README feature tables against code/tests and list unbacked claims in your handoff for the final docs fold (do not edit README.md/README.ru.md yourself).`,
  },
  orchestrator: {
    areas: 'orchestrator',
    brief: `The Go agent platform (services/agent-orchestrator). Fix the confirmed audit items: (1) remove the simulated agent ("type: scripted", Scripted config, scriptedExecutor) from the shipped binary — product executors are real commands only; for demos/tests build a test-only agent executable outside the shipped binary; (2) replace the ~2000-line in-memory fakeagwp server that backs most unit tests with a shared harness that starts the REAL ag-server (move the integration harness into an importable internal/testutil package; tests run against the real server as CI already builds it); delete fakeagwp; (3) the advisor / reviewers / semantic index are deterministic keyword heuristics presented as intelligence: add real model-backed implementations behind the existing interfaces via a provider-agnostic LLM client (OpenAI-compatible chat-completions API; configuration: base URL, model, API-key ENV VAR NAME, timeouts, retries, budget limits — NO key, URL or model hardcoded in code; no key ever logged), with prompts that treat repository content as untrusted data, bounded context, structured outputs validated against the AGWP types, and the rule that a model may only PROPOSE (comments/suggestions), never integrate. Keep the deterministic implementations selectable and honestly labelled "heuristic" in config/docs/UI. Verify the model-backed path LIVE in this session: openrouter.ai is reachable and the sandbox proxy injects the Authorization header (no key needed or allowed in code/config files; the client must still be configurable); pick a cheap model, run the advisor/reviewer end to end against a real ag-server with a real conflicting proposal, and record the exact evidence (model id, prompts' shape, outputs, token use) in docs/guides/orchestrator.md and your handoff. In CI no key exists: those live tests must be opt-in (env AG_LLM_LIVE=1) and the unit tests for the client's protocol handling may use a local httptest server (name it as a test double of a third-party API in the docs). Update docs/guides/orchestrator.md, the service README and examples/orchestrator.yaml accordingly.`,
  },
  docs_site: {
    areas: 'docs site (CLI/docs track tail)',
    brief: `Finish the CLI/docs track's second half (dev/handoff/cli-docs.md, dev/tracks/tracks.md "cli-docs"): the documentation site (MkDocs Material was chosen: strict mode checks links/anchors, offline search; build it with pinned dependencies; keep docs/ the source of truth), .github/workflows/docs.yml (strict build on every push/PR; Pages deploy guarded so it is skipped/non-fatal when Pages is not enabled — the owner must enable Settings → Pages → GitHub Actions), generated reference pages (completions, man, help topics, cli-json, configuration) kept fresh by a check, docs/tutorials/ (first agent-safe repo in 15 minutes, run a team server, policy cookbook with every policy JSON validated by a test through the real parser, migrating a Git repository, troubleshooting a blocked integration tied to the real refusal strings), docs/faq.md, docs/glossary.md, docs/comparison.md (honest vs Git, GitHub PRs/merge queue, Gerrit, Jujutsu — no claim you cannot back), and the snippet-test harness that executes the tutorials' command snippets in CI. Also: add scripts/check_no_stubs.py (+ CI step in the docs-and-demo job) that fails on todo!/unimplemented!/TODO/FIXME/XXX/HACK/"for now"/placeholder implementations in production source (allow-list file with a reason per entry) so the "no stubs" bar stays enforced. Fix the CLI inconsistencies the handoff lists only if you can do it compatibly (e.g. add --stream alongside positional; exit code 4 for not-found/conflict is a breaking change: document, do not change).`,
  },
  security_tail: {
    areas: 'security track tail',
    brief: `Finish the security/quality track (dev/handoff/security.md): land .github/workflows/security.yml from branch origin/wip/security-track with the scripts it references (scripts/security_exceptions.py + tests, scripts/third_party_licenses.sh, scripts/sbom.sh) and make it GREEN in Actions; fuzz.yml (PR smoke ~60 s per relevant target + nightly longer); quality.yml (cargo-llvm-cov with per-crate numbers recorded in docs/architecture/testing.md and a modest floor on core crates; cargo-mutants on ag-merge/ag-tree/ag-diff/ag-policy/ag-object scheduled/manual, triage survivors, add killing tests); CodeQL (rust, javascript-typescript, go, python); dependency-review; extend .github/dependabot.yml (npm, pip, gomod, docker, cargo, actions); least-privilege permissions on every workflow; THIRD_PARTY_LICENSES with freshness check; the control-DB compat fixture regenerated from a MAIN build (the old one is schema v2 from another branch) with crates/ag-server/tests/compat_control.rs. Fuzzing was only 15–45 s per target: run the 8 highest-value targets (object envelope, changeset, tree manifest, policy/attestation JSON, AGWP bodies, path normalisation, merge/diff invariants, git-bridge parsing) for >= 5 minutes each locally (nightly toolchain via rustup in a scratch dir, mind disk), fix every crash with a regression test, and fix the signed_json harness split_at bug. Docs: security.md "Fuzzing" section with targets/minutes/findings, testing.md, compatibility.md, supply-chain and operator hardening checklist (each item verified against code), SECURITY.md reconciled. Be honest about what runs where.`,
  },
  mcp_tail: {
    areas: 'MCP track tail',
    brief: `Finish the MCP track (dev/handoff/mcp.md, docs/adr/0008-mcp-server.md): docs/guides/mcp.md (install and configuration for Claude Code, Claude Desktop, the Claude Agent SDK in TypeScript and Python; security section: what the allow-list protects and what it does not; troubleshooting; restore the ADR links that were removed), add it to docs/README.md; tests/protocol.rs (version negotiation, JSON-RPC error codes, cancellation, progress, --max-message-bytes) and tests/cli.rs (--check, --list-tools, flag errors); examples/agents/ — a runnable scripted client (not an LLM) executed in CI, plus a REAL end-to-end with a model: openrouter.ai is reachable in this session with the Authorization header injected by the sandbox proxy, so write a small agent loop (OpenAI-compatible tool-calling against a cheap model) that drives ag-mcp over stdio against a real ag-server to claim a task, edit a file and open a proposal; run it live here and record the exact evidence (model id, tool-call transcript summary) in the guide; make it opt-in (env var) in CI since CI has no key. Add ag-mcp to the release archives if release.yml does not list it yet (coordinate: additive edit). Known limits to fix if cheap: a second task_claim by the same agent starts a second run; --ca-file replaces the built-in roots (provide an additive option).`,
  },
  web_tail: {
    areas: 'web track tail',
    brief: `Finish the Web UI track (dev/handoff/web.md): (1) the flaky accessibility race (axe serious scrollable-region-focusable on .wrap in the dark-theme credentials/roles page — useScrollRegion/DataTable) — find the root cause and fix it, not retry it; (2) make Firefox and WebKit projects green in web.yml (branch origin/web-product-ci holds the full 5-project matrix; Firefox timeline-tab test and a keyboard test were unexplained; WebKit CSP inline-style allowance needs confirming) and put them in main's matrix as required jobs; (3) the first-run "connect your first agent" onboarding panel whose commands are TESTED (the e2e runs the shown commands against the real server), the version-mismatch banner (UI build vs server version from the real API), the 401/expired-session page; (4) replace the fake IdP in the OIDC e2e by a REAL OIDC provider in CI (Keycloak service container, or another real IdP image you can pin) for login-redirect round trip, token expiry and logout, keeping the fast fake only if you document it as a test double; the server's own OIDC validation also gets a test against that real IdP; (5) a real screen-reader/keyboard pass is not possible here — list it as unverified. Update docs/guides/web-ui.md with what is verified where.`,
  },
}

const LANES = [
  { key: 'A-server', wps: ['server_audit', 'server_hardening', 'ha_tail', 'perf'] },
  { key: 'B-core', wps: ['core_engine_storage', 'policy_cli_execution', 'clients_ops', 'orchestrator'] },
  { key: 'C-docs', wps: ['docs_site', 'security_tail', 'mcp_tail', 'web_tail'] },
]

const REPORT = {
  type: 'object',
  properties: {
    wp: { type: 'string' },
    done: { type: 'boolean' },
    ci_green: { type: 'boolean' },
    commits: { type: 'array', items: { type: 'string' } },
    handoff_file: { type: 'string' },
    fixed_findings: { type: 'integer' },
    not_fixed: { type: 'string' },
    changelog: { type: 'array', items: { type: 'string' } },
    readme_notes: { type: 'array', items: { type: 'string' } },
    notes: { type: 'string' },
  },
  required: ['wp', 'done', 'ci_green', 'commits', 'handoff_file', 'not_fixed', 'changelog', 'readme_notes', 'notes'],
}
const VERIFY = {
  type: 'object',
  properties: {
    head_sha: { type: 'string' },
    all_green: { type: 'boolean' },
    failing: { type: 'array', items: { type: 'object', properties: { workflow: { type: 'string' }, jobs: { type: 'array', items: { type: 'string' } } }, required: ['workflow', 'jobs'] } },
    notes: { type: 'string' },
  },
  required: ['head_sha', 'all_green', 'failing', 'notes'],
}

phase('Lanes')
const laneResults = await parallel(LANES.map(lane => async () => {
  const out = []
  for (const key of lane.wps) {
    const wp = WPS[key]
    log(`lane ${lane.key}: starting ${key}`)
    const r = await agent(
      `${COMMON}\n\nWORK PACKAGE "${key}" (audit areas: ${wp.areas}).\n${wp.brief}`,
      { label: `${lane.key}:${key}`, phase: 'Lanes', isolation: 'worktree', schema: REPORT },
    )
    out.push(r || { wp: key, done: false, ci_green: false, commits: [], handoff_file: '', not_fixed: 'agent returned nothing', changelog: [], readme_notes: [], notes: 'null result' })
    if (r && !r.ci_green) log(`lane ${lane.key}: ${key} finished with CI not confirmed green — notes: ${(r.notes || '').slice(0, 200)}`)
  }
  return { lane: lane.key, reports: out }
}))

phase('Verify')
let verdict = null
for (let round = 1; round <= 3; round++) {
  verdict = await agent(
    `You are the CI verifier for ${REPO} (read-only). 1) git fetch origin main; take origin/main HEAD sha. 2) Poll with a Bash until-loop (curl -s "https://api.github.com/repos/${REPO}/actions/runs?head_sha=<sha>&per_page=40", sleeping inside the loop, up to ~60 minutes; restart on a newer HEAD) until every workflow run for that sha has completed. Workflows absent for a sha are fine only if path-filtered (Containers, Packaging, Web UI, Windows, Docs, Security/Fuzz/Quality on schedule); if the HEAD commit touched their paths and they are absent, say so. If several recent commits touched path-filtered workflows' paths, also check the latest run of each such workflow on main. 3) For each non-success run list failing job names (/actions/runs/<id>/jobs). Return all_green only if every completed run relevant to HEAD (and the latest run of each path-filtered workflow) is success; continue-on-error steps are fine.`,
    { label: `verify:r${round}`, phase: 'Verify', schema: VERIFY },
  )
  if (verdict && verdict.all_green) break
  log(`round ${round}: main not green: ${JSON.stringify(verdict && verdict.failing)}`)
  if (round === 3) break
  await agent(
    `${COMMON}\n\nGENERIC FIX ROUND: main is not green at ${verdict && verdict.head_sha}. Failing: ${JSON.stringify(verdict && verdict.failing)}. Notes: ${verdict && verdict.notes}. Diagnose each failure from the real CI logs (mcp__github__get_job_logs WITHOUT return_content gives a URL; curl -sL it and grep), fix root causes (no skipping tests, no loosening assertions), land, and wait for CI. Several workflows may have different owners — fix all of them; they are your job now. Return the REPORT with wp "generic-fix".`,
    { label: `generic-fix:r${round}`, phase: 'Verify', isolation: 'worktree', schema: REPORT },
  )
}

phase('Fold')
const reports = laneResults.filter(Boolean).flatMap(l => l.reports)
const fold = await agent(
  `${COMMON}\n\nFOLD ROUND — documentation consolidation by the orchestrator's delegate. Inputs: every dev/handoff/*.md (read them all), dev/STATUS.md, dev/audit/CONFIRMED.md, git log since 5d35ee2, and these structured reports from the work packages (changelog entries / README notes / open items):\n${JSON.stringify(reports.map(r => ({ wp: r.wp, done: r.done, ci_green: r.ci_green, not_fixed: r.not_fixed, changelog: r.changelog, readme_notes: r.readme_notes })))}\nThe CI verifier said: ${JSON.stringify(verdict)}\nDo: (1) update CHANGELOG.md (Unreleased: Added/Changed/Fixed/Known limitations), ROADMAP.md (move built things to "Built since 0.1", rewrite "Next" to what is really left — honestly), docs/architecture/adr-traceability.md, docs/README.md index, and BOTH README.md and README.ru.md (feature cards, platform/clients rows: Windows, MCP, embedded UI, HA, release/install paths; the "Status and honest limits" table rewritten from verified facts only — remove limits that are resolved, keep or add the ones that are real; keep the Russian README in sync; run scripts/gen_readme_assets.py --check and regenerate the SVGs only if their text changed) from VERIFIED facts only (handoffs with run links; if a handoff says unverified, the README says so); (2) rewrite dev/STATUS.md to the true current state (tracks → done/open, owner action items, next steps) and delete handoff files that are fully obsolete only if their content is folded; (3) run scripts/check_docs.py, scripts/gen_cli_docs.py --check, scripts/gen_readme_assets.py --check and every other doc check the CI runs; land on main and wait for CI green. Return the REPORT with wp "fold".`,
  { label: 'fold-docs', phase: 'Fold', isolation: 'worktree', schema: REPORT },
)

return { lanes: laneResults, verdict, fold }
