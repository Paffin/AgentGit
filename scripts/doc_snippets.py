#!/usr/bin/env python3
"""Run the command snippets of the documentation against the real binaries, and check what they print.

    python3 scripts/doc_snippets.py [--bin-dir DIR] [PAGE.md ...]     # default: every page with a doctest marker
    python3 scripts/doc_snippets.py --list                            # what would run, without running it

The tutorials under docs/tutorials/ are the documentation people copy from, so every shell block in them is
executed. The binaries (`ag`, and `ag-server` for the pages that need it) come from --bin-dir (default: the
directory of $AG, else target/debug, else target/release). Nothing is mocked: a tutorial that starts a server
starts the real `ag-server` and talks to it with the real `ag`.

Markup (HTML comments, invisible on the rendered page and on GitHub):

    <!-- doctest -->                  the next ```bash block is executed
    ```bash
    ag init --namespace me/hello
    ag integrate nope               # expect-exit 1
    ```

    <!-- expect -->                   the next ```text block must appear in the output of the block before it
    ```text
    Integrated (fast-forward): stream is now at ... (version 1), 1 file(s) changed.
    ```

    <!-- doctest-skip: why -->       the next shell block is shown but not executed (the reason is mandatory)
    <!-- doctest-port 7878 -->        this literal port in the page is replaced by a free one while the page runs
    <!-- doctest-needs ag-server -->  the page needs this binary (see --optional)
    <!-- policy: NAME -->              the next ```json block is a policy document: it is written to
                                    $AGDT_POLICIES/NAME.json while the page runs, so a snippet can apply the very
                                    JSON that the page shows. (crates/ag-policy/tests/cookbook.rs parses every such
                                    block with the real policy parser.) `policy-invalid: NAME: why` and `json`
                                    mark other JSON blocks, which the harness only skips.

How it runs. A page is one scratch directory (with its own HOME, so nothing of the user's is touched) and ONE bash
process that executes all its blocks in order, so `cd`, `export` and shell variables carry over from block to
block, like in a terminal. Every top-level statement must exit with 0 unless it ends in `# expect-exit N`;
the first statement that does not stops the page. A statement is a line, or several lines when it has a
heredoc, a trailing backslash or an open quote/compound. Background jobs (`ag-server serve ... &`) are killed
when the page ends. A page has a time limit (--timeout).

Matching `expect` blocks: lines are compared one by one after trimming trailing whitespace; `...` inside a line
matches any text, and a line that is only `...` matches any number of lines (including none). The expected
lines must appear as consecutive lines of the block's output (stdout and stderr together), anywhere in it.

Rules the harness enforces so that the pages cannot quietly stop being tested: a page with a doctest marker may
not contain a shell block without a marker (`doctest` or `doctest-skip: reason`), an `expect` block must follow a
`doctest` block, and every marker must be followed by the block it announces.
"""
from __future__ import annotations

import argparse
import dataclasses
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
SENTINEL = "@@AGDT@@"
FENCE = re.compile(r"^(?P<indent>\s*)(?P<fence>`{3,}|~{3,})\s*(?P<lang>[A-Za-z0-9_+-]*)\s*$")
MARK = re.compile(r"^\s*<!--\s*(?P<body>.*?)\s*-->\s*$")
SHELL_LANGS = {"bash", "sh", "shell", "console"}
HEREDOC = re.compile(r"<<-?\s*(?P<q>['\"]?)(?P<tag>[A-Za-z_][A-Za-z0-9_]*)(?P=q)")
EXPECT_EXIT = re.compile(r"#\s*expect-exit\s+(\d+)\s*$")


class DocError(Exception):
    """A problem with the markup of a page (not a failing snippet)."""


@dataclasses.dataclass
class Stmt:
    text: str
    expect_exit: int
    line: int  # 1-based line of the page where the statement starts


@dataclasses.dataclass
class Block:
    stmts: list[Stmt]
    line: int
    expects: list[tuple[list[str], int]] = dataclasses.field(default_factory=list)  # (expected lines, line)


