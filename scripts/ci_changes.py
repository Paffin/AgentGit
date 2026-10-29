#!/usr/bin/env python3
"""Select fast CI jobs from the complete Git diff; unknown/missing history runs all jobs."""
from __future__ import annotations

import json
import os
import re
from pathlib import Path
import subprocess

FLAGS = ('rust', 'sdk', 'go', 'tooling', 'rust_deps', 'npm_deps', 'python_deps', 'go_deps')


def classify(paths):
    selected = dict.fromkeys(FLAGS, False)
    for path in paths:
        if path.startswith('.github/'):
            return dict.fromkeys(FLAGS, True)
        known = False
        if path.startswith(('crates/', 'proto/', 'migrations/', '.cargo/')) or path in ('Cargo.toml', 'Cargo.lock'):
            selected['rust'] = True
            known = True
        if path.startswith(('sdk/', 'proto/', 'crates/ag-server/', 'crates/ag-protocol/')) or path == 'docs/protocol/openapi.json':
            selected['sdk'] = True
            known = True
        if path.startswith('services/agent-orchestrator/'):
            selected['go'] = True
            known = True
        if path.startswith(('scripts/', 'tests/', 'security/', 'deploy/', 'examples/')):
            selected['tooling'] = True
            known = True
        if path.endswith(('Cargo.toml', 'Cargo.lock')) or path == 'deny.toml':
            selected['rust_deps'] = True
        if path.endswith(('package.json', 'package-lock.json')):
            selected['npm_deps'] = True
        if path.startswith('sdk/python/') and path.endswith(('pyproject.toml', 'uv.lock')):
            selected['python_deps'] = True
        if path.startswith('services/agent-orchestrator/') and path.endswith(('go.mod', 'go.sum', 'Dockerfile')):
            selected['go_deps'] = True
        if path.startswith(('apps/web/', 'docs/', 'deploy/', 'packaging/', 'examples/')):
            known = True  # Web and Docs have their own path-filtered workflows.
        if path.endswith(('.md', '.txt')) or path in ('.gitignore', '.dockerignore', 'LICENSE', 'NOTICE', 'mkdocs.yml'):
            known = True
        if not known:
            return dict.fromkeys(FLAGS, True)
    return selected


def changed_paths(event, name):
    if name == 'pull_request':
        base = event['pull_request']['base']['sha']
        if not re.fullmatch(r'[0-9a-f]{40}', base):
            return None
        revision = base + '...HEAD'
    elif name == 'push' and event.get('before', '').strip('0'):
        if not re.fullmatch(r'[0-9a-f]{40}', event['before']):
            return None
        revision = event['before'] + '..HEAD'
    else:
        return None
    try:
        result = subprocess.run(['git', 'diff', '--no-renames', '--name-only', '-z', revision, '--'], check=True, capture_output=True)
    except (OSError, subprocess.CalledProcessError):
        return None
    return result.stdout.decode('utf-8', errors='surrogateescape').rstrip('\0').split('\0') if result.stdout else []


def main():
    try:
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
        paths = changed_paths(event, os.environ.get('GITHUB_EVENT_NAME', ''))
    except (OSError, KeyError, ValueError):
        paths = None
    flags = dict.fromkeys(FLAGS, True) if paths is None else classify(paths)
    output = ''.join(f'{key}={str(value).lower()}\n' for key, value in flags.items())
    print(output, end='')
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
            stream.write(output)


if __name__ == '__main__':
    main()
