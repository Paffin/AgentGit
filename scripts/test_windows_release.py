#!/usr/bin/env python3
"""Offline tests for the native Windows archive and per-user installer."""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
PACKAGE = Path(os.environ.get("AG_TEST_PACKAGE", str(SCRIPTS / "package_release.py")))
INSTALLER = Path(os.environ.get("AG_TEST_WINDOWS_INSTALLER", str(SCRIPTS / "install.ps1")))
PWSH = os.environ.get("AG_TEST_PWSH") or shutil.which("pwsh")
TARGET = "x86_64-pc-windows-msvc"
VERSION = "9.9.9-rc.1"
BINARIES = ["ag.exe", "ag-server.exe", "ag-mcp.exe"]


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], capture_output=True, text=True, **kwargs)


class WindowsArchive(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for name in BINARIES:
            (self.bin / name).write_bytes((name + "\n").encode())

    def archive(self, destination="dist", epoch="1700000000"):
        env = dict(os.environ, SOURCE_DATE_EPOCH=epoch)
        return run(sys.executable, PACKAGE, "archive", "--version", VERSION,
                   "--target", TARGET, "--bin-dir", self.bin, "--out", self.root / destination, env=env)

    def test_zip_contains_supported_windows_binaries_and_docs(self):
        result = self.archive()
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = Path(result.stdout.strip())
        self.assertEqual(archive.name, f"agentgit-{VERSION}-{TARGET}.zip")
        with zipfile.ZipFile(archive) as package:
            names = package.namelist()
            top = f"agentgit-{VERSION}-{TARGET}/"
            for name in BINARIES + ["LICENSE", "NOTICE", "README.md", "THIRD_PARTY_LICENSES.md"]:
                self.assertIn(top + name, names)
            self.assertFalse(any("ag-worker" in name or "orchestrator" in name for name in names))
            self.assertTrue(all(name.startswith(top) for name in names))
            self.assertEqual(package.read(top + "ag.exe"), b"ag.exe\n")

    def test_zip_is_deterministic_and_handles_early_epochs(self):
        first = self.archive("one", "1")
        second = self.archive("two", "1")
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(Path(first.stdout.strip()).read_bytes(), Path(second.stdout.strip()).read_bytes())
        with zipfile.ZipFile(Path(first.stdout.strip())) as archive:
            self.assertTrue(all(entry.date_time[0] >= 1980 for entry in archive.infolist()))

    def test_missing_supported_binary_does_not_create_archive(self):
        (self.bin / "ag-mcp.exe").unlink()
        result = self.archive()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ag-mcp.exe", result.stderr)
        self.assertFalse(list(self.root.rglob("*.zip")))


class ReleaseInputs(unittest.TestCase):
    def test_release_checksum_recipe_includes_windows_zip(self):
        import glob
        import shlex
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        recipe = next(line.strip().split("|", 1)[0].strip() for line in workflow.splitlines()
                      if "package_release.py sums dist/" in line)
        patterns = shlex.split(recipe)[3:]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "dist").mkdir()
            for name in ["linux-x64.tar.gz", "linux-arm.tar.gz", "mac-x64.tar.gz", "mac-arm.tar.gz",
                         "windows.zip", "ag.cdx.json", "server.cdx.json", "worker.cdx.json", "mcp.cdx.json"]:
                (root / "dist" / name).write_bytes(name.encode())
            files = [path for pattern in patterns for path in glob.glob(str(root / pattern))]
            result = run(sys.executable, PACKAGE, "sums", *files)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len(result.stdout.strip().splitlines()), 9)
            self.assertIn("windows.zip", result.stdout)
        self.assertIn("set -- dist/*.zip; test $# = 1", workflow)
        release = workflow.split("  release:\n", 1)[1]
        self.assertIn("windows-binaries", release.split("    steps:", 1)[0])


