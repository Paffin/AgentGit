"""Keep the accelerated browser pipeline complete and tied to the shipped runtime."""
import json
from pathlib import Path
import re
import os
import subprocess
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parent.parent


class BrowserPipeline(unittest.TestCase):
    def setUp(self):
        self.jobs = yaml.safe_load((ROOT / '.github/workflows/web.yml').read_text())['jobs']
        self.browser = self.jobs['e2e']

    def test_the_pipeline_contract_is_executed_by_ci(self):
        ci = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        commands = '\n'.join(step.get('run', '') for job in ci['jobs'].values() for step in job.get('steps', []))
        self.assertIn('test_web_e2e_workflow', commands)

    def test_runtime_matches_the_browser_library_and_is_immutable(self):
        image = self.browser.get('container', {}).get('image', '')
        match = re.fullmatch(r'mcr\.microsoft\.com/playwright:v([^/]+)-noble@sha256:[0-9a-f]{64}', image)
        self.assertIsNotNone(match, 'browser runtime needs a version and immutable digest')
        package = json.loads((ROOT / 'apps/web/package.json').read_text())
        self.assertEqual(match.group(1), package['devDependencies']['@playwright/test'])

    def test_identity_browser_runs_in_the_same_pinned_runtime(self):
        job = self.jobs['identity-provider']
        self.assertEqual(job['env']['PLAYWRIGHT_IMAGE'], self.browser['container']['image'])
        commands = '\n'.join(step.get('run', '') for step in job.get('steps', []))
        self.assertNotIn('playwright install', commands)
        execution = next(step['run'] for step in job.get('steps', []) if 'keycloak.spec.ts' in step.get('run', ''))
        with tempfile.TemporaryDirectory(prefix='browser runtime ') as directory:
            root = Path(directory)
            docker = root / 'docker'
            docker.write_text('#!/usr/bin/env python3\nimport json, os, sys\n'
                              'open(os.environ["CAPTURE"], "w").write(json.dumps(sys.argv[1:]))\n')
            docker.chmod(0o755)
            env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH'],
                       GITHUB_WORKSPACE=str(root / 'source tree'), RUNNER_TEMP=str(root / 'evidence'),
                       PLAYWRIGHT_IMAGE=job['env']['PLAYWRIGHT_IMAGE'], CAPTURE=str(root / 'args.json'),
                       AG_E2E_KEYCLOAK='http://localhost:8180')
            subprocess.run(['bash', '-euc', execution], env=env, check=True)
            args = json.loads((root / 'args.json').read_text())
        self.assertEqual(args[:4], ['run', '--rm', '--network=host', '--ipc=host'])
        self.assertIn(env['GITHUB_WORKSPACE'] + ':/work', args)
        self.assertIn(env['RUNNER_TEMP'] + ':/evidence', args)
        self.assertIn('AG_SKIP_REPORT=/evidence/skips.jsonl', args)
        self.assertIn('AG_SERVER_BIN=/work/target/debug/ag-server', args)
        self.assertIn('AG_CLI_BIN=/work/target/debug/ag', args)
        self.assertIn('AG_E2E_UI=embedded', args)
        self.assertIn('AG_E2E_KEYCLOAK=http://localhost:8180', args)
        self.assertIn('AG_REQUIRE_COMPONENTS=keycloak', args)
        self.assertIn('CI=true', args)
        self.assertEqual(args[-7:], [env['PLAYWRIGHT_IMAGE'], 'npx', 'playwright', 'test',
                                    '--project=chromium', '--no-deps', 'keycloak.spec.ts'])

    def test_all_browser_projects_run_once_without_filtering_scenarios(self):
        projects = [row['project'] for row in self.browser['strategy']['matrix']['include']]
        self.assertCountEqual(projects, ['screens', 'chromium', 'firefox', 'webkit', 'tablet', 'phone'])
        runs = '\n'.join(step.get('run', '') for step in self.browser['steps'])
        self.assertIn('--no-deps', runs, 'screens have their own job and must not run three more times')
        self.assertIn('--project=${{ matrix.project }}', runs)
        self.assertNotIn('--grep', runs)
        self.assertNotIn('playwright install', runs)

    def test_browser_runtime_has_owned_home_and_external_ui_fixture(self):
        runs = '\n'.join(step.get('run', '') for step in self.browser['steps'])
        self.assertIn('chown "$(id -u):$(id -g)" "$HOME"', runs)
        self.assertIn('tar -xzf "$RUNNER_TEMP/web-dist.tar.gz" -C apps/web', runs)

    def test_browser_jobs_use_one_shared_embedded_server_build(self):
        builds = [name for name, job in self.jobs.items() if name != 'identity-provider'
                  and any('cargo build -p agentgit-server -p agentgit-cli' in step.get('run', '')
                          for step in job.get('steps', []))]
        self.assertEqual(len(builds), 1)
        self.assertIn(builds[0], self.browser['needs'])
        self.assertEqual(self.jobs[builds[0]]['runs-on'], 'ubuntu-24.04')
        self.assertTrue(any('sha256sum --check' in step.get('run', '') for step in self.browser['steps']))
        uploads = [step['with']['name'] for step in self.jobs[builds[0]]['steps']
                   if step.get('uses', '').startswith('actions/upload-artifact@')]
        downloads = [step['with']['name'] for step in self.browser['steps']
                     if step.get('uses', '').startswith('actions/download-artifact@')]
        self.assertCountEqual(downloads, uploads + ['web-production-ui'])
        self.assertEqual(len(downloads), 2)
        execution = [step for step in self.browser['steps'] if 'playwright test' in step.get('run', '')]
        self.assertEqual(execution[0]['env']['AG_E2E_UI'], 'embedded')
        self.assertTrue(any('check_skips.py' in step.get('run', '') for step in self.browser['steps']))


if __name__ == '__main__':
    unittest.main()
