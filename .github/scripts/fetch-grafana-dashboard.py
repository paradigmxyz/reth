#!/usr/bin/env python3
"""
Fetch a Grafana dashboard and convert it to the portable import format.

Fetches the dashboard via API, replaces internal datasource/variable references
with template variables, and adds __inputs/__requires/__elements so the JSON is
importable on any Grafana instance.

Usage:
    export FETCH_GRAFANA_DASHBOARD_URL=https://<NAMESPACE>.grafana.net
    export FETCH_GRAFANA_DASHBOARD_TOKEN=glsa_...

    python3 .github/scripts/fetch-grafana-dashboard.py <dashboard-uid> > output.json
"""

import json
import os
import sys
import urllib.request

PANEL_TYPE_NAMES = {
    "bargauge": "Bar gauge",
    "gauge": "Gauge",
    "heatmap": "Heatmap",
    "piechart": "Pie chart",
    "stat": "Stat",
    "table": "Table",
    "timeseries": "Time series",
    "barchart": "Bar chart",
    "text": "Text",
    "dashlist": "Dashboard list",
    "logs": "Logs",
    "nodeGraph": "Node Graph",
    "histogram": "Histogram",
    "candlestick": "Candlestick",
    "state-timeline": "State timeline",
    "status-history": "Status history",
    "geomap": "Geomap",
    "canvas": "Canvas",
    "news": "News",
    "xychart": "XY Chart",
    "trend": "Trend",
    "datagrid": "Datagrid",
    "flamegraph": "Flame Graph",
    "traces": "Traces",
}


def fetch_json(base_url: str, token: str, path: str) -> dict:
    url = f"{base_url}{path}"
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read())


def fetch_dashboard(base_url: str, token: str, uid: str) -> dict:
    return fetch_json(base_url, token, f"/api/dashboards/uid/{uid}")


def fetch_grafana_version(base_url: str) -> str:
    req = urllib.request.Request(f"{base_url}/api/health")
    with urllib.request.urlopen(req) as resp:
        data = json.loads(resp.read())
    # version string like "13.0.0-23940615780.patch2" -> take just the semver part
    version = data.get("version", "")
    # strip build metadata after the first hyphen if it looks like a pre-release
    parts = version.split("-")
    return parts[0] if parts else version


def collect_panel_types(panels: list) -> set[str]:
    types = set()
    for panel in panels:
        ptype = panel.get("type", "")
        if ptype and ptype != "row":
            types.add(ptype)
        # nested panels inside collapsed rows
        for sub in panel.get("panels", []):
            sub_type = sub.get("type", "")
            if sub_type and sub_type != "row":
                types.add(sub_type)
    return types


def has_expression_datasource(dashboard: dict) -> bool:
    return "__expr__" in json.dumps(dashboard)


def make_exportable(dashboard: dict, grafana_version: str = "") -> dict:
    dash = json.loads(json.dumps(dashboard))  # deep copy

    # --- Strip internal fields ---
    dash.pop("id", None)

    # --- Rewrite links: point to the public repo instead of internal ---
    dash["links"] = [
        {
            "asDropdown": False,
            "icon": "external link",
            "includeVars": False,
            "keepTime": False,
            "tags": [],
            "targetBlank": True,
            "title": "Source (GitHub)",
            "tooltip": "View source file in repository",
            "type": "link",
            "url": "https://github.com/paradigmxyz/reth/tree/main/etc/grafana/dashboards",
        }
    ]

    # --- Datasource: victoriametrics -> prometheus ---
    dash_str = json.dumps(dash)
    dash_str = dash_str.replace("victoriametrics-metrics-datasource", "prometheus")
    dash = json.loads(dash_str)

    # --- Templating: instance_label constant -> ${VAR_INSTANCE_LABEL} ---
    # Also strip default-value fields the API returns that are not needed for import
    STRIP_VAR_DEFAULTS = {"allowCustomValue", "regexApplyTo"}
    for var in dash.get("templating", {}).get("list", []):
        if var.get("name") == "instance_label" and var.get("type") == "constant":
            var["query"] = "${VAR_INSTANCE_LABEL}"
            var["current"] = {
                "value": "${VAR_INSTANCE_LABEL}",
                "text": "${VAR_INSTANCE_LABEL}",
                "selected": False,
            }
            var["options"] = [
                {
                    "value": "${VAR_INSTANCE_LABEL}",
                    "text": "${VAR_INSTANCE_LABEL}",
                    "selected": False,
                }
            ]
        # Clear current values for query/datasource vars (not meaningful for import)
        elif var.get("type") in ("query", "datasource"):
            var["current"] = {}
        # Remove noisy default fields
        for field in STRIP_VAR_DEFAULTS:
            var.pop(field, None)
        # Strip falsy defaults on query/datasource vars (API returns them, export omits them)
        if var.get("type") in ("query", "datasource"):
            for field in ("hide", "multi", "skipUrlSync"):
                if not var.get(field):
                    var.pop(field, None)

    # --- Build __inputs ---
    inputs = [
        {
            "name": "DS_PROMETHEUS",
            "label": "Prometheus",
            "description": "",
            "type": "datasource",
            "pluginId": "prometheus",
            "pluginName": "Prometheus",
        },
    ]

    if has_expression_datasource(dash):
        inputs.append(
            {
                "name": "DS_EXPRESSION",
                "label": "Expression",
                "description": "",
                "type": "datasource",
                "pluginId": "__expr__",
            }
        )

    inputs.append(
        {
            "name": "VAR_INSTANCE_LABEL",
            "type": "constant",
            "label": "Instance Label",
            "value": "job",
            "description": "",
        }
    )

    # --- Build __requires ---
    requires = []

    if has_expression_datasource(dash):
        requires.append({"type": "datasource", "id": "__expr__", "version": "1.0.0"})

    panel_types = collect_panel_types(dash.get("panels", []))
    for pt in sorted(panel_types):
        requires.append(
            {
                "type": "panel",
                "id": pt,
                "name": PANEL_TYPE_NAMES.get(pt, pt),
                "version": "",
            }
        )

    requires.append(
        {"type": "grafana", "id": "grafana", "name": "Grafana", "version": grafana_version}
    )
    requires.append(
        {
            "type": "datasource",
            "id": "prometheus",
            "name": "Prometheus",
            "version": "1.0.0",
        }
    )

    # --- Assemble output (with __inputs/__requires/__elements first) ---
    output = {
        "__inputs": inputs,
        "__elements": {},
        "__requires": requires,
    }
    output.update(dash)

    return output


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <dashboard-uid>", file=sys.stderr)
        sys.exit(1)

    uid = sys.argv[1]
    base_url = os.environ.get("FETCH_GRAFANA_DASHBOARD_URL", "").rstrip("/")
    token = os.environ.get("FETCH_GRAFANA_DASHBOARD_TOKEN", "")

    if not base_url or not token:
        print(
            "Error: FETCH_GRAFANA_DASHBOARD_URL and FETCH_GRAFANA_DASHBOARD_TOKEN env vars required",
            file=sys.stderr,
        )
        sys.exit(1)

    resp = fetch_dashboard(base_url, token, uid)
    dashboard = resp["dashboard"]

    grafana_version = fetch_grafana_version(base_url)
    exported = make_exportable(dashboard, grafana_version)
    print(json.dumps(exported, indent=2))


if __name__ == "__main__":
    main()