@unittest.skipUnless(PWSH, "PowerShell not available; Windows CI runs installer tests")
class WindowsInstaller(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.dist = self.root / "dist"
        self.dist.mkdir()
        self.prefix = self.root / "Agent's [tools] $(never-run)"
        self.top = f"agentgit-{VERSION}-{TARGET}"
        self.archive = self.dist / f"{self.top}.zip"
        self.make_archive()

    def make_archive(self, missing=None, extra=None):
        with zipfile.ZipFile(self.archive, "w") as archive:
            for name in BINARIES:
                if name != missing:
                    archive.writestr(f"{self.top}/{name}", ("binary-" + name).encode())
            if extra:
                archive.writestr(extra, "untrusted extra")
        self.write_sum()

    def write_sum(self):
        digest = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        (self.dist / "SHA256SUMS").write_text(f"{digest}  {self.archive.name}\n")

    def install(self, *args):
        return run(PWSH, "-NoProfile", "-File", INSTALLER, "-Version", VERSION,
                   "-BaseUrl", self.dist, "-Prefix", self.prefix, "-NoAttestation", *args)

    def test_installs_supported_binaries_without_changing_profile(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in BINARIES:
            self.assertEqual((self.prefix / "bin" / name).read_bytes(), ("binary-" + name).encode())
        self.assertIn("ag-server.exe", result.stdout)
        self.assertIn("dev --open", result.stdout)
        self.assertIn("Agent''s [tools] $(never-run)", result.stdout)
        self.assertFalse((self.root / "never-run").exists())

    def test_checksum_failure_does_not_touch_existing_install(self):
        (self.prefix / "bin").mkdir(parents=True)
        old = self.prefix / "bin" / "ag.exe"
        old.write_bytes(b"old installation")
        self.archive.write_bytes(self.archive.read_bytes() + b"corruption")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum", result.stderr.lower())
        self.assertEqual(old.read_bytes(), b"old installation")
        self.assertFalse((self.prefix / "bin" / "ag-server.exe").exists())

    def test_missing_binary_is_rejected_before_installing_anything(self):
        self.make_archive(missing="ag-mcp.exe")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ag-mcp.exe", result.stderr)
        self.assertFalse(self.prefix.exists())

    def test_extra_archive_paths_are_not_extracted(self):
        self.make_archive(extra=f"{self.top}/../../escaped.txt")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / "escaped.txt").exists())
        self.assertEqual(sorted(p.name for p in (self.prefix / "bin").iterdir()), sorted(BINARIES))

    def discovered(self, releases=None, failure=False):
        import json
        payload = json.dumps(releases or []).replace("'", "''")
        response = "throw 'Network unavailable'" if failure else f"return @(ConvertFrom-Json '{payload}')"
        installer = str(INSTALLER).replace("'", "''")
        prefix = str(self.prefix).replace("'", "''")
        command = ("function Invoke-RestMethod { param($Uri, $Headers) " + response + " }; "
                   + f"& '{installer}' -Prefix '{prefix}' -DryRun -NoAttestation")
        return run(PWSH, "-NoProfile", "-Command", command)

    @staticmethod
    def release(version, preview=False, asset=True, draft=False):
        name = f"agentgit-{version}-{TARGET}.zip"
        return {"tag_name": "v" + version, "draft": draft, "prerelease": preview,
                "assets": [{"name": name}] if asset else []}

    def test_discovery_prefers_a_compatible_stable_release(self):
        result = self.discovered([self.release("3.0.0-beta.1", True), self.release("2.0.0")])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("AgentGit 2.0.0 for Windows", result.stdout)

    def test_discovery_skips_non_windows_and_draft_releases(self):
        result = self.discovered([self.release("9.0.0", asset=False),
                                  self.release("8.0.0", draft=True),
                                  self.release("3.0.0-beta.1", True)])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Using published prerelease v3.0.0-beta.1", result.stdout)

    def test_real_rest_json_array_paginates_to_a_compatible_release(self):
        import http.server
        import json
        import threading
        from urllib.parse import parse_qs, urlparse
        seen = []
        pages = {
            "1": [self.release(f"9.0.{i}", asset=False) for i in range(100)],
            "2": [self.release("2.0.0")],
        }

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(handler):
                page = parse_qs(urlparse(handler.path).query)["page"][0]
                seen.append(page)
                payload = json.dumps(pages[page]).encode()
                handler.send_response(200)
                handler.send_header("Content-Type", "application/json")
                handler.end_headers()
                handler.wfile.write(payload)

            def log_message(self, *args):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        script = self.root / "local-rest-installer.ps1"
        script.write_text(INSTALLER.read_text().replace(
            "https://api.github.com/repos/Paffin/AgentGit/releases",
            f"http://127.0.0.1:{server.server_port}/releases"))
        try:
            result = run(PWSH, "-NoProfile", "-File", script, "-Prefix", self.prefix,
                         "-DryRun", "-NoAttestation", timeout=20)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("AgentGit 2.0.0 for Windows", result.stdout)
        self.assertEqual(seen, ["1", "2"])

    def test_no_windows_archive_or_network_failure_installs_nothing(self):
        absent = self.discovered([self.release("0.3.0-beta.7", True, asset=False)])
        self.assertNotEqual(absent.returncode, 0)
        self.assertIn("No Windows archive published yet", absent.stderr)
        failed = self.discovered(failure=True)
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("Network unavailable", failed.stderr)
        self.assertFalse(self.prefix.exists())

    def unauthenticated_github(self, require=False):
        tools = self.root / "tools"
        tools.mkdir()
        command = tools / ("gh.cmd" if os.name == "nt" else "gh")
        content = "@echo authentication required 1>&2\n@exit /b 1\n" if os.name == "nt" else "#!/bin/sh\necho 'authentication required' >&2\nexit 1\n"
        command.write_text(content)
        command.chmod(0o755)
        env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ.get("PATH", ""))
        args = ["-RequireAttestation"] if require else []
        return run(PWSH, "-NoProfile", "-File", INSTALLER, "-Version", VERSION,
                   "-BaseUrl", self.dist, "-Prefix", self.prefix, *args, env=env)

    def test_unauthenticated_gh_stderr_allows_checksum_only_install(self):
        result = self.unauthenticated_github()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("checksum", result.stdout.lower())
        self.assertTrue((self.prefix / "bin" / "ag.exe").is_file())

    def test_required_attestation_rejects_unauthenticated_gh_before_install(self):
        result = self.unauthenticated_github(require=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("RequireAttestation", result.stderr)
        self.assertFalse(self.prefix.exists())

    def test_dry_run_does_not_download_or_create_directories(self):
        self.archive.unlink()
        result = self.install("-DryRun")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("dry run", result.stdout.lower())
        self.assertFalse(self.prefix.exists())


if __name__ == "__main__":
    unittest.main()
