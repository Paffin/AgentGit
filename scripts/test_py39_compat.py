#!/usr/bin/env python3
"""The lightweight scripts must run on Python 3.9 (what an older laptop or CI image has), not only on the 3.12 the
developers use. The reviewer who tried `python3 scripts/e2e_smoke.py` on 3.9 got `TypeError: unsupported operand
type(s) for |`: an annotation `str | None` is evaluated when the function is defined, and `type | None` exists only
from Python 3.10. `from __future__ import annotations` turns annotations into strings and fixes it.

    python3 scripts/test_py39_compat.py       # on any Python; CI also runs it (and the unit tests of the scripts) on 3.9

Two kinds of check. The static ones run on whatever Python runs this file: no file under scripts/, sdk/tools,
examples/agents or tests/conformance parses differently under the 3.9 grammar, evaluates a `X | Y` annotation at run
time, or uses a union in `isinstance`. The import ones matter on 3.9 itself: the modules that are safe to import (they
have no side effects at import) are imported, which evaluates every annotation for real.

Scripts that read TOML (`tomllib`, Python 3.11+) are not imported on 3.9; they say so in their own docstring.
"""

from __future__ import annotations

import ast
import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.dont_write_bytecode = True
# The directories whose Python runs outside a project environment. The SDKs (3.10+) and the docs toolchain have their own.
DIRS = ["scripts", "sdk/tools", "examples/agents", "tests/conformance"]
# Safe to import: no work at import time. (check_docs, check_deploy and e2e_observability run or parse arguments.)
IMPORTABLE = [
    "scripts/ag_testkit.py",
    "scripts/check_skips.py",
    "scripts/e2e_smoke.py",
    "scripts/package_release.py",
    "scripts/check_release.py",
    "examples/agents/agentgit_stack.py",
    "examples/agents/mcp_client.py",
]


def python_files() -> list[Path]:
    out: list[Path] = []
    for d in DIRS:
        out += sorted(p for p in (ROOT / d).rglob("*.py") if not any(x.startswith(".venv") for x in p.parts))
    return out


def has_future_annotations(tree: ast.Module) -> bool:
    return any(
        isinstance(n, ast.ImportFrom) and n.module == "__future__" and any(a.name == "annotations" for a in n.names)
        for n in tree.body
    )


def has_union(node: ast.AST | None) -> bool:
    return node is not None and any(isinstance(n, ast.BinOp) and isinstance(n.op, ast.BitOr) for n in ast.walk(node))


def evaluated_annotations(tree: ast.Module) -> list[tuple[int, ast.AST]]:
    """The annotations Python evaluates when the module is imported: those of function signatures and of module
    and class level variables (not of local variables)."""
    found: list[tuple[int, ast.AST]] = []

    def visit(body: list[ast.stmt]) -> None:
        for n in body:
            if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
                args = n.args
                for a in args.posonlyargs + args.args + args.kwonlyargs + [x for x in (args.vararg, args.kwarg) if x]:
                    if a.annotation is not None:
                        found.append((a.lineno, a.annotation))
                if n.returns is not None:
                    found.append((n.lineno, n.returns))
                # annotations inside a function body are not evaluated, but nested classes/defs are defined only
                # when it runs; they are covered by the same rule when they are reached at run time, so skip them
            elif isinstance(n, ast.ClassDef):
                visit(n.body)
            elif isinstance(n, ast.AnnAssign):
                found.append((n.lineno, n.annotation))
            elif isinstance(n, (ast.If, ast.Try, ast.With)):
                for field in ("body", "orelse", "finalbody"):
                    visit(getattr(n, field, []))

    visit(tree.body)
    return found


def runtime_unions(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path), feature_version=(3, 9))
    problems = []
    if not has_future_annotations(tree):
        for line, ann in evaluated_annotations(tree):
            if has_union(ann):
                problems.append(f"{path.relative_to(ROOT)}:{line}: an annotation with `|` is evaluated at run time (add `from __future__ import annotations`)")
    for n in ast.walk(tree):
        if isinstance(n, ast.Call) and getattr(n.func, "id", "") in ("isinstance", "issubclass") and len(n.args) == 2 and has_union(n.args[1]):
            problems.append(f"{path.relative_to(ROOT)}:{n.lineno}: a `|` union in {n.func.id}() needs Python 3.10 (use a tuple)")  # type: ignore[attr-defined]
        if isinstance(n, getattr(ast, "Match", ())):
            problems.append(f"{path.relative_to(ROOT)}:{n.lineno}: `match` needs Python 3.10")
    return problems


class Static(unittest.TestCase):
    def test_the_files_were_found(self):
        names = {p.name for p in python_files()}
        self.assertTrue({"e2e_smoke.py", "check_skips.py", "mcp_client.py"} <= names, names)

    def test_every_file_parses_with_the_3_9_grammar(self):
        for p in python_files():
            with self.subTest(file=str(p.relative_to(ROOT))):
                ast.parse(p.read_text(encoding="utf-8"), filename=str(p), feature_version=(3, 9))

    def test_no_annotation_with_a_union_is_evaluated_at_run_time(self):
        problems: list[str] = []
        for p in python_files():
            problems += runtime_unions(p)
        self.assertEqual(problems, [], "\n".join(problems))

    def test_the_check_itself_finds_the_bug_the_reviewer_hit(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            bad = Path(d) / "bad.py"
            bad.write_text("def f(x: str | None = None) -> int | None:\n    return None\n")
            good = Path(d) / "good.py"
            good.write_text("from __future__ import annotations\n\ndef f(x: str | None = None) -> int | None:\n    y: int | None = None\n    return y\n")
            local = Path(d) / "local.py"
            local.write_text("def f():\n    y: int | None = None\n    return y\n")
            iso = Path(d) / "iso.py"
            iso.write_text("def f(x):\n    return isinstance(x, int | str)\n")
            global ROOT
            old, ROOT = ROOT, Path(d)
            try:
                self.assertEqual(len(runtime_unions(bad)), 2)
                self.assertEqual(runtime_unions(good), [])
                self.assertEqual(runtime_unions(local), [])
                self.assertEqual(len(runtime_unions(iso)), 1)
            finally:
                ROOT = old


class Imports(unittest.TestCase):
    def test_the_modules_that_are_safe_to_import_import(self):
        for rel in IMPORTABLE:
            with self.subTest(module=rel):
                path = ROOT / rel
                spec = importlib.util.spec_from_file_location("compat_" + path.stem, path)
                assert spec is not None and spec.loader is not None
                module = importlib.util.module_from_spec(spec)
                sys.modules[spec.name] = module
                try:
                    spec.loader.exec_module(module)
                finally:
                    sys.modules.pop(spec.name, None)


if __name__ == "__main__":
    unittest.main()
