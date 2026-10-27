#!/usr/bin/env python3
"""Run the published five-minute workflow against explicit CLI/server binaries.

Used after unpacking a release archive. Standard library only; binds port zero,
uses an isolated config directory, never prints the generated credentials.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time


def run(ag: str, cwd: Path, env: dict, *args: str, expected: int = 0) -> str:
    result = subprocess.run([ag, *args], cwd=cwd, env=env, text=True, capture_output=True, timeout=30)
    if result.returncode != expected:
        raise RuntimeError(f"ag {args[0]} failed with exit {result.returncode}: {result.stderr}")
    return result.stdout


def server_url(process: subprocess.Popen, log: Path) -> str:
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"dev server exited with {process.returncode}")
        match = re.search(r"http://127\.0\.0\.1:\d+", log.read_text())
        if match:
            return match.group()
        time.sleep(0.1)
    raise RuntimeError("dev server did not print a loopback URL within 30 seconds")


def verify(ag: str, server: str, root: Path) -> dict:
    env = dict(os.environ, AG_CONFIG=str(root / 'config/config.json'))
    for key in list(env):
        if key.startswith('AG_TOKEN') or key in ('AG_KEY_DIR', 'AG_BOOTSTRAP_TOKEN'):
            env.pop(key)
    data = root / 'server-data'
    log = root / 'server.log'
    with log.open('w') as output:
        process = subprocess.Popen([server, 'dev', '--port', '0', '--data-dir', str(data)],
                                   env=env, stdout=output, stderr=output)
        try:
            url = server_url(process, log)
            run(ag, root, env, 'login', url, '--token-file', str(data / 'dev/token'))
            run(ag, root, env, 'clone', url + '/demo/welcome')
            repo = root / 'welcome'
            (repo / 'src/greeting.py').write_text('def greet(name):\n    return f"Hi, {name}!"\n')
            run(ag, repo, env, 'snapshot', '-m', 'First change from an archive')
            pushed = json.loads(run(ag, repo, env, '--json', 'push', '-t', 'First change from an archive'))
            proposal = pushed['proposal']['id']
            run(ag, repo, env, 'remote', 'integrate', proposal)
            run(ag, repo, env, 'pull')
            assert 'Hi, {name}' in run(ag, repo, env, 'cat', 'main:src/greeting.py')
            assert 'Repository is healthy' in run(ag, repo, env, 'verify')
            (repo / 'SECURITY.md').write_text('# Security\n\nContact the maintainer.\n')
            run(ag, repo, env, 'snapshot', '-m', 'Protected-path change')
            pushed = json.loads(run(ag, repo, env, '--json', 'push', '-t', 'Protected-path change'))
            protected = pushed['proposal']['id']
            explanation = json.loads(run(ag, repo, env, '--json', 'remote', 'explain', protected, expected=3))
            assert explanation['standing'] == 'blocked'
            assert 'insufficient_approvals' in [item['code'] for item in explanation['explanations']]
            reviewer = dict(env, AG_TOKEN=(data / 'dev/reviewer-token').read_text().strip())
            run(ag, repo, reviewer, 'remote', 'review', protected, '--approve')
            ready = json.loads(run(ag, repo, env, '--json', 'remote', 'explain', protected))
            assert ready['standing'] == 'ready'
            run(ag, repo, env, 'remote', 'integrate', protected)
            closed = json.loads(run(ag, repo, env, '--json', 'remote', 'explain', protected, expected=4))
            assert closed['standing'] == 'refused'
            run(ag, data / 'repos/demo/welcome', env, 'backup', str(root / 'backup'))
            assert 'Repository is healthy' in run(ag, root / 'backup', env, 'verify')
            return {'cli_version': run(ag, root, env, '--version').strip(),
                    'dev_login_clone_push_integrate_pull_verify': 'passed',
                    'protected_path_review_and_remote_explain': 'passed', 'server_backup_restore_verify': 'passed'}
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ag', required=True)
    parser.add_argument('--server', required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='agentgit-onboarding-') as tmp:
        result = verify(str(Path(args.ag).resolve()), str(Path(args.server).resolve()), Path(tmp))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
