#!/usr/bin/env python3
"""Manifest and byte-level verification for prebuilt, immutable release candidates."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import sys

from request_release import GitHub, required_assets, resolve_candidate, validate_request

EXTRA = {'candidate.json', 'candidate-images.json', 'RELEASE_NOTES.md'}


def candidate_assets(version):
    """New candidates include Windows; older published POSIX releases stay recognizable."""
    return required_assets(version) | EXTRA | {f'agentgit-{version}-x86_64-pc-windows-msvc.zip'}


def validate(directory, sha, version, run_id):
    validate_request('candidate/source', sha, version)
    if type(run_id) not in (str, int) or not str(run_id).isdigit() or int(run_id) <= 0:
        raise ValueError('invalid preparation run')
    directory = Path(directory)
    expected = candidate_assets(version)
    actual = {p.name for p in directory.iterdir() if p.is_file()}
    if actual != expected or any(p.is_symlink() or not p.is_file() for p in directory.iterdir()):
        raise ValueError('candidate files differ from the required release set')
    metadata = json.loads((directory / 'candidate.json').read_text())
    if type(metadata.get('run_id')) is not int or metadata != {'sha': sha, 'version': version, 'run_id': int(run_id)}:
        raise ValueError('candidate source, version or preparation run does not match')
    images = json.loads((directory / 'candidate-images.json').read_text())
    if images.get('sha') != sha or images.get('version') != version:
        raise ValueError('container images belong to another candidate')
    if set(images) != {'sha', 'version', 'server', 'cli'}:
        raise ValueError('unexpected image metadata')
    for kind in ('server', 'cli'):
        if not re.fullmatch(r'sha256:[0-9a-f]{64}', images[kind]):
            raise ValueError('invalid container digest')
    sums = {}
    for line in (directory / 'SHA256SUMS').read_text().splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  ([A-Za-z0-9_.-]+)', line)
        if not match or match[2] in sums:
            raise ValueError('invalid or duplicate checksum entry')
        sums[match[2]] = match[1]
    if set(sums) != expected - {'SHA256SUMS'}:
        raise ValueError('checksums do not cover the complete candidate')
    for name, digest in sums.items():
        content = (directory / name).read_bytes()
        if not content or hashlib.sha256(content).hexdigest() != digest:
            raise ValueError('candidate checksum mismatch: ' + name)
    return images


def create(directory, sha, version, run_id):
    validate_request(os.environ.get('GITHUB_REPOSITORY', 'test/repo'), sha, version)
    directory = Path(directory)
    (directory / 'candidate.json').write_text(json.dumps({'sha': sha, 'version': version, 'run_id': int(run_id)}))
    expected = candidate_assets(version)
    if {p.name for p in directory.iterdir()} != expected:
        raise ValueError('incomplete candidate')
    sums = ''.join(hashlib.sha256((directory / name).read_bytes()).hexdigest() + '  ' + name + '\n'
                   for name in sorted(expected - {'SHA256SUMS'}))
    (directory / 'SHA256SUMS').write_text(sums)
    validate(directory, sha, version, run_id)


def main():
    mode = sys.argv[1]
    if mode == 'resolve':
        run, _ = resolve_candidate(GitHub(os.environ['GH_TOKEN']), os.environ['GITHUB_REPOSITORY'], os.environ['GITHUB_SHA'])
        with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
            stream.write('run_id=' + str(run['id']) + '\n')
        return
    args = (sys.argv[2], os.environ['GITHUB_SHA'], os.environ['VERSION'], os.environ.get('PREPARE_RUN_ID', os.environ['GITHUB_RUN_ID']))
    if mode == 'create':
        create(*args)
    elif mode == 'verify':
        images = validate(*args)
        with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
            for kind in ('server', 'cli'):
                stream.write(kind + '=' + images[kind] + '\n')
    else:
        raise ValueError('unknown candidate operation')


if __name__ == '__main__':
    main()
