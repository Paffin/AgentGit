"""Prepare a main commit in the background, or publish its ready candidate immediately."""
from __future__ import annotations

import json
import os
import re
import time
import urllib.error
import urllib.request

PREPARE_WORKFLOW = '.github/workflows/prepare-release.yml'
TARGETS = ('x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu', 'aarch64-apple-darwin', 'x86_64-apple-darwin')


class GitHub:
    def __init__(self, token):
        self.token = token

    def request(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request('https://api.github.com' + path, data=data, method=method, headers={
            'Authorization': 'Bearer ' + self.token,
            'Accept': 'application/vnd.github+json',
            'X-GitHub-Api-Version': '2022-11-28',
            'Content-Type': 'application/json',
            'User-Agent': 'agentgit-release-request',
        })
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = response.read()
            return json.loads(payload) if payload else None

    def get(self, path):
        return self.request('GET', path)

    def optional(self, path):
        try:
            return self.get(path)
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            return None

    def post(self, path, body):
        return self.request('POST', path, body)


def candidate_run(runs, sha=None):
    matches = [run for run in runs
               if run.get('path') == PREPARE_WORKFLOW
               and run.get('event') in ('workflow_dispatch', 'schedule')
               and run.get('head_branch') == 'main'
               and (sha is None or run.get('head_sha') == sha)]
    return max(matches, key=lambda run: run['id'], default=None)


def checks_pending(runs, sha=None):
    run = candidate_run(runs, sha)
    if run is None:
        return ['Prepared release candidate']
    if run['status'] != 'completed':
        return ['Prepared release candidate']
    if run['conclusion'] != 'success':
        raise RuntimeError('release blocked by candidate preparation: ' + str(run['conclusion']))
    return []


def workflow_runs(api, base, sha):
    runs = []
    page = 1
    while True:
        result = api.get(f'{base}/actions/runs?head_sha={sha}&per_page=100&page={page}')
        batch = result['workflow_runs']
        runs.extend(batch)
        if len(batch) < 100:
            return runs
        page += 1


def is_current(api, base, sha):
    return api.get(base + '/git/ref/heads/main')['object']['sha'] == sha


def wait_checks(api, base, sha, *, timeout=5400, sleep=time.sleep, clock=time.monotonic):
    deadline = clock() + timeout
    runs = workflow_runs(api, base, sha)
    if candidate_run(runs, sha) is None:
        api.post(base + '/actions/workflows/prepare-release.yml/dispatches', {'ref': 'main', 'inputs': {'force': 'false'}})
    while is_current(api, base, sha):
        if not checks_pending(workflow_runs(api, base, sha), sha):
            resolve_candidate(api, base.removeprefix('/repos/'), sha)
            return
        if clock() >= deadline:
            raise RuntimeError('release preparation did not finish')
        print('Preparing release candidate for ' + sha[:12], flush=True)
        sleep(20)


def resolve_candidate(api, repo, sha):
    validate_request(repo, sha, '0.0.0')
    base = '/repos/' + repo
    runs = workflow_runs(api, base, sha)
    if checks_pending(runs, sha):
        raise RuntimeError('No ready candidate for this exact commit; run Prepare release first')
    run = candidate_run(runs, sha)
    page = 1
    while True:
        batch = api.get(f"{base}/actions/runs/{run['id']}/artifacts?per_page=100&page={page}")['artifacts']
        for artifact in batch:
            if artifact['name'] == 'release-dist' and not artifact.get('expired', True) and artifact.get('size_in_bytes', 0) > 0:
                return run, artifact
        if len(batch) < 100:
            break
        page += 1
    raise RuntimeError('Prepared candidate is missing or expired; prepare it again')


def tag_commit(api, base, ref):
    obj = ref['object']
    for _ in range(8):
        if obj['type'] == 'commit':
            return obj['sha']
        if obj['type'] != 'tag':
            break
        obj = api.get(base + '/git/tags/' + obj['sha'])['object']
    raise RuntimeError('release tag does not resolve to a commit')


def ensure_tag(api, base, tag, sha):
    ref = api.optional(base + '/git/ref/tags/' + tag)
    if ref is None:
        api.post(base + '/git/refs', {'ref': 'refs/tags/' + tag, 'sha': sha})
    elif tag_commit(api, base, ref) != sha:
        raise RuntimeError('release tag already names another commit; it will not be moved')


def validate_request(repo, sha, version):
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_.-]+', repo):
        raise ValueError('invalid repository')
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('invalid commit')
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', version):
        raise ValueError('invalid release version')



def required_assets(version):
    from package_release import RUST_BINARIES
    return ({f'agentgit-{version}-{target}.tar.gz' for target in TARGETS}
            | {f'agentgit-{version}-{binary}.cdx.json' for binary in RUST_BINARIES}
            | {'SHA256SUMS', 'agentgit.rb'})


def is_published(release, version):
    if release is None or release.get('draft') is not False:
        return False
    uploaded = {asset['name'] for asset in release.get('assets', [])
                if asset.get('state') == 'uploaded' and asset.get('size', 0) > 0}
    return required_assets(version).issubset(uploaded)


def request_release(api, repo, sha, version, wait=None, *, ready_only=False):
    validate_request(repo, sha, version)
    base, tag = '/repos/' + repo, 'v' + version
    release = api.optional(base + '/releases/tags/' + tag)
    if is_published(release, version):
        return 'already published ' + tag
    ref = api.optional(base + '/git/ref/tags/' + tag)
    if ref is not None:
        pinned = tag_commit(api, base, ref)
        if pinned != sha:
            return f'{tag} already pinned to {pinned[:12]}; bump the version to release another commit'
    if not is_current(api, base, sha):
        return 'main moved; no release requested'
    if ready_only:
        resolve_candidate(api, repo, sha)
    else:
        (wait or wait_checks)(api, base, sha)
    if not is_current(api, base, sha):
        return 'main moved; no release requested'
    ensure_tag(api, base, tag, sha)
    api.post(base + '/actions/workflows/release.yml/dispatches', {
        'ref': tag, 'inputs': {'version': version, 'publish': 'true'},
    })
    return 'requested ' + tag


def main():
    import check_release
    version = check_release.workspace_version()
    check_release.resolve('v' + version, False)
    result = request_release(GitHub(os.environ['GH_TOKEN']), os.environ['GITHUB_REPOSITORY'],
                             os.environ['RELEASE_SHA'], version, ready_only=os.environ.get('READY_ONLY') == 'true')
    print(result)


if __name__ == '__main__':
    main()
