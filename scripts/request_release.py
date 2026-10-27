"""Request the existing release pipeline only for a fully checked main commit."""
from __future__ import annotations

import json
import os
import re
import time
import urllib.error
import urllib.request

REQUIRED = {
    '.github/workflows/ci.yml': 'CI',
    '.github/workflows/codeql.yml': 'CodeQL',
    '.github/workflows/containers.yml': 'Containers',
    '.github/workflows/docs.yml': 'Docs',
    '.github/workflows/packaging.yml': 'Packaging',
    '.github/workflows/quality.yml': 'Quality',
    '.github/workflows/security.yml': 'Security',
    '.github/workflows/web.yml': 'Web UI',
    '.github/workflows/windows.yml': 'Windows',
}
IGNORED = frozenset(('.github/workflows/request-release.yml', '.github/workflows/release.yml'))
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


def checks_pending(runs):
    latest = {}
    for run in sorted(runs, key=lambda item: item['id'], reverse=True):
        if run['path'] not in IGNORED:
            latest.setdefault(run['path'], run)
    failed = [run['name'] + ' (' + path + ')' for path, run in latest.items()
              if run['status'] == 'completed' and run['conclusion'] != 'success']
    if failed:
        raise RuntimeError('release blocked by checks: ' + ', '.join(sorted(failed)))
    pending = set(REQUIRED).difference(latest)
    pending = pending.union(path for path, run in latest.items() if run['status'] != 'completed')
    return sorted(REQUIRED[path] if path in REQUIRED else latest[path]['name'] + ' (' + path + ')'
                  for path in pending)


def workflow_runs(api, base, sha):
    runs = []
    page = 1
    while True:
        result = api.get(f'{base}/actions/runs?head_sha={sha}&event=push&per_page=100&page={page}')
        batch = result['workflow_runs']
        runs.extend(batch)
        if len(batch) < 100:
            return runs
        page += 1


def is_current(api, base, sha):
    return api.get(base + '/git/ref/heads/main')['object']['sha'] == sha


def wait_checks(api, base, sha, *, timeout=5400, sleep=time.sleep, clock=time.monotonic):
    deadline = clock() + timeout
    while is_current(api, base, sha):
        pending = checks_pending(workflow_runs(api, base, sha))
        if not pending:
            return
        if clock() >= deadline:
            raise RuntimeError('release checks did not finish: ' + ', '.join(pending))
        print('Waiting for: ' + ', '.join(pending), flush=True)
        sleep(20)


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
    from package_release import BINARIES
    return ({f'agentgit-{version}-{target}.tar.gz' for target in TARGETS}
            | {f'agentgit-{version}-{binary}.cdx.json' for binary in BINARIES}
            | {'SHA256SUMS', 'agentgit.rb'})


def is_published(release, version):
    if release is None or release.get('draft') is not False:
        return False
    uploaded = {asset['name'] for asset in release.get('assets', [])
                if asset.get('state') == 'uploaded' and asset.get('size', 0) > 0}
    return required_assets(version).issubset(uploaded)


def request_release(api, repo, sha, version, wait=None):
    validate_request(repo, sha, version)
    base, tag = '/repos/' + repo, 'v' + version
    release = api.optional(base + '/releases/tags/' + tag)
    if is_published(release, version):
        return 'already published ' + tag
    if not is_current(api, base, sha):
        return 'main moved; no release requested'
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
                             os.environ['RELEASE_SHA'], version)
    print(result)


if __name__ == '__main__':
    main()
