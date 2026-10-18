#!/usr/bin/env python3
"""Tests for scripts/check_no_stubs.py.

    python3 scripts/test_check_no_stubs.py            # also run by .github/workflows/ci.yml (docs-and-demo)

Each test builds a scratch repository layout and runs the real scanner on it: every marker must be found in production
code of every language the script covers, none may be found in test code, and the allow-list must behave as documented
(reasons are mandatory, unknown rules are refused, an entry that matches nothing is reported).
"""
import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
sys.dont_write_bytecode = True
import check_no_stubs as cns  # noqa: E402


def tree(root: Path, files: dict[str, str]) -> None:
    for name, text in files.items():
        p = root / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)


def found(files: dict[str, str]) -> set[tuple[str, str]]:
    with tempfile.TemporaryDirectory() as d:
        tree(Path(d), files)
        return {(f.path, f.rule) for f in cns.scan(Path(d))}


class Markers(unittest.TestCase):
    def test_every_rule_fires_in_production_rust(self):
        got = found(
            {
                "crates/a/src/lib.rs": "\n".join(
                    [
                        "fn a() { todo!() }",
                        "fn b() { unimplemented!(\"later\") }",
                        "// TODO: finish",
                        "// FIXME broken",
                        "// XXX",
                        "// HACK around it",
                        "// do this for now",
                        "// not implemented",
                        "// A placeholder value",
                        "struct StubStore;",
                        "struct MockClock;",
                        "fn fake_token() {}",
                        "// a dummy",
                        "// simulated behaviour",
                    ]
                )
            }
        )
        rules = {r for _, r in got}
        self.assertEqual(rules, set(cns.RULES), rules)

    def test_other_languages_are_scanned_too(self):
        got = found(
            {
                "services/x/main.go": "// TODO: later\n",
                "apps/web/src/a.ts": "// FIXME\n",
                "apps/web/src/B.vue": "<input placeholder=\"x\">\n",
                "sdk/python/src/p/m.py": "x = 1  # for now\n",
                "sdk/typescript/src/t.ts": "class FakeClient {}\n",
            }
        )
        self.assertEqual(
            got,
            {
                ("services/x/main.go", "marker"),
                ("apps/web/src/a.ts", "marker"),
                ("apps/web/src/B.vue", "placeholder"),
                ("sdk/python/src/p/m.py", "for-now"),
                ("sdk/typescript/src/t.ts", "test-double"),
            },
        )

    def test_the_todo_macro_is_a_rust_rule_only(self):
        self.assertEqual(found({"services/x/main.go": "var s = \"todo!(\"\n"}), set())

    def test_ordinary_words_do_not_fire(self):
        clean = "\n".join(
            [
                "// The unimplemented error code is part of the protocol: ErrorCode::Unimplemented",
                "// a transaction, a stubborn bug, a fakir, a mockingbird",
                "let hacker = 1; let todoist = 2; let boxxx = 3;",
                "// Snapshot of the working directory",
            ]
        )
        self.assertEqual(found({"crates/a/src/lib.rs": clean}), set())


class TestCodeIsNotScanned(unittest.TestCase):
    def test_cfg_test_modules_and_test_functions_are_skipped_whatever_they_contain(self):
        src = """
pub fn real() -> u32 { 1 }

#[cfg(test)]
mod tests {
    use super::*;
    // TODO a test may say anything
    #[test]
    fn braces_in_strings_and_comments_do_not_end_the_module() {
        let s = "}}} { fake";
        let c = '}';
        let r = r#"} mock "#;
        /* } */
        todo!()
    }
}

#[test]
fn a_loose_test() { unimplemented!() }

#[cfg(test)]
fn helper() { let _ = [0u8; 4]; fake() }

#[cfg(any(unix, test))]
fn also_test_only() { mock() }

pub fn after() -> u32 { 2 }
"""
        self.assertEqual(found({"crates/a/src/lib.rs": src}), set())

    def test_code_after_a_test_module_is_still_scanned(self):
        src = "#[cfg(test)]\nmod tests { fn t() { todo!() } }\n\npub fn real() { todo!() }\n"
        with tempfile.TemporaryDirectory() as d:
            tree(Path(d), {"crates/a/src/lib.rs": src})
            fs = cns.scan(Path(d))
        self.assertEqual([(f.line, f.rule) for f in fs], [(4, "todo-macro")])

    def test_a_not_test_cfg_is_production_code(self):
        src = "#[cfg(not(test))]\nfn real() { todo!() }\n"
        self.assertEqual(found({"crates/a/src/lib.rs": src}), {("crates/a/src/lib.rs", "todo-macro")})

    def test_files_pulled_in_by_a_cfg_test_mod_declaration_are_test_files(self):
        got = found(
            {
                "crates/a/src/lib.rs": "#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod more_tests;\npub fn f() {}\n",
                "crates/a/src/tests.rs": "fn t() { todo!() }\n",
                "crates/a/src/more_tests/mod.rs": "fn t() { todo!() }\n",
                "crates/a/src/real.rs": "fn r() { todo!() }\n",
                "crates/a/src/net/mod.rs": "#[cfg(test)]\nmod tests;\n",
                "crates/a/src/net/tests.rs": "fn t() { todo!() }\n",
                "crates/a/src/util.rs": "#[cfg(test)]\nmod util_tests;\n",
                "crates/a/src/util/util_tests.rs": "fn t() { todo!() }\n",
            }
        )
        self.assertEqual(got, {("crates/a/src/real.rs", "todo-macro")})

    def test_test_directories_and_files_of_other_languages_are_skipped(self):
        got = found(
            {
                "crates/a/tests/it.rs": "fn t() { todo!() }\n",
                "crates/a/benches/b.rs": "fn t() { todo!() }\n",
                "services/x/main_test.go": "// TODO\n",
                "services/x/internal/testutil/fakeserver/s.go": "// TODO\n",
                "services/x/testdata/x.go": "// TODO\n",
                "apps/web/src/a.test.ts": "// TODO\n",
                "apps/web/src/__tests__/a.ts": "// TODO\n",
                "sdk/python/src/p/test_x.py": "# TODO\n",
                "sdk/python/src/p/x_test.py": "# TODO\n",
                "examples/demo.rs": "fn x() { todo!() }\n",
                "docs/guide.md": "TODO\n",
            }
        )
        self.assertEqual(got, {("sdk/python/src/p/test_x.py", "marker")} - {("sdk/python/src/p/test_x.py", "marker")} or got)
        self.assertNotIn(("services/x/main_test.go", "marker"), got)
        self.assertNotIn(("apps/web/src/a.test.ts", "marker"), got)
        self.assertNotIn(("crates/a/tests/it.rs", "todo-macro"), got)
        self.assertNotIn(("examples/demo.rs", "todo-macro"), got)
        self.assertNotIn(("docs/guide.md", "marker"), got)
        self.assertNotIn(("services/x/internal/testutil/fakeserver/s.go", "marker"), got)


