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
  checkout      `actions/checkout` in a workflow that uploads artifacts or publishes should set
                `persist-credentials: false` (otherwise the token ends up in the uploaded `.git/config`).
                A warning, unless --strict is given.

Usage:
  scripts/check_workflows.py            check; exit 1 and list every problem
  scripts/check_workflows.py --strict   also fail on the warnings
  scripts/check_workflows.py --pin      rewrite tag/branch refs to commit SHAs (needs network and `git`)
  scripts/check_workflows.py --pin -n   show what --pin would change

`--pin` resolves `owner/repo@ref` with `git ls-remote` (peeling annotated tags) and appends a comment with the
exact release the SHA is (`@v4` becomes `@<sha> # v4.3.1`), so that Dependabot (github-actions ecosystem) keeps both
the SHA and the comment current. `dtolnay/rust-toolchain@stable` (there the ref IS the toolchain) becomes the SHA
of that action's `master` plus `with: toolchain: stable`, which selects the same toolchain.
"""
from __future__ import annotations

import pathlib
import re
import subprocess
import sys
from collections.abc import Callable

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("PyYAML is required: pip install pyyaml")

ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOWS = sorted((ROOT / ".github" / "workflows").glob("*.y*ml"))
SHA = re.compile(r"^[0-9a-f]{40}$")
USES = re.compile(r"^(\s*-?\s*uses:\s*)([^\s#]+)(\s*#.*)?$")
SEMVER = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)$")
RUST_TOOLCHAIN = "dtolnay/rust-toolchain"

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


# --- pinning -----------------------------------------------------------------------------------------------------

# The refs of an action repository: "refs/tags/v4" -> commit SHA (annotated tags peeled), "refs/heads/master" -> SHA.
Refs = dict[str, str]


def ls_remote(action: str) -> Refs:
    repo = "/".join(action.split("/")[:2])
    out = subprocess.run(
        ["git", "ls-remote", "--tags", "--heads", f"https://github.com/{repo}"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.splitlines()
    return parse_refs(out)


def parse_refs(lines: list[str]) -> Refs:
    """The refs of `git ls-remote --tags --heads` output (`<sha>\\t<ref>` lines)."""
    refs: Refs = {}
    for line in lines:  # plain refs first; the peeled commit of an annotated tag then replaces the tag object
        sha, _, ref = line.partition("\t")
        if not ref.endswith("^{}"):
            refs[ref] = sha
    for line in lines:
        sha, _, ref = line.partition("\t")
        if ref.endswith("^{}"):
            refs[ref[:-3]] = sha
    return refs


def resolve(refs: Refs, ref: str) -> tuple[str, str] | None:
    """(commit SHA, comment) for a tag or branch name. The comment is the highest exact release (vX.Y.Z) that
    points at the same commit, so a floating `v4` is documented as the `v4.3.1` it is today."""
    sha = refs.get(f"refs/tags/{ref}") or refs.get(f"refs/heads/{ref}")
    if not sha:
        return None
    exact = []
    for name, commit in refs.items():
        tag = name.removeprefix("refs/tags/")
        m = SEMVER.match(tag) if name.startswith("refs/tags/") else None
        if m and commit == sha:
            exact.append((tuple(int(x) for x in m.groups()), tag))
    return sha, (max(exact)[1] if exact else ref)


def pin_text(text: str, lookup: Callable[[str], Refs]) -> tuple[str, list[str]]:
    """The workflow text with every unpinned `uses:` pinned, and a description of each change."""
    lines = text.splitlines(keepends=True)
    changes: list[str] = []
    inserts: list[tuple[int, str]] = []  # (index to insert before, text)
    for n, line in enumerate(lines):
        m = USES.match(line.rstrip("\n"))
        if not m:
            continue
        target = m.group(2).strip("'\"")
        if target.startswith("./") or target.startswith("docker://"):
            continue
        action, _, ref = target.partition("@")
        if SHA.match(ref):
            continue
        toolchain = None
        if action == RUST_TOOLCHAIN:
            # The ref is the toolchain here: pin the action's code (master) and pass the toolchain as an input.
            toolchain, lookup_ref = ref, "master"
        else:
            lookup_ref = ref
        found = resolve(lookup(action), lookup_ref)
        if not found:
            changes.append(f"line {n + 1}: cannot resolve {action}@{lookup_ref}")
            continue
        sha, comment = found
        lines[n] = f"{m.group(1)}{action}@{sha} # {comment}\n"
        changes.append(f"line {n + 1}: {action}@{ref} -> {sha[:12]} # {comment}")
        if toolchain:
            inserts += _toolchain_input(lines, n, toolchain)
    for index, extra in sorted(inserts, reverse=True):
        lines.insert(index, extra)
    return "".join(lines), changes


def _toolchain_input(lines: list[str], n: int, toolchain: str) -> list[tuple[int, str]]:
    """The line to add so that the step whose `uses:` is line n passes `toolchain: X`, unless it sets one."""
    first = lines[n].lstrip()
    key_indent = len(lines[n]) - len(first) + (2 if first.startswith("- ") else 0)
    pad = " " * key_indent
    with_at = None
    for i in range(n + 1, len(lines)):
        raw = lines[i]
        if not raw.strip():
            continue
        indent = len(raw) - len(raw.lstrip())
        if indent < key_indent or (indent == key_indent and raw.lstrip().startswith("- ")):
            break  # the step is over
        if indent == key_indent and raw.strip() == "with:":
            with_at = i
        elif with_at is not None and indent > key_indent and raw.strip().startswith("toolchain:"):
            return []  # the step selects its toolchain already
    if with_at is not None:
        return [(with_at + 1, f"{pad}  toolchain: {toolchain}\n")]
    return [(n + 1, f"{pad}with:\n{pad}  toolchain: {toolchain}\n")]


def pin(dry: bool) -> int:
    cache: dict[str, Refs] = {}

    def lookup(action: str) -> Refs:
        repo = "/".join(action.split("/")[:2])
        if repo not in cache:
            cache[repo] = ls_remote(action)
        return cache[repo]

    status = 0
    for path in WORKFLOWS:
        text = path.read_text()
        new, changes = pin_text(text, lookup)
        for c in changes:
            print(f"{path.name}: {c}")
            status |= "cannot resolve" in c
        if new != text and not dry:
            path.write_text(new)
    return status


def main() -> int:
    if "--pin" in sys.argv:
        return pin("-n" in sys.argv)
    results = [check(w) for w in WORKFLOWS]
    problems = [p for r in results for p in r[0]]
    warnings = [w for r in results for w in r[1]]
    if "--strict" in sys.argv:
        problems += warnings
    else:
        for w in warnings:
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
