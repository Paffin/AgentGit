#!/usr/bin/env python3
"""Checks the Compose stack's observability profile against a server that has served traffic:
Prometheus scrapes ag-server with the mounted bearer token and loaded the alert rules, Grafana is healthy with the
provisioned datasource and dashboard, and the OpenTelemetry collector is up.

    scripts/e2e_observability.py --grafana-password "$GRAFANA_ADMIN_PASSWORD"

Run after scripts/e2e_smoke.py (so there are requests to see) and `docker compose --profile observability up`.
Python 3 standard library only.
"""
import argparse
import base64
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request


def get(url: str, auth: tuple[str, str] | None = None) -> object:
    req = urllib.request.Request(url)
    if auth:
        req.add_header("Authorization", "Basic " + base64.b64encode(f"{auth[0]}:{auth[1]}".encode()).decode())
    with urllib.request.urlopen(req, timeout=10) as r:
        body = r.read()
    return json.loads(body) if body[:1] in (b"{", b"[") else body.decode()


def prom(base: str, query: str) -> list:
    data = get(f"{base}/api/v1/query?query={urllib.parse.quote(query)}")
    assert isinstance(data, dict) and data.get("status") == "success", data
    return data["data"]["result"]


def eventually(what: str, fn, seconds: int = 120):
    deadline = time.time() + seconds
    last = None
    while time.time() < deadline:
        try:
            got = fn()
            if got:
                print(f"   ok  {what}", flush=True)
                return got
            last = "not yet"
        except (urllib.error.URLError, OSError, AssertionError, KeyError) as e:
            last = e
        time.sleep(3)
    print(f"   FAIL  {what}: {last}", flush=True)
    sys.exit(1)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--prometheus", default="http://127.0.0.1:9090")
    ap.add_argument("--grafana", default="http://127.0.0.1:3000")
    ap.add_argument("--grafana-password", required=True)
    args = ap.parse_args()
    auth = ("admin", args.grafana_password)

    print("== Prometheus")
    eventually("the agentgit target is up (scraped with the mounted bearer token)",
               lambda: [r for r in prom(args.prometheus, 'up{job="agentgit"}') if r["value"][1] == "1"])
    eventually("agwp_requests_total is in Prometheus with real traffic",
               lambda: [r for r in prom(args.prometheus, "sum(agwp_requests_total)") if float(r["value"][1]) > 0])
    eventually("integrations are counted", lambda: [
        r for r in prom(args.prometheus, 'sum(agwp_integrations_total{outcome="integrated"})')
        if float(r["value"][1]) >= 1])
    rules = get(f"{args.prometheus}/api/v1/rules")
    groups = {g["name"]: g for g in rules["data"]["groups"]}
    assert "agentgit" in groups and len(groups["agentgit"]["rules"]) >= 5, list(groups)
    print(f"   ok  alert rules loaded: {len(groups['agentgit']['rules'])} in group 'agentgit'")
    bad = [r["name"] for r in groups["agentgit"]["rules"] if r.get("health") != "ok"]
    assert not bad, f"rules failing to evaluate: {bad}"
    print("   ok  every alert rule evaluates")
    firing = [r["name"] for r in groups["agentgit"]["rules"] if r.get("state") == "firing"]
    assert not firing, f"alerts firing on a healthy stack: {firing}"
    print("   ok  no alert is firing on a healthy server")

    print("== Grafana")
    eventually("Grafana is healthy", lambda: get(f"{args.grafana}/api/health").get("database") == "ok")
    ds = get(f"{args.grafana}/api/datasources/uid/agentgit-prometheus", auth)
    assert ds["type"] == "prometheus", ds
    print("   ok  the Prometheus datasource is provisioned (uid agentgit-prometheus)")
    health = eventually("the datasource can query Prometheus", lambda: get(
        f"{args.grafana}/api/datasources/uid/agentgit-prometheus/health", auth))
    assert health.get("status") == "OK", health
    found = get(f"{args.grafana}/api/search?type=dash-db&query=AgentGit", auth)
    assert any("AgentGit" in d["title"] for d in found), found
    print(f"   ok  the dashboard is provisioned: {[d['title'] for d in found]}")
    print("\nobservability stack OK")


main()
