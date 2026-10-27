"""Keep the accelerated browser pipeline complete and tied to the shipped runtime."""
import json
from pathlib import Path
import re
import unittest

import yaml

ROOT = Path(__file__).resolve().parent.parent


class BrowserPipeline(unittest.TestCase):
    def setUp(self):
        self.jobs = yaml.safe_load((ROOT / '.github/workflows/web.yml').read_text())['jobs']
        self.browser = self.jobs['e2e']

    def test_the_pipeline_contract_is_executed_by_ci(self):
        ci = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        commands = '\n'.join(step.get('run', '') for job in ci['jobs'].values() for step in job['steps'])
        self.assertIn('test_web_e2e_workflow', commands)

    def test_runtime_matches_the_browser_library_and_is_immutable(self):
        image = self.browser.get('container', {}).get('image', '')
        match = re.fullmatch(r'mcr\.microsoft\.com/playwright:v([^/]+)-noble@sha256:[0-9a-f]{64}', image)
        self.assertIsNotNone(match, 'browser runtime needs a version and immutable digest')
        package = json.loads((ROOT / 'apps/web/package.json').read_text())
        self.assertEqual(match.group(1), package['devDependencies']['@playwright/test'])

    def test_all_browser_projects_run_once_without_filtering_scenarios(self):
        projects = [row['project'] for row in self.browser['strategy']['matrix']['include']]
        self.assertCountEqual(projects, ['screens', 'chromium', 'firefox', 'webkit', 'tablet', 'phone'])
        runs = '\n'.join(step.get('run', '') for step in self.browser['steps'])
        self.assertIn('--no-deps', runs, 'screens have their own job and must not run three more times')
        self.assertIn('--project=${{ matrix.project }}', runs)
        self.assertNotIn('--grep', runs)
        self.assertNotIn('playwright install', runs)

    def test_browser_jobs_use_one_shared_embedded_server_build(self):
        builds = [name for name, job in self.jobs.items() if name != 'identity-provider'
                  and any('cargo build -p agentgit-server -p agentgit-cli' in step.get('run', '')
                          for step in job['steps'])]
        self.assertEqual(len(builds), 1)
        self.assertIn(builds[0], self.browser['needs'])
        self.assertEqual(self.jobs[builds[0]]['runs-on'], 'ubuntu-24.04')
        self.assertTrue(any('sha256sum --check' in step.get('run', '') for step in self.browser['steps']))
        uploads = [step['with']['name'] for step in self.jobs[builds[0]]['steps']
                   if step.get('uses', '').startswith('actions/upload-artifact@')]
        downloads = [step['with']['name'] for step in self.browser['steps']
                     if step.get('uses', '').startswith('actions/download-artifact@')]
        self.assertEqual(downloads, uploads)
        self.assertEqual(len(downloads), 1)
        execution = [step for step in self.browser['steps'] if 'playwright test' in step.get('run', '')]
        self.assertEqual(execution[0]['env']['AG_E2E_UI'], 'embedded')
        self.assertTrue(any('check_skips.py' in step.get('run', '') for step in self.browser['steps']))


if __name__ == '__main__':
    unittest.main()
