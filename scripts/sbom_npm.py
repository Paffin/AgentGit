#!/usr/bin/env python3
"""A CycloneDX 1.5 SBOM of the production dependencies of an npm project, from its package-lock.json.

  sbom_npm.py apps/web > agentgit-web-ui.cdx.json

Why not `npm sbom`: with `--omit dev` npm 10.9 lists 13 of the 37 production packages of apps/web (it drops what is
only reachable through peer dependencies - `vue` itself among them), which would make a bill of materials that
looks complete and is not. The lock file is the source of truth for what `npm ci` installs, so this reads it.

"Production" is what the lock does not mark `dev` or `devOptional`. The web UI is bundled by Vite, whose own
packages are all dev dependencies; what ends up in the bundle is a subset of the listed packages.

Reproducible: no timestamp, and the serial number is derived from the content.
"""
from __future__ import annotations

import base64
import hashlib
import json
import pathlib
import sys
import urllib.parse
import uuid

ALGORITHMS = {"sha512": "SHA-512", "sha384": "SHA-384", "sha256": "SHA-256", "sha1": "SHA-1"}


def purl(name: str, version: str) -> str:
    # pkg:npm/%40scope/name@1.2.3 - the scope's @ is percent-encoded, the slash is not
    return "pkg:npm/" + urllib.parse.quote(name, safe="/").replace("@", "%40") + "@" + version


def hashes(integrity: str | None) -> list[dict]:
    out = []
    for item in (integrity or "").split():
        algo, _, b64 = item.partition("-")
        if algo in ALGORITHMS and b64:
            out.append({"alg": ALGORITHMS[algo], "content": base64.b64decode(b64).hex()})
    return out


def component(name: str, pkg: dict) -> dict:
    ref = purl(name, pkg["version"])
    c: dict = {"type": "library", "bom-ref": ref, "name": name, "version": pkg["version"], "scope": "required", "purl": ref}
    lic = pkg.get("license")
    if isinstance(lic, str) and lic:
        c["licenses"] = [{"expression": lic}] if any(w in lic for w in (" OR ", " AND ", "(")) else [{"license": {"id": lic}}]
    if h := hashes(pkg.get("integrity")):
        c["hashes"] = h
    if pkg.get("resolved"):
        c["externalReferences"] = [{"type": "distribution", "url": pkg["resolved"]}]
    return c


def build(project: pathlib.Path) -> dict:
    lock = json.loads((project / "package-lock.json").read_text())
    manifest = json.loads((project / "package.json").read_text())
    unique: dict[str, dict] = {}  # the same name@version installed in two places is one component
    for key, pkg in sorted(lock["packages"].items()):
        if not key or pkg.get("dev") or pkg.get("devOptional") or pkg.get("link"):
            continue
        c = component(key.rsplit("node_modules/", 1)[-1], pkg)
        unique.setdefault(c["bom-ref"], c)
    comps = [unique[ref] for ref in sorted(unique)]
    root_ref = purl(manifest["name"], manifest["version"])
    body = {
        "version": 1,
        "metadata": {
            "tools": {"components": [{"type": "application", "name": "scripts/sbom_npm.py", "author": "AgentGit"}]},
            "component": {"type": "application", "bom-ref": root_ref, "name": manifest["name"], "version": manifest["version"]},
        },
        "components": comps,
        "dependencies": [{"ref": root_ref, "dependsOn": sorted({c["bom-ref"] for c in comps})}],
    }
    digest = hashlib.sha256(json.dumps(body, sort_keys=True).encode()).hexdigest()
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": "urn:uuid:" + str(uuid.uuid5(uuid.NAMESPACE_URL, digest)),
        **body,
    }


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    json.dump(build(pathlib.Path(sys.argv[1])), sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
