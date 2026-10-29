#!/usr/bin/env python3
"""Decide whether a release candidate needs preparing; never publishes a version."""
from __future__ import annotations

import os
import subprocess

import check_release
from request_release import GitHub, is_published


def main():
    version = check_release.workspace_version()
    check_release.resolve('v' + version, False)
    api = GitHub(os.environ['GH_TOKEN'])
    release = api.optional('/repos/' + os.environ['GITHUB_REPOSITORY'] + '/releases/tags/v' + version)
    needed = os.environ.get('FORCE') == 'true' or not is_published(release, version)
    # check_release.resolve returns the release facts; the CLI is the stable output contract.
    output = subprocess.check_output(['python3', 'scripts/check_release.py', 'v' + version], text=True)
    output += 'needed=' + str(needed).lower() + '\n'
    output += 'commit_date=' + subprocess.check_output(['git', 'show', '-s', '--format=%cs', 'HEAD'], text=True)
    output += 'epoch=' + subprocess.check_output(['git', 'show', '-s', '--format=%ct', 'HEAD'], text=True)
    with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
        stream.write(output)
    print('Preparing ' + version if needed else version + ' is already published; nothing to prepare')


if __name__ == '__main__':
    main()