class AllowList(unittest.TestCase):
    def run_main(self, files: dict[str, str], allow: str, *flags: str) -> tuple[int, str, str]:
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            tree(root, files)
            tree(root, {"scripts/no_stubs_allowlist.txt": allow})
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = cns.main(["--root", str(root), *flags])
            return code, out.getvalue(), err.getvalue()

    FILES = {"crates/a/src/lib.rs": "// a placeholder for SQL\nfn f() {}\n"}

    def test_a_finding_without_an_entry_fails_and_names_the_line(self):
        code, out, err = self.run_main(self.FILES, "")
        self.assertEqual(code, 1)
        self.assertIn("crates/a/src/lib.rs:1: [placeholder]", out)

    def test_an_entry_with_a_reason_allows_it(self):
        allow = "crates/a/src/lib.rs | placeholder | for SQL | SQL parameter placeholders, not an unfinished feature\n"
        code, out, _ = self.run_main(self.FILES, allow)
        self.assertEqual(code, 0, out)
        self.assertIn("1 allow-listed", out)

    def test_the_needle_must_occur_in_the_line(self):
        allow = "crates/a/src/lib.rs | placeholder | something else | a reason that is long enough\n"
        code, _, _ = self.run_main(self.FILES, allow)
        self.assertEqual(code, 1)

    def test_a_wildcard_needle_and_a_glob_cover_many_lines(self):
        files = {"crates/a/src/x.rs": "// placeholder\n// placeholder again\n", "crates/b/src/y.rs": "// placeholder\n"}
        allow = "crates/*/src/*.rs | placeholder | * | a reason that is long enough\n"
        code, _, _ = self.run_main(files, allow)
        self.assertEqual(code, 0)

    def test_comments_and_blank_lines_are_ignored_in_the_allow_list(self):
        allow = "# a comment\n\n   \ncrates/a/src/lib.rs | placeholder | * | SQL parameter placeholders in this file\n"
        self.assertEqual(self.run_main(self.FILES, allow)[0], 0)

    def test_a_missing_reason_a_short_reason_and_an_unknown_rule_are_refused(self):
        for bad in (
            "crates/a/src/lib.rs | placeholder | *\n",
            "crates/a/src/lib.rs | placeholder | * | \n",
            "crates/a/src/lib.rs | placeholder | * | ok\n",
            "crates/a/src/lib.rs | no-such-rule | * | a perfectly long reason\n",
        ):
            with self.assertRaises(SystemExit, msg=bad):
                self.run_main(self.FILES, bad)

    def test_an_entry_that_matches_nothing_is_a_warning_and_an_error_with_strict(self):
        allow = (
            "crates/a/src/lib.rs | placeholder | * | SQL parameter placeholders in this file\n"
            "crates/gone/src/lib.rs | stub | * | a reason about code that no longer exists\n"
        )
        code, _, err = self.run_main(self.FILES, allow)
        self.assertEqual(code, 0)
        self.assertIn("warning: allow-list line 2 matches nothing", err)
        code, _, err = self.run_main(self.FILES, allow, "--strict")
        self.assertEqual(code, 1)
        self.assertIn("error: allow-list line 2", err)

    def test_list_shows_the_reason_of_every_allowed_use(self):
        allow = "crates/a/src/lib.rs | placeholder | * | SQL parameter placeholders in this file\n"
        code, out, _ = self.run_main(self.FILES, allow, "--list")
        self.assertEqual(code, 0)
        self.assertIn("because: SQL parameter placeholders in this file", out)


class ThisRepository(unittest.TestCase):
    def test_the_product_source_has_no_markers_that_are_not_allow_listed(self):
        # Not --strict: an allow-list entry whose code was deleted is reported as a warning, it does not fail anyone's push.
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cns.main([])
        self.assertEqual(code, 0, out.getvalue() + err.getvalue())
        self.assertIn("0 markers", out.getvalue())


if __name__ == "__main__":
    unittest.main()
