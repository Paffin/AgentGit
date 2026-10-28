import unittest
import contextlib
import io
from pathlib import Path
from unittest.mock import MagicMock, patch
import urllib.error

import yaml

from request_release import GitHub, TARGETS, checks_pending, main, request_release, required_assets, wait_checks, workflow_runs


REPO = 'acme/project'
SHA = 'a' * 40
BASE = '/repos/' + REPO
REQUIRED = ['CI', 'CodeQL', 'Containers', 'Docs', 'Packaging', 'Quality', 'Security', 'Web UI', 'Windows']
PATHS = dict(zip(REQUIRED, ['ci', 'codeql', 'containers', 'docs', 'packaging', 'quality', 'security', 'web', 'windows']))


def green():
    return [dict(id=i + 1, name=name, path='.github/workflows/' + PATHS[name] + '.yml', status='completed', conclusion='success')
            for i, name in enumerate(REQUIRED)]


class Api:
    def __init__(self):
        self.values = {BASE + '/git/ref/heads/main': {'object': {'sha': SHA}}}
        self.writes = []

    def get(self, path):
        return self.values[path]

    def optional(self, path):
        return self.values.get(path)

    def post(self, path, body):
        self.writes.append((path, body))


class ReleaseRequest(unittest.TestCase):
    def test_every_required_workflow_runs_for_a_release_changelog_change(self):
        from request_release import REQUIRED as workflows
        root = Path(__file__).resolve().parent.parent
        for workflow in workflows:
            config = yaml.load((root / workflow).read_text(), Loader=yaml.BaseLoader)
            push = config["on"]["push"]
            if "paths" in push:
                self.assertIn("CHANGELOG.md", push["paths"], workflow)

    def test_checks_finish_before_tag_and_dispatch(self):
        api = Api()
        seen = []
        def wait(client, base, sha):
            self.assertEqual(client.writes, [])
            seen.append(sha)
        result = request_release(api, REPO, SHA, '0.3.0-beta.2', wait)
        self.assertEqual(seen, [SHA])
        self.assertEqual(result, 'requested v0.3.0-beta.2')
        self.assertEqual(api.writes, [
            (BASE + '/git/refs', {'ref': 'refs/tags/v0.3.0-beta.2', 'sha': SHA}),
            (BASE + '/actions/workflows/release.yml/dispatches',
             {'ref': 'v0.3.0-beta.2', 'inputs': {'version': '0.3.0-beta.2', 'publish': 'true'}}),
        ])

    def test_existing_published_version_does_not_wait_or_write(self):
        api = Api()
        api.values[BASE + '/releases/tags/v0.3.0-beta.2'] = {
            'draft': False,
            'assets': [dict(name=name, state='uploaded', size=1) for name in required_assets('0.3.0-beta.2')],
        }
        result = request_release(api, REPO, SHA, '0.3.0-beta.2', lambda *_: self.fail('must not wait'))
        self.assertEqual(result, 'already published v0.3.0-beta.2')
        self.assertEqual(api.writes, [])

    def test_an_incomplete_published_page_is_repaired(self):
        api = Api()
        api.values[BASE + '/releases/tags/v0.3.0-beta.2'] = {'draft': False, 'assets': []}
        self.assertEqual(request_release(api, REPO, SHA, '0.3.0-beta.2', lambda *_: None), 'requested v0.3.0-beta.2')

    def test_unusable_release_assets_do_not_hide_an_incomplete_release(self):
        for broken in ({'state': 'new', 'size': 1}, {'state': 'uploaded', 'size': 0}, {'state': 'uploaded'}):
            with self.subTest(broken=broken):
                api = Api()
                assets = [dict(name=name, state='uploaded', size=1) for name in required_assets('0.3.0-beta.2')]
                assets[0] = dict(name=assets[0]['name'], **broken)
                api.values[BASE + '/releases/tags/v0.3.0-beta.2'] = {'draft': False, 'assets': assets}
                api.values[BASE + '/git/ref/tags/v0.3.0-beta.2'] = {'object': {'type': 'commit', 'sha': SHA}}
                self.assertEqual(request_release(api, REPO, SHA, '0.3.0-beta.2', lambda *_: None), 'requested v0.3.0-beta.2')
                self.assertEqual(len(api.writes), 1)

    def test_a_superseded_main_commit_cannot_tag(self):
        api = Api()
        api.values[BASE + '/git/ref/heads/main'] = {'object': {'sha': 'b' * 40}}
        self.assertEqual(request_release(api, REPO, SHA, '0.3.0-beta.2'), 'main moved; no release requested')
        self.assertEqual(api.writes, [])

    def test_main_moving_while_checks_run_cannot_tag(self):
        api = Api()
        def wait(client, *_):
            client.values[BASE + '/git/ref/heads/main'] = {'object': {'sha': 'b' * 40}}
        self.assertEqual(request_release(api, REPO, SHA, '0.3.0-beta.2', wait), 'main moved; no release requested')
        self.assertEqual(api.writes, [])

    def test_failed_checks_cannot_write(self):
        api = Api()
        def wait(*_):
            raise RuntimeError('Security failed')
        with self.assertRaisesRegex(RuntimeError, 'Security failed'):
            request_release(api, REPO, SHA, '0.3.0-beta.2', wait)
        self.assertEqual(api.writes, [])

    def test_existing_exact_tag_can_retry_dispatch_without_retagging(self):
        api = Api()
        api.values[BASE + '/git/ref/tags/v0.3.0-beta.2'] = {'object': {'type': 'commit', 'sha': SHA}}
        request_release(api, REPO, SHA, '0.3.0-beta.2', lambda *_: None)
        self.assertEqual(len(api.writes), 1)
        self.assertTrue(api.writes[0][0].endswith('/dispatches'))

    def test_tag_for_another_commit_is_never_moved(self):
        api = Api()
        api.values[BASE + '/git/ref/tags/v0.3.0-beta.2'] = {'object': {'type': 'commit', 'sha': 'b' * 40}}
        with self.assertRaisesRegex(RuntimeError, 'another commit'):
            request_release(api, REPO, SHA, '0.3.0-beta.2', lambda *_: None)
        self.assertEqual(api.writes, [])

    def test_annotated_tag_is_resolved_before_retrying(self):
        api = Api()
        api.values[BASE + '/git/ref/tags/v0.3.0-beta.2'] = {'object': {'type': 'tag', 'sha': 'c' * 40}}
        api.values[BASE + '/git/tags/' + 'c' * 40] = {'object': {'type': 'commit', 'sha': SHA}}
        request_release(api, REPO, SHA, '0.3.0-beta.2', lambda *_: None)
        self.assertEqual(len(api.writes), 1)

    def test_missing_and_pending_checks_are_not_success(self):
        self.assertIn('Windows', checks_pending(green()[:-1]))
        runs = green()
        runs[0].update(status='in_progress', conclusion=None)
        self.assertIn('CI', checks_pending(runs))

    def test_new_failed_run_cannot_be_hidden_by_old_success(self):
        runs = green() + [dict(id=100, name='CI', path='.github/workflows/ci.yml', status='completed', conclusion='failure')]
        with self.assertRaisesRegex(RuntimeError, 'CI'):
            checks_pending(runs)

    def test_all_triggered_workflows_must_pass(self):
        runs = green() + [dict(id=100, name='Fuzz', path='.github/workflows/fuzz.yml', status='completed', conclusion='failure')]
        with self.assertRaisesRegex(RuntimeError, 'Fuzz'):
            checks_pending(runs)
        self.assertEqual(checks_pending(green()), [])

    def test_another_workflow_with_the_same_name_cannot_hide_failed_ci(self):
        runs = green()
        runs[0]['conclusion'] = 'failure'
        runs.append(dict(id=100, name='CI', path='.github/workflows/other.yml', status='completed', conclusion='success'))
        with self.assertRaisesRegex(RuntimeError, 'CI'):
            checks_pending(runs)

    def test_only_the_actual_request_workflow_is_ignored(self):
        runs = green() + [dict(id=100, name='Request release', path='.github/workflows/other.yml', status='completed', conclusion='failure')]
        with self.assertRaisesRegex(RuntimeError, 'Request release'):
            checks_pending(runs)

    def test_optional_pending_workflows_are_waited_for_and_identified_by_path(self):
        for name in ('Fuzz', 'CI'):
            with self.subTest(name=name):
                runs = green() + [dict(id=100, name=name, path='.github/workflows/other.yml', status='in_progress', conclusion=None)]
                self.assertEqual(checks_pending(runs), [name + ' (.github/workflows/other.yml)'])

    def test_request_workflow_does_not_wait_for_itself(self):
        runs = green() + [dict(id=100, name='Request release', path='.github/workflows/request-release.yml', status='in_progress', conclusion=None)]
        self.assertEqual(checks_pending(runs), [])


