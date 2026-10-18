#!/usr/bin/env python3
"""Tests for scripts/doc_snippets.py (the harness that runs the commands in the documentation).

    python3 scripts/test_doc_snippets.py            # also run by .github/workflows/docs.yml

They test the harness itself: how statements are cut, how expected output is matched, which markup mistakes are
refused, and that a page really stops at a failing statement, enforces `# expect-exit`, substitutes its port, extracts
policy documents and does not leave background jobs behind. The pages they run only use plain shell commands, so no
`ag` binary is needed; the harness insists that `<bin-dir>/ag` exists, so each test makes an empty executable of that
name. The real pages, against the real binaries, are run by the `Docs` workflow.
"""
import os
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
sys.dont_write_bytecode = True
import doc_snippets as ds  # noqa: E402

FENCE = "```"


def page_text(body: str) -> str:
    return textwrap.dedent(body).lstrip("\n").replace("FENCE", FENCE)


class Splitting(unittest.TestCase):
    def test_one_statement_per_line_with_comments_and_blank_lines_ignored(self):
        stmts = ds.split_statements(["# a comment", "", "echo one", "echo two  # trailing"], 10)
        self.assertEqual([s.text for s in stmts], ["echo one", "echo two  # trailing"])
        self.assertEqual([s.line for s in stmts], [12, 13])

    def test_heredocs_continuations_and_open_quotes_make_one_statement(self):
        lines = [
            "cat > f <<'EOF'",
            "line one",
            "$not_expanded",
            "EOF",
            "echo a \\",
            "  b",
            "printf '%s\\n' 'multi",
            "line'",
            "if true; then",
            "  echo yes",
            "fi",
            "echo last",
        ]
        stmts = ds.split_statements(lines, 1)
        self.assertEqual(len(stmts), 5, [s.text for s in stmts])
        self.assertTrue(stmts[0].text.endswith("EOF"))
        self.assertEqual(stmts[-1].text, "echo last")

    def test_two_heredocs_in_one_statement(self):
        stmts = ds.split_statements(["cat <<A; cat <<B", "a", "A", "b", "B", "echo done"], 1)
        self.assertEqual(len(stmts), 2)

    def test_expect_exit_is_read_from_the_comment(self):
        stmts = ds.split_statements(["false  # expect-exit 1", "true", "cat <<EOF  # expect-exit 0", "x", "EOF"], 1)
        self.assertEqual([s.expect_exit for s in stmts], [1, 0, 0])

    def test_an_unfinished_statement_at_the_end_of_a_block_is_an_error(self):
        with self.assertRaises(ds.DocError):
            ds.split_statements(["echo 'never closed"], 1)
        with self.assertRaises(ds.DocError):
            ds.split_statements(["cat <<EOF", "no terminator"], 1)


class Matching(unittest.TestCase):
    ACTUAL = ["Initialized in /tmp/x/hello", "  namespace: me/hello", "Snapshot 123abc in workspace 'default'", "done"]

    def test_literal_lines_must_match_whole_lines(self):
        self.assertTrue(ds.find_expected(["  namespace: me/hello"], self.ACTUAL))
        self.assertFalse(ds.find_expected(["namespace: me/hello"], self.ACTUAL))  # the indentation is part of it
        self.assertFalse(ds.find_expected(["Snapshot"], self.ACTUAL))

    def test_ellipsis_inside_a_line_matches_any_text(self):
        self.assertTrue(ds.find_expected(["Initialized in .../hello"], self.ACTUAL))
        self.assertTrue(ds.find_expected(["Snapshot ... in workspace '...'"], self.ACTUAL))
        self.assertFalse(ds.find_expected(["Snapshot ... in repository"], self.ACTUAL))

    def test_lines_must_be_consecutive_unless_separated_by_an_ellipsis_line(self):
        self.assertTrue(ds.find_expected(["Initialized in ...", "  namespace: me/hello"], self.ACTUAL))
        self.assertFalse(ds.find_expected(["Initialized in ...", "Snapshot ..."], self.ACTUAL))
        self.assertTrue(ds.find_expected(["Initialized in ...", "...", "Snapshot ..."], self.ACTUAL))
        self.assertTrue(ds.find_expected(["Initialized in ...", "...", "done"], self.ACTUAL))
        self.assertTrue(ds.find_expected(["...", "done"], self.ACTUAL))
        self.assertFalse(ds.find_expected(["done", "...", "Initialized in ..."], self.ACTUAL), "order matters")

    def test_blank_edges_and_trailing_whitespace_are_layout(self):
        self.assertTrue(ds.find_expected(["", "done", ""], self.ACTUAL))
        self.assertTrue(ds.line_matches("done", "done   "))


