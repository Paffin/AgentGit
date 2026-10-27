#!/usr/bin/env python3
"""Tests for scripts/mkdocs_hooks.py, the hooks that adapt docs/ to the MkDocs site.

    .venv-docs/bin/python scripts/test_mkdocs_hooks.py      # also run by .github/workflows/docs.yml

The functions are tested directly on small inputs (they only read a config mapping and a page's source path), and,
when MkDocs is importable, by building a throw-away site with the real MkDocs and the real hook file and reading the
HTML it produced: a link out of `docs/` must become a link to the repository, a page missing from the navigation must
still be reachable, and a link to a file that does not exist must still fail a strict build.
"""
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from types import SimpleNamespace

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
sys.dont_write_bytecode = True
import ag_testkit  # noqa: E402
import mkdocs_hooks as hooks  # noqa: E402


def project(root: Path, files: dict[str, str]) -> None:
    for name, text in files.items():
        p = root / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(textwrap.dedent(text).lstrip("\n"))


MKDOCS_YML = """
site_name: t
repo_url: https://example.invalid/acme/repo
docs_dir: docs
strict: true
hooks:
  - {hooks}
nav:
  - Home: README.md
  - Guide: guide.md
validation:
  anchors: warn
  unrecognized_links: warn
  omitted_files: warn
"""


