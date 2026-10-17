#!/usr/bin/env python3
"""Check that the documentation still matches the code.

    python3 scripts/check_docs.py

Checks (all must pass; used by CI):

1. relative Markdown links and images point at files that exist;
2. every `snake_case_identifier` in backticks that looks like a test or code name
   (three or more words) occurs somewhere in the Rust sources, scripts or examples;
3. every `ag <command>` mentioned in the docs names a real top-level command
   (taken from `ag --help`, binary from $AG or target/{release,debug}/ag).
"""
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
GENERATED = {"docs/reference/cli.md"}

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
                if w != w.lower() and w != w.upper() and not w.startswith("_"):
                    # CamelCase variants reach users as snake_case through serde's rename_all
                    corpus.add(re.sub(r"(?<!^)(?=[A-Z])", "_", w).lower())
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
else:
    print("note: ag binary not found, skipping command checks (build it or set $AG)", file=sys.stderr)

if errors:
    print("\n".join(sorted(set(errors))))
    print(f"\n{len(set(errors))} documentation problem(s)", file=sys.stderr)
    sys.exit(1)
print(f"documentation checks passed ({len(DOC_FILES)} files)")
