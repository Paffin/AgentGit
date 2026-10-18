#!/usr/bin/env python3
"""Tests of scripts/check_dependabot.py on small throw-away trees and on the repository itself.

Run:  python3 scripts/test_check_dependabot.py
"""
import pathlib
import sys
import tempfile
import textwrap
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import check_dependabot as cd  # noqa: E402


def tree(files: dict[str, str], dependabot: str) -> pathlib.Path:
    root = pathlib.Path(tempfile.mkdtemp())
    for name, content in {**files, ".github/dependabot.yml": textwrap.dedent(dependabot)}.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    return root


def entries(*pairs):
    body = "".join(
        f"  - package-ecosystem: {eco}\n    directory: {d}\n    schedule:\n      interval: weekly\n" for eco, d in pairs
    )
    return "version: 2\nupdates:\n" + body


class Discovery(unittest.TestCase):
    def test_every_manifest_kind_is_found(self):
        root = tree(
            {
                "Cargo.lock": "",
                "fuzz/Cargo.lock": "",
                "web/package-lock.json": "{}",
                "sdk/uv.lock": "",
                "svc/go.mod": "",
                "svc/Dockerfile": "",
                "deploy/compose.yaml": "",
                "deploy/docker-compose.dev.yml": "",
                ".github/workflows/ci.yml": "",
                "node_modules/x/package-lock.json": "{}",  # vendored: ignored
                "target/Cargo.lock": "",
            },
            "version: 2\nupdates: []\n",
        )
        self.assertEqual(
            cd.discover(root),
            {
                ("cargo", "/"),
                ("cargo", "/fuzz"),
                ("npm", "/web"),
                ("uv", "/sdk"),
                ("gomod", "/svc"),
                ("docker", "/svc"),
                ("docker-compose", "/deploy"),
                ("github-actions", "/"),
            },
        )


class Judgement(unittest.TestCase):
    def test_a_covered_tree_passes(self):
        root = tree({"Cargo.lock": "", "web/package-lock.json": "{}"}, entries(("cargo", "/"), ("npm", "/web")))
        self.assertEqual(cd.problems(root, {}), [])

    def test_a_manifest_without_an_update_entry_fails(self):
        root = tree({"Cargo.lock": "", "web/package-lock.json": "{}"}, entries(("cargo", "/")))
        self.assertEqual(len(cd.problems(root, {})), 1)
        self.assertIn("/web has a npm manifest", cd.problems(root, {})[0])

    def test_an_entry_for_a_directory_without_a_manifest_fails(self):
        root = tree({"Cargo.lock": ""}, entries(("cargo", "/"), ("gomod", "/gone")))
        self.assertIn("where no such manifest exists", cd.problems(root, {})[0])

    def test_the_wrong_ecosystem_for_a_directory_fails(self):
        root = tree({"web/package-lock.json": "{}"}, entries(("pip", "/web")))
        self.assertEqual(len(cd.problems(root, {})), 2)

    def test_the_documented_gap_is_allowed_only_while_it_exists(self):
        gap = {("pip", "/docs"): "hash-locked"}
        root = tree({"docs/requirements.txt": ""}, entries())
        self.assertEqual(cd.problems(root, gap), [])  # deliberately not updated
        root = tree({}, entries())
        self.assertIn("no longer exists", cd.problems(root, gap)[0])
        root = tree({"docs/requirements.txt": ""}, entries(("pip", "/docs")))
        self.assertTrue(any("remove the exception" in p for p in cd.problems(root, gap)))

    def test_an_update_without_a_schedule_is_rejected(self):
        root = tree({"Cargo.lock": ""}, "version: 2\nupdates:\n  - package-ecosystem: cargo\n    directory: /\n")
        with self.assertRaises(SystemExit):
            cd.problems(root, {})


class Repository(unittest.TestCase):
    def test_the_repository_is_fully_covered(self):
        self.assertEqual(cd.problems(cd.ROOT), [])


if __name__ == "__main__":
    unittest.main()