class Functions(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        project(
            self.root,
            {
                "mkdocs.yml": "site_name: t\n",
                "ROADMAP.md": "# Roadmap\n",
                "crates/x/src/lib.rs": "// code\n",
                "docs/README.md": "# Home\n",
                "docs/guide.md": "# Guide\n",
                "docs/adr/0001-a.md": "# A\n",
                "docs/sub/page.md": "# Page\n",
            },
        )
        self.config = {
            "config_file_path": str(self.root / "mkdocs.yml"),
            "docs_dir": str(self.root / "docs"),
            "repo_url": "https://example.invalid/acme/repo/",
            "extra": {"source_ref": "main"},
        }

    def tearDown(self):
        self.tmp.cleanup()

    def rewrite(self, src: str, text: str) -> str:
        page = SimpleNamespace(file=SimpleNamespace(src_path=src))
        return hooks.on_page_markdown(text, page, self.config, None)

    def test_links_that_leave_docs_point_at_the_repository(self):
        out = self.rewrite("sub/page.md", "[r](../../ROADMAP.md) and [c](../../crates/x/src/lib.rs#L3) and [d](../../crates/x)")
        self.assertIn("(https://example.invalid/acme/repo/blob/main/ROADMAP.md)", out)
        self.assertIn("(https://example.invalid/acme/repo/blob/main/crates/x/src/lib.rs#L3)", out)
        self.assertIn("(https://example.invalid/acme/repo/tree/main/crates/x)", out)

    def test_the_ref_comes_from_the_environment_when_set(self):
        import os

        os.environ["DOCS_SOURCE_REF"] = "abc123"
        try:
            out = self.rewrite("guide.md", "[r](../ROADMAP.md)")
        finally:
            del os.environ["DOCS_SOURCE_REF"]
        self.assertIn("/blob/abc123/ROADMAP.md", out)

    def test_links_inside_docs_external_links_and_anchors_are_left_alone(self):
        text = "[a](guide.md) [b](sub/page.md#x) [c](https://example.com/x) [d](#top) [e](mailto:a@b.c) [f](../ROADMAP.md.nope)"
        out = self.rewrite("README.md", text)
        self.assertIn("(guide.md)", out)
        self.assertIn("(sub/page.md#x)", out)
        self.assertIn("(https://example.com/x)", out)
        self.assertIn("(#top)", out)
        self.assertIn("(mailto:a@b.c)", out)

    def test_a_link_to_a_file_that_does_not_exist_is_not_rewritten_so_that_the_build_can_fail_on_it(self):
        out = self.rewrite("guide.md", "[x](../does-not-exist.md)")
        self.assertIn("(../does-not-exist.md)", out)

    def test_code_blocks_are_left_alone(self):
        text = "```md\n[r](../ROADMAP.md)\n```\n[r](../ROADMAP.md)"
        out = self.rewrite("guide.md", text)
        self.assertEqual(out.count("blob/main/ROADMAP.md"), 1)
        self.assertIn("```md\n[r](../ROADMAP.md)\n```", out)

    def test_a_directory_without_an_index_page_links_to_the_repository(self):
        out = self.rewrite("README.md", "[adrs](adr/)")
        self.assertIn("(https://example.invalid/acme/repo/tree/main/docs/adr)", out)

    def test_pages_missing_from_nav_are_appended_once(self):
        files = SimpleNamespace(
            documentation_pages=lambda: [SimpleNamespace(src_uri=u) for u in ("README.md", "guide.md", "new.md", "adr/0001-a.md")]
        )
        config = {"nav": [{"Home": "README.md"}, {"Guide": [{"Overview": "guide.md"}]}]}
        hooks.on_files(files, config)
        self.assertEqual(config["nav"][-1], {"More": ["adr/0001-a.md", "new.md"]})
        hooks.on_files(files, config)  # `mkdocs serve` calls it again on every rebuild
        self.assertEqual([list(n)[0] for n in config["nav"]].count("More"), 1)

    def test_without_a_nav_nothing_is_changed(self):
        config = {}
        files = SimpleNamespace(documentation_pages=lambda: [SimpleNamespace(src_uri="a.md")])
        self.assertIs(hooks.on_files(files, config), files)
        self.assertNotIn("nav", config)


try:
    import mkdocs  # noqa: F401

    HAVE_MKDOCS = True
except ImportError:
    HAVE_MKDOCS = False


class RealBuild(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # docs.yml runs this with the pinned venv and AG_REQUIRE_COMPONENTS=mkdocs: there a missing MkDocs fails.
        ag_testkit.require_component(
            "mkdocs", "test_mkdocs_hooks.RealBuild", HAVE_MKDOCS, "MkDocs is not installed in this Python"
        )

    def build(self, files: dict[str, str]) -> tuple[int, str, Path]:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        project(root, files)
        (root / "mkdocs.yml").write_text(textwrap.dedent(MKDOCS_YML).lstrip("\n").replace("{hooks}", str(SCRIPTS / "mkdocs_hooks.py")))
        p = subprocess.run(
            [sys.executable, "-m", "mkdocs", "build", "--strict", "-d", str(root / "out")],
            cwd=root, capture_output=True, text=True, env={"PATH": "/usr/bin:/bin", "NO_MKDOCS_2_WARNING": "true", "HOME": str(root)},
        )
        return p.returncode, p.stdout + p.stderr, root / "out"

    SITE = {
        "ROADMAP.md": "# Roadmap\n",
        "docs/README.md": "# Home\n\nSee the [roadmap](../ROADMAP.md) and the [guide](guide.md#section).\n",
        "docs/guide.md": "# Guide\n\n## Section\n\ntext\n",
        "docs/extra.md": "# Extra\n",
    }

    def test_the_site_builds_links_out_of_docs_to_the_repository_and_keeps_unlisted_pages(self):
        code, out, site = self.build(self.SITE)
        self.assertEqual(code, 0, out)
        index = (site / "index.html").read_text()
        self.assertIn("https://example.invalid/acme/repo/blob/main/ROADMAP.md", index)
        self.assertTrue((site / "extra" / "index.html").exists() or (site / "extra.html").exists())
        self.assertIn("extra.md", out, "the page missing from nav is reported")

    def test_a_broken_link_a_broken_anchor_and_a_missing_file_still_fail_a_strict_build(self):
        for bad in ("[x](nope.md)", "[x](guide.md#nope)", "[x](../not-there.md)"):
            files = dict(self.SITE)
            files["docs/guide.md"] = f"# Guide\n\n## Section\n\n{bad}\n"
            code, out, _ = self.build(files)
            self.assertNotEqual(code, 0, f"{bad} was accepted\n{out}")


if __name__ == "__main__":
    unittest.main()
