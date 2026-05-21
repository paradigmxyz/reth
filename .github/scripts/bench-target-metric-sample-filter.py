#!/usr/bin/env python3
"""Build a low-pass filter for target metric sample archives."""

from __future__ import annotations

import json
import re
import sys

TARGET_METRIC_BLOCK_HEIGHT_QUERY = "reth_blockchain_tree_canonical_chain_height"
SELECTOR_RE = re.compile(
    r"^(?P<name>[a-zA-Z_:][a-zA-Z0-9_:]*)(?:\{(?P<labels>[^}]*)\})?$"
)


def parse_metric_name(query: str) -> str:
    query = query.strip()
    if query.startswith("sum(") and query.endswith(")"):
        query = query[4:-1].strip()

    match = SELECTOR_RE.match(query)
    if not match:
        raise ValueError(f"Unsupported target metric query: {query}")
    return match.group("name")


def histogram_sum_name(query: str) -> str:
    name = parse_metric_name(query)
    if name.endswith("_sum"):
        return name
    return f"{name}_sum"


def target_metric_sample_names(config: dict) -> list[str]:
    names = {TARGET_METRIC_BLOCK_HEIGHT_QUERY}

    for counter in config.get("counters", []):
        names.add(parse_metric_name(counter["query"]))

    for gauge in config.get("gauges", []):
        names.add(parse_metric_name(gauge["query"]))

    for histogram in config.get("histograms", []):
        names.add(histogram_sum_name(histogram["query"]))

    return sorted(names)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: bench-target-metric-sample-filter.py <target-metrics-config>", file=sys.stderr)
        return 2

    with open(sys.argv[1]) as f:
        config = json.load(f)

    names = target_metric_sample_names(config)
    name_pattern = "|".join(re.escape(name) for name in names)

    print(rf'"name"[[:space:]]*:[[:space:]]*"({name_pattern})"')
    print(json.dumps(names, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
