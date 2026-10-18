# Handoff: orchestrator (Go agent platform, `services/agent-orchestrator`)

Bar from the owner: no mocks, no stubs, no simulated behaviour and no hardcoded values in product code. This
package removes the simulated agent from the product, replaces the in-memory fake server with the real `ag-server`
in every test, adds model-backed advisor / reviewers / index behind the existing interfaces and verifies that path
live, and fixes the confirmed audit items of the area.

## Done

1. **No simulated agent in the shipped binary.** The built-in `scripted` executor (a canned agent that signed
   provenance with free-text model and rationale) is gone from code, configuration and docs. Demos and tests run
   `internal/testutil/testagent`, a test-only program, as an ordinary `type: command` executor. The manual clock moved out
   of the product package (`internal/clock`) to `internal/testutil/fakeclock`, so its entry in
   `scripts/no_stubs_allowlist.txt` is removed.
2. **`fakeagwp` is deleted** (about 2000 lines). `internal/testutil` is the shared importable harness: a real
   `ag-server` per test (`StartServerWith`, optional TLS), a world on it (repository, alice/bob/carol, CI worker with
   signed attestations, per-agent credentials, policy), a forwarding fault-injecting proxy for what a healthy server
   cannot do on demand (500, dropped connection, cut event stream, an older server's capabilities), `fakeclock`
   (moved out of the product package) and `LLMServer`. Every package's tests (`events`, `execsched`, `review`,
   `advisor`, `semindex`, `scheduler`, `session`, `app`, `config`, `integration`, `cmd`) now run against the real server.
   Real-server behaviour that differed from the fake and was fixed in the tests or the product: agent proposals need
   an approval under the default policy; agent credentials last at most one hour; a run abandoned by the reaper must
   not count as an attempt (`session.LostRunPrefix`).
3. **Model-backed components** (`mode: model`, default `heuristic`) behind the existing interfaces, through a
   provider-agnostic OpenAI-compatible client (`internal/llm`): configurable base URL, model, key *variable name*,
   timeout, retries (Retry-After, jittered backoff), request and token budgets, concurrency, input and output bounds,
   JSON mode. No default endpoint, model or key; a key pasted where a name belongs is refused and never echoed; a
   base URL with credentials or a query is refused; the key is a `secret.Secret` that only reaches the
   `Authorization` header. Prompts fence repository text with a random per-request id (`llm.Guard`) and say it is
   data; secrets are masked before text leaves the machine (`review.RedactSecrets`) and the advisor does not send a
   file with one; answers are strict, validated JSON; a model's confidence is capped at `medium`; models only post
   `comment` reviews; failures fall back to the heuristic answer with a visible note; comment markers are stable so
   non-deterministic text does not duplicate comments after a restart.
   - `advisor.ModelAdvisor`: asked only about what the rules are not sure of; audits the lines it drops or invents.
   - `review.LLMReviewer`: a clearly separated section of the heuristic comment; findings bounded; paths resolved
     against the change (exact, diff prefix, or the tail of exactly one path).
   - `semindex.LLMJudge`: assessment (`conflict` / `compatible` / `unsure`) next to the static severity, per pair and
     head, bounded pairs per pass, retry delay for failed pairs.
   - New config keys: `llm.*`, `advisor.mode`, `advisor.model_file_bytes`, `review.mode`, `review.model_diff_bytes`,
     `review.model_max_findings`, `index.mode`, `index.judge_patch_bytes`, `index.judge_pairs_per_pass`. `/status` has
     an `llm` section (model, endpoint, totals), `/metrics` has `orchestrator_llm_requests_total`,
     `orchestrator_llm_tokens_total`, `orchestrator_llm_request_seconds_total`.
4. **Confirmed audit items (7), each with a test that fails without the fix:** retry/reconnect jitter is random again
   (was a constant 0.5); the response-size bound is configuration (`server.max_response_bytes`); an initial import
   says which files it did not inspect; the retry budget and backoff survive an orchestrator restart (recovered from
   the run history on the server); a failing conflict prediction is counted, logged, shown in `/status`
   (`predict_error`) and can hold tasks back (`schedule.on_predict_error`); role patterns in `check` follow the
   server's globset rules; credentials pasted where a variable name belongs are never echoed and executor `env`
   values are redacted in the effective configuration.
5. **Contested items, fixed:** proposal analysis uses `index.max_file_bytes` and configurable bounds, what the
   index bounds drop is counted; the advisor carries a too-large flag instead of an invented buffer; review comments
   name the real reviewer identity (asked from the server after preflight); the execution scheduler's refresh /
   recent / jobs bounds are configuration and an unreadable queue is reported.
6. **CI:** the orchestrator job now also runs the worked example end to end and builds and starts the Docker image
   (proof for the old "Dockerfile has not been built" note, which is gone), and has an opt-in live step for the model
   path (runs on a push only when the repository defines the `OPENROUTER_API_KEY` secret and the `AG_LLM_MODEL` variable).

## Evidence

- Commits on `main`: `f954fbd` (no simulated agent, real server in every test), `9f2dd17` (audit items), `7546171`
  (model-backed components), `46f13e5` (live tests), `d455ece` (docs, CI, path resolution, steadier retry tests).
- CI of `9f2dd17`, all six workflows green, including "the worked example runs end to end" and "the image builds
  and starts": https://github.com/Paffin/AgentGit/actions/runs/39139989404 (CI), .../39139989365 (CodeQL),
  .../39139989381 (Windows), .../39139989398 (Security), .../39139989406 (Packaging), .../39139989367 (Docs).
