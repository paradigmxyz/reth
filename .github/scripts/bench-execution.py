#!/usr/bin/env python3
"""Publish one benchmark execution manifest; detailed phase reports stay separate.

Schema: tempo-apps-internal/apps/perf/sql/benchmark-executions.sql.
No DDL is performed. Missing credentials or upload errors leave the local
manifest in the Actions artifact and must be reported by the workflow.
"""
import argparse
import json
import os
from pathlib import Path
import re
import sys
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit
from urllib.request import Request, urlopen

PERF = "https://dev-eu-tempo-internal-perf.tail388b2e.ts.net"


def read_json(path):
    return json.loads(path.read_text()) if path.is_file() else {}


def build_manifest(root, env, status):
    repository = env["GITHUB_REPOSITORY"]
    run_id = env["GITHUB_RUN_ID"]
    attempt = env.get("GITHUB_RUN_ATTEMPT", "1")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository) or not run_id.isdigit() or not attempt.isdigit():
        raise ValueError("Invalid GitHub execution identity")
    execution_id = f"{repository.replace('/', ':')}:{run_id}:{attempt}"
    summary = read_json(root / "summary.json")
    labels_file = root / "run-order.txt"
    labels = labels_file.read_text().splitlines() if labels_file.exists() else [
        p.name for p in sorted(root.iterdir()) if p.is_dir() and re.fullmatch(r"(baseline|feature)-\d+", p.name)
    ]
    phases = []
    for label in labels:
        if not re.fullmatch(r"(baseline|feature)-\d+", label):
            continue
        side = label.split("-")[0]
        side_summary = summary.get(side, {})
        report_path = root / f"report-{label}.json"
        if not report_path.exists():
            report_path = root / label / "report.json"
        report = read_json(report_path)
        metadata = report.get("metadata") or {}
        runtime = read_json(root / label / "phase-runtime.json")
        id_file = root / f"clickhouse-run-id-{label}.txt"
        phases.append({
            "label": label,
            "commit": metadata.get("node_commit_sha") or metadata.get("git-sha") or runtime.get("commit") or side_summary.get("commit") or side_summary.get("ref", ""),
            "ref": metadata.get("git-ref") or runtime.get("name") or side_summary.get("name") or summary.get(f"{side}_ref", ""),
            "report_id": id_file.read_text().strip() if id_file.exists() else "",
            "recorded": bool(report) or any((root / label).glob("*.csv")),
        })
    observability = summary.get("observability", {})
    grafana_url = summary.get("grafana_url") or observability.get("grafana_url") or observability.get("metrics_url", "")
    if grafana_url:
        parts = urlsplit(grafana_url)
        query = dict(parse_qsl(parts.query))
        query["var-execution_id"] = execution_id
        grafana_url = urlunsplit(parts._replace(query=urlencode(query)))
    pr = env.get("BENCH_PR", "")
    if pr and (not pr.isdigit() or int(pr) <= 0):
        raise ValueError("BENCH_PR must be a positive PR number")
    return {
        "execution_id": execution_id,
        "repository": repository,
        "run_id": run_id,
        "run_attempt": int(attempt),
        "workflow": env.get("GITHUB_WORKFLOW_REF", "").split("@")[0],
        "pr_number": int(pr or 0),
        "benchmark_id": summary.get("benchmark_id") or env.get("BENCHMARK_ID") or env.get("BENCH_ID", ""),
        "status": "failure" if summary.get("exit_code", 0) else status,
        "phases": json.dumps(phases, separators=(",", ":")),
        "grafana_url": grafana_url,
    }


def upload(row, env):
    endpoint = env.get("CLICKHOUSE_URL") or env.get("CLICKHOUSE_HOST", "")
    if not endpoint:
        raise RuntimeError("ClickHouse is not configured; execution manifest was not published")
    if "://" not in endpoint:
        endpoint = f"https://{endpoint}:8443"
    parts = urlsplit(endpoint)
    query = dict(parse_qsl(parts.query))
    # Reporter URLs may carry authentication in query parameters. Never log them.
    url_user = query.pop("user", "")
    url_password = query.pop("password", "")
    user = env.get("CLICKHOUSE_USER") or url_user
    password = env.get("CLICKHOUSE_PASSWORD") or url_password
    query["database"] = env.get("CLICKHOUSE_DATABASE") or query.get("database", "default")
    query["query"] = "INSERT INTO benchmark_executions FORMAT JSONEachRow"
    url = urlunsplit(parts._replace(query=urlencode(query)))
    request = Request(url, data=(json.dumps(row) + "\n").encode(), method="POST",
                      headers={"Content-Type": "application/json",
                               "X-ClickHouse-User": user, "X-ClickHouse-Key": password})
    # Deliberately no retry: an ambiguous timeout may already have inserted.
    with urlopen(request, timeout=30) as response:
        response.read()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("results", type=Path)
    parser.add_argument("--status", choices=("success", "failure", "cancelled"), default="success")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    args.results.mkdir(parents=True, exist_ok=True)
    row = build_manifest(args.results, os.environ, args.status)
    (args.results / "execution.json").write_text(json.dumps(row, indent=2) + "\n")
    print(f"Benchmark: {PERF}/execution/{row['execution_id']}")
    if not args.dry_run:
        try:
            upload(row, os.environ)
        except Exception as error:
            # HTTP errors can contain credential-bearing URLs and server bodies.
            print(f"::warning::Execution manifest upload failed ({type(error).__name__}); see execution.json artifact", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