@dataclasses.dataclass
class Page:
    path: Path
    blocks: list[Block]
    port: str | None
    needs: list[str]
    policies: dict[str, str] = dataclasses.field(default_factory=dict)


# -- parsing ------------------------------------------------------------------------------------------------


def _complete(text: str) -> bool:
    """True if `text` is a complete shell statement (bash can parse it to the end)."""
    p = subprocess.run(["bash", "-n"], input=text, capture_output=True, text=True)
    if p.returncode == 0:
        return True
    return not re.search(r"unexpected end of file|unexpected EOF|looking for matching", p.stderr)


def split_statements(lines: list[str], first_line: int) -> list[Stmt]:
    """Cut a block into statements: a line, or more when a heredoc, a continuation or an open construct needs it."""
    stmts: list[Stmt] = []
    i = 0
    while i < len(lines):
        start = i
        cur = [lines[i]]
        i += 1
        if not cur[0].strip() or cur[0].lstrip().startswith("#"):
            continue
        while True:
            text = "\n".join(cur)
            heredocs = [m.group("tag") for m in HEREDOC.finditer(text)]
            open_heredoc = False
            if heredocs:
                # every heredoc tag needs its terminator line somewhere after its introducing line
                body = cur[1:]
                tags = list(heredocs)
                for ln in body:
                    if tags and ln.strip() == tags[0]:
                        tags.pop(0)
                open_heredoc = bool(tags)
            if open_heredoc or cur[-1].rstrip().endswith("\\") or not _complete(text):
                if i >= len(lines):
                    raise DocError(f"line {first_line + start}: the statement is not finished at the end of the block")
                cur.append(lines[i])
                i += 1
                continue
            break
        text = "\n".join(cur)
        # the marker sits on the last line of the statement, or on the first when a heredoc follows
        m = EXPECT_EXIT.search(cur[-1]) or EXPECT_EXIT.search(cur[0])
        stmts.append(Stmt(text=text, expect_exit=int(m.group(1)) if m else 0, line=first_line + start))
    return stmts


def parse_page(path: Path) -> Page:
    lines = path.read_text().split("\n")
    blocks: list[Block] = []
    port: str | None = None
    needs: list[str] = []
    pending: str | None = None  # "doctest" | "skip" | "expect" | "policy" | "json"
    pending_name = ""
    policies: dict[str, str] = {}
    pending_line = 0
    last_block: Block | None = None
    uses_markers = any(
        MARK.match(ln) and MARK.match(ln).group("body").startswith(("doctest", "expect")) for ln in lines
    )
    i = 0
    while i < len(lines):
        ln = lines[i]
        m = MARK.match(ln)
        if m:
            body = m.group("body")
            if body == "doctest":
                pending, pending_line = "doctest", i + 1
            elif body.startswith("doctest-skip"):
                if not re.match(r"doctest-skip:\s*\S", body):
                    raise DocError(f"{path}:{i + 1}: doctest-skip needs a reason (`<!-- doctest-skip: why -->`)")
                pending, pending_line = "skip", i + 1
            elif body == "expect":
                pending, pending_line = "expect", i + 1
            elif re.match(r"policy:\s*[A-Za-z0-9_-]+$", body):
                pending, pending_line = "policy", i + 1
                pending_name = body.split(":", 1)[1].strip()
            elif body.startswith("policy-invalid:") or body == "json":
                pending, pending_line = "json", i + 1
            elif body.startswith("doctest-port"):
                mm = re.match(r"doctest-port\s+(\d+)$", body)
                if not mm:
                    raise DocError(f"{path}:{i + 1}: expected `<!-- doctest-port NNNN -->`")
                port = mm.group(1)
            elif body.startswith("doctest-needs"):
                needs += body.split()[1:]
            i += 1
            continue
        f = FENCE.match(ln)
        if f:
            fence, lang = f.group("fence"), f.group("lang")
            j = i + 1
            while j < len(lines) and not (lines[j].strip().startswith(fence) and lines[j].strip().strip(fence[0]) == ""):
                j += 1
            if j >= len(lines):
                raise DocError(f"{path}:{i + 1}: unclosed code fence")
            body_lines = [re.sub(rf"^{f.group('indent')}", "", x) for x in lines[i + 1 : j]]
            if pending == "doctest":
                if lang not in SHELL_LANGS:
                    raise DocError(f"{path}:{pending_line}: a doctest block must be ```bash (found ```{lang})")
                last_block = Block(stmts=split_statements(body_lines, i + 2), line=i + 1)
                blocks.append(last_block)
            elif pending == "expect":
                if last_block is None:
                    raise DocError(f"{path}:{pending_line}: an expect block needs a doctest block before it")
                last_block.expects.append(([x.rstrip() for x in body_lines], i + 1))
            elif pending == "policy":
                if lang != "json":
                    raise DocError(f"{path}:{pending_line}: a policy block must be ```json (found ```{lang})")
                if pending_name in policies:
                    raise DocError(f"{path}:{pending_line}: policy name '{pending_name}' is used twice")
                policies[pending_name] = "\n".join(body_lines) + "\n"
            elif pending in ("skip", "json"):
                pass
            elif uses_markers and lang in SHELL_LANGS:
                raise DocError(
                    f"{path}:{i + 1}: shell block without `<!-- doctest -->` or `<!-- doctest-skip: reason -->` "
                    "in a page that is tested"
                )
            pending = None
            i = j + 1
            continue
        if pending and ln.strip():
            raise DocError(f"{path}:{pending_line}: the marker is not followed by a code block (found text at line {i + 1})")
        i += 1
    if pending:
        raise DocError(f"{path}:{pending_line}: the marker is not followed by a code block")
    return Page(path=path, blocks=blocks, port=port, needs=needs, policies=policies)


