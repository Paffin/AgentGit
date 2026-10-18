# Recorded scanner output

Fixtures of `scripts/test_security_exceptions.py`. Each file is **real output of the real tool**, recorded on
2026-10-18 and trimmed to a few entries; none is hand-written. They are recorded rather than produced in the test
because the advisory databases change every day, and a unit test of the report parser must not.

| file | tool and command | what it shows |
|---|---|---|
| `npm-sdk-all.json` | npm 10.9.4, `npm audit --json` in `sdk/typescript` | two critical `tinypool` advisories (accepted in `security/exceptions.toml`), a moderate `vitest`/`@vitest/mocker`, a low `esbuild` |
| `npm-web-prod.json` | npm 10.9.4, `npm audit --omit=dev --json` in `apps/web` | no production advisories |
| `pip-audit.json` | pip-audit 2.10.1, `--format json` over `uv export --frozen --no-dev` of `sdk/python` | no vulnerabilities |
| `pip-audit-vulnerable.json` | pip-audit 2.10.1 over `urllib3==1.24.1` | two vulnerabilities with aliases |
| `govulncheck-vulnerable.json` | govulncheck v1.9.0 `-format json ./...` in `services/agent-orchestrator`, analysed with **go1.24.7** | two advisories whose vulnerable function is called (`GO-2025-4007`, `GO-2025-4008`) and one that is only imported (`GO-2025-4006`); the full run had 36 called ones |
| `govulncheck-clean.json` | the same with **go1.27.2** (the toolchain of the orchestrator image) | nothing |

Re-record with `scripts/security_exceptions.py`'s inputs: run the commands above and keep a few entries.
