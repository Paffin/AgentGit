#!/usr/bin/env python3
"""Only a version change (or explicit manual request) starts candidate preparation."""
from __future__ import annotations

import os
import re
import subprocess


def changed_version(event, before):
    if event == 'workflow_dispatch':
        return True
    if not re.fullmatch(r'[0-9a-f]{40}', before or '') or not before.strip('0'):
        return True
    def manifest(ref):
        return subprocess.check_output(['git', 'show', ref + ':Cargo.toml'], text=True)
    try:
        old = manifest(before)
    except subprocess.CalledProcessError:
        subprocess.run(['git', 'fetch', '--depth=1', 'origin', before], check=True, capture_output=True)
        old = manifest(before)
    current = manifest('HEAD')
    def version(text):
        section = text.split('[workspace.package]', 1)[1].split('\n[', 1)[0]
        return re.search(r'^version\s*=\s*"([^"]+)"', section, re.M).group(1)
    return version(old) != version(current)


def main():
    needed = changed_version(os.environ['EVENT'], os.environ.get('BEFORE', ''))
    with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
        stream.write('release=' + str(needed).lower() + '\n')


if __name__ == '__main__':
    main()
