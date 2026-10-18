#!/usr/bin/env python3
"""Checks the files under deploy/ so they cannot rot silently.

Always:
  * every YAML/JSON file parses;
  * every metric used by the alert rules and the Grafana dashboard is one ag-server declares
    (names are read from crates/ag-server/src/metrics.rs);
  * the generated PrometheusRule is up to date (scripts/gen_prometheusrule.py --check).

When the tools are on PATH (CI installs them), also:
  * `kustomize build` of every kustomization, validated with `kubeconform --strict`;
  * `promtool check rules/config`;
  * `docker compose config` for the Compose stack.

    python3 scripts/check_deploy.py            # tools that are missing are reported and skipped
    python3 scripts/check_deploy.py --strict   # a missing tool is a failure (CI)
"""
import json
import pathlib
import re
import shutil
import subprocess
import sys

import yaml

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEPLOY = ROOT / "deploy"
STRICT = "--strict" in sys.argv
failures: list[str] = []
skipped: list[str] = []


def fail(msg: str) -> None:
    failures.append(msg)
    print(f"FAIL  {msg}")


def ok(msg: str) -> None:
    print(f"ok    {msg}")


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def have(tool: str) -> bool:
    if shutil.which(tool):
        return True
    (failures if STRICT else skipped).append(f"{tool} not found")
    print(f"{'FAIL' if STRICT else 'skip'}  {tool} not found")
    return False


# ---- parse everything ---------------------------------------------------------------------
for path in sorted(DEPLOY.rglob("*")):
    if path.suffix in (".yml", ".yaml"):
        try:
            list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError as e:
            fail(f"{path.relative_to(ROOT)}: {e}")
    elif path.suffix == ".json":
        try:
            json.loads(path.read_text())
        except json.JSONDecodeError as e:
            fail(f"{path.relative_to(ROOT)}: {e}")
ok("all YAML and JSON files under deploy/ parse")

# ---- metrics referenced vs metrics declared -----------------------------------------------------
src = (ROOT / "crates/ag-server/src/metrics.rs").read_text()
declared: set[str] = set()
for m in re.finditer(r'(HistogramOpts|Opts|IntCounter|IntGauge)::new\(\s*"([a-z_]+)"', src):
    kind, name = m.groups()
    full = f"agwp_{name}"
    declared.add(full)
    if kind == "HistogramOpts":
        declared.update({full + "_bucket", full + "_sum", full + "_count"})
referenced: dict[str, str] = {}
exprs: list[tuple[str, str]] = []
for rule_group in yaml.safe_load((DEPLOY / "observability/alerts.yml").read_text())["groups"]:
    for r in rule_group["rules"]:
        exprs.append((f"alert {r['alert']}", r["expr"]))
dash = json.loads((DEPLOY / "observability/grafana/dashboards/agentgit-overview.json").read_text())
for p in dash["panels"]:
    for t in p.get("targets", []):
        exprs.append((f"dashboard panel '{p['title']}'", t["expr"]))
for where, expr in exprs:
    for name in re.findall(r"\bagwp_[a-z_]+\b", expr):
        referenced.setdefault(name, where)
unknown = {n: w for n, w in referenced.items() if n not in declared}
if unknown:
    for n, w in sorted(unknown.items()):
        fail(f"{w} uses {n}, which ag-server does not export")
else:
    ok(f"{len(referenced)} metrics used by alerts and dashboard are all exported")

# ---- the S3 server the tests use is the one the stack runs ---------------------------------------
# scripts/s3-test.sh downloads a pinned SeaweedFS release and runs the object-store suites against it; the Compose
# stack runs an image of SeaweedFS. If the two drift apart, "SeaweedFS passes our S3 suites" stops being true of
# what is deployed.
compose_versions = set(re.findall(r"chrislusf/seaweedfs:([\w.]+)", (DEPLOY / "compose/compose.yaml").read_text()))
m = re.search(r'^SEAWEEDFS_VERSION="([\w.]+)"', (ROOT / "scripts/s3-test.sh").read_text(), re.M)
if not m:
    fail("scripts/s3-test.sh does not pin SEAWEEDFS_VERSION")
elif compose_versions != {m.group(1)}:
    fail(
        f"scripts/s3-test.sh tests SeaweedFS {m.group(1)} but deploy/compose/compose.yaml runs "
        f"{', '.join(sorted(compose_versions)) or 'no SeaweedFS image'}"
    )
else:
    ok(f"the S3 suites run against the SeaweedFS release the Compose stack uses ({m.group(1)})")

