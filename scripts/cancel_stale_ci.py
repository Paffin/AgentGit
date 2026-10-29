#!/usr/bin/env python3
"""A trusted main push cancels obsolete validation/preparation, never published releases."""
from __future__ import annotations

import os
import urllib.error

from request_release import GitHub

CANCELLABLE = frozenset('.github/workflows/' + name + '.yml' for name in (
    'qualification', 'prepare-release', 'full-ci', 'codeql', 'containers', 'fuzz',
    'oci-runtimes', 'packaging', 'perf', 'quality', 'security', 'windows',
))


def cancel_obsolete(api, repo, sha):
    base = '/repos/' + repo
    cancelled = []
    seen = set()
    for status in ('queued', 'in_progress', 'pending'):
        page = 1
        while True:
            batch = api.get(f'{base}/actions/runs?branch=main&status={status}&per_page=100&page={page}')['workflow_runs']
            for run in batch:
                if (run['id'] in seen or run.get('head_branch') != 'main' or run.get('head_sha') == sha
                        or run.get('path') not in CANCELLABLE or run.get('status') == 'completed'):
                    continue
                seen.add(run['id'])
                try:
                    api.post(f"{base}/actions/runs/{run['id']}/cancel", {})
                    cancelled.append(run['id'])
                except urllib.error.HTTPError as error:
                    if error.code != 409:  # already completed between listing and cancellation
                        raise
            if len(batch) < 100:
                break
            page += 1
    return cancelled


def main():
    if os.environ.get('GITHUB_EVENT_NAME') != 'push' or os.environ.get('GITHUB_REF') != 'refs/heads/main':
        raise RuntimeError('stale-run cancellation is main-push-only')
    result = cancel_obsolete(GitHub(os.environ['GH_TOKEN']), os.environ['GITHUB_REPOSITORY'], os.environ['GITHUB_SHA'])
    print('Cancelled obsolete validation/preparation runs:', result)


if __name__ == '__main__':
    main()