# -- matching -----------------------------------------------------------------------------------------------


def line_matches(expected: str, actual: str) -> bool:
    actual = actual.rstrip()
    if "..." not in expected:
        return expected == actual
    pattern = ".*".join(re.escape(part) for part in expected.split("..."))
    return re.fullmatch(pattern, actual) is not None


def find_expected(expected: list[str], actual: list[str]) -> bool:
    """Do the `expected` lines appear, in order and consecutive, somewhere in `actual`? (`...` line = any lines)"""
    # drop blank edges: they are layout in the page, not output
    while expected and not expected[0]:
        expected = expected[1:]
    while expected and not expected[-1]:
        expected = expected[:-1]
    if not expected:
        return True
    memo: dict[tuple[int, int], bool] = {}

    def go(e: int, a: int) -> bool:
        if e == len(expected):
            return True
        key = (e, a)
        if key in memo:
            return memo[key]
        if expected[e] == "...":
            res = any(go(e + 1, a + k) for k in range(0, len(actual) - a + 1))
        elif a < len(actual) and line_matches(expected[e], actual[a]):
            res = go(e + 1, a + 1)
        else:
            res = False
        memo[key] = res
        return res

    return any(go(0, s) for s in range(len(actual) + 1))


# -- running ------------------------------------------------------------------------------------------------


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def build_script(page: Page, port: str | None, real_port: int | None) -> tuple[str, list[tuple[int, int, Stmt]]]:
    """The bash program for a page and the table that maps its sentinels back to statements."""
    sub = (lambda t: t.replace(port, str(real_port))) if port and real_port else (lambda t: t)
    out = [
        f'__agdt_end() {{ local rc=$?; printf "\\n{SENTINEL} %s %s %s\\n" "$1" "$2" "$rc"; '
        '[ "$rc" = "$3" ] || exit 97; }',
    ]
    table: list[tuple[int, int, Stmt]] = []
    for b, block in enumerate(page.blocks):
        out.append(f'printf "{SENTINEL} BLOCK %s\\n" {b}')
        for s, stmt in enumerate(block.stmts):
            out.append(sub(stmt.text))
            out.append(f"__agdt_end {b} {s} {stmt.expect_exit}")
            table.append((b, s, stmt))
    return "\n".join(out) + "\n", table


@dataclasses.dataclass
class Result:
    ok: bool
    messages: list[str]


