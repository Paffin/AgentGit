#!/usr/bin/env python3
"""The release inventory must contain an SBOM for every executable in the archive."""
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


class ReleaseInventory(unittest.TestCase):
    def test_every_shipped_binary_has_a_bill_of_materials(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / 'scripts').mkdir()
            (root / 'bin').mkdir()
            shutil.copy(ROOT / 'scripts/sbom.sh', root / 'scripts/sbom.sh')
            # Emulate cargo-cyclonedx output, not the inventory assembled by sbom.sh.
            cargo = root / 'bin/cargo'
            cargo.write_text('#!/usr/bin/env python3\n' + '''import json, pathlib
for crate, binary in [('ag-cli','ag'), ('ag-server','ag-server'), ('ag-worker','ag-worker'), ('ag-mcp','ag-mcp')]:
    path = pathlib.Path('crates') / crate
    path.mkdir(parents=True, exist_ok=True)
    names = ['agentgit-types', 'tokio'] + ['dep-' + str(i) for i in range(51)]
    doc = {'bomFormat':'CycloneDX', 'specVersion':'1.5', 'components':[{'name':n} for n in names]}
    (path / (binary + '_bin.cdx.json')).write_text(json.dumps(doc))
''')
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=str(root / 'bin') + os.pathsep + os.environ['PATH'])
            result = subprocess.run(['bash', str(root / 'scripts/sbom.sh'), '--version', 'test', '--no-web'], env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            names = {p.name for p in (root / 'dist').glob('*.cdx.json')}
            self.assertEqual(names, {f'agentgit-test-{b}.cdx.json' for b in ('ag', 'ag-server', 'ag-worker', 'ag-mcp')})
            self.assertEqual(list((root / 'crates').rglob('*.cdx.json')), [])


if __name__ == '__main__':
    unittest.main()
