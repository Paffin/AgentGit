"""A ready candidate must be complete, immutable and bound to the exact source/run."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from release_candidate import create, validate
from request_release import required_assets, resolve_candidate, request_release
from test_request_release import Api, BASE, REPO, SHA, green

VERSION = '1.2.3'


class CandidateFiles(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        for name in required_assets(VERSION):
            (self.root / name).write_bytes(b'checked artifact')
        (self.root / 'RELEASE_NOTES.md').write_text('Release notes')
        (self.root / 'candidate-images.json').write_text(json.dumps({
            'sha': SHA, 'version': VERSION, 'server': 'sha256:' + 'a' * 64, 'cli': 'sha256:' + 'b' * 64,
        }))
        create(self.root, SHA, VERSION, 7)

    def tearDown(self):
        self.temp.cleanup()

    def test_complete_candidate_round_trip(self):
        result = validate(self.root, SHA, VERSION, 7)
        self.assertEqual(result['server'], 'sha256:' + 'a' * 64)

    def test_wrong_source_version_or_run_is_rejected(self):
        for args in (('b' * 40, VERSION, 7), (SHA, '1.2.4', 7), (SHA, VERSION, 8)):
            with self.assertRaises(ValueError):
                validate(self.root, *args)

    def test_tampering_or_missing_archive_is_rejected(self):
        archive = next(self.root.glob('*.tar.gz'))
        archive.write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError, 'checksum'):
            validate(self.root, SHA, VERSION, 7)
        archive.unlink()
        with self.assertRaises(ValueError):
            validate(self.root, SHA, VERSION, 7)

    def test_path_traversal_duplicates_extra_files_and_symlinks_are_rejected(self):
        sums = self.root / 'SHA256SUMS'
        original = sums.read_text()
        for suffix in ('0' * 64 + '  ../outside\n', original.splitlines()[0] + '\n'):
            sums.write_text(original + suffix)
            with self.assertRaises(ValueError):
                validate(self.root, SHA, VERSION, 7)
        sums.write_text(original)
        (self.root / 'extra.txt').write_text('not part of the candidate')
        with self.assertRaises(ValueError):
            validate(self.root, SHA, VERSION, 7)
        (self.root / 'extra.txt').unlink()
        archive = next(self.root.glob('*.tar.gz'))
        archive.unlink()
        archive.symlink_to(self.root / 'RELEASE_NOTES.md')
        with self.assertRaises(ValueError):
            validate(self.root, SHA, VERSION, 7)

    def test_invalid_image_reference_is_rejected_even_with_updated_checksum(self):
        path = self.root / 'candidate-images.json'
        value = json.loads(path.read_text())
        value['server'] = 'untrusted.example/image:latest'
        path.write_text(json.dumps(value))
        with self.assertRaisesRegex(ValueError, 'digest'):
            create(self.root, SHA, VERSION, 7)


class StaleRuns(unittest.TestCase):
    def test_only_old_main_validation_is_cancelled(self):
        from cancel_stale_ci import cancel_obsolete
        api = Api()
        def run(number, **kwargs):
            return dict(id=number, path='.github/workflows/full-ci.yml',
                        head_branch='main', head_sha='b' * 40, status='in_progress', **kwargs)
        rows = [run(1), dict(run(2), head_sha=SHA), dict(run(3), head_branch='feature'),
                dict(run(4), path='.github/workflows/release.yml'), dict(run(5), status='completed')]
        for status in ('queued', 'in_progress', 'pending'):
            api.values[BASE + '/actions/runs?branch=main&status=' + status + '&per_page=100&page=1'] = {'workflow_runs': rows}
        self.assertEqual(cancel_obsolete(api, REPO, SHA), [1])
        self.assertEqual(api.writes, [(BASE + '/actions/runs/1/cancel', {})])


class CandidateAPI(unittest.TestCase):
    def api(self, artifact=None):
        api = Api()
        api.values[BASE + '/actions/runs?head_sha=' + SHA + '&per_page=100&page=1'] = {'workflow_runs': green()}
        api.values[BASE + '/actions/runs/1/artifacts?per_page=100&page=1'] = {
            'artifacts': [artifact] if artifact else [],
        }
        return api

    def test_expired_or_missing_artifact_cannot_be_published(self):
        for artifact in (None, dict(name='release-dist', expired=True, size_in_bytes=5),
                         dict(name='release-dist', expired=False, size_in_bytes=0)):
            api = self.api(artifact)
            with self.assertRaisesRegex(RuntimeError, 'missing or expired'):
                request_release(api, REPO, SHA, VERSION, ready_only=True)
            self.assertEqual(api.writes, [])

    def test_ready_only_does_not_dispatch_hidden_preparation(self):
        api = self.api(dict(name='release-dist', expired=False, size_in_bytes=5))
        request_release(api, REPO, SHA, VERSION, ready_only=True)
        self.assertEqual(len(api.writes), 2)  # immutable tag, then fast publication
        self.assertFalse(any('prepare-release.yml/dispatches' in path for path, _ in api.writes))
        api.values[BASE + '/actions/runs?head_sha=' + SHA + '&per_page=100&page=1'] = {'workflow_runs': []}
        api.writes.clear()
        with self.assertRaisesRegex(RuntimeError, 'No ready candidate'):
            request_release(api, REPO, SHA, VERSION, ready_only=True)
        self.assertEqual(api.writes, [])

    def test_pinned_candidate_is_ready_while_main_keeps_advancing(self):
        api = self.api(dict(name='release-dist', expired=False, size_in_bytes=5))
        key = BASE + '/actions/runs?head_sha=' + SHA + '&per_page=100&page=1'
        api.values[key]['workflow_runs'][0]['head_branch'] = 'ci/release/v1.2.3'
        api.values[BASE + '/git/ref/heads/ci/release/v1.2.3'] = {'object': {'type': 'commit', 'sha': SHA}}
        api.values[BASE + '/git/ref/heads/main'] = {'object': {'sha': 'b' * 40}}
        request_release(api, REPO, 'b' * 40, VERSION, ready_only=True)
        self.assertEqual(api.writes[0][1]['sha'], SHA)
        self.assertEqual(api.writes[1][1]['ref'], 'v1.2.3')

    def test_artifact_pagination_is_checked(self):
        api = self.api()
        api.values[BASE + '/actions/runs/1/artifacts?per_page=100&page=1'] = {'artifacts': [dict(name='other')] * 100}
        api.values[BASE + '/actions/runs/1/artifacts?per_page=100&page=2'] = {
            'artifacts': [dict(name='release-dist', expired=False, size_in_bytes=5)],
        }
        self.assertEqual(resolve_candidate(api, REPO, SHA)[0]['id'], 1)


if __name__ == '__main__':
    unittest.main()
