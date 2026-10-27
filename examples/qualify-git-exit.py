# SPDX-License-Identifier: Apache-2.0
"""Exercise the public rc.3 Git adapter and its documented losses."""
import json
import atexit
import shutil
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(tempfile.mkdtemp(prefix='agentgit-git-exit-'))
atexit.register(shutil.rmtree, root, ignore_errors=True)
ag = str(Path(os.environ['AG_BIN_DIR']) / 'ag')
env = dict(os.environ, AG_IDENTITY='user:qa', AG_CONFIG=str(root / 'config'))
for name in list(env):
    if name.startswith('AG_TOKEN') or name == 'AG_KEY_DIR':
        env.pop(name)


def run(command, cwd=root):
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True, timeout=60)
    if result.returncode:
        raise RuntimeError(f'{command[0]} {command[1]}: {result.stderr}')
    return result.stdout.strip()


def git(*args, cwd=root):
    return run(['git', *args], cwd)


def agent(*args, cwd=root):
    return run([ag, *args], cwd)


source = root / 'source'
source.mkdir()
git('init', '-b', 'main', cwd=source)
git('config', 'user.name', 'QA', cwd=source)
git('config', 'user.email', 'qa@example.invalid', cwd=source)
(source / 'README.md').write_text('# Git exit fixture\n')
pointer = 'version https://git-lfs.github.com/spec/v1\noid sha256:' + 'a' * 64 + '\nsize 123\n'
(source / 'asset.bin').write_text(pointer)
(source / 'run.sh').write_text('#!/bin/sh\nprintf done\\n\n')
(source / 'run.sh').chmod(0o755)
(source / 'link').symlink_to('README.md')
git('add', '.', cwd=source)
git('-c', 'commit.gpgsign=false', 'commit', '-m', 'Original Git history', cwd=source)
git('tag', 'fixture-tag', cwd=source)
base_tree = git('rev-parse', 'HEAD^{tree}', cwd=source)
workspace = root / 'agentgit'
workspace.mkdir()
agent('init', '--namespace', 'qa/git-exit', cwd=workspace)
imported = json.loads(agent('--json', 'git', 'import', str(source), '--stream', 'main', cwd=workspace))
agent('sync', cwd=workspace)
(workspace / 'README.md').write_text('# Git exit fixture\n\nLanded through AgentGit.\n')
agent('snapshot', '-m', 'Land a change through AgentGit', cwd=workspace)
proposal = json.loads(agent('--json', 'propose', '-t', 'A Git exit path', cwd=workspace))['id']
agent('integrate', proposal, cwd=workspace)
mirror = root / 'mirror.git'
agent('git', 'export', 'main', str(mirror), '--branch', 'main', cwd=workspace)
assert git('show', 'main:README.md', cwd=mirror) == '# Git exit fixture\n\nLanded through AgentGit.'
assert git('show', 'main:asset.bin', cwd=mirror) == pointer.strip()
assert git('rev-parse', 'main^1^{tree}', cwd=mirror) == base_tree
assert git('rev-list', '--count', 'main', cwd=mirror) == '2'
assert git('tag', '--list', cwd=mirror) == ''
# Compare the final tree independently through Git's own index.
(source / 'README.md').write_bytes((workspace / 'README.md').read_bytes())
git('add', 'README.md', cwd=source)
assert git('write-tree', cwd=source) == git('rev-parse', 'main^{tree}', cwd=mirror)
# A real gitlink entry is reported and omitted, not interpreted as file content.
sub_source = root / 'with-gitlink'
git('clone', '--quiet', str(source), str(sub_source))
git('config', 'user.name', 'QA', cwd=sub_source)
git('config', 'user.email', 'qa@example.invalid', cwd=sub_source)
head = git('rev-parse', 'HEAD', cwd=sub_source)
git('update-index', '--add', '--cacheinfo', f'160000,{head},deps/embedded', cwd=sub_source)
git('-c', 'commit.gpgsign=false', 'commit', '-m', 'Add gitlink', cwd=sub_source)
sub_workspace = root / 'sub-agentgit'
sub_workspace.mkdir()
agent('init', '--namespace', 'qa/gitlink', cwd=sub_workspace)
sub_import = json.loads(agent('--json', 'git', 'import', str(sub_source), '--stream', 'main', cwd=sub_workspace))
listing = agent('ls', 'main', cwd=sub_workspace)
assert 'deps/embedded' not in listing
assert any('submodule' in key.lower() and value for key, value in sub_import.items()), sub_import
report = {'version': agent('--version'), 'platform': 'Linux x86_64', 'import_result': imported,
          'all_imported_tree_shas_preserved': True, 'final_tree_matches_git_index': True,
          'exported_commits': 2, 'lfs_pointer_bytes_preserved_payload_not_fetched': True,
          'tags_exported': False, 'gitlink_result': sub_import,
          'gitlink_omitted_from_tree': True,
          'metadata_not_exported': ['reviews', 'policies', 'validation attestations', 'audit'],
          'scope': 'One small linear history, executable, symlink, LFS pointer and gitlink fixture; not every Git feature.'}
(Path(os.environ.get('AG_EVIDENCE_DIR', '.')) / 'git-exit-evidence.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
