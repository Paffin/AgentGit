#!/usr/bin/env python3
"""Tests of scripts/check_workflows.py: the lint rules and the --pin rewriting.

The refs in CHECKOUT_REFS and TOOLCHAIN_REFS are lines of a real `git ls-remote --tags --heads` of
github.com/actions/checkout and github.com/dtolnay/rust-toolchain (recorded 2026-10-18, trimmed), so the tests need no
network. Pinning itself is also exercised against the real repositories' workflows by running
`scripts/check_workflows.py` in CI, which fails on any unpinned action.

Run:  python3 -m unittest scripts/test_check_workflows.py
"""
import pathlib
import sys
import tempfile
import textwrap
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import check_workflows as cw  # noqa: E402

CHECKOUT_REFS = cw.parse_refs(
    textwrap.dedent(
        """\
        f548e57e544e1ff5a4c46bf1e1b8685f8e4a348a\trefs/heads/main
        11d5960a326750d5838078e36cf38b85af677262\trefs/tags/v4
        08eba0b27e820071cde6df949e0beb9ba4906955\trefs/tags/v4.3.0
        34e114876b0b11c390a56381ad16ebd13914f8d5\trefs/tags/v4.3.1
        11d5960a326750d5838078e36cf38b85af677262\trefs/tags/v4.3.1^{}
        3d3c42e5aac5ba805825da76410c181273ba90b1\trefs/tags/v7
        9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0\trefs/tags/v7.0.0
        3d3c42e5aac5ba805825da76410c181273ba90b1\trefs/tags/v7.0.1
        """
    ).splitlines()
)
TOOLCHAIN_REFS = cw.parse_refs(
    [
        "683b588c487a69d01f6c6ddf40f036ff682cc2c8\trefs/heads/master",
        "b725eda525362e7cf34af9a7243cac1e3f01a7ec\trefs/heads/stable",
        "683b588c487a69d01f6c6ddf40f036ff682cc2c8\trefs/tags/v1",
    ]
)
CHECKOUT_V7 = "3d3c42e5aac5ba805825da76410c181273ba90b1"
MASTER = "683b588c487a69d01f6c6ddf40f036ff682cc2c8"


def lookup(action: str):
    return {"actions/checkout": CHECKOUT_REFS, "dtolnay/rust-toolchain": TOOLCHAIN_REFS}.get(action, {})


class Resolve(unittest.TestCase):
    def test_a_floating_major_tag_is_documented_as_the_release_it_is(self):
        self.assertEqual(cw.resolve(CHECKOUT_REFS, "v7"), (CHECKOUT_V7, "v7.0.1"))

    def test_an_annotated_tag_resolves_to_the_commit_not_the_tag_object(self):
        # v4.3.1 is annotated: the plain ref is the tag object, the peeled line the commit
        self.assertEqual(cw.resolve(CHECKOUT_REFS, "v4.3.1")[0], "11d5960a326750d5838078e36cf38b85af677262")

    def test_the_highest_exact_release_wins(self):
        self.assertEqual(cw.resolve(CHECKOUT_REFS, "v4"), ("11d5960a326750d5838078e36cf38b85af677262", "v4.3.1"))

    def test_a_branch_without_a_release_keeps_its_name(self):
        self.assertEqual(cw.resolve(CHECKOUT_REFS, "main"), ("f548e57e544e1ff5a4c46bf1e1b8685f8e4a348a", "main"))

    def test_an_unknown_ref_is_none(self):
        self.assertIsNone(cw.resolve(CHECKOUT_REFS, "v99"))


class PinText(unittest.TestCase):
    def pin(self, text):
        return cw.pin_text(textwrap.dedent(text), lookup)

    def test_a_tag_becomes_a_sha_with_the_release_in_a_comment(self):
        new, changes = self.pin(
            """\
            steps:
              - uses: actions/checkout@v7
            """
        )
        self.assertIn(f"- uses: actions/checkout@{CHECKOUT_V7} # v7.0.1\n", new)
        self.assertEqual(len(changes), 1)

    def test_a_pinned_action_is_untouched(self):
        text = f"steps:\n  - uses: actions/checkout@{CHECKOUT_V7} # v7.0.1\n"
        self.assertEqual(cw.pin_text(text, lookup), (text, []))

    def test_local_actions_and_docker_images_are_untouched(self):
        text = "steps:\n  - uses: ./.github/actions/x\n  - uses: docker://alpine:3.20\n"
        self.assertEqual(cw.pin_text(text, lookup), (text, []))

    def test_an_unresolvable_action_is_reported_and_left_alone(self):
        text = "steps:\n  - uses: actions/checkout@v99\n"
        new, changes = cw.pin_text(text, lookup)
        self.assertEqual(new, text)
        self.assertIn("cannot resolve actions/checkout@v99", changes[0])

    def test_rust_toolchain_stable_becomes_master_plus_the_toolchain_input(self):
        new, _ = self.pin(
            """\
            steps:
              - uses: dtolnay/rust-toolchain@stable
              - run: cargo build
            """
        )
        self.assertEqual(
            new,
            textwrap.dedent(
                f"""\
                steps:
                  - uses: dtolnay/rust-toolchain@{MASTER} # master
                    with:
                      toolchain: stable
                  - run: cargo build
                """
            ),
        )

    def test_rust_toolchain_keeps_its_other_inputs(self):
        new, _ = self.pin(
            """\
            steps:
              - uses: dtolnay/rust-toolchain@stable
                with:
                  components: rustfmt, clippy
              - run: cargo fmt
            """
        )
        self.assertIn("    with:\n      toolchain: stable\n      components: rustfmt, clippy\n", new)

    def test_an_explicit_toolchain_input_wins_over_the_ref(self):
        text = "steps:\n  - uses: dtolnay/rust-toolchain@stable\n    with:\n      toolchain: 1.89.0\n  - run: x\n"
        new, _ = cw.pin_text(text, lookup)
        self.assertEqual(new.count("toolchain:"), 1)
        self.assertIn("toolchain: 1.89.0", new)
        self.assertIn(f"@{MASTER} # master", new)

    def test_a_toolchain_input_of_the_next_step_is_not_mistaken_for_this_ones(self):
        text = (
            "steps:\n  - uses: dtolnay/rust-toolchain@stable\n  - uses: other/thing@v1\n    with:\n      toolchain: x\n"
        )
        new, _ = cw.pin_text(text, lookup)
        self.assertIn("    with:\n      toolchain: stable\n  - uses: other/thing@v1", new)

    def test_the_result_is_valid_yaml_and_passes_the_pinned_rule(self):
        import yaml

        new, _ = self.pin(
            """\
            on: push
            permissions:
              contents: read
            jobs:
              a:
                runs-on: ubuntu-latest
                steps:
                  - uses: actions/checkout@v7
                  - uses: dtolnay/rust-toolchain@stable
                    with:
                      components: clippy
            """
        )
        steps = yaml.safe_load(new)["jobs"]["a"]["steps"]
        self.assertEqual(steps[1]["with"], {"toolchain": "stable", "components": "clippy"})
        self.assertTrue(all(cw.SHA.match(s["uses"].partition("@")[2]) for s in steps))