class Markup(unittest.TestCase):
    def parse(self, body: str) -> ds.Page:
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "page.md"
            p.write_text(page_text(body))
            return ds.parse_page(p)

    def test_a_tested_page_may_not_contain_an_unmarked_shell_block(self):
        with self.assertRaisesRegex(ds.DocError, "without"):
            self.parse(
                """
                <!-- doctest -->
                FENCEbash
                true
                FENCE

                FENCEbash
                this one is shown but never run
                FENCE
                """
            )

    def test_skipping_needs_a_reason(self):
        with self.assertRaisesRegex(ds.DocError, "reason"):
            self.parse("<!-- doctest-skip -->\nFENCEbash\ntrue\nFENCE\n")
        page = self.parse("<!-- doctest-skip: needs Docker -->\nFENCEbash\ndocker run x\nFENCE\n")
        self.assertEqual(page.blocks, [])

    def test_expect_needs_a_doctest_block_before_it_and_markers_need_their_block(self):
        with self.assertRaisesRegex(ds.DocError, "doctest block before"):
            self.parse("<!-- expect -->\nFENCEtext\nx\nFENCE\n")
        with self.assertRaisesRegex(ds.DocError, "not followed by a code block"):
            self.parse("<!-- doctest -->\n\nsome prose\n")
        with self.assertRaisesRegex(ds.DocError, "not followed by a code block"):
            self.parse("<!-- doctest -->\n")

    def test_a_doctest_block_must_be_shell(self):
        with self.assertRaisesRegex(ds.DocError, "must be"):
            self.parse("<!-- doctest -->\nFENCEpython\nprint(1)\nFENCE\n")

    def test_port_needs_and_policy_blocks_are_collected(self):
        page = self.parse(
            """
            <!-- doctest-port 7878 -->
            <!-- doctest-needs ag-server -->
            <!-- policy: a-recipe -->
            FENCEjson
            {"max_changed_files": 3}
            FENCE
            <!-- policy-invalid: nope: whatever -->
            FENCEjson
            {"x": 1}
            FENCE
            <!-- doctest -->
            FENCEbash
            true
            FENCE
            """
        )
        self.assertEqual(page.port, "7878")
        self.assertEqual(page.needs, ["ag-server"])
        self.assertEqual(list(page.policies), ["a-recipe"])
        self.assertEqual(len(page.blocks), 1)

    def test_a_policy_name_is_used_once(self):
        with self.assertRaisesRegex(ds.DocError, "twice"):
            self.parse("<!-- policy: a -->\nFENCEjson\n{}\nFENCE\n<!-- policy: a -->\nFENCEjson\n{}\nFENCE\n")

    def test_a_page_without_markers_is_not_tested_and_its_shell_blocks_are_free(self):
        page = self.parse("# Title\n\nFENCEbash\nanything --goes\nFENCE\n")
        self.assertEqual(page.blocks, [])


