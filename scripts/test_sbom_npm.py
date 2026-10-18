#!/usr/bin/env python3
"""Tests of scripts/sbom_npm.py, on the real apps/web lock file and on small lock files written by the tests.

Run:  python3 -m unittest scripts/test_sbom_npm.py
"""
import base64
import json
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import sbom_npm  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent


def project(packages: dict, name="demo", version="1.2.3") -> pathlib.Path:
    d = pathlib.Path(tempfile.mkdtemp())
    (d / "package.json").write_text(json.dumps({"name": name, "version": version}))
    (d / "package-lock.json").write_text(json.dumps({"lockfileVersion": 3, "packages": {"": {"name": name}, **packages}}))
    return d


class Units(unittest.TestCase):
    def test_purl_encodes_the_scope(self):
        self.assertEqual(sbom_npm.purl("@vue/shared", "3.5.44"), "pkg:npm/%40vue/shared@3.5.44")
        self.assertEqual(sbom_npm.purl("pinia", "3.0.4"), "pkg:npm/pinia@3.0.4")

    def test_integrity_becomes_a_hex_digest(self):
        raw = bytes(range(64))
        b64 = base64.b64encode(raw).decode()
        self.assertEqual(sbom_npm.hashes(f"sha512-{b64}"), [{"alg": "SHA-512", "content": raw.hex()}])
        self.assertEqual(sbom_npm.hashes(None), [])
        self.assertEqual(sbom_npm.hashes("md5-AAAA"), [])

    def test_dev_packages_and_links_are_left_out(self):
        d = project(
            {
                "node_modules/a": {"version": "1.0.0", "license": "MIT"},
                "node_modules/b": {"version": "2.0.0", "dev": True, "license": "MIT"},
                "node_modules/c": {"version": "3.0.0", "devOptional": True},
                "node_modules/d": {"link": True, "version": "4.0.0"},
            }
        )
        names = [c["name"] for c in sbom_npm.build(d)["components"]]
        self.assertEqual(names, ["a"])

    def test_the_same_name_and_version_in_two_places_is_one_component(self):
        d = project(
            {
                "node_modules/a": {"version": "1.0.0", "license": "MIT"},
                "node_modules/x/node_modules/a": {"version": "1.0.0", "license": "MIT"},
                "node_modules/y/node_modules/a": {"version": "2.0.0", "license": "MIT"},
            }
        )
        refs = [c["bom-ref"] for c in sbom_npm.build(d)["components"]]
        self.assertEqual(refs, ["pkg:npm/a@1.0.0", "pkg:npm/a@2.0.0"])

    def test_a_licence_expression_is_not_a_single_id(self):
        d = project(
            {
                "node_modules/a": {"version": "1", "license": "MIT"},
                "node_modules/b": {"version": "1", "license": "(MIT OR Apache-2.0)"},
            }
        )
        comps = {c["name"]: c for c in sbom_npm.build(d)["components"]}
        self.assertEqual(comps["a"]["licenses"], [{"license": {"id": "MIT"}}])
        self.assertEqual(comps["b"]["licenses"], [{"expression": "(MIT OR Apache-2.0)"}])

    def test_the_output_is_reproducible(self):
        d = project({"node_modules/a": {"version": "1.0.0"}})
        self.assertEqual(sbom_npm.build(d), sbom_npm.build(d))
        d2 = project({"node_modules/a": {"version": "1.0.1"}})
        self.assertNotEqual(sbom_npm.build(d)["serialNumber"], sbom_npm.build(d2)["serialNumber"])


class WebUi(unittest.TestCase):
    def setUp(self):
        self.doc = sbom_npm.build(ROOT / "apps" / "web")
        self.lock = json.loads((ROOT / "apps" / "web" / "package-lock.json").read_text())

    def test_every_production_package_of_the_lock_is_listed(self):
        prod = {
            (k.rsplit("node_modules/", 1)[-1], p["version"])
            for k, p in self.lock["packages"].items()
            if k and not p.get("dev") and not p.get("devOptional")
        }
        listed = {(c["name"], c["version"]) for c in self.doc["components"]}
        self.assertEqual(listed, prod)
        self.assertGreater(len(listed), 30)

    def test_the_direct_dependencies_are_there(self):
        names = {c["name"] for c in self.doc["components"]}
        self.assertTrue({"vue", "pinia", "vue-router"} <= names)

    def test_no_development_tool_is_listed(self):
        names = {c["name"] for c in self.doc["components"]}
        self.assertFalse(names & {"vite", "vitest", "eslint", "typescript", "@playwright/test"})

    def test_it_is_a_cyclonedx_1_5_document(self):
        self.assertEqual((self.doc["bomFormat"], self.doc["specVersion"]), ("CycloneDX", "1.5"))
        self.assertRegex(self.doc["serialNumber"], r"^urn:uuid:[0-9a-f-]{36}$")
        root = self.doc["metadata"]["component"]["bom-ref"]
        self.assertEqual(self.doc["dependencies"][0]["ref"], root)


if __name__ == "__main__":
    unittest.main()
