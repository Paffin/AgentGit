"""The programs under examples/ are what the READMEs and docs/guides/sdks.md quote. They run here,
against a real server, so that they cannot rot."""

from __future__ import annotations

import re
import runpy
from pathlib import Path
from typing import Any

import pytest

from tests.harness import RunningServer

EXAMPLES = Path(__file__).resolve().parents[1] / "examples"


@pytest.fixture(scope="module", autouse=True)
def _env(server: RunningServer) -> Any:
    mp = pytest.MonkeyPatch()
    mp.setenv("AGENTGIT_URL", server.base_url)
    mp.setenv("AGENTGIT_TOKEN", server.bootstrap_token)
    mp.setenv("AGENTGIT_ADMIN_TOKEN", server.bootstrap_token)
    yield
    mp.undo()


def run(name: str, capsys: pytest.CaptureFixture[str]) -> tuple[dict[str, Any], list[str]]:
    capsys.readouterr()
    namespace = runpy.run_path(str(EXAMPLES / name), run_name="__main__")
    return namespace, capsys.readouterr().out.splitlines()


def test_quickstart(capsys: pytest.CaptureFixture[str]) -> None:
    ns, out = run("quickstart.py", capsys)
    assert ns["quickstart_result"]["status"] == "integrated"
    assert ns["quickstart_result"]["readme"] == "# hello\n"
    assert out[0] == "integrated"
    assert re.fullmatch(r"[0-9a-f]{12} add a greeting", out[-1])


def test_async_quickstart(capsys: pytest.CaptureFixture[str]) -> None:
    ns, out = run("async_quickstart.py", capsys)
    assert ns["async_result"] == {"status": "integrated"}
    assert out[-1] == "integrated"


def test_agent(capsys: pytest.CaptureFixture[str]) -> None:
    ns, out = run("agent.py", capsys)
    assert re.fullmatch(r"agent:docs-bot#run_\w+", ns["agent_result"]["run_identity"])
    assert ns["agent_result"]["verified"] is True
    assert ns["agent_result"]["held"] == ["insufficient_approvals"]
    assert ns["agent_result"]["landed"] == "integrated"
    assert "True" in out
    assert "integrated" in out


def test_events(capsys: pytest.CaptureFixture[str]) -> None:
    ns, _ = run("events.py", capsys)
    assert ns["events_result"]["seen"] == ["proposal.create", "stream.integrate"]


def test_errors(capsys: pytest.CaptureFixture[str]) -> None:
    _, out = run("errors.py", capsys)
    assert out[:3] == ["not_found", "404", "False"]
    assert out[3].startswith("sdk-")
    assert "idempotency_key_reused" in out
    assert out[-1] == "True True"


def test_objects(capsys: pytest.CaptureFixture[str]) -> None:
    ns, out = run("objects.py", capsys)
    assert ns["objects_result"]["stored"] == 2
    assert ns["objects_result"]["new_head"] == ns["objects_result"]["id"]
    assert out == ["2 0", "integrated"]


def test_signing(capsys: pytest.CaptureFixture[str]) -> None:
    ns, out = run("signing.py", capsys)
    assert ns["signing_result"]["blob"] == "6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545"
    assert ns["signing_result"]["valid"] is True
    assert ns["signing_result"]["tampered"] is False
    assert out[0] == "6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545"


def test_migration(capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AGENTGIT_BUNDLE", str(EXAMPLES.parents[1] / "fixtures/git-migration.bundle"))
    monkeypatch.setenv("AGENTGIT_REPO", "examples/migration")
    monkeypatch.setenv("AGENTGIT_PUBLISH", "1")
    ns, _ = run("migration.py", capsys)
    assert ns["migration_result"].state == "imported"
    assert len(ns["migration_result"].report["branches"]) == 2
