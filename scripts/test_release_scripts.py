#!/usr/bin/env python3
"""Tests for the release and install tooling: check_release.py, package_release.py, render_packaging.py and
install.sh. No network: archives are built here from stub binaries and served from a directory.

    python3 scripts/test_release_scripts.py            # also run by .github/workflows/packaging.yml
"""
import gzip
import hashlib
import io
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import textwrap
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import ag_testkit  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
PY = sys.executable


def run(*cmd, env=None, cwd=None, check=False):
    e = dict(os.environ)
    e.update(env or {})
    p = subprocess.run([str(c) for c in cmd], capture_output=True, text=True, env=e, cwd=cwd)
    if check and p.returncode:
        raise AssertionError(f"{cmd} -> {p.returncode}\n{p.stdout}\n{p.stderr}")
    return p


class CheckRelease(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.3.0"\n\n[workspace.dependencies]\n')
        (self.root / "CHANGELOG.md").write_text(
            "# Changelog\n\n## [Unreleased]\n\n- upcoming\n\n## [0.3.0] — 2026-01-01\n\n- three\n\n## [0.2.0]\n\n- two\n"
        )

    def tearDown(self):
        self.tmp.cleanup()

    def check(self, *args):
        return run(PY, SCRIPTS / "check_release.py", *args, env={"AG_RELEASE_ROOT": str(self.root)})

    def kv(self, out):
        return dict(line.split("=", 1) for line in out.strip().splitlines())

    def test_a_matching_tag_passes(self):
        p = self.check("v0.3.0")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(
            self.kv(p.stdout),
            {"version": "0.3.0", "cargo_version": "0.3.0", "prerelease": "false", "dry_run": "false",
             "notes_section": "0.3.0"},
        )

    def test_notes_are_the_section(self):
        self.assertEqual(self.check("v0.3.0", "--notes").stdout.strip(), "- three")

    def test_a_tag_that_disagrees_with_cargo_toml_fails(self):
        p = self.check("v0.2.0")
        self.assertEqual(p.returncode, 1)
        self.assertIn("does not match the workspace version 0.3.0", p.stderr)

    def test_a_tag_without_a_changelog_section_fails(self):
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.4.0"\n')
        p = self.check("v0.4.0")
        self.assertEqual(p.returncode, 1)
        self.assertIn("no (non-empty) section for [0.4.0]", p.stderr)

    def test_an_empty_section_fails(self):
        (self.root / "CHANGELOG.md").write_text("## [Unreleased]\n\n## [0.3.0]\n\n## [0.2.0]\n- x\n")
        p = self.check("v0.3.0")
        self.assertEqual(p.returncode, 1)
        self.assertIn("empty section", p.stderr)

    def test_a_prerelease_tag(self):
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.4.0-rc.1"\n')
        (self.root / "CHANGELOG.md").write_text("## [0.4.0-rc.1] — 2026\n\n- rc\n")
        p = self.check("v0.4.0-rc.1")
        self.assertEqual(self.kv(p.stdout)["prerelease"], "true")

    def test_garbage_is_refused(self):
        for bad in ("0.3.0", "v1.2", "release-1", "v1.2.3;rm", ""):
            self.assertEqual(self.check(bad).returncode, 1, bad)

    def test_dry_run_accepts_any_semver_and_falls_back_to_unreleased(self):
        for ref in ("0.0.0-rc.dry", "v0.0.0-rc.dry"):
            p = self.check(ref, "--dry-run")
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(
                self.kv(p.stdout),
                {"version": "0.0.0-rc.dry", "cargo_version": "0.3.0", "prerelease": "true", "dry_run": "true",
                 "notes_section": "Unreleased"},
            )
        self.assertEqual(self.check("0.0.0-rc.dry", "--dry-run", "--notes").stdout.strip(), "- upcoming")

    def test_dry_run_with_an_empty_unreleased_uses_the_workspace_versions_section(self):
        (self.root / "CHANGELOG.md").write_text("## [Unreleased]\n\n## [0.3.0] — 2026\n- three\n")
        p = self.check("0.0.0-rc.dry", "--dry-run")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(self.kv(p.stdout)["notes_section"], "0.3.0")

    def test_dry_run_uses_the_real_section_when_there_is_one(self):
        self.assertEqual(self.kv(self.check("0.3.0", "--dry-run").stdout)["notes_section"], "0.3.0")

    def test_dry_run_still_rejects_non_versions(self):
        self.assertEqual(self.check("latest", "--dry-run").returncode, 1)

    def test_the_real_repository_passes_a_dry_run(self):
        p = run(PY, SCRIPTS / "check_release.py", "0.0.0-rc.dry", "--dry-run")
        self.assertEqual(p.returncode, 0, p.stderr)

    def workflow_mode(self, *, event="workflow_dispatch", ref_type="tag", ref_name="v0.3.0",
                      version="0.3.0", publish="true"):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        step = workflow.split("      - id: mode\n", 1)[1].split("      - id: v\n", 1)[0]
        script = textwrap.dedent(step.split("        run: |\n", 1)[1])
        output, environment = self.root / "output", self.root / "environment"
        result = run("bash", "-c", script, cwd=ROOT, env={
            "EVENT": event, "REF_TYPE": ref_type, "REF_NAME": ref_name,
            "INPUT_VERSION": version, "INPUT_PUBLISH": publish,
            "GITHUB_OUTPUT": str(output), "GITHUB_ENV": str(environment),
        })
        values = self.kv(environment.read_text()) if environment.exists() else {}
        facts = self.kv(output.read_text()) if output.exists() else {}
        return result, values, facts

    def test_manual_publish_accepts_bare_version_and_validates_the_actual_tag(self):
        result, values, facts = self.workflow_mode()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(facts["publish"], "true")
        checked = self.check(values["ref"])
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertEqual(values["ref"], "v0.3.0")

    def test_manual_publish_accepts_prefixed_version(self):
        result, values, facts = self.workflow_mode(version="v0.3.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((values["ref"], facts["publish"]), ("v0.3.0", "true"))

    def test_manual_publish_refuses_a_branch(self):
        result, values, facts = self.workflow_mode(ref_type="branch", ref_name="main")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Nothing was published", result.stdout)
        self.assertEqual((values, facts), ({}, {}))

    def test_manual_publish_refuses_a_different_version(self):
        result, values, facts = self.workflow_mode(version="0.2.0")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((values, facts), ({}, {}))

    def test_manual_rehearsal_keeps_its_independent_version(self):
        result, values, facts = self.workflow_mode(ref_type="branch", ref_name="main",
                                                   version="0.0.0-rc.dry", publish="false")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((values["ref"], facts["publish"]), ("0.0.0-rc.dry", "false"))
        self.assertEqual(self.check(values["ref"], "--dry-run").returncode, 0)

    def test_pushed_tag_uses_its_own_version(self):
        result, values, facts = self.workflow_mode(event="push", version="ignored", publish="false")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((values["ref"], facts["publish"]), ("v0.3.0", "true"))


def stub_bin(path: pathlib.Path, name: str, version: str):
    path.mkdir(parents=True, exist_ok=True)
    f = path / name
    f.write_text(f'#!/bin/sh\necho "{name} {version} (stub)"\n')
    f.chmod(0o755)


def host_target() -> str:
    out = run("uname", "-sm").stdout.split()
    arch = {"x86_64": "x86_64", "amd64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}[out[1]]
    return f"{arch}-apple-darwin" if out[0] == "Darwin" else f"{arch}-unknown-linux-gnu"


class Packaging(unittest.TestCase):
    version = "9.9.9-rc.1"

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.bin = self.dir / "bin"
        for b in ("ag", "ag-server", "ag-worker", "ag-mcp", "orchestrator"):
            stub_bin(self.bin, b, self.version)
        self.dist = self.dir / "dist"
        self.target = host_target()

    def tearDown(self):
        self.tmp.cleanup()

    def archive(self, out=None, target=None, epoch="1700000000"):
        p = run(PY, SCRIPTS / "package_release.py", "archive", "--version", self.version,
                "--target", target or self.target, "--bin-dir", self.bin, "--out", out or self.dist,
                env={"SOURCE_DATE_EPOCH": epoch}, check=True)
        return pathlib.Path(p.stdout.strip().splitlines()[-1])

    def release_dir(self, targets=None):
        """A directory laid out like a GitHub release: archives + SHA256SUMS."""
        for t in targets or [self.target]:
            self.archive(target=t)
        sums = run(PY, SCRIPTS / "package_release.py", "sums", *sorted(self.dist.glob("*.tar.gz")), check=True).stdout
        (self.dist / "SHA256SUMS").write_text(sums)
        return self.dist

    def test_the_archive_has_the_documented_layout(self):
        a = self.archive()
        self.assertEqual(a.name, f"agentgit-{self.version}-{self.target}.tar.gz")
        top = a.name[: -len(".tar.gz")]
        with tarfile.open(a) as t:
            members = {m.name: m for m in t.getmembers()}
        self.assertEqual(
            sorted(members),
            sorted([top] + [f"{top}/{n}" for n in ("ag", "ag-server", "ag-worker", "ag-mcp", "orchestrator", "LICENSE", "NOTICE", "README.md", "THIRD_PARTY_LICENSES.md")]),
        )
        for n in ("ag", "ag-server", "ag-worker", "ag-mcp", "orchestrator"):
            self.assertEqual(members[f"{top}/{n}"].mode, 0o755)
        for m in members.values():
            self.assertEqual((m.uid, m.gid, m.mtime), (0, 0, 1700000000))

    def test_the_archive_is_deterministic(self):
        a = self.archive(out=self.dir / "one").read_bytes()
        b = self.archive(out=self.dir / "two").read_bytes()
        self.assertEqual(hashlib.sha256(a).hexdigest(), hashlib.sha256(b).hexdigest())
        self.assertNotEqual(a, self.archive(out=self.dir / "three", epoch="1700000001").read_bytes())
        self.assertEqual(gzip.decompress(a)[:0], b"")  # a valid gzip stream

    def test_optional_directories_are_included_only_when_present(self):
        comp = self.dir / "completions"
        comp.mkdir()
        (comp / "ag.bash").write_text("# completion\n")
        out = run(PY, SCRIPTS / "package_release.py", "archive", "--version", self.version, "--target", self.target,
                  "--bin-dir", self.bin, "--out", self.dist, "--include-dir", f"completions={comp}",
                  "--include-dir", f"man={self.dir / 'nope'}", env={"SOURCE_DATE_EPOCH": "1"}, check=True)
        with tarfile.open(out.stdout.strip()) as t:
            names = t.getnames()
        self.assertTrue(any(n.endswith("completions/ag.bash") for n in names), names)
        self.assertFalse(any("/man" in n for n in names), names)

    def test_a_missing_binary_is_an_error(self):
        (self.bin / "ag-worker").unlink()
        p = run(PY, SCRIPTS / "package_release.py", "archive", "--version", "1.0.0", "--target", self.target,
                "--bin-dir", self.bin, "--out", self.dist)
        self.assertEqual(p.returncode, 1)
        self.assertIn("ag-worker", p.stderr)

    def test_sums_match_sha256sum_format(self):
        d = self.release_dir()
        line = (d / "SHA256SUMS").read_text().strip()
        digest, name = re.fullmatch(r"([0-9a-f]{64})  (\S+)", line).groups()
        self.assertEqual(hashlib.sha256((d / name).read_bytes()).hexdigest(), digest)
        if shutil.which("sha256sum"):
            self.assertEqual(run("sha256sum", "-c", "SHA256SUMS", cwd=d).returncode, 0)

    # ---- install.sh ------------------------------------------------------------------------------------
    def install(self, *args, release=None, shell="sh"):
        base = release or self.release_dir()
        prefix = self.dir / "prefix"
        p = run(shell, SCRIPTS / "install.sh", "--version", self.version, "--prefix", prefix,
                "--base-url", f"file://{base}", "--no-attestation", *args)
        return p, prefix

    def test_install_sh_installs_and_runs(self):
        p, prefix = self.install()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("checksum ok", p.stdout)
        for b in ("ag", "ag-server", "ag-worker", "ag-mcp", "orchestrator"):
            f = prefix / "bin" / b
            self.assertTrue(os.access(f, os.X_OK), f)
        self.assertEqual(run(prefix / "bin" / "ag", "--version").stdout.strip(), f"ag {self.version} (stub)")
        self.assertTrue((prefix / "share/doc/agentgit/LICENSE").is_file())
        self.assertEqual(list((prefix / "bin").glob(".*")), [], "no temporary files left behind")

    def test_install_sh_works_under_dash_when_available(self):
        ag_testkit.require_component("dash", self.id(), shutil.which("dash"), "dash is not installed")
        p, prefix = self.install(shell="dash")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)

    def test_install_sh_overwrites_an_older_install(self):
        p, prefix = self.install()
        (prefix / "bin" / "ag").write_text("#!/bin/sh\necho old\n")
        p2, _ = self.install()
        self.assertEqual(p2.returncode, 0, p2.stderr)
        self.assertEqual(run(prefix / "bin" / "ag", "--version").stdout.strip(), f"ag {self.version} (stub)")

    def test_a_tampered_archive_installs_nothing(self):
        d = self.release_dir()
        archive = next(d.glob("*.tar.gz"))
        archive.write_bytes(archive.read_bytes() + b"x")
        p, prefix = self.install(release=d)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("checksum mismatch", p.stderr)
        self.assertFalse((prefix / "bin").exists(), "nothing may be installed after a failed check")

    def remove_archive_binary(self, release, binary):
        archive = next(release.glob("*.tar.gz"))
        with tarfile.open(archive) as source:
            members = [(m, source.extractfile(m).read() if m.isfile() else None)
                       for m in source.getmembers() if not m.name.endswith("/" + binary)]
        with tarfile.open(archive, "w:gz") as target:
            for member, content in members:
                target.addfile(member, io.BytesIO(content) if content is not None else None)
        (release / "SHA256SUMS").write_text(f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")

    def test_an_incomplete_archive_does_not_replace_an_existing_install(self):
        for binary in ("ag-mcp", "orchestrator"):
            with self.subTest(binary=binary):
                d = self.release_dir()
                self.remove_archive_binary(d, binary)
                old = self.dir / "prefix/bin/ag"
                old.parent.mkdir(parents=True, exist_ok=True)
                old.write_text("old installation")
                result, _ = self.install(release=d)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(binary, result.stderr)
                self.assertEqual(old.read_text(), "old installation")

    def test_pre_beta4_archives_install_without_an_orchestrator(self):
        self.version = "0.3.0-beta.3"
        d = self.release_dir()
        self.remove_archive_binary(d, "orchestrator")
        result, prefix = self.install(release=d)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((prefix / "bin/ag").is_file())
        self.assertFalse((prefix / "bin/orchestrator").exists())

    def test_old_github_cli_explains_how_to_enable_attestation_verification(self):
        d = self.release_dir()
        bindir = self.dir / "old-gh"
        bindir.mkdir()
        gh = bindir / "gh"
        gh.write_text('#!/bin/sh\n[ "$1" = auth ] && exit 0\nexit 1\n')
        gh.chmod(0o755)
        result = run("sh", SCRIPTS / "install.sh", "--version", self.version,
                     "--prefix", self.dir / "old-gh-prefix", "--base-url", f"file://{d}",
                     "--require-attestation", env={"PATH": str(bindir) + os.pathsep + os.environ["PATH"]})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not support", result.stderr)
        self.assertFalse((self.dir / "old-gh-prefix/bin").exists())

    def test_a_missing_checksum_entry_installs_nothing(self):
        d = self.release_dir()
        (d / "SHA256SUMS").write_text(f"{'0' * 64}  agentgit-{self.version}-some-other-target.tar.gz\n")
        p, prefix = self.install(release=d)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("no entry for", p.stderr)
        self.assertFalse((prefix / "bin").exists())

    def test_a_missing_archive_is_a_clear_error(self):
        d = self.release_dir()
        next(d.glob("*.tar.gz")).unlink()
        p, _ = self.install(release=d)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("could not download", p.stderr)

    def test_dry_run_downloads_and_installs_nothing(self):
        p, prefix = self.install("--dry-run", release=self.dir / "does-not-exist")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("dry run", p.stdout)
        self.assertIn(f"agentgit-{self.version}-{self.target}.tar.gz", p.stdout)
        self.assertFalse(prefix.exists())

    def test_target_override_and_unsupported_targets(self):
        p = run("sh", SCRIPTS / "install.sh", "--version", "1.2.3", "--dry-run", "--target", "aarch64-apple-darwin")
        self.assertIn("agentgit-1.2.3-aarch64-apple-darwin.tar.gz", p.stdout)
        p = run("sh", SCRIPTS / "install.sh", "--version", "1.2.3", "--dry-run", "--target", "x86_64-pc-windows-msvc")
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("unsupported target", p.stderr)

    def test_bad_versions_and_options(self):
        for v in ("1.2", "abc", "1.2.3/../x", "1.2.3 4"):
            p = run("sh", SCRIPTS / "install.sh", "--version", v, "--dry-run")
            self.assertNotEqual(p.returncode, 0, v)
        self.assertNotEqual(run("sh", SCRIPTS / "install.sh", "--frobnicate").returncode, 0)
        self.assertEqual(run("sh", SCRIPTS / "install.sh", "--help").returncode, 0)

    def test_a_leading_v_is_accepted(self):
        p = run("sh", SCRIPTS / "install.sh", "--version", "v1.2.3", "--dry-run")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("AgentGit 1.2.3 for", p.stdout)

    def test_require_attestation_fails_without_gh(self):
        d = self.release_dir()
        env_path = str(self.dir / "emptybin")
        os.makedirs(env_path)
        for tool in ("sh", "cp", "chmod", "mv", "mkdir", "mktemp", "rm", "tar", "awk", "cut", "sed", "grep", "uname",
                     "sha256sum", "shasum", "dirname", "cat"):
            src = shutil.which(tool)
            if src:
                os.symlink(src, os.path.join(env_path, tool))
        p = run("sh", SCRIPTS / "install.sh", "--version", self.version, "--prefix", self.dir / "p2",
                "--base-url", f"file://{d}", "--require-attestation", env={"PATH": env_path})
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("--require-attestation", p.stderr)
        self.assertFalse((self.dir / "p2/bin").exists())

    # ---- render_packaging.py ---------------------------------------------------------------------------------
    def sums_for(self, targets):
        lines = []
        for i, t in enumerate(targets):
            ext = "zip" if "windows" in t else "tar.gz"
            lines.append(f"{hashlib.sha256(str(i).encode()).hexdigest()}  agentgit-{self.version}-{t}.{ext}")
        p = self.dir / "SHA256SUMS.fixture"
        p.write_text("\n".join(lines) + "\n")
        return p, {t: line.split()[0] for t, line in zip(targets, lines)}

    def test_templates_are_filled_from_sha256sums(self):
        targets = ["aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-gnu",
                   "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
        sums, digests = self.sums_for(targets)
        out = self.dir / "rendered"
        run(PY, SCRIPTS / "render_packaging.py", "--version", f"v{self.version}", "--sums", sums, "--out", out,
            check=True)
        rb = (out / "agentgit.rb").read_text()
        self.assertIn(f'version "{self.version}"', rb)
        for t in targets[:4]:
            self.assertIn(digests[t], rb)
            self.assertIn(f"releases/download/v{self.version}/agentgit-{self.version}-{t}.tar.gz", rb)
        self.assertNotIn("@", rb.replace("#{bin}", ""))
        self.assertTrue(rb.startswith("# Rendered by"), rb[:80])
        manifest = json.loads((out / "agentgit.json").read_text())
        self.assertEqual(manifest["version"], self.version)
        self.assertEqual(manifest["architecture"]["64bit"]["hash"], digests["x86_64-pc-windows-msvc"])
        self.assertTrue(manifest["architecture"]["64bit"]["url"].endswith(".zip"))
        if shutil.which("ruby"):
            self.assertEqual(run("ruby", "-c", out / "agentgit.rb").returncode, 0)

    def test_a_missing_checksum_is_an_error_unless_allowed(self):
        sums, _ = self.sums_for(["x86_64-unknown-linux-gnu"])
        p = run(PY, SCRIPTS / "render_packaging.py", "--version", self.version, "--sums", sums, "--out", self.dir / "o1")
        self.assertEqual(p.returncode, 1)
        self.assertIn("aarch64-apple-darwin", p.stderr)
        self.assertFalse((self.dir / "o1" / "agentgit.rb").exists())
        p = run(PY, SCRIPTS / "render_packaging.py", "--version", self.version, "--sums", sums, "--out", self.dir / "o2",
                "--only", "homebrew", "--allow-missing")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("0" * 64, (self.dir / "o2" / "agentgit.rb").read_text())
        self.assertFalse((self.dir / "o2" / "agentgit.json").exists())

    def test_a_checksum_for_another_version_does_not_count(self):
        sums, _ = self.sums_for(["x86_64-unknown-linux-gnu"])
        p = run(PY, SCRIPTS / "render_packaging.py", "--version", "1.0.0", "--sums", sums, "--out", self.dir / "o3",
                "--only", "homebrew")
        self.assertEqual(p.returncode, 1)

    def test_base_url_can_point_at_a_local_directory(self):
        sums, _ = self.sums_for(["aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-gnu",
                                 "x86_64-unknown-linux-gnu"])
        run(PY, SCRIPTS / "render_packaging.py", "--version", self.version, "--sums", sums, "--out", self.dir / "o4",
            "--only", "homebrew", "--base-url", "file:///tmp/dist/", check=True)
        self.assertIn(f'url "file:///tmp/dist/agentgit-{self.version}-aarch64-apple-darwin.tar.gz"',
                      (self.dir / "o4" / "agentgit.rb").read_text())


class BuildxCacheCheck(unittest.TestCase):
    """scripts/check_buildx_cache.py must not pass a log that shows a rebuild, nor one with no cargo step at all."""

    LOG = (
        "#5 [chef 3/4] RUN --mount=type=cache,target=/r cargo install cargo-chef --locked --version 0.1.78\n"
        "#5 CACHED\n"
        "#7 [ui 3/6] RUN --mount=type=cache,target=/root/.npm     cd apps/web && npm ci\n"
        "#7 CACHED\n"
        "#8 [ui 6/6] RUN AG_UI_SKIP_INSTALL=1 bash scripts/build_ui.sh\n"
        "#8 CACHED\n"
        "#9 [build 3/7] RUN --mount=type=cache,target=/r     cargo chef cook --release --locked -p agentgit-server\n"
        "#9 CACHED\n"
        "#11 [build 6/7] RUN --mount=type=cache,target=/r     cargo build --release --locked -p agentgit-server\n"
        "#11 {state}\n"
    )

    def check(self, log: str) -> subprocess.CompletedProcess:
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as f:
            f.write(log)
        try:
            return run(PY, SCRIPTS / "check_buildx_cache.py", f.name)
        finally:
            os.unlink(f.name)

    def test_a_fully_cached_log_passes(self):
        self.assertEqual(self.check(self.LOG.format(state="CACHED")).returncode, 0)

    def test_a_rebuilt_step_fails(self):
        p = self.check(self.LOG.format(state="DONE 241.3s"))
        self.assertEqual(p.returncode, 1)
        self.assertIn("cargo build --release", p.stderr)

    def test_a_rebuilt_web_ui_fails(self):
        log = self.LOG.format(state="CACHED").replace("#8 CACHED", "#8 DONE 31.0s")
        p = self.check(log)
        self.assertEqual(p.returncode, 1)
        self.assertIn("scripts/build_ui.sh", p.stderr)

    def test_a_log_without_the_web_ui_steps_fails(self):
        log = "".join(l + "\n" for l in self.LOG.format(state="CACHED").split("\n") if l and not l.startswith(("#7", "#8")))
        p = self.check(log)
        self.assertEqual(p.returncode, 1)
        self.assertIn("no RUN step containing 'npm ci'", p.stderr)

    def test_a_log_without_the_steps_fails_instead_of_passing_vacuously(self):
        p = self.check("#1 [internal] load build definition\n#1 DONE 0.1s\n")
        self.assertEqual(p.returncode, 1)
        self.assertIn("no RUN step containing", p.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