class Running(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.bin = Path(self.tmp.name) / "bin"
        self.bin.mkdir()
        ag = self.bin / "ag"
        ag.write_text("#!/bin/sh\nexit 0\n")  # never run: the pages below use only plain shell commands
        ag.chmod(0o755)

    def tearDown(self):
        self.tmp.cleanup()

    def run_page(self, body: str, timeout: int = 60):
        p = Path(self.tmp.name) / "page.md"
        p.write_text(page_text(body))
        page = ds.parse_page(p)
        return ds.run_page(page, self.bin, timeout, keep=False)

    def test_a_passing_page_with_state_carried_between_blocks(self):
        res = self.run_page(
            """
            <!-- doctest -->
            FENCEbash
            export WHO=alice
            mkdir work && cd work
            N=$(printf 'three')
            FENCE

            <!-- doctest -->
            FENCEbash
            echo "$WHO is in $(basename "$PWD") with $N"
            false   # expect-exit 1
            FENCE

            <!-- expect -->
            FENCEtext
            alice is in work with three
            FENCE
            """
        )
        self.assertTrue(res.ok, res.messages)

    def test_the_first_unexpected_exit_status_stops_the_page_and_is_reported(self):
        res = self.run_page(
            """
            <!-- doctest -->
            FENCEbash
            echo before
            false
            echo after > ../after.txt
            FENCE
            """
        )
        self.assertFalse(res.ok)
        self.assertIn("exit status 1, expected 0", res.messages[0])
        self.assertIn("false", res.messages[0])
        self.assertFalse(any((Path(self.tmp.name) / "after.txt").exists() for _ in [0]))

    def test_an_exit_status_that_was_expected_to_be_nonzero_but_was_zero_fails(self):
        res = self.run_page("<!-- doctest -->\nFENCEbash\ntrue   # expect-exit 3\nFENCE\n")
        self.assertFalse(res.ok)
        self.assertIn("expected 3", res.messages[0])

    def test_an_expected_output_that_is_not_there_fails_and_shows_the_actual_output(self):
        res = self.run_page(
            """
            <!-- doctest -->
            FENCEbash
            echo hello
            FENCE

            <!-- expect -->
            FENCEtext
            goodbye
            FENCE
            """
        )
        self.assertFalse(res.ok)
        self.assertIn("does not contain", res.messages[0])
        self.assertIn("hello", res.messages[0])

    def test_output_is_matched_per_block_not_across_the_page(self):
        res = self.run_page(
            """
            <!-- doctest -->
            FENCEbash
            echo first
            FENCE

            <!-- doctest -->
            FENCEbash
            echo second
            FENCE

            <!-- expect -->
            FENCEtext
            first
            FENCE
            """
        )
        self.assertFalse(res.ok, "`first` was printed by the block before, not by the block the expectation follows")

    def test_stderr_counts_as_output(self):
        res = self.run_page(
            "<!-- doctest -->\nFENCEbash\necho oops >&2\nFENCE\n<!-- expect -->\nFENCEtext\noops\nFENCE\n"
        )
        self.assertTrue(res.ok, res.messages)

    def test_the_page_port_is_replaced_by_a_free_one(self):
        res = self.run_page(
            """
            <!-- doctest-port 7878 -->
            <!-- doctest -->
            FENCEbash
            P=7878
            test "$P" != "$(printf '%s%s' 78 78)"
            test "$P" -gt 0
            FENCE
            """
        )
        self.assertTrue(res.ok, res.messages)

    def test_policy_documents_are_written_where_the_page_can_read_them(self):
        res = self.run_page(
            """
            <!-- policy: team -->
            FENCEjson
            {"max_changed_files": 3}
            FENCE

            <!-- doctest -->
            FENCEbash
            cat "$AGDT_POLICIES/team.json"
            FENCE

            <!-- expect -->
            FENCEtext
            {"max_changed_files": 3}
            FENCE
            """
        )
        self.assertTrue(res.ok, res.messages)

    def test_the_environment_is_isolated_from_the_user_s(self):
        os.environ["AG_IDENTITY"] = "user:leak"
        try:
            res = self.run_page(
                """
                <!-- doctest -->
                FENCEbash
                test -z "${AG_IDENTITY:-}"
                test "$HOME" != "$OLDHOME"
                FENCE
                """.replace("$OLDHOME", os.path.expanduser("~"))
            )
        finally:
            del os.environ["AG_IDENTITY"]
        self.assertTrue(res.ok, res.messages)

    def test_background_jobs_end_with_the_page_and_do_not_block_it(self):
        marker = Path(self.tmp.name) / "still-running"
        res = self.run_page(
            f"""
            <!-- doctest -->
            FENCEbash
            (sleep 3; touch {marker}) > /dev/null 2>&1 &
            echo started
            FENCE
            """
        )
        self.assertTrue(res.ok, res.messages)
        time.sleep(4)
        self.assertFalse(marker.exists(), "the background job outlived the page")

    def test_a_page_that_never_finishes_is_killed_at_the_time_limit(self):
        t0 = time.monotonic()
        res = self.run_page("<!-- doctest -->\nFENCEbash\nsleep 30\nFENCE\n", timeout=2)
        self.assertFalse(res.ok)
        self.assertLess(time.monotonic() - t0, 15)
        self.assertTrue(any("timed out" in m for m in res.messages), res.messages)


class Command(unittest.TestCase):
    def test_the_command_line_runs_listed_pages_and_reports_failures(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "bin").mkdir()
            ag = Path(d) / "bin" / "ag"
            ag.write_text("#!/bin/sh\nexit 0\n")
            ag.chmod(0o755)
            good = Path(d) / "good.md"
            good.write_text(page_text("<!-- doctest -->\nFENCEbash\ntrue\nFENCE\n"))
            bad = Path(d) / "bad.md"
            bad.write_text(page_text("<!-- doctest -->\nFENCEbash\nfalse\nFENCE\n"))
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
            run = lambda *a: subprocess.run(  # noqa: E731
                [sys.executable, str(SCRIPTS / "doc_snippets.py"), "--bin-dir", str(Path(d) / "bin"), *a],
                capture_output=True, text=True, env=env,
            )
            r = run(str(good))
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertIn("1 page(s) run, 0 failed", r.stdout)
            r = run(str(good), str(bad))
            self.assertEqual(r.returncode, 1)
            self.assertIn("FAIL", r.stdout)
            r = run("--list", str(good))
            self.assertEqual(r.returncode, 0)
            self.assertIn("1 statement(s)", r.stdout)

    def test_a_page_that_needs_a_missing_binary_fails_unless_it_is_optional(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "bin").mkdir()
            ag = Path(d) / "bin" / "ag"
            ag.write_text("#!/bin/sh\nexit 0\n")
            ag.chmod(0o755)
            page = Path(d) / "needs.md"
            page.write_text(page_text("<!-- doctest-needs no-such-binary -->\n<!-- doctest -->\nFENCEbash\ntrue\nFENCE\n"))
            base = [sys.executable, str(SCRIPTS / "doc_snippets.py"), "--bin-dir", str(Path(d) / "bin"), str(page)]
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
            r = subprocess.run(base, capture_output=True, text=True, env=env)
            self.assertEqual(r.returncode, 1, r.stdout)
            self.assertIn("needs no-such-binary", r.stdout)
            r = subprocess.run([*base, "--optional", "no-such-binary"], capture_output=True, text=True, env=env)
            self.assertEqual(r.returncode, 0, r.stdout)
            self.assertIn("skipped", r.stdout)


if __name__ == "__main__":
    unittest.main()
