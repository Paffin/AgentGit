# SPDX-License-Identifier: Apache-2.0
"""Qualify a live SQLite/filesystem backup using public rc.3 binaries."""
import concurrent.futures
import json
import atexit
import shutil
import os
from pathlib import Path
import re
import subprocess
import tempfile
import threading
import time

BIN = Path(os.environ['AG_BIN_DIR'])
root = Path(tempfile.mkdtemp(prefix='agentgit-live-backup-'))
atexit.register(shutil.rmtree, root, ignore_errors=True)
data = root / 'server'
log = root / 'server.log'
env = dict(os.environ, AG_CONFIG=str(root / 'config.json'))
for name in list(env):
    if name.startswith('AG_TOKEN') or name in ('AG_KEY_DIR', 'AG_BOOTSTRAP_TOKEN'):
        env.pop(name)


def run(*args, cwd=root, extra=None, expected=0):
    result = subprocess.run([str(BIN / 'ag'), *args], cwd=cwd,
                            env={**env, **(extra or {})}, text=True,
                            capture_output=True, timeout=60)
    if result.returncode != expected:
        raise RuntimeError(f'ag {args[0]}: expected {expected}, got {result.returncode}: {result.stderr}')
    return result.stdout


def read_json(*args, cwd=root):
    return json.loads(run('--json', *args, cwd=cwd))


with log.open('w') as output:
    server = subprocess.Popen([str(BIN / 'ag-server'), 'dev', '--port', '0', '--data-dir', str(data)],
                              stdout=output, stderr=output, env=env)
    try:
        for _ in range(300):
            match = re.search(r'http://127\.0\.0\.1:\d+', log.read_text())
            if match:
                break
            if server.poll() is not None:
                raise RuntimeError('Server exited before readiness')
            time.sleep(0.1)
        else:
            raise RuntimeError('No server URL')
        url = match.group()
        run('login', url, '--token-file', str(data / 'dev/token'))
        run('clone', url + '/demo/welcome', 'writer')
        writer = root / 'writer'
        payload = writer / 'payload'
        payload.mkdir()
        # Several real objects keep the object-copy phase observable while writes continue.
        for n in range(64):
            (payload / f'{n:03}.bin').write_bytes(os.urandom(524288))
        (writer / 'counter.json').write_text('{"n":0}')
        (writer / 'events.txt').write_text('')
        run('snapshot', '-m', 'Backup qualification fixture', cwd=writer)
        proposal = read_json('push', '-t', 'Backup qualification fixture', cwd=writer)['proposal']['id']
        run('remote', 'integrate', proposal, cwd=writer)
        run('clone', url + '/demo/welcome', 'review')
        review = root / 'review'
        (review / 'SECURITY.md').write_text('# Security\n\nA pending reviewed policy-sensitive change.\n')
        run('snapshot', '-m', 'Preserve a review in backup', cwd=review)
        pending = read_json('push', '-t', 'Pending reviewed change', cwd=review)['proposal']['id']
        run('remote', 'review', pending, '--approve', cwd=review,
            extra={'AG_TOKEN': (data / 'dev/reviewer-token').read_text().strip()})
        expected_review = read_json('proposal', 'show', pending, cwd=data / 'repos/demo/welcome')
        expected_policy = read_json('policy', 'show', 'main', cwd=data / 'repos/demo/welcome')
        acknowledged = []
        started = threading.Event()

        def write_changes():
            for n in range(1, 81):
                (writer / 'counter.json').write_text(json.dumps({'n': n}))
                (writer / 'events.txt').write_text(''.join(f'{i}\n' for i in range(1, n + 1)))
                run('snapshot', '-m', f'Concurrent write {n}', cwd=writer)
                pid = read_json('push', '-t', f'Concurrent write {n}', cwd=writer)['proposal']['id']
                run('remote', 'integrate', pid, cwd=writer)
                acknowledged.append(n)
                started.set()

        records = []
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
            task = pool.submit(write_changes)
            if not started.wait(30):
                task.result(timeout=1)
                raise RuntimeError('Writer did not start')
            for index in range(6):
                before = len(acknowledged)
                dest = root / f'backup-{index}'
                run('backup', str(dest), cwd=data / 'repos/demo/welcome')
                after = len(acknowledged)
                assert 'Repository is healthy' in run('verify', cwd=dest)
                n = json.loads(run('cat', 'main:counter.json', cwd=dest))['n']
                rows = run('cat', 'main:events.txt', cwd=dest).splitlines()
                assert rows == [str(i) for i in range(1, n + 1)], 'Torn backup tree'
                preserved = read_json('proposal', 'show', pending, cwd=dest)
                assert preserved == expected_review, 'Backup lost proposal/review data'
                assert read_json('policy', 'show', 'main', cwd=dest) == expected_policy, 'Backup lost policy'
                records.append({'writes_before': before, 'writes_after': after, 'restored_counter': n,
                                'tree_coherent': True, 'review_preserved': True})
            task.result(timeout=120)
        assert acknowledged == list(range(1, 81)), 'An acknowledged write was lost'
        assert any(r['writes_after'] > r['writes_before'] for r in records), 'No observed write overlapped backup'
        assert 'Repository is healthy' in run('verify', cwd=data / 'repos/demo/welcome')
        assert json.loads(run('cat', 'main:counter.json', cwd=data / 'repos/demo/welcome'))['n'] == 80
        report = {'version': run('--version').strip(), 'platform': 'Linux x86_64; live server; SQLite/filesystem',
                  'acknowledged_writes': 80, 'backups': records, 'final_server_healthy': True,
                  'scope': 'Concurrent backup is verified; fault-injection/replay tests are recorded separately.'}
        (Path(os.environ.get('AG_EVIDENCE_DIR', '.')) / 'live-backup-evidence.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))
    finally:
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=10)
