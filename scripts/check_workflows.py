#!/usr/bin/env python3
"""Lints .github/workflows for the supply-chain rules AgentGit holds itself to.

Checks (offline, no dependencies beyond PyYAML):

  pinned        every `uses:` of a third-party or first-party action is pinned to a full commit SHA
                (`owner/repo@<40 hex>  # vX.Y.Z`); a tag or branch can be moved by whoever owns it.
                Local actions (`./...`) and `docker://` images with a digest are fine.
  permissions   every workflow declares `permissions:` at the top level, or on every job (least privilege:
                the default token of a repository can write everything).
  triggers      no `pull_request_target` and no `workflow_run` (both run with secrets on attacker-influenced
                code); `issue_comment` / `issues` only in workflows that do not check out code.
  injection     no `${{ ... }}` expression that a pull request, issue or comment author controls
                (`github.event.pull_request.title`, `head_ref`, `comment.body`, `inputs.*`, ...) written
                directly into a `run:` script - pass it through `env:` instead.
  checkout      (warning only) `actions/checkout` in a workflow that uploads artifacts or publishes should set
                `persist-credentials: false` (otherwise the token ends up in the uploaded `.git/config`).

Usage:
  scripts/check_workflows.py            check; exit 1 and list every problem
  scripts/check_workflows.py --pin      rewrite tag/branch refs to commit SHAs (needs network and `git`)
  scripts/check_workflows.py --pin -n   show what --pin would change

`--pin` resolves `owner/repo@ref` with `git ls-remote` (peeling annotated tags) and appends `# ref` so that
Dependabot (github-actions ecosystem) keeps both the SHA and the comment current.
"""
from __future__ import annotations

import pathlib
import re
import subprocess
import sys

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("PyYAML is required: pip install pyyaml")

ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOWS = sorted((ROOT / ".github" / "workflows").glob("*.y*ml"))
SHA = re.compile(r"^[0-9a-f]{40}$")
USES = re.compile(r"^(\s*-?\s*uses:\s*)([^\s#]+)(\s*#.*)?$")

# Expressions an outsider can influence. Writing one into a shell script makes the script injectable.
TAINTED = re.compile(
    r"\$\{\{\s*(?:"
    r"github\.event\.(?:issue|pull_request|comment|review|review_comment|discussion|head_commit|commits|pages)"
    r"[\w.\[\]*'\"-]*\.(?:title|body|label|name|email|message|ref|head_ref|default_branch)"
    r"|github\.head_ref"
    r"|github\.event\.workflow_run\.(?:head_branch|head_commit\.message)"
    r"|inputs\.[\w-]+"
    r"|github\.event\.inputs\.[\w-]+"
    r")\s*\}\}"
)


def load(path: pathlib.Path):
    return yaml.safe_load(path.read_text())


def uses_refs(path: pathlib.Path):
    """(line number, action, ref, raw line) for every remote `uses:`."""
    for n, line in enumerate(path.read_text().splitlines(), 1):
        m = USES.match(line)
        if not m:
            continue
        target = m.group(2).strip("'\"")
        if target.startswith("./") or target.startswith("docker://"):
            continue
        action, _, ref = target.partition("@")
        yield n, action, ref, line


def check(path: pathlib.Path) -> tuple[list[str], list[str]]:
    rel = path.relative_to(ROOT)
    problems: list[str] = []
    warnings: list[str] = []
    doc = load(path)
    if not isinstance(doc, dict):
        return [f"{rel}: not a workflow"], []

    for n, action, ref, _ in uses_refs(path):
        if not SHA.match(ref):
            problems.append(f"{rel}:{n}: {action}@{ref or '(no ref)'} is not pinned to a commit SHA")

    jobs = doc.get("jobs") or {}
    if "permissions" not in doc and not all("permissions" in (j or {}) for j in jobs.values()):
        problems.append(f"{rel}: no top-level `permissions:` (and not every job sets its own)")

    triggers = doc.get(True, doc.get("on")) or {}
    names = set(triggers) if isinstance(triggers, dict) else ({triggers} if isinstance(triggers, str) else set(triggers))
    for banned in ("pull_request_target", "workflow_run"):
        if banned in names:
            problems.append(f"{rel}: trigger `{banned}` runs with secrets on code a pull request controls")

    uploads = False
    checkouts: list[tuple[str, dict]] = []
    for job_name, job in jobs.items():
        for i, step in enumerate((job or {}).get("steps") or []):
            run = step.get("run")
            if isinstance(run, str) and TAINTED.search(run):
                m = TAINTED.search(run)
                problems.append(
                    f"{rel}: job `{job_name}` step {i + 1}: `{m.group(0)}` is expanded inside a shell script; "
                    "pass it through `env:` and quote the variable"
                )
            uses = str(step.get("uses", ""))
            if uses.startswith("actions/upload-artifact") or "publish" in uses or "push: true" in str(step.get("with")):
                uploads = True
            if uses.startswith("actions/checkout"):
                checkouts.append((job_name, step.get("with") or {}))
    if uploads:
        for job_name, with_ in checkouts:
            if with_.get("persist-credentials") is not False:
                warnings.append(
                    f"{rel}: job `{job_name}`: actions/checkout without `persist-credentials: false` in a workflow "
                    "that uploads artifacts"
                )
    return problems, warnings


def resolve(action: str, ref: str) -> str | None:
    repo = "/".join(action.split("/")[:2])
    url = f"https://github.com/{repo}"
    out = subprocess.run(
        ["git", "ls-remote", url, f"refs/tags/{ref}", f"refs/tags/{ref}^{{}}", f"refs/heads/{ref}"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.splitlines()
    peeled = next((l.split()[0] for l in out if l.endswith("^{}")), None)
    plain = next((l.split()[0] for l in out if not l.endswith("^{}")), None)
    return peeled or plain


def pin(dry: bool) -> int:
    changed = 0
    for path in WORKFLOWS:
        lines = path.read_text().splitlines(keepends=True)
        for n, action, ref, line in uses_refs(path):
            if SHA.match(ref):
                continue
            sha = resolve(action, ref)
            if not sha:
                print(f"{path.name}:{n}: cannot resolve {action}@{ref}", file=sys.stderr)
                continue
            m = USES.match(line.rstrip("\n"))
            assert m
            lines[n - 1] = f"{m.group(1)}{action}@{sha} # {ref}\n"
            print(f"{path.name}:{n}: {action}@{ref} -> {sha[:12]}")
            changed += 1
        if changed and not dry:
            path.write_text("".join(lines))
    return 0


def main() -> int:
    if "--pin" in sys.argv:
        return pin("-n" in sys.argv)
    results = [check(w) for w in WORKFLOWS]
    problems = [p for r in results for p in r[0]]
    for w in (w for r in results for w in r[1]):
        print(f"warning: {w}", file=sys.stderr)
    for p in problems:
        print(p, file=sys.stderr)
    if problems:
        print(f"\n{len(problems)} problem(s). See the docstring of scripts/check_workflows.py.", file=sys.stderr)
        return 1
    print(f"{len(WORKFLOWS)} workflows: actions pinned, permissions declared, no risky triggers or injections")
    return 0


if __name__ == "__main__":
    sys.exit(main())