class Lint(unittest.TestCase):
    """check() on throw-away workflows."""

    def check(self, text, name="w.yml"):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            path = root / ".github" / "workflows" / name
            path.parent.mkdir(parents=True)
            path.write_text(text)
            with mock.patch.object(cw, "ROOT", root):
                return cw.check(path)

    @staticmethod
    def workflow(on="on: push", perms="permissions:\n  contents: read\n", job_perms="", steps=None):
        steps = steps or [
            f"- uses: actions/checkout@{CHECKOUT_V7} # v7.0.1\n  with:\n    persist-credentials: false",
            "- run: echo hello",
        ]
        body = "".join("      " + "\n      ".join(step.splitlines()) + "\n" for step in steps)
        return f"{on}\n{perms}jobs:\n  a:\n    runs-on: ubuntu-latest\n{job_perms}    steps:\n{body}"

    def test_a_good_workflow_has_no_findings(self):
        self.assertEqual(self.check(self.workflow()), ([], []))

    def test_an_unpinned_action_is_a_problem(self):
        problems, _ = self.check(self.workflow(steps=["- uses: actions/checkout@v7"]))
        self.assertEqual(len(problems), 1)
        self.assertIn("not pinned", problems[0])

    def test_missing_permissions_is_a_problem_unless_every_job_sets_them(self):
        problems, _ = self.check(self.workflow(perms=""))
        self.assertTrue(any("no top-level `permissions:`" in p for p in problems))
        per_job = self.workflow(perms="", job_perms="    permissions:\n      contents: read\n")
        self.assertEqual(self.check(per_job)[0], [])

    def test_pull_request_target_and_workflow_run_are_problems(self):
        for trigger in ("pull_request_target", "workflow_run"):
            problems, _ = self.check(self.workflow(on=f"on:\n  {trigger}:\n    types: [opened]"))
            self.assertTrue(any(trigger in p for p in problems), trigger)

    def test_an_expression_an_outsider_controls_in_a_script_is_a_problem(self):
        bad = self.workflow(steps=["- run: echo ${{ github.event.pull_request.title }}"])
        self.assertTrue(any("expanded inside a shell script" in p for p in self.check(bad)[0]))
        # the same through env: is the safe way
        safe = self.workflow(
            steps=['- run: echo "$TITLE"\n  env:\n    TITLE: ${{ github.event.pull_request.title }}']
        )
        self.assertEqual(self.check(safe)[0], [])

    def test_inputs_in_a_script_are_a_problem(self):
        problems, _ = self.check(self.workflow(steps=["- run: echo ${{ inputs.version }}"]))
        self.assertTrue(any("inputs.version" in p for p in problems))

    def test_checkout_credentials_are_a_warning_where_artifacts_are_uploaded(self):
        steps = [
            f"- uses: actions/checkout@{CHECKOUT_V7} # v7.0.1",
            f"- uses: actions/upload-artifact@{CHECKOUT_V7} # v7\n  with:\n    name: x\n    path: y",
        ]
        problems, warnings = self.check(self.workflow(steps=steps))
        self.assertEqual(problems, [])
        self.assertEqual(len(warnings), 1)
        self.assertIn("persist-credentials", warnings[0])


class Repository(unittest.TestCase):
    def test_every_workflow_of_the_repository_passes_strictly(self):
        results = [cw.check(w) for w in cw.WORKFLOWS]
        self.assertGreater(len(results), 5)
        self.assertEqual([p for r in results for p in r[0] + r[1]], [])

    def test_the_pinned_comment_names_a_release(self):
        # `# v4.3.1`, `# master`: Dependabot reads the comment to keep it in step with the SHA
        for w in cw.WORKFLOWS:
            for n, action, ref, line in cw.uses_refs(w):
                self.assertRegex(line, r"@[0-9a-f]{40} # \S+", f"{w.name}:{n} {action}")


if __name__ == "__main__":
    unittest.main()