- CI of `f49fbd8` (code, docs, handoff; all 10 workflows green): CI https://github.com/Paffin/AgentGit/actions/runs/39144037391,
  Windows .../39144037404, Docs .../39144037386, Security .../39144037412, CodeQL .../39144037407, Quality
  .../39144037394, Containers .../39144037368, Web UI .../39144037433, Packaging .../39144037364, Performance
  .../39144037390. The previous commit `d455ece` had one red run: Windows, `an_operator_reaps_stale_runs_their_leases_and_their_credentials`
  (a Rust server test of another package, timing: the live run went stale on the slow runner between its heartbeat
  and the reap). Not orchestrator code; `f49fbd8` makes the test beat again right before each reap, and the rerun passed.
  If it recurs, the test's 1.3 s sleep and 1 s threshold are the knobs.
- This handoff's own text was written before `f49fbd8`'s run finished; the commit that adds these links changes only
  this file (`git log -1 -- dev/handoff/orchestrator.md`), and its runs are not claimed here.
- Local gates before each push: `go vet ./...`, `gofmt -l .` (empty), `AG_SERVER_BIN=... go test -race -count=1 ./...`
  (all packages ok), `python3 scripts/check_no_stubs.py --strict`, `python3 scripts/check_docs.py`.
- **Live model run, 2026-10-18** (`AG_LLM_LIVE=1 AG_LLM_LIVE_MODEL=openai/gpt-4o-mini`, endpoint
  `https://openrouter.ai/api/v1`, real `ag-server` built from this tree). Reachability first: a plain `curl` to
  `/chat/completions` through the sandbox proxy returned a completion with usage (the proxy injects `Authorization`;
  no key was in code, config or the test process environment, and `git grep` finds none). Then
  `go test -count=1 -v -run Live ./internal/llm ./internal/advisor ./internal/review ./internal/semindex ./internal/app`:
  5 tests passed, 1 request each. Provider-reported model id `openai/gpt-4o-mini` every time.

  | Test | Prompt (system + user bytes), fenced pieces | Tokens | Result |
  |---|---|---|---|
  | client | 747 + 241, `a README line` holding "Ignore all previous instructions ..." | 295 + 30 | `{"verdict": "injection"}`: classified, did not obey |
  | advisor, real conflict (two incompatible edits of `Total`) | 1680 + 1272, `BASE` / `OURS` / `THEIRS` of `calc/total.go` | 835 + 192 | stayed "needs a person", labelled as written by the model, posted as one comment by `service:advisor`; proposal head/state and `main` unchanged |
  | reviewer, real proposal (SQL concatenation, ignored `Scan` error) | 1749 + 695, `title of the proposal`, `diff of store/users.go` | 664 + 192 | `high` SQL injection at `store/users.go:10`, `warn` ignored error at `:12`, in a `comment` review beside the heuristic finding |
  | semindex judge, two real overlapping proposals | 1609 + 1701, six fenced pieces | 991 + 71 | `conflict` / `medium`; static severity `medium` unchanged and shown next to it |
  | whole application (`app.Run`) | as the reviewer | 632 + 104 (from `/status`) | comment with a `high` SQL-injection finding; `/status` has the `llm` section and no key |

  Observed, and why answers are validated: in an earlier run the reviewer's model named the file `users.go` (the
  location was dropped; now resolved by tail match, `TestAModelNamingAFileLooselyStillLocatesTheFinding`); in the
  index run the model's explanation swapped proposals A and B. The advisor test returned a combined function in one
  run and none in another (both marked "needs a person"): the model is not deterministic.
- Test doubles, named in `docs/guides/orchestrator.md` and the service README: the test-only agent
  (`internal/testutil/testagent`), the forwarding fault proxy, the fake clock, and `testutil.LLMServer` (a double of
  a third-party API; `httptest`). Everything else is the real server.

## NOT verified

- The key-in-`Authorization` path against a real provider: the sandbox proxy added the credential, so the live
  tests ran without `AG_LLM_LIVE_KEY_ENV`. That path is covered only by the `LLMServer` double (header present, key
  scrubbed from errors, logs, `/status`, comments).
- Any model other than one cheap model through one provider; long files (above `model_file_bytes` the advisor falls
  back to the rules), non-English repositories, adversarial content beyond the unit-tested prompt injection and one
  live injection string; real token cost over time (the budget counters restart with the process).
- The live CI step has never run in Actions (the repository may not define the secret and variable); it is written
  to be skipped otherwise.
- The Docker image is built and started (`version`, `config`) in CI, but not run against a server inside a container.
- Non-Linux platforms (sessions are Unix-specific; the integration suite is Linux-only). Scale (many repositories,
  thousands of open proposals) was not measured.
- The worked-example transcript in the guide is from an earlier real run; CI now runs the script but does not
  compare its output to the transcript.

## Next steps

1. Define `OPENROUTER_API_KEY` and `AG_LLM_MODEL` in the repository to make the live CI step real, then look at its
   log for the transcript; if a key-bearing path matters, run the live tests once with `AG_LLM_LIVE_KEY_ENV=<name>`.
2. Decide whether a model's `needs_person: true` answer that still contains a resolution should be offered as a
   draft (today the advisor withholds it and says so).
3. README.md / README.ru.md / CHANGELOG.md / ROADMAP.md were not touched (owner rule); notes for them are in the final report.
4. If a second provider is wanted, nothing in the client is provider-specific; add a live run record for it.