# ---- image tags ------------------------------------------------------------------------------------
# release.yml pushes `ghcr.io/paffin/agentgit-server:X.Y.Z` (and `:latest` only for a stable release); a default of
# `latest` would silently move under an operator. Every default names the workspace version, which is what the
# release workflow's check_release.py demands the tag to equal.
cargo_version = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)
tag_sources = {
    "deploy/kubernetes/base/kustomization.yaml": r"newTag:\s*([\w.-]+)",
    "deploy/kubernetes/base/deployment.yaml": r"agentgit-server:([\w.-]+)",
    "deploy/compose/compose.yaml": r"AG_VERSION:-([\w.-]+)\}",
}
for rel, pat in tag_sources.items():
    m = re.search(pat, (ROOT / rel).read_text())
    if not m:
        fail(f"{rel}: no default image tag found")
    elif m.group(1) != cargo_version:
        fail(f"{rel}: default image tag is {m.group(1)!r}, the workspace version (what release.yml publishes) is {cargo_version!r}")
    else:
        ok(f"{rel}: default image tag is the workspace version {cargo_version}")

# ---- the job label alerts select on is the one the deployments produce ------------------------------
alert_jobs = {j for _, e in exprs for j in re.findall(r'job="([^"]+)"', e)}
prom_jobs = {
    j.strip("\"'")
    for f in (DEPLOY / "observability").glob("prometheus*.yml")
    for j in re.findall(r"job_name:\s*(\S+)", f.read_text())
}
sm = yaml.safe_load((DEPLOY / "kubernetes/overlays/prometheus-operator/servicemonitor.yaml").read_text())
job_label = sm["spec"].get("jobLabel")
svc = yaml.safe_load((DEPLOY / "kubernetes/base/service.yaml").read_text())
sm_job = svc["metadata"].get("labels", {}).get(job_label) if job_label else svc["metadata"]["name"]
if alert_jobs != prom_jobs:
    fail(f"alerts select job {sorted(alert_jobs)} but the Prometheus scrape config names {sorted(prom_jobs)}")
elif {sm_job} != alert_jobs:
    fail(f"the ServiceMonitor produces job={sm_job!r} (jobLabel={job_label!r}) but alerts select {sorted(alert_jobs)}")
else:
    ok(f"alerts, the scrape config and the ServiceMonitor agree on job={sm_job!r}")

# ---- generated rule ----------------------------------------------------------------------------
r = run([sys.executable, str(ROOT / "scripts/gen_prometheusrule.py"), "--check"])
if r.returncode:
    fail(r.stderr.strip())
else:
    ok("PrometheusRule is in sync with alerts.yml")

# ---- external tools --------------------------------------------------------------------------------
if have("kustomize") and have("kubeconform"):
    for kdir in sorted(p.parent for p in (DEPLOY / "kubernetes").rglob("kustomization.yaml")):
        built = run(["kustomize", "build", str(kdir)])
        rel = kdir.relative_to(ROOT)
        if built.returncode:
            fail(f"kustomize build {rel}: {built.stderr.strip()}")
            continue
        v = run(
            [
                "kubeconform", "-strict", "-summary", "-kubernetes-version", "1.30.0",
                "-schema-location", "default",
                "-schema-location",
                "https://raw.githubusercontent.com/datreeio/CRDs-catalog/main/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json",
            ],
            input=built.stdout,
        )
        if v.returncode:
            fail(f"kubeconform {rel}: {v.stdout.strip()} {v.stderr.strip()}")
        else:
            ok(f"{rel}: builds and validates ({v.stdout.strip().splitlines()[-1]})")

if have("promtool"):
    for cmd, f in [("check rules", "alerts.yml"), ("check config --syntax-only", "prometheus.yml"),
                   ("check config --syntax-only", "prometheus.compose.yml")]:
        r = run(["promtool", *cmd.split(), str(DEPLOY / "observability" / f)])
        if r.returncode:
            fail(f"promtool {cmd} {f}: {r.stdout.strip()} {r.stderr.strip()}")
        else:
            ok(f"promtool {cmd} {f}")

if have("docker"):
    env = {"PATH": "/usr/bin:/bin:/usr/local/bin", "AG_BOOTSTRAP_TOKEN": "x", "AG_METRICS_TOKEN": "y",
           "POSTGRES_PASSWORD": "p", "S3_ACCESS_KEY": "a", "S3_SECRET_KEY": "s", "GRAFANA_ADMIN_PASSWORD": "g"}
    r = run(["docker", "compose", "--profile", "observability", "config", "-q"], cwd=DEPLOY / "compose", env=env)
    if r.returncode:
        fail(f"docker compose config: {r.stderr.strip()}")
    else:
        ok("docker compose config (with the observability profile)")
    # Required secrets must be required: no value, no start.
    r = run(["docker", "compose", "config", "-q"], cwd=DEPLOY / "compose", env={"PATH": env["PATH"]})
    if r.returncode == 0:
        fail("the Compose stack starts without AG_BOOTSTRAP_TOKEN etc.; the :? guards are missing")
    else:
        ok("the Compose stack refuses to start without its secrets")

print()
if skipped:
    print("skipped: " + ", ".join(skipped))
if failures:
    print(f"{len(failures)} problem(s)")
    sys.exit(1)
print("deploy checks passed")
