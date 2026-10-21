#!/usr/bin/env python3
"""Check that the documentation still matches the code.

    python3 scripts/check_docs.py [--require-ag]

Checks (all must pass; used by CI):

1. relative Markdown links and images point at files that exist;
2. every `snake_case_identifier` in backticks that looks like a test or code name
   (three or more words) occurs somewhere in the Rust sources, scripts or examples;
3. every `ag <command>` mentioned in the docs names a real top-level command
   (taken from `ag --help`, binary from $AG or target/{release,debug}/ag);
4. every `AG_*` environment variable named in backticks exists in the sources, and every one that the `ag`
   command line reads is documented in docs/reference/configuration.md; the `docs/*.md` pages that `ag`'s help topics
   and messages point at exist;
5. every ADR is listed in docs/adr/README.md;
6. every message in a table that follows `<!-- refusals -->` (the troubleshooting tutorial) is a message the code
   can print: its fixed parts, between the `{placeholders}`, occur in the Rust sources.

The command check needs the `ag` binary. Without one it is skipped with a note, unless `--require-ag` is given or
the CI environment variable is set: a green run in CI must mean that the commands were checked.

The generated reference pages (docs/reference/*, see gen_cli_docs.py) are checked by that script, not here.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC_FILES = sorted(
    [ROOT / "README.md", ROOT / "README.ru.md", ROOT / "ROADMAP.md", ROOT / "CHANGELOG.md",
     ROOT / "CONTRIBUTING.md", ROOT / "SECURITY.md"]
    + list((ROOT / "docs").rglob("*.md"))
)
# The ADR is a historical record in Russian and is not code-checked.
SKIP_PREFIXES = ("docs/adr/0001",)
GENERATED = {"docs/reference/cli.md", "docs/reference/help-topics.md", "docs/reference/cli-json.md"}
REQUIRE_AG = "--require-ag" in sys.argv or bool(os.environ.get("CI"))

errors: list[str] = []


def rel(p: Path) -> str:
    return str(p.relative_to(ROOT))


def strip_code(text: str) -> str:
    return re.sub(r"```.*?```", "", text, flags=re.S)


# 1. links ------------------------------------------------------------------------------------
LINK = re.compile(r"!?\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
for doc in DOC_FILES:
    if not doc.exists() or rel(doc).startswith(SKIP_PREFIXES):
        continue
    for target in LINK.findall(strip_code(doc.read_text())):
        if re.match(r"^(https?:|mailto:|#)", target):
            continue
        path = target.split("#")[0]
        if not path:
            continue
        if not (doc.parent / path).resolve().exists():
            errors.append(f"{rel(doc)}: broken link -> {target}")

# 2. identifiers --------------------------------------------------------------------------------
corpus: set[str] = set()
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
for pattern in ("crates/**/*.rs", "examples/*", "scripts/*", "tests/**/*.py", "tests/**/*.json", ".github/**/*.yml"):
    for f in ROOT.glob(pattern):
        if f.is_file():
            for w in WORD.findall(f.read_text(errors="ignore")):
                corpus.add(w)
                if w.startswith("SYS_"):
                    # system call names reach the docs without the libc prefix (`SYS_io_uring_enter`)
                    corpus.add(w[4:])
                if w != w.lower() and w != w.upper() and not w.startswith("_"):
                    # CamelCase variants reach users as snake_case through serde's rename_all
                    corpus.add(re.sub(r"(?<!^)(?=[A-Z])", "_", w).lower())
# Prometheus metric names are assembled at runtime as "agwp_" + the registered name.
for m in re.finditer(r'(?:HistogramOpts|Opts|IntCounter|IntGauge)::new\(\s*"([a-z_]+)"',
                     (ROOT / "crates/ag-server/src/metrics.rs").read_text()):
    corpus.add(f"agwp_{m.group(1)}")
IDENT = re.compile(r"^[a-z][a-z0-9]*(?:_[a-z0-9]+){2,}$")
# words that appear in docs as JSON keys, env vars, CLI values etc. but are not Rust names
ALLOW = {"max_age_secs", "environment_digest", "required_validations", "trusted_issuers", "protected_paths",
         "agent_autonomy", "min_for_agents", "forbid_self_approval", "allowed_authors", "max_changed_files",
         "require_provenance_for_agents", "auto_integrate", "allowed_paths", "max_files", "min_approvals",
         "missing_validation", "insufficient_approvals", "changes_requested", "missing_provenance",
         "stale_or_missing_token", "held_by_other", "stream_head_unpublished", "reflog_mismatch",
         "audit_chain_broken", "corrupt_object", "bad_blob"}
for doc in DOC_FILES:
    if not doc.exists() or rel(doc).startswith(SKIP_PREFIXES) or rel(doc) in GENERATED:
        continue
    for tok in re.findall(r"`([^`\n]+)`", strip_code(doc.read_text())):
        tok = tok.strip()
        if IDENT.match(tok) and tok not in corpus and tok not in ALLOW:
            errors.append(f"{rel(doc)}: `{tok}` is not found in the code (renamed test? typo?)")

# 3. commands -------------------------------------------------------------------------------------
def find_ag() -> str | None:
    if os.environ.get("AG"):
        return os.environ["AG"]
    for r in ("target/release/ag", "target/debug/ag"):
        if (ROOT / r).exists():
            return str(ROOT / r)
    return None


ag = find_ag()
if ag:
    help_text = subprocess.run([ag, "--help"], capture_output=True, text=True,
                               env=dict(os.environ, COLUMNS="100", AG_IDENTITY="user:docs")).stdout
    m = re.search(r"^Commands:\n((?:  .*\n?)+)", help_text, re.M)
    commands = {re.match(r"^  ([a-z-]+)", l).group(1) for l in m.group(1).splitlines() if re.match(r"^  [a-z]", l)}
    for doc in DOC_FILES:
        if not doc.exists() or rel(doc).startswith(SKIP_PREFIXES) or rel(doc) in GENERATED:
            continue
        text = doc.read_text()
        spans = re.findall(r"```[a-z]*\n(.*?)```", text, flags=re.S) + re.findall(r"`([^`\n]+)`", strip_code(text))
        for span in spans:
            for line in span.splitlines():
                line = re.sub(r"^\s*[$#]\s*", "", line).strip()
                mm = re.match(r"^(?:[A-Z_]+=\S+\s+)*ag\s+(.*)$", line)
                if not mm:
                    continue
                toks = mm.group(1).split()
                i = 0
                while i < len(toks) and toks[i].startswith("-"):
                    i += 2 if toks[i] in ("--as", "-C", "--repo") else 1
                if i >= len(toks) or not re.match(r"^[a-z][a-z-]*$", toks[i]):
                    continue
                cmd = toks[i]
                # skip prose such as "ag is a ..." and shell constructs
                if cmd in commands or cmd in {"is", "the", "and", "to", "or", "for", "a", "in", "can", "has", "does"}:
                    continue
                errors.append(f"{rel(doc)}: `ag {cmd}` is not a command of this build ({line[:70]!r})")
elif REQUIRE_AG:
    errors.append("the ag binary was not found, so the `ag <command>` check could not run "
                  "(build it with `cargo build -p agentgit-cli` or set $AG; CI and --require-ag insist on it)")
else:
    print("note: ag binary not found, skipping command checks (build it or set $AG)", file=sys.stderr)

# 4. environment variables ---------------------------------------------------------------------------
# The variables named in the docs exist in the code, and the ones the `ag` command line reads are documented.
CONFIG_PAGE = ROOT / "docs" / "reference" / "configuration.md"
ENV_VAR = re.compile(r"\bAG_[A-Z][A-Z0-9_]*")
# Set by the build script for `--version`, not read at run time (AG_BUILD_COMMIT and AG_BUILD_DATE are documented).
BUILD_INTERNAL = {"AG_LONG_VERSION", "AG_GIT_COMMIT"}
# The orchestrator is Go, the SDKs are TypeScript and Python, the deployment files are YAML/TOML: variables defined
# there are real too.
env_corpus = set(corpus)
for pattern in ("services/**/*.go", "sdk/python/src/**/*.py", "sdk/typescript/src/**/*.ts", "deploy/**/*.yaml",
                "deploy/**/*.yml", "deploy/**/*.toml", "deploy/**/*.example", "deploy/docker/*"):
    for f in ROOT.glob(pattern):
        if f.is_file():
            env_corpus.update(ENV_VAR.findall(f.read_text(errors="ignore")))
for doc in DOC_FILES:
    if not doc.exists() or rel(doc).startswith(SKIP_PREFIXES) or rel(doc) in GENERATED:
        continue
    for tok in re.findall(r"`([^`\n]+)`", doc.read_text()):
        for var in ENV_VAR.findall(tok):
            # `AG_TOKEN_<REMOTE>` is written `AG_TOKEN_` in the code
            if var not in env_corpus and var.rstrip("_") + "_" not in env_corpus:
                errors.append(f"{rel(doc)}: the environment variable `{var}` is not found in the code")
cli_vars: set[str] = set()
for f in (ROOT / "crates" / "ag-cli" / "src").rglob("*.rs"):
    for m in re.finditer(r'"(AG_[A-Z][A-Z0-9_]*)"', f.read_text()):
        cli_vars.add(m.group(1))
for var in sorted(cli_vars - BUILD_INTERNAL):
    if var not in CONFIG_PAGE.read_text():
        errors.append(f"{rel(CONFIG_PAGE)}: `ag` reads {var} but the page does not mention it")

# The terminal points at the docs (`ag help` topics, hints, `ag init`): those pages must exist.
for f in [*(ROOT / "crates" / "ag-cli" / "help").glob("*.md"), *(ROOT / "crates" / "ag-cli" / "src").rglob("*.rs")]:
    for m in re.finditer(r"\bdocs/[A-Za-z0-9_./-]+\.md\b", f.read_text()):
        if not (ROOT / m.group(0)).exists():
            errors.append(f"{rel(f)}: points at {m.group(0)}, which does not exist")

# 5. the ADR index -------------------------------------------------------------------------------------
adr_index = (ROOT / "docs" / "adr" / "README.md").read_text()
for adr in sorted((ROOT / "docs" / "adr").glob("[0-9][0-9][0-9][0-9]-*.md")):
    if f"]({adr.name})" not in adr_index:
        errors.append(f"docs/adr/README.md does not list {adr.name}")

# 6. refusal messages ----------------------------------------------------------------------------------
# A table that follows `<!-- refusals -->` lists messages in its first column, as the code writes them, with
# `{placeholders}`. The fixed text around the placeholders must occur in the Rust sources.
rust_text = "\n".join(f.read_text(errors="ignore") for f in (ROOT / "crates").rglob("*.rs"))
refusal_rows = 0
for doc in DOC_FILES:
    if not doc.exists() or rel(doc).startswith(SKIP_PREFIXES):
        continue
    lines = doc.read_text().split("\n")
    for i, ln in enumerate(lines):
        if ln.strip() != "<!-- refusals -->":
            continue
        for row in lines[i + 1:]:
            if not row.startswith("|"):
                if refusal_rows or row.strip():
                    break
                continue
            first = row.split("|")[1].strip()
            m = re.fullmatch(r"`(.+)`", first)
            if not m or set(first) <= {"-", " ", ":"} or first == "Message":
                continue
            refusal_rows += 1
            for piece in re.split(r"\{[^}]*\}", m.group(1)):
                piece = piece.strip()
                if len(piece) >= 6 and piece not in rust_text:
                    errors.append(f"{rel(doc)}: the refusal message `{m.group(1)}` is not in the code "
                                  f"(`{piece}` was not found); was it reworded?")
if refusal_rows == 0:
    errors.append("no `<!-- refusals -->` table found: the troubleshooting tutorial lost its message table")

if errors:
    print("\n".join(sorted(set(errors))))
    print(f"\n{len(set(errors))} documentation problem(s)", file=sys.stderr)
    sys.exit(1)
print(f"documentation checks passed ({len(DOC_FILES)} files)")
