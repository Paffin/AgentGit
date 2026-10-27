# SPDX-License-Identifier: Apache-2.0
"""Lose an integration response, SIGKILL the real server, restart and replay."""
import http.client
import http.server
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
import urllib.request

BIN = Path(os.environ['AG_BIN_DIR'])
root = Path(tempfile.mkdtemp(prefix='agentgit-restart-'))
atexit.register(shutil.rmtree, root, ignore_errors=True)
data = root / 'data'
env = dict(os.environ, AG_CONFIG=str(root / 'config'))
for key in list(env):
    if key.startswith('AG_TOKEN') or key in ('AG_KEY_DIR', 'AG_BOOTSTRAP_TOKEN'):
        env.pop(key)
output = (root / 'server.log').open('w')
server = subprocess.Popen([str(BIN / 'ag-server'), 'dev', '--port', '0', '--data-dir', str(data)],
                          env=env, stdout=output, stderr=output)


def run(*args, cwd=root):
    r = subprocess.run([str(BIN / 'ag'), *args], cwd=cwd, env=env, text=True, capture_output=True, timeout=30)
    if r.returncode:
        raise RuntimeError(f'ag {args[0]} failed: {r.stderr}')
    return r.stdout


try:
    for _ in range(300):
        match = re.search(r'http://127\.0\.0\.1:\d+', (root / 'server.log').read_text())
        if match:
            break
        time.sleep(0.1)
    else:
        raise RuntimeError('No server URL')
    url = match.group()
    token = (data / 'dev/token').read_text().strip()

    def api(path, body=None, base=url):
        raw = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(base + path, data=raw,
                                     headers={'Authorization': 'Bearer ' + token,
                                              'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=30) as response:
            return json.load(response)

    run('login', url, '--token-file', str(data / 'dev/token'))
    run('clone', url + '/demo/welcome', 'work')
    work = root / 'work'
    (work / 'recovered.txt').write_text('one integration, despite a lost response\n')
    run('snapshot', '-m', 'A request whose response is lost', cwd=work)
    proposal = json.loads(run('--json', 'push', '-t', 'Recover an unknown outcome', cwd=work))['proposal']['id']
    path = '/v1/repos/demo/welcome/proposals/' + proposal + '/integrate'
    request = {'idempotency_key': 'restart-qualification-1'}
    before = api('/v1/repos/demo/welcome/streams/head?stream=main')
    observed = {}

    class LoseResponse(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            self.rfile.read(int(self.headers['Content-Length']))
            try:
                observed['response'] = api(path, request)
                server.kill()
                server.wait(timeout=10)
            except Exception as error:
                observed['error'] = str(error)
            # No status line or body reaches the client.
            self.close_connection = True

    proxy = http.server.HTTPServer(('127.0.0.1', 0), LoseResponse)
    thread = threading.Thread(target=proxy.handle_request)
    thread.start()
    try:
        api(path, request, 'http://127.0.0.1:' + str(proxy.server_port))
        raise AssertionError('The client unexpectedly received a response')
    except (http.client.RemoteDisconnected, ConnectionError):
        pass
    thread.join(timeout=30)
    proxy.server_close()
    assert 'error' not in observed, observed
    assert observed['response'], observed
    server = subprocess.Popen([str(BIN / 'ag-server'), 'serve', '--data-dir', str(data),
                               '--listen', url.removeprefix('http://')],
                              env=env, stdout=output, stderr=output)
    for _ in range(300):
        try:
            after_restart = api('/v1/repos/demo/welcome/streams/head?stream=main')
            break
        except Exception:
            if server.poll() is not None:
                raise RuntimeError('Server failed to restart')
            time.sleep(0.1)
    else:
        raise RuntimeError('Restart timed out')
    retried = api(path, request)
    after_retry = api('/v1/repos/demo/welcome/streams/head?stream=main')
    assert retried == observed['response'], 'Retry did not return the original outcome'
    assert after_restart == after_retry, 'Retry changed the stream a second time'
    assert before != after_retry, 'Integration never moved the stream'
    assert 'Repository is healthy' in run('verify', cwd=data / 'repos/demo/welcome')
    report = {'version': run('--version').strip(), 'platform': 'Linux x86_64; SQLite/filesystem',
              'client_observed_disconnection': True, 'server_restarted_after_sigkill': True,
              'same_key_returned_identical_outcome': True, 'stream_unchanged_by_retry': True,
              'repository_healthy': True,
              'scope': 'Real response loss and hard restart after commit; not hardware power loss. In-transaction failure checkpoints are covered by separate engine tests.'}
    (Path(os.environ.get('AG_EVIDENCE_DIR', '.')) / 'restart-evidence.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
finally:
    if server.poll() is None:
        server.terminate()
        server.wait(timeout=10)
    output.close()
