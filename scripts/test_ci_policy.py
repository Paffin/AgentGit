"""Safety and scope contracts for fast CI and exact-commit release qualification."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import yaml

from ci_changes import FLAGS, changed_paths, classify, last_successful_base

ROOT = Path(__file__).resolve().parent.parent


def workflow(name):
    return yaml.safe_load((ROOT / '.github/workflows' / (name + '.yml')).read_text())


class ChangeSelection(unittest.TestCase):
    def test_docs_do_not_compile_rust_or_sdks(self):
        flags = classify(['README.md', 'docs/guides/agents.md'])
        self.assertFalse(any(flags.values()))

    def test_web_code_does_not_launch_backend_compilation(self):
        self.assertFalse(any(classify(['apps/web/src/views/HomeView.vue']).values()))
        self.assertTrue(classify(['apps/web/package-lock.json'])['npm_deps'])

    def test_api_changes_check_sdk_contracts(self):
        flags = classify(['crates/ag-server/src/api/proposals.rs'])
        self.assertTrue(flags['rust'])
        self.assertTrue(flags['sdk'])
        self.assertFalse(flags['go'])

    def test_known_component_changes_are_scoped(self):
        self.assertTrue(classify(['services/agent-orchestrator/internal/app/run.go'])['go'])
        self.assertTrue(classify(['sdk/python/uv.lock'])['python_deps'])
        self.assertTrue(classify(['Cargo.lock'])['rust_deps'])
        self.assertTrue(classify(['deploy/compose/compose.yaml'])['tooling'])

    def test_workflow_and_unknown_paths_fail_closed(self):
        for path in ('.github/workflows/ci.yml', 'future-runtime/code.rs'):
            self.assertEqual(classify([path]), dict.fromkeys(FLAGS, True))

    def test_main_covers_changes_since_last_green_run_and_does_not_reuse_itself(self):
        class API:
            def get(self, path):
                self.path = path
                return {'workflow_runs': [{'head_sha': 'b' * 40}, {'head_sha': 'a' * 40}]}
        api = API()
        self.assertEqual(last_successful_base(api, 'org/repo', 'ci.yml', 'b' * 40), 'a' * 40)
        self.assertIn('status=success', api.path)
        with patch.object(api, 'get', side_effect=RuntimeError('network')):
            self.assertIsNone(last_successful_base(api, 'org/repo', 'ci.yml'))
        self.assertIsNone(last_successful_base(api, 'org/repo', 'untrusted.yml'))

    def test_deleted_and_renamed_files_are_both_classified(self):
        old = os.getcwd()
        with tempfile.TemporaryDirectory() as directory:
            os.chdir(directory)
            try:
                def git(*args):
                    return subprocess.check_output(['git', *args], stderr=subprocess.DEVNULL).decode().strip()
                git('init', '-q')
                git('config', 'user.email', 'ci@example.invalid')
                git('config', 'user.name', 'CI test')
                Path('crates/test').mkdir(parents=True)
                Path('crates/test/code.rs').write_text('old code')
                git('add', '.')
                git('commit', '-qm', 'base')
                base = git('rev-parse', 'HEAD')
                git('mv', 'crates/test/code.rs', 'README.md')
                git('commit', '-qm', 'rename')
                paths = changed_paths({'before': base}, 'push')
                self.assertIn('crates/test/code.rs', paths)
                self.assertTrue(classify(paths)['rust'])
                self.assertIsNone(changed_paths({'before': '0' * 40}, 'push'))
                self.assertIsNone(changed_paths({'before': '--bad'}, 'push'))
                self.assertIsNone(changed_paths({'before': 'f' * 40}, 'push'))
            finally:
                os.chdir(old)


class PipelinePolicy(unittest.TestCase):
    def test_slow_suites_are_not_triggered_on_every_commit(self):
        for name in ('full-ci', 'qualification', 'codeql', 'containers', 'windows',
                     'quality', 'packaging', 'oci-runtimes', 'fuzz', 'perf'):
            doc = workflow(name)
            triggers = doc.get('on', doc.get(True))
            self.assertNotIn('push', triggers, name)
            self.assertNotIn('pull_request', triggers, name)
            self.assertTrue(doc['concurrency']['cancel-in-progress'], name)

    def test_quick_ci_is_path_aware_and_has_an_always_reported_gate(self):
        jobs = workflow('ci')['jobs']
        self.assertIn('ci_changes.py', jobs['changes']['steps'][-1]['run'])
        self.assertEqual(jobs['complete']['if'], 'always()')
        self.assertEqual(set(jobs['complete']['needs']), set(jobs) - {'complete', 'superseded'})
        self.assertEqual(jobs['rust']['runs-on'], 'ubuntu-24.04')
        commands = '\n'.join(step.get('run', '') for step in jobs['rust']['steps'])
        self.assertIn('--lib --bins', commands)
        self.assertNotIn('cargo test --workspace --locked', commands)
        self.assertIn("--test '*'", commands)

    def test_linux_full_tests_are_already_the_coverage_run(self):
        self.assertEqual(workflow('full-ci')['jobs']['test']['strategy']['matrix']['os'], ['macos-latest'])
        coverage = workflow('quality')['jobs']['coverage']
        self.assertIn('cargo llvm-cov --workspace', '\n'.join(s.get('run', '') for s in coverage['steps']))

    def test_shared_tools_are_built_once_for_sdk_runtime_matrix(self):
        jobs = workflow('full-ci')['jobs']
        for name in ('sdk', 'sdk-runtimes', 'orchestrator', 'mcp'):
            self.assertEqual(jobs[name]['needs'], 'tools')
            commands = '\n'.join(s.get('run', '') for s in jobs[name]['steps'])
            self.assertNotIn('cargo build', commands)
            self.assertIn('sha256sum --check', commands)

    def test_browser_matrix_is_preserved_but_explicit(self):
        jobs = workflow('web')['jobs']
        for job in ('server-build', 'e2e', 'identity-provider'):
            self.assertEqual(jobs[job]['if'], 'inputs.full')
        self.assertEqual(len(jobs['e2e']['strategy']['matrix']['include']), 6)

    def test_release_only_promotes_with_a_five_minute_execution_budget(self):
        jobs = workflow('release')['jobs']
        self.assertEqual(set(jobs), {'publish', 'install'})
        self.assertEqual(jobs['publish']['timeout-minutes'] + jobs['install']['timeout-minutes'], 5)
        commands = '\n'.join(step.get('run', '') for job in jobs.values() for step in job['steps'])
        self.assertNotIn('cargo ', commands)
        self.assertIn('release_candidate.py resolve', commands)
        self.assertIn('release_candidate.py verify', commands)
        self.assertIn('--source-digest "$GITHUB_SHA"', commands)
        self.assertEqual(jobs['install']['needs'], 'publish')
        prepare = workflow('prepare-release')['jobs']
        self.assertIn('qualify', prepare['release']['needs'])
        self.assertEqual(prepare['qualify']['uses'], './.github/workflows/qualification.yml')
        for job in ('binaries', 'image'):
            self.assertEqual(prepare[job]['needs'], 'verify')

    def test_preparation_cannot_publish_public_versions(self):
        doc = workflow('prepare-release')
        self.assertNotIn('push', doc.get('on', doc.get(True)))
        commands = '\n'.join(step.get('run', '') for job in doc['jobs'].values() for step in job.get('steps', []))
        self.assertNotIn('gh release create', commands)
        self.assertNotIn('gh release upload', commands)
        self.assertNotIn('$image:$VERSION', commands)
        self.assertIn('$image:candidate-$GITHUB_SHA', commands)
        self.assertTrue(doc['concurrency']['cancel-in-progress'])

    def test_fast_ci_never_publishes_and_release_jobs_are_not_cancelled(self):
        ci = workflow('ci')
        self.assertEqual(ci['permissions'], {'contents': 'read'})
        self.assertTrue(ci['concurrency']['cancel-in-progress'])
        self.assertFalse(workflow('release')['concurrency']['cancel-in-progress'])


if __name__ == '__main__':
    unittest.main()
