#!/usr/bin/env python3
"""Reuse trusted main-commit CI; never launch another test matrix for a release."""
import os
import time

from request_release import GitHub, workflow_runs


def pending_checks(runs, sha):
    paths = {'.github/workflows/' + name + '.yml' for name in ('ci', 'security', 'web')}
    latest = {}
    for run in runs:
        path = run.get('path')
        if (path in paths and run.get('head_sha') == sha
                and run.get('event') == 'push' and run.get('head_branch') == 'main'):
            if path not in latest or run['id'] > latest[path]['id']:
                latest[path] = run
    # Web is path-filtered. CI and Security run on every main push.
    pending = sorted({'.github/workflows/ci.yml', '.github/workflows/security.yml'} - latest.keys())
    for path, run in latest.items():
        if run['status'] != 'completed':
            pending.append(path)
        elif run['conclusion'] != 'success':
            raise RuntimeError('release blocked by ' + path + ': ' + str(run['conclusion']))
    return pending


def main():
    api = GitHub(os.environ['GH_TOKEN'])
    base = '/repos/' + os.environ['GITHUB_REPOSITORY']
    sha = os.environ['GITHUB_SHA']
    deadline = time.monotonic() + 840
    while True:
        pending = pending_checks(workflow_runs(api, base, sha), sha)
        if not pending:
            print('Existing main-commit checks passed; no tests restarted.')
            return
        if time.monotonic() >= deadline:
            raise RuntimeError('commit checks did not finish: ' + ', '.join(pending))
        print('Waiting for existing checks: ' + ', '.join(pending), flush=True)
        time.sleep(15)


if __name__ == '__main__':
    main()