def run_page(page: Page, bin_dir: Path, timeout: int, keep: bool, show: bool = False) -> Result:
    msgs: list[str] = []
    work = Path(tempfile.mkdtemp(prefix="agdoc-"))
    home = work / "home"
    cwd = work / "work"
    home.mkdir()
    cwd.mkdir()
    real_port = free_port() if page.port else None
    script, table = build_script(page, page.port, real_port)
    script_path = work / "page.sh"
    script_path.write_text(script)
    policies = work / "policies"
    policies.mkdir()
    for name, text in page.policies.items():
        (policies / f"{name}.json").write_text(text)
    log_path = work / "out.log"
    env = {
        "PATH": f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}",
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "TMPDIR": str(work),
        "NO_COLOR": "1",
        "LC_ALL": "C",
        "USER": "tutorial",
        "TERM": "dumb",
        "AG_DOCTEST": "1",
        "AGDT_POLICIES": str(policies),
    }
    for keep_var in ("SYSTEMROOT", "RUST_BACKTRACE"):
        if keep_var in os.environ:
            env[keep_var] = os.environ[keep_var]
    ok = True
    with open(log_path, "wb") as log:
        proc = subprocess.Popen(
            ["bash", "--noprofile", "--norc", str(script_path)],
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            code = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            ok = False
            code = -1
            msgs.append(f"{page.path.name}: timed out after {timeout}s")
        finally:
            try:
                os.killpg(proc.pid, signal.SIGKILL)  # background jobs (a server) end with the page
            except ProcessLookupError:
                pass
            proc.wait()
    text = log_path.read_bytes().decode("utf-8", "replace")
    # split into per-statement output
    outputs: dict[int, list[str]] = {}  # block -> lines
    stmt_out: dict[tuple[int, int], str] = {}
    results: dict[tuple[int, int], int] = {}
    buf: list[str] = []
    cur_block = -1
    for raw in text.split("\n"):
        mm = re.match(rf"^{SENTINEL} (BLOCK|\d+) (\d+)(?: (\d+))?$", raw)
        if mm:
            if mm.group(1) == "BLOCK":
                cur_block = int(mm.group(2))
                buf = []
            else:
                b, s, rc = int(mm.group(1)), int(mm.group(2)), int(mm.group(3))
                # drop the blank line that the sentinel's leading newline added
                if buf and buf[-1] == "":
                    buf.pop()
                stmt_out[(b, s)] = "\n".join(buf)
                outputs.setdefault(b, []).extend(buf)
                results[(b, s)] = rc
                buf = []
            continue
        buf.append(raw)
    if show:
        for b, s, stmt in table:
            if (b, s) in results:
                print(f"$ {stmt.text}   # exit {results[(b, s)]}")
                if stmt_out[(b, s)]:
                    print(stmt_out[(b, s)])
    failed_at: tuple[int, int] | None = None
    for b, s, stmt in table:
        if (b, s) not in results:
            if failed_at is None and ok:
                # the statement that was running when the page stopped (timeout or crash)
                failed_at = (b, s)
                ok = False
                tail = "\n".join(buf[-20:])
                msgs.append(
                    f"{page.path.name}:{stmt.line}: the statement never finished (page exit status {code})\n"
                    f"    $ {stmt.text}\n    output so far:\n" + _indent(tail)
                )
            continue
        if results[(b, s)] != stmt.expect_exit:
            ok = False
            msgs.append(
                f"{page.path.name}:{stmt.line}: exit status {results[(b, s)]}, expected {stmt.expect_exit}\n"
                f"    $ {stmt.text}\n    output:\n" + _indent(stmt_out[(b, s)])
            )
            break
    if ok:
        for b, block in enumerate(page.blocks):
            got = outputs.get(b, [])
            for expected, line in block.expects:
                if not find_expected(expected, got):
                    ok = False
                    msgs.append(
                        f"{page.path.name}:{line}: the output of the block at line {block.line} does not contain\n"
                        + _indent("\n".join(expected))
                        + "\n    the output was:\n"
                        + _indent("\n".join(got))
                    )
    if keep or not ok:
        msgs.append(f"    (scratch directory kept: {work})")
    else:
        shutil.rmtree(work, ignore_errors=True)
    return Result(ok=ok, messages=msgs)


def _indent(text: str) -> str:
    return "\n".join("      | " + ln for ln in text.split("\n"))


# -- driver -------------------------------------------------------------------------------------------------


def shown(path: Path) -> Path:
    """The path as it is printed: relative to the repository when it is inside it."""
    return path.relative_to(ROOT) if path.is_relative_to(ROOT) else path


def find_pages() -> list[Path]:
    pages = []
    for p in sorted(DOCS.rglob("*.md")):
        if re.search(r"^\s*<!--\s*doctest\s*-->", p.read_text(), re.M):
            pages.append(p)
    return pages


def default_bin_dir() -> Path:
    if os.environ.get("AG"):
        return Path(os.environ["AG"]).resolve().parent
    for rel in ("target/debug", "target/release"):
        if (ROOT / rel / "ag").exists():
            return ROOT / rel
    sys.exit("ag binary not found: build it (cargo build -p ag-cli -p ag-server) or pass --bin-dir")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("pages", nargs="*", type=Path)
    ap.add_argument("--bin-dir", type=Path)
    ap.add_argument("--list", action="store_true", help="parse the pages and list what would run")
    ap.add_argument("--timeout", type=int, default=300, help="seconds per page (default 300)")
    ap.add_argument("--keep", action="store_true", help="keep the scratch directories")
    ap.add_argument("--show", action="store_true", help="print every statement with its output and exit status")
    ap.add_argument(
        "--skip-needing",
        action="append",
        default=[],
        metavar="BIN",
        help="never run pages that need this binary, even if it exists (cargo test uses it for ag-server: the "
        "binary in target/ may be stale; the Docs workflow builds it fresh)",
    )
    ap.add_argument(
        "--optional",
        action="append",
        default=[],
        metavar="BIN",
        help="a binary that may be missing: pages that need it are skipped with a note instead of failing",
    )
    args = ap.parse_args(argv)

    pages = [p.resolve() for p in args.pages] or find_pages()
    if not pages:
        print("no page has a doctest marker", file=sys.stderr)
        return 1
    parsed: list[Page] = []
    for p in pages:
        try:
            parsed.append(parse_page(p))
        except DocError as e:
            print(f"error: {e}", file=sys.stderr)
            return 1
    if args.list:
        for pg in parsed:
            n = sum(len(b.stmts) for b in pg.blocks)
            e = sum(len(b.expects) for b in pg.blocks)
            print(f"{shown(pg.path)}: {len(pg.blocks)} block(s), {n} statement(s), {e} expectation(s)")
        return 0

    bin_dir = (args.bin_dir or default_bin_dir()).resolve()
    if not (bin_dir / "ag").exists():
        print(f"error: {bin_dir}/ag does not exist", file=sys.stderr)
        return 1
    failures = 0
    ran = 0
    for pg in parsed:
        rel = shown(pg.path)
        excluded = [n for n in pg.needs if n in args.skip_needing]
        if excluded:
            print(f"skipped {rel}: needs {', '.join(excluded)}, which this run does not use")
            continue
        missing = [n for n in pg.needs if not (bin_dir / n).exists() and not shutil.which(n)]
        if missing:
            if all(n in args.optional for n in missing):
                print(f"skipped {rel}: needs {', '.join(missing)}, which is not built here")
                continue
            print(f"FAIL {rel}: needs {', '.join(missing)}, which was not found in {bin_dir} or on PATH")
            failures += 1
            continue
        t0 = time.monotonic()
        res = run_page(pg, bin_dir, args.timeout, args.keep, args.show)
        ran += 1
        stmts = sum(len(b.stmts) for b in pg.blocks)
        if res.ok:
            print(f"ok   {rel}: {stmts} statement(s), {sum(len(b.expects) for b in pg.blocks)} expectation(s), "
                  f"{time.monotonic() - t0:.1f}s")
        else:
            failures += 1
            print(f"FAIL {rel}")
            for m in res.messages:
                print(m)
    print(f"{ran} page(s) run, {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