class ReleaseSafety(unittest.TestCase):
    def test_only_a_real_not_found_is_optional(self):
        api = GitHub('test-token')
        for status in (403, 429, 500):
            with patch.object(api, 'get', side_effect=urllib.error.HTTPError('https://api.github.com', status, 'failure', {}, None)):
                with self.assertRaises(urllib.error.HTTPError):
                    api.optional('/not-found')
        with patch.object(api, 'get', side_effect=urllib.error.HTTPError('https://api.github.com', 404, 'missing', {}, None)):
            self.assertIsNone(api.optional('/not-found'))

    def test_invalid_inputs_fail_before_any_request(self):
        for repo, sha, version in [('bad/repo/extra', SHA, '1.0.0'), (REPO, 'main', '1.0.0'), (REPO, SHA, '../main')]:
            with self.assertRaises(ValueError):
                request_release(Api(), repo, sha, version)

    def test_a_draft_release_is_not_treated_as_published(self):
        api = Api()
        api.values[BASE + '/releases/tags/v0.3.0-beta.2'] = {'draft': True}
        self.assertEqual(request_release(api, REPO, SHA, '0.3.0-beta.2', lambda *_: None), 'requested v0.3.0-beta.2')

    def test_pending_checks_timeout_without_creating_a_tag(self):
        api = Api()
        api.values[BASE + '/actions/runs?head_sha=' + SHA + '&event=push&per_page=100&page=1'] = {'workflow_runs': []}
        with self.assertRaisesRegex(RuntimeError, 'did not finish'):
            wait_checks(api, BASE, SHA, timeout=0, clock=lambda: 0, sleep=lambda _: self.fail('no sleep'))
        self.assertEqual(api.writes, [])

    def test_all_workflow_pages_are_read(self):
        api = Api()
        prefix = BASE + '/actions/runs?head_sha=' + SHA + '&event=push&per_page=100&page='
        api.values[prefix + '1'] = {'workflow_runs': green() * 11 + [green()[0]]}
        api.values[prefix + '2'] = {'workflow_runs': [dict(id=100, name='Fuzz', path='.github/workflows/fuzz.yml', status='completed', conclusion='failure')]}
        runs = workflow_runs(api, BASE, SHA)
        self.assertEqual(len(runs), 101)
        with self.assertRaisesRegex(RuntimeError, 'Fuzz'):
            checks_pending(runs)

    def test_wait_returns_only_after_all_checks_pass(self):
        api = Api()
        key = BASE + '/actions/runs?head_sha=' + SHA + '&event=push&per_page=100&page=1'
        api.values[key] = {'workflow_runs': []}
        sleeps = []
        visible = io.BytesIO()
        console = io.TextIOWrapper(visible, encoding='utf-8')
        def advance(delay):
            self.assertIn(b'Waiting for: CI', visible.getvalue(), 'show progress before waiting')
            sleeps.append(delay)
            api.values[key] = {'workflow_runs': green()}
        with contextlib.redirect_stdout(console):
            wait_checks(api, BASE, SHA, timeout=30, clock=lambda: 0, sleep=advance)
        self.assertEqual(sleeps, [20])

    def test_transport_encodes_json_and_accepts_empty_dispatch_response(self):
        api = GitHub('private-test-token')
        for payload, expected in [(b'{"ok":true}', {'ok': True}), (b'', None)]:
            response = MagicMock()
            response.__enter__.return_value.read.return_value = payload
            with patch('urllib.request.urlopen', return_value=response) as send:
                self.assertEqual(api.post('/repos/acme/project/example', {'value': 1}), expected)
                req = send.call_args.args[0]
                self.assertEqual(req.full_url, 'https://api.github.com/repos/acme/project/example')
                self.assertEqual(req.get_header('Authorization'), 'Bearer private-test-token')
                self.assertEqual(req.data, b'{"value": 1}')

    def test_entrypoint_checks_version_before_requesting(self):
        with patch.dict('os.environ', {'GH_TOKEN': 'test', 'GITHUB_REPOSITORY': REPO, 'RELEASE_SHA': SHA}), \
             patch('check_release.workspace_version', return_value='1.2.3'), \
             patch('check_release.resolve') as validate, \
             patch('request_release.request_release', return_value='requested v1.2.3') as request, \
             patch('builtins.print'):
            main()
        validate.assert_called_once_with('v1.2.3', False)
        self.assertEqual(request.call_args.args[1:], (REPO, SHA, '1.2.3'))

    def test_workflow_is_main_only_and_never_checks_out_pull_request_code(self):
        root = Path(__file__).resolve().parent.parent
        doc = yaml.safe_load((root / '.github/workflows/request-release.yml').read_text())
        triggers = doc.get('on', doc.get(True))
        self.assertEqual(set(triggers), {'push', 'workflow_dispatch'})
        self.assertEqual(triggers['push']['branches'], ['main'])
        job = doc['jobs']['request']
        self.assertEqual(job['if'], "github.ref == 'refs/heads/main'")
        checkout = job['steps'][0]['with']
        self.assertEqual(checkout['ref'], '${{ github.sha }}')
        self.assertFalse(checkout['persist-credentials'])
        self.assertEqual(job['permissions'], {'contents': 'write', 'actions': 'write'})
        release = yaml.safe_load((root / '.github/workflows/release.yml').read_text())
        targets = [row['target'] for row in release['jobs']['binaries']['strategy']['matrix']['include']]
        self.assertCountEqual(TARGETS, targets)


if __name__ == '__main__':
    unittest.main()
