#!/usr/bin/env python3
"""Parse benchmark CSV output and generate a summary JSON + markdown comparison.

Usage:
    bench-reth-summary.py <combined_csv> <gas_csv> \
        --output-summary <summary.json> \
        --output-markdown <comment.md> \
        --baseline-csv <baseline_combined.csv> \
        [--repo <owner/repo>] \
        [--baseline-ref <sha>] \
        [--feature-name <name>] \
        [--feature-sha <sha>]

Generates a paired statistical comparison between baseline and feature.
Matches blocks by number and computes per-block diffs to cancel out gas
variance. Fails if baseline or feature CSV is missing or empty.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import random
import sys
from pathlib import Path

GIGAGAS = 1_000_000_000
BOOTSTRAP_ITERATIONS = 10_000
# t-critical for between-pairing CI (df=3, 4 cross-pairings).
# Set conservatively to reduce false positives from run-level bias.
T_BETWEEN_PAIRINGS = 4.5


def _opt_int(row: dict, key: str) -> int | None:
    """Return int value for a CSV field, or None if missing/empty."""
    v = row.get(key)
    if v is None or v == "":
        return None
    return int(v)


def parse_combined_csv(path: str) -> list[dict]:
    """Parse combined_latency.csv into a list of per-block dicts."""
    rows = []
    with open(path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append(
                {
                    "block_number": int(row["block_number"]),
                    "gas_used": int(row["gas_used"]),
                    "gas_limit": int(row["gas_limit"]),
                    "transaction_count": int(row["transaction_count"]),
                    "new_payload_latency_us": int(row["new_payload_latency"]),
                    "fcu_latency_us": int(row["fcu_latency"]),
                    "total_latency_us": int(row["total_latency"]),
                    "persistence_wait_us": _opt_int(row, "persistence_wait"),
                    "execution_cache_wait_us": _opt_int(row, "execution_cache_wait"),
                    "sparse_trie_wait_us": _opt_int(row, "sparse_trie_wait"),
                }
            )
    return rows


def parse_gas_csv(path: str) -> list[dict]:
    """Parse total_gas.csv into a list of per-block dicts."""
    rows = []
    with open(path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append(
                {
                    "block_number": int(row["block_number"]),
                    "gas_used": int(row["gas_used"]),
                    "time_us": int(row["time"]),
                }
            )
    return rows


def stddev(values: list[float], mean: float) -> float:
    if len(values) < 2:
        return 0.0
    return math.sqrt(sum((v - mean) ** 2 for v in values) / (len(values) - 1))


def percentile(sorted_vals: list[float], pct: int) -> float:
    if not sorted_vals:
        return 0.0
    idx = int(len(sorted_vals) * pct / 100)
    idx = min(idx, len(sorted_vals) - 1)
    return sorted_vals[idx]


def compute_stats(combined: list[dict]) -> dict:
    """Compute per-run statistics from parsed CSV data."""
    n = len(combined)
    if n == 0:
        return {}

    latencies_ms = [r["new_payload_latency_us"] / 1_000 for r in combined]
    sorted_lat = sorted(latencies_ms)
    mean_lat = sum(latencies_ms) / n
    std_lat = stddev(latencies_ms, mean_lat)

    mgas_s_values = []
    for r in combined:
        lat_s = r["new_payload_latency_us"] / 1_000_000
        if lat_s > 0:
            mgas_s_values.append(r["gas_used"] / lat_s / 1_000_000)
    mean_mgas_s = sum(mgas_s_values) / len(mgas_s_values) if mgas_s_values else 0

    total_latencies_ms = [r["total_latency_us"] / 1_000 for r in combined]
    wall_clock_s = sum(total_latencies_ms) / 1_000
    mean_total_lat_ms = sum(total_latencies_ms) / n

    # Persistence wait mean (for main table)
    persist_values_ms = []
    for r in combined:
        v = r.get("persistence_wait_us")
        if v is not None:
            persist_values_ms.append(v / 1_000)
    mean_persist_ms = sum(persist_values_ms) / len(persist_values_ms) if persist_values_ms else 0.0

    return {
        "n": n,
        "total_gas": sum(r["gas_used"] for r in combined),
        "total_txs": sum(r["transaction_count"] for r in combined),
        "mean_ms": mean_lat,
        "stddev_ms": std_lat,
        "p50_ms": percentile(sorted_lat, 50),
        "p90_ms": percentile(sorted_lat, 90),
        "p99_ms": percentile(sorted_lat, 99),
        "mean_mgas_s": mean_mgas_s,
        "wall_clock_s": wall_clock_s,
        "mean_total_lat_ms": mean_total_lat_ms,
        "mean_persist_ms": mean_persist_ms,
    }


def compute_wait_stats(combined: list[dict], field: str) -> dict:
    """Compute mean/p50/p95 for a wait time field (in ms)."""
    values_ms = []
    for r in combined:
        v = r.get(field)
        if v is not None:
            values_ms.append(v / 1_000)
    if not values_ms:
        return {}
    n = len(values_ms)
    mean_val = sum(values_ms) / n
    sorted_vals = sorted(values_ms)
    return {
        "mean_ms": mean_val,
        "p50_ms": percentile(sorted_vals, 50),
        "p95_ms": percentile(sorted_vals, 95),
    }


def load_reports(paths: list[str] | None) -> list[dict]:
    """Load txgen JSON reports, ignoring missing optional paths."""
    reports = []
    for path in paths or []:
        p = Path(path)
        if not p.exists():
            continue
        with p.open() as f:
            reports.append(json.load(f))
    return reports


def _metric_aliases(name: str) -> set[str]:
    """Return accepted Prometheus sample names for a logical metric name."""
    normalized = name.replace(".", "_")
    aliases = {name, normalized}
    if not normalized.startswith("reth_"):
        aliases.add(f"reth_{normalized}")
    return aliases


def _samples_for_metric(report: dict, name: str, suffix: str = "") -> list[dict]:
    aliases = {f"{alias}{suffix}" for alias in _metric_aliases(name)}
    return [s for s in report.get("samples", []) if s.get("name") in aliases]


def _finite_float(value) -> float | None:
    try:
        v = float(value)
    except (TypeError, ValueError):
        return None
    return v if math.isfinite(v) else None


def _bucket_le(value) -> float | None:
    if value in ("+Inf", "Inf", "inf"):
        return math.inf
    return _finite_float(value)


def _histogram_quantile(report: dict, name: str, q: float) -> float | None:
    """Compute a quantile from Prometheus histogram bucket deltas for one report."""
    buckets: dict[float, float] = {}
    grouped: dict[tuple[tuple[str, str], ...], list[dict]] = {}
    for sample in _samples_for_metric(report, name, "_bucket"):
        labels = sample.get("labels") or {}
        le = _bucket_le(labels.get("le"))
        if le is None:
            continue
        key = tuple(sorted(labels.items()))
        grouped.setdefault(key, []).append(sample)

    for samples in grouped.values():
        samples.sort(key=lambda s: s.get("offset_ms", 0))
        if len(samples) < 2:
            continue
        le = _bucket_le((samples[-1].get("labels") or {}).get("le"))
        if le is None:
            continue
        delta = samples[-1].get("value", 0) - samples[0].get("value", 0)
        if delta > 0:
            buckets[le] = buckets.get(le, 0.0) + delta

    if not buckets:
        samples = []
        for sample in _samples_for_metric(report, name):
            quantile = _finite_float((sample.get("labels") or {}).get("quantile"))
            if quantile is not None and abs(quantile - q) < 1e-9:
                samples.append(sample)
        if not samples:
            return None
        samples.sort(key=lambda s: s.get("offset_ms", 0))
        return samples[-1].get("value")

    ordered = sorted(buckets.items())
    total = ordered[-1][1]
    if total <= 0:
        return None
    wanted = total * q
    prev_le = 0.0
    prev_count = 0.0
    for le, count in ordered:
        if count >= wanted:
            bucket_count = count - prev_count
            if bucket_count <= 0:
                return le
            pos = (wanted - prev_count) / bucket_count
            return prev_le if math.isinf(le) else prev_le + (le - prev_le) * pos
        prev_le = le
        prev_count = count
    return ordered[-1][0]


def _counter_delta(report: dict, name: str) -> float | None:
    samples = _samples_for_metric(report, name)
    if len(samples) < 2:
        return None
    samples.sort(key=lambda s: s.get("offset_ms", 0))
    return max(0.0, samples[-1].get("value", 0) - samples[0].get("value", 0))


def _gauge_values(report: dict, name: str) -> list[float]:
    return [s.get("value", 0.0) for s in _samples_for_metric(report, name)]


def _avg(values: list[float]) -> float | None:
    return sum(values) / len(values) if values else None


def _max(values: list[float]) -> float | None:
    return max(values) if values else None


def _report_metric_value(report: dict, metric: dict) -> float | None:
    kind = metric["kind"]
    name = metric["name"]
    if kind == "hist_p95":
        value = _histogram_quantile(report, name, 0.95)
    elif kind == "hist_p50":
        value = _histogram_quantile(report, name, 0.50)
    elif kind == "counter_delta":
        value = _counter_delta(report, name)
    elif kind == "gauge_avg":
        value = _avg(_gauge_values(report, name))
    elif kind == "gauge_max":
        value = _max(_gauge_values(report, name))
    else:
        value = None
    if value is None or not math.isfinite(value):
        return None
    return value


def _report_metric_values(reports: list[dict], metric: dict) -> list[float]:
    values = []
    for report in reports:
        value = _report_metric_value(report, metric)
        if value is not None:
            values.append(value)
    return values


def _summarize_report_metric(reports: list[dict], metric: dict) -> float | None:
    return _avg(_report_metric_values(reports, metric))


def _ratio(num: float | None, den: float | None) -> float | None:
    if num is None or den is None or den <= 0:
        return None
    return num / den


def _format_metric_value(value: float | None, unit: str) -> str:
    if value is None:
        return "—"
    if unit == "seconds_ms":
        return fmt_ms(value * 1_000)
    if unit == "bytes":
        return format_bytes(value)
    if unit == "percent":
        return f"{value * 100:.1f}%"
    if unit == "count":
        return f"{value:,.0f}"
    if unit == "float":
        return f"{value:.2f}"
    return str(value)


def _format_simple_change(base: float | None, feat: float | None, unit: str = "") -> str:
    if base is None or feat is None:
        return "—"
    if unit == "percent":
        return f"{(feat - base) * 100:+.1f}pp"
    if base == 0:
        return "same" if feat == 0 else "n/a"
    return f"{(feat - base) / base * 100:+.2f}%"


def _metric_diffs(values_a: list[float], values_b: list[float]) -> list[float]:
    return [b - a for a in values_a for b in values_b]


def _metric_ci(values_a: list[float], values_b: list[float]) -> float:
    diffs = _metric_diffs(values_a, values_b)
    if not diffs:
        return 0.0
    return _between_pairing_ci(diffs)


def _directions_agree(diffs: list[float]) -> bool:
    nonzero = [d for d in diffs if d != 0]
    if not nonzero:
        return True
    return all(d > 0 for d in nonzero) or all(d < 0 for d in nonzero)


def _signal_icon(
    base: float | None,
    feat: float | None,
    ci: float,
    values_a: list[float],
    values_b: list[float],
    lower_is_better: bool | None,
) -> str:
    if base is None or feat is None or lower_is_better is None:
        return "⚪"
    diff = feat - base
    diffs = _metric_diffs(values_a, values_b)
    significant = len(values_a) >= 2 and len(values_b) >= 2 and abs(diff) > ci and _directions_agree(diffs)
    if not significant:
        return "⚪"
    return "✅" if (diff < 0) == lower_is_better else "❌"


def _format_delta(value: float, unit: str) -> str:
    sign = "+" if value >= 0 else ""
    if unit == "ms":
        return f"{sign}{value:.2f}ms"
    if unit == "seconds_ms":
        return f"{sign}{value * 1_000:.2f}ms"
    if unit == "bytes":
        return f"{sign}{format_bytes(value)}"
    if unit == "percent":
        return f"{sign}{value * 100:.1f}pp"
    if unit == "count":
        return f"{sign}{value:,.0f}"
    return f"{sign}{value:.2f}"


def _format_ci(value: float, unit: str) -> str:
    if unit == "ms":
        return f"±{value:.2f}ms"
    if unit == "seconds_ms":
        return f"±{value * 1_000:.2f}ms"
    if unit == "bytes":
        return f"±{format_bytes(value)}"
    if unit == "percent":
        return f"±{value * 100:.1f}pp"
    if unit == "count":
        return f"±{value:,.0f}"
    return f"±{value:.2f}"


def _format_change_with_ci(
    base: float | None,
    feat: float | None,
    ci: float,
    values_a: list[float],
    values_b: list[float],
    unit: str = "",
    lower_is_better: bool | None = True,
) -> str:
    if base is None or feat is None:
        return "—"
    icon = _signal_icon(base, feat, ci, values_a, values_b, lower_is_better)
    diff = feat - base
    display_zero = diff == 0 or \
        (unit == "seconds_ms" and abs(diff * 1_000) < 0.005) or \
        (unit == "ms" and abs(diff) < 0.005) or \
        (unit == "percent" and abs(diff * 100) < 0.05)
    if display_zero:
        return f"{icon} same ({_format_ci(ci, unit)})"
    pct = "" if unit == "percent" or abs(base) < 1e-12 else f" ({diff / base * 100:+.2f}%)"
    return f"{icon} {_format_delta(diff, unit)} ({_format_ci(ci, unit)}){pct}"


def format_bytes(value: float) -> str:
    units = ["B", "KiB", "MiB", "GiB"]
    v = float(value)
    for unit in units:
        if abs(v) < 1024 or unit == units[-1]:
            return f"{v:.1f}{unit}" if unit != "B" else f"{v:.0f}B"
        v /= 1024
    return f"{v:.1f}GiB"


def summarize_prometheus_metrics(baseline_reports: list[dict], feature_reports: list[dict]) -> dict:
    """Summarize selected reth Prometheus metrics from txgen report samples."""
    phase_defs = [
        {"key": "evm_execution", "label": "EVM execution", "name": "sync_execution_execution_histogram", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "pre_execution", "label": "Pre-execution", "name": "sync_execution_pre_execution_histogram", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "transaction_execution", "label": "Transaction execution", "name": "sync_execution_transaction_execution_histogram", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "post_execution", "label": "Post-execution", "name": "sync_execution_post_execution_histogram", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "payload_validation", "label": "Payload validation", "name": "sync_block_validation_payload_validation_histogram", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "state_root", "label": "State root", "name": "sync_block_validation_state_root_histogram", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "bal_validation", "label": "BAL validation", "name": "execution_block_access_list_validation_time_seconds", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "persistence", "label": "Persistence task", "name": "consensus_engine_beacon_persistence_duration", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "db_commit", "label": "DB commit", "name": "database_transaction_commit_whole_duration_seconds", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
    ]
    cache_defs = [
        {"key": "execution_cache_wait", "label": "Execution cache wait p95", "name": "consensus_engine_beacon_execution_cache_wait_duration", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "sparse_trie_total", "label": "Sparse trie total p95", "name": "tree_root_sparse_trie_total_duration_histogram", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "sparse_trie_cache_wait", "label": "Sparse trie cache wait p95", "name": "tree_root_sparse_trie_cache_wait_duration_histogram", "kind": "hist_p95", "unit": "seconds_ms", "lower_is_better": True},
        {"key": "sparse_trie_retained_memory", "label": "Retained sparse trie memory", "name": "tree_root_sparse_trie_retained_memory_bytes", "kind": "gauge_max", "unit": "bytes", "lower_is_better": True},
        {"key": "multiproof_account_nodes", "label": "Multiproof account nodes", "name": "sparse_state_trie_multiproof_total_account_nodes", "kind": "hist_p95", "unit": "count", "lower_is_better": True},
        {"key": "multiproof_storage_nodes", "label": "Multiproof storage nodes", "name": "sparse_state_trie_multiproof_total_storage_nodes", "kind": "hist_p95", "unit": "count", "lower_is_better": True},
    ]
    bal_defs = [
        {"key": "bal_valid", "label": "Valid BALs", "name": "execution_block_access_list_valid_total", "kind": "counter_delta", "unit": "count", "lower_is_better": None},
        {"key": "bal_invalid", "label": "Invalid BALs", "name": "execution_block_access_list_invalid_total", "kind": "counter_delta", "unit": "count", "lower_is_better": True},
        {"key": "bal_size", "label": "BAL size p95", "name": "execution_block_access_list_size_bytes", "kind": "hist_p95", "unit": "bytes", "lower_is_better": None},
        {"key": "bal_account_changes", "label": "Account changes p95", "name": "execution_block_access_list_account_changes", "kind": "hist_p95", "unit": "count", "lower_is_better": None},
        {"key": "bal_storage_changes", "label": "Storage changes p95", "name": "execution_block_access_list_storage_changes", "kind": "hist_p95", "unit": "count", "lower_is_better": None},
        {"key": "bal_balance_changes", "label": "Balance changes p95", "name": "execution_block_access_list_balance_changes", "kind": "hist_p95", "unit": "count", "lower_is_better": None},
        {"key": "bal_nonce_changes", "label": "Nonce changes p95", "name": "execution_block_access_list_nonce_changes", "kind": "hist_p95", "unit": "count", "lower_is_better": None},
        {"key": "bal_code_changes", "label": "Code changes p95", "name": "execution_block_access_list_code_changes", "kind": "hist_p95", "unit": "count", "lower_is_better": None},
    ]

    def make_rows(defs: list[dict]) -> list[dict]:
        rows = []
        for d in defs:
            b_values = _report_metric_values(baseline_reports, d)
            f_values = _report_metric_values(feature_reports, d)
            b = _avg(b_values)
            f = _avg(f_values)
            if b is None and f is None:
                continue
            if (b or 0) == 0 and (f or 0) == 0 and not d.get("keep_zero", False):
                continue
            unit = d["unit"]
            ci = _metric_ci(b_values, f_values)
            rows.append({
                "key": d["key"],
                "label": d["label"],
                "unit": unit,
                "baseline": b,
                "feature": f,
                "ci": ci,
                "baseline_fmt": _format_metric_value(b, unit),
                "feature_fmt": _format_metric_value(f, unit),
                "change": _format_change_with_ci(b, f, ci, b_values, f_values, unit, d.get("lower_is_better", True)),
            })
        return rows

    # Cache hit rates from avg hit/miss gauges.
    cache_rows = make_rows(cache_defs)
    for prefix, label in [("account", "Account cache hit rate"), ("storage", "Storage cache hit rate"), ("code", "Code cache hit rate")]:
        hits_metric = {"name": f"sync_caching_{prefix}_cache_hits", "kind": "gauge_avg"}
        miss_metric = {"name": f"sync_caching_{prefix}_cache_misses", "kind": "gauge_avg"}

        def rates(reports: list[dict]) -> list[float]:
            out = []
            for report in reports:
                hits = _report_metric_value(report, hits_metric)
                misses = _report_metric_value(report, miss_metric)
                rate = _ratio(hits, (hits or 0) + (misses or 0))
                if rate is not None:
                    out.append(rate)
            return out

        b_rates = rates(baseline_reports)
        f_rates = rates(feature_reports)
        b_rate = _avg(b_rates)
        f_rate = _avg(f_rates)
        if b_rate is not None or f_rate is not None:
            ci = _metric_ci(b_rates, f_rates)
            cache_rows.insert(0, {
                "key": f"{prefix}_cache_hit_rate",
                "label": label,
                "unit": "percent",
                "baseline": b_rate,
                "feature": f_rate,
                "ci": ci,
                "baseline_fmt": _format_metric_value(b_rate, "percent"),
                "feature_fmt": _format_metric_value(f_rate, "percent"),
                "change": _format_change_with_ci(b_rate, f_rate, ci, b_rates, f_rates, "percent", False),
            })

    return {
        "available": bool(baseline_reports and feature_reports),
        "phases": make_rows(phase_defs),
        "cache_trie": cache_rows,
        "bal": make_rows(bal_defs),
    }


def _paired_data(
    baseline: list[dict], feature: list[dict]
) -> tuple[list[tuple[float, float]], list[float], list[float], list[float], list[float]]:
    """Match blocks and return paired latencies and per-block diffs.

    Returns:
        pairs: list of (baseline_ms, feature_ms) tuples
        lat_diffs_ms: list of feature − baseline latency diffs in ms
        mgas_diffs: list of feature − baseline Mgas/s diffs
        total_lat_diffs_ms: list of feature − baseline total latency diffs in ms
        persist_diffs_ms: list of feature − baseline persistence wait diffs in ms
    """
    baseline_by_block = {r["block_number"]: r for r in baseline}
    feature_by_block = {r["block_number"]: r for r in feature}
    common_blocks = sorted(set(baseline_by_block) & set(feature_by_block))

    pairs = []
    lat_diffs_ms = []
    mgas_diffs = []
    total_lat_diffs_ms = []
    persist_diffs_ms = []
    for bn in common_blocks:
        b = baseline_by_block[bn]
        f = feature_by_block[bn]
        b_ms = b["new_payload_latency_us"] / 1_000
        f_ms = f["new_payload_latency_us"] / 1_000
        pairs.append((b_ms, f_ms))
        lat_diffs_ms.append(f_ms - b_ms)
        b_lat_s = b["new_payload_latency_us"] / 1_000_000
        f_lat_s = f["new_payload_latency_us"] / 1_000_000
        if b_lat_s > 0 and f_lat_s > 0:
            mgas_diffs.append(
                f["gas_used"] / f_lat_s / 1_000_000
                - b["gas_used"] / b_lat_s / 1_000_000
            )
        total_lat_diffs_ms.append(
            f["total_latency_us"] / 1_000 - b["total_latency_us"] / 1_000
        )
        b_persist = (b.get("persistence_wait_us") or 0) / 1_000
        f_persist = (f.get("persistence_wait_us") or 0) / 1_000
        persist_diffs_ms.append(f_persist - b_persist)
    return pairs, lat_diffs_ms, mgas_diffs, total_lat_diffs_ms, persist_diffs_ms


def _bootstrap_ci(rng: random.Random, diffs: list[float], n_iter: int = BOOTSTRAP_ITERATIONS) -> float:
    """Compute 95% bootstrap CI half-width for the mean of *diffs*."""
    if len(diffs) < 2:
        return 0.0
    n = len(diffs)
    boot_means = sorted(
        sum(rng.choices(diffs, k=n)) / n for _ in range(n_iter)
    )
    lo = int(n_iter * 0.025)
    hi = int(n_iter * 0.975)
    return (boot_means[hi] - boot_means[lo]) / 2


def _bootstrap_percentile_ci(
    rng: random.Random,
    pairs: list[tuple[float, float]],
    pct: int,
    n_iter: int = BOOTSTRAP_ITERATIONS,
) -> float:
    """Compute 95% bootstrap CI half-width for a difference-of-percentiles."""
    if len(pairs) < 2:
        return 0.0
    n = len(pairs)
    boot_diffs = []
    for _ in range(n_iter):
        sample = rng.choices(pairs, k=n)
        b_sorted = sorted(p[0] for p in sample)
        f_sorted = sorted(p[1] for p in sample)
        boot_diffs.append(percentile(f_sorted, pct) - percentile(b_sorted, pct))
    boot_diffs.sort()
    lo = int(n_iter * 0.025)
    hi = int(n_iter * 0.975)
    return (boot_diffs[hi] - boot_diffs[lo]) / 2


def _between_pairing_ci(pairing_means: list[float]) -> float:
    """Compute CI half-width from the variance between pairing means.

    This captures run-level bias (thermal drift, background load) that
    per-block pooling cannot detect. Uses T_BETWEEN_PAIRINGS as the
    t-critical value (conservative for df=3).
    """
    n = len(pairing_means)
    if n < 2:
        return 0.0
    mean = sum(pairing_means) / n
    sd = math.sqrt(sum((x - mean) ** 2 for x in pairing_means) / (n - 1))
    se = sd / math.sqrt(n)
    return T_BETWEEN_PAIRINGS * se


def _cross_pair_directions(
    baseline_runs: list[list[dict]],
    feature_runs: list[list[dict]],
) -> dict[str, bool]:
    """Check if the direction of change agrees across all cross-pairings.

    With ABBA runs [B1, B2] and [F1, F2], generates all 4 pairings:
    (B1,F1), (B2,F2), (B1,F2), (B2,F1). For each metric, returns True
    only if the mean diff has the same sign across all pairings.

    With a single run pair, always returns True (no cross-check possible).
    """
    if len(baseline_runs) < 2 or len(feature_runs) < 2:
        return {
            "lat": True, "mgas": True, "total_lat": True, "persist": True,
        }

    cross_pairs = []
    for b in baseline_runs:
        for f in feature_runs:
            cross_pairs.append(_paired_data(b, f))

    def _signs_agree(diffs_per_pair: list[list[float]]) -> bool:
        means = []
        for diffs in diffs_per_pair:
            if diffs:
                means.append(sum(diffs) / len(diffs))
        if len(means) < 2:
            return True
        return all(m > 0 for m in means) or all(m < 0 for m in means) or all(m == 0 for m in means)

    return {
        "lat": _signs_agree([cp[1] for cp in cross_pairs]),
        "mgas": _signs_agree([cp[2] for cp in cross_pairs]),
        "total_lat": _signs_agree([cp[3] for cp in cross_pairs]),
        "persist": _signs_agree([cp[4] for cp in cross_pairs]),
    }


def compute_paired_stats(
    baseline_runs: list[list[dict]],
    feature_runs: list[list[dict]],
) -> dict:
    """Compute paired statistics between baseline and feature runs.

    Uses all cross-pairings (B1-F1, B2-F2, B1-F2, B2-F1) for pooled
    diffs and CIs. The final CI for each metric is the max of:
    - Within-block bootstrap CI (captures per-block variance)
    - Between-pairing CI (captures run-level bias)

    Also checks that the direction of change agrees across all pairings.
    """
    # Generate all cross-pairings for pooled stats
    all_pairs = []
    all_lat_diffs = []
    all_mgas_diffs = []
    all_total_lat_diffs = []
    all_persist_diffs = []
    blocks_per_pair = []
    # Per-pairing means for between-pairing CI
    per_pairing_lat_means = []
    per_pairing_mgas_means = []
    per_pairing_total_lat_means = []
    per_pairing_persist_means = []
    for baseline in baseline_runs:
        for feature in feature_runs:
            pairs, lat_diffs, mgas_diffs, total_lat_diffs, persist_diffs = _paired_data(baseline, feature)
            all_pairs.extend(pairs)
            all_lat_diffs.extend(lat_diffs)
            all_mgas_diffs.extend(mgas_diffs)
            all_total_lat_diffs.extend(total_lat_diffs)
            all_persist_diffs.extend(persist_diffs)
            blocks_per_pair.append(len(pairs))
            if lat_diffs:
                per_pairing_lat_means.append(sum(lat_diffs) / len(lat_diffs))
            if mgas_diffs:
                per_pairing_mgas_means.append(sum(mgas_diffs) / len(mgas_diffs))
            if total_lat_diffs:
                per_pairing_total_lat_means.append(sum(total_lat_diffs) / len(total_lat_diffs))
            if persist_diffs:
                per_pairing_persist_means.append(sum(persist_diffs) / len(persist_diffs))

    if not all_lat_diffs:
        return {}

    n = len(all_lat_diffs)
    mean_diff = sum(all_lat_diffs) / n

    rng = random.Random(42)

    # Within-block bootstrap CI
    ci_within = _bootstrap_ci(rng, all_lat_diffs)
    mgas_ci_within = _bootstrap_ci(rng, all_mgas_diffs) if all_mgas_diffs else 0.0
    wall_ci_within = _bootstrap_ci(rng, all_total_lat_diffs) if all_total_lat_diffs else 0.0
    persist_ci_within = _bootstrap_ci(rng, all_persist_diffs) if all_persist_diffs else 0.0

    # Between-pairing CI (run-level variance floor)
    ci_between = _between_pairing_ci(per_pairing_lat_means)
    mgas_ci_between = _between_pairing_ci(per_pairing_mgas_means)
    wall_ci_between = _between_pairing_ci(per_pairing_total_lat_means)
    persist_ci_between = _between_pairing_ci(per_pairing_persist_means)

    # Final CI = max(within, between)
    ci = max(ci_within, ci_between)
    mgas_ci = max(mgas_ci_within, mgas_ci_between)
    wall_clock_ci_ms = max(wall_ci_within, wall_ci_between)
    persist_ci_ms = max(persist_ci_within, persist_ci_between)

    # Bootstrap CI for percentile diffs
    base_lats = sorted([p[0] for p in all_pairs])
    feature_lats = sorted([p[1] for p in all_pairs])
    p50_diff = percentile(feature_lats, 50) - percentile(base_lats, 50)
    p90_diff = percentile(feature_lats, 90) - percentile(base_lats, 90)
    p99_diff = percentile(feature_lats, 99) - percentile(base_lats, 99)

    p50_ci_within = _bootstrap_percentile_ci(rng, all_pairs, 50)
    p90_ci_within = _bootstrap_percentile_ci(rng, all_pairs, 90)
    p99_ci_within = _bootstrap_percentile_ci(rng, all_pairs, 99)

    # Between-pairing CI for percentile diffs
    per_pairing_p50_diffs = []
    per_pairing_p90_diffs = []
    per_pairing_p99_diffs = []
    for baseline in baseline_runs:
        for feature in feature_runs:
            pairs_i, _, _, _, _ = _paired_data(baseline, feature)
            if pairs_i:
                b_sorted = sorted(p[0] for p in pairs_i)
                f_sorted = sorted(p[1] for p in pairs_i)
                per_pairing_p50_diffs.append(percentile(f_sorted, 50) - percentile(b_sorted, 50))
                per_pairing_p90_diffs.append(percentile(f_sorted, 90) - percentile(b_sorted, 90))
                per_pairing_p99_diffs.append(percentile(f_sorted, 99) - percentile(b_sorted, 99))

    p50_ci = max(p50_ci_within, _between_pairing_ci(per_pairing_p50_diffs))
    p90_ci = max(p90_ci_within, _between_pairing_ci(per_pairing_p90_diffs))
    p99_ci = max(p99_ci_within, _between_pairing_ci(per_pairing_p99_diffs))

    mean_mgas_diff = sum(all_mgas_diffs) / len(all_mgas_diffs) if all_mgas_diffs else 0.0

    # Cross-pair direction agreement
    directions = _cross_pair_directions(baseline_runs, feature_runs)

    return {
        "n": n,
        "mean_diff_ms": mean_diff,
        "ci_ms": ci,
        "p50_diff_ms": p50_diff,
        "p50_ci_ms": p50_ci,
        "p90_diff_ms": p90_diff,
        "p90_ci_ms": p90_ci,
        "p99_diff_ms": p99_diff,
        "p99_ci_ms": p99_ci,
        "mean_mgas_diff": mean_mgas_diff,
        "mgas_ci": mgas_ci,
        "wall_clock_ci_ms": wall_clock_ci_ms,
        "persist_ci_ms": persist_ci_ms,
        "blocks": max(blocks_per_pair),
        "directions_agree": directions,
    }



def format_duration(seconds: float) -> str:
    if seconds >= 60:
        return f"{seconds / 60:.1f}min"
    return f"{seconds}s"


def format_gas(gas: int) -> str:
    if gas >= GIGAGAS:
        return f"{gas / GIGAGAS:.1f}G"
    if gas >= 1_000_000:
        return f"{gas / 1_000_000:.1f}M"
    return f"{gas:,}"



def fmt_ms(v: float) -> str:
    return f"{v:.2f}ms"


def fmt_mgas(v: float) -> str:
    return f"{v:.2f}"


def fmt_s(v: float) -> str:
    return f"{v:.2f}s"


def display_bal_mode(bal_mode: str | None) -> str | None:
    if not bal_mode or bal_mode == "false":
        return None
    if bal_mode == "both":
        return "true"
    return bal_mode


def significance(pct: float, ci_pct: float, lower_is_better: bool, directions_agree: bool = True) -> str:
    """Return significance label: 'good', 'bad', or 'neutral'.

    A result is only significant if:
    1. The CI doesn't cross zero (|pct| > ci_pct), AND
    2. All cross-pairings agree on direction (directions_agree=True).
    """
    significant = abs(pct) > ci_pct and directions_agree
    if not significant:
        return "neutral"
    elif (pct < 0) == lower_is_better:
        return "good"
    else:
        return "bad"


def change_str(pct: float, ci_pct: float, lower_is_better: bool, directions_agree: bool = True) -> str:
    """Format change% with paired CI significance.

    Significant if the CI doesn't cross zero (i.e. |pct| > ci_pct)
    AND all cross-pairings agree on direction.
    """
    sig = significance(pct, ci_pct, lower_is_better, directions_agree)
    emoji = {"good": "✅", "bad": "❌", "neutral": "⚪"}[sig]
    qualifier = "" if directions_agree else " ↕"
    return f"{pct:+.2f}% {emoji}{qualifier} (±{ci_pct:.2f}%)"


def compute_changes(
    baseline_stats: dict, feature_stats: dict, paired_stats: dict
) -> dict:
    """Pre-compute change percentages and significance for each metric."""
    def pct(base: float, feat: float) -> float:
        return (feat - base) / base * 100.0 if base > 0 else 0.0

    def ci_pct(ci_ms: float, base_ms: float) -> float:
        return ci_ms / base_ms * 100.0 if base_ms > 0 else 0.0

    dirs = paired_stats.get("directions_agree", {})

    metrics = [
        ("mean", "mean_ms", "ci_ms", "mean_ms", True, dirs.get("lat", True)),
        ("p50", "p50_ms", "p50_ci_ms", "p50_ms", True, dirs.get("lat", True)),
        ("p90", "p90_ms", "p90_ci_ms", "p90_ms", True, dirs.get("lat", True)),
        ("p99", "p99_ms", "p99_ci_ms", "p99_ms", True, dirs.get("lat", True)),
        ("mgas_s", "mean_mgas_s", "mgas_ci", "mean_mgas_s", False, dirs.get("mgas", True)),
        ("wall_clock", "wall_clock_s", "wall_clock_ci_ms", "mean_total_lat_ms", True, dirs.get("total_lat", True)),
        ("persist_wait", "mean_persist_ms", "persist_ci_ms", "mean_persist_ms", True, dirs.get("persist", True)),
    ]
    changes = {}
    for name, stat_key, ci_key, base_key, lower_is_better, dir_agree in metrics:
        p = pct(baseline_stats[stat_key], feature_stats[stat_key])
        c = ci_pct(paired_stats[ci_key], baseline_stats[base_key])
        changes[name] = {
            "pct": round(p, 4),
            "ci_pct": round(c, 4),
            "sig": significance(p, c, lower_is_better, dir_agree),
            "directions_agree": dir_agree,
        }
    return changes


def generate_comparison_table(
    run1: dict,
    run2: dict,
    paired: dict,
    repo: str,
    baseline_ref: str,
    baseline_name: str,
    feature_name: str,
    feature_sha: str,
    big_blocks: bool = False,
    warmup_blocks: str | None = None,
    wait_time: str | None = None,
    bal_mode: str | None = None,
    prometheus: dict | None = None,
) -> str:
    """Generate a markdown comparison table between baseline and feature."""
    n = paired["blocks"]

    def pct(base: float, feat: float) -> float:
        return (feat - base) / base * 100.0 if base > 0 else 0.0

    gas_pct = pct(run1["mean_mgas_s"], run2["mean_mgas_s"])
    wall_pct = pct(run1["wall_clock_s"], run2["wall_clock_s"])

    p50_pct = pct(run1["p50_ms"], run2["p50_ms"])
    p90_pct = pct(run1["p90_ms"], run2["p90_ms"])
    p99_pct = pct(run1["p99_ms"], run2["p99_ms"])

    persist_pct = pct(run1["mean_persist_ms"], run2["mean_persist_ms"])

    # Bootstrap CIs as % of baseline percentile
    p50_ci_pct = paired["p50_ci_ms"] / run1["p50_ms"] * 100.0 if run1["p50_ms"] > 0 else 0.0
    p90_ci_pct = paired["p90_ci_ms"] / run1["p90_ms"] * 100.0 if run1["p90_ms"] > 0 else 0.0
    p99_ci_pct = paired["p99_ci_ms"] / run1["p99_ms"] * 100.0 if run1["p99_ms"] > 0 else 0.0

    # CI as a percentage of baseline
    mgas_ci_pct = paired["mgas_ci"] / run1["mean_mgas_s"] * 100.0 if run1["mean_mgas_s"] > 0 else 0.0
    wall_ci_pct = paired["wall_clock_ci_ms"] / run1["mean_total_lat_ms"] * 100.0 if run1["mean_total_lat_ms"] > 0 else 0.0
    persist_ci_pct = paired["persist_ci_ms"] / run1["mean_persist_ms"] * 100.0 if run1["mean_persist_ms"] > 0 else 0.0

    dirs = paired.get("directions_agree", {})
    lat_agree = dirs.get("lat", True)
    mgas_agree = dirs.get("mgas", True)
    total_agree = dirs.get("total_lat", True)
    persist_agree = dirs.get("persist", True)

    base_url = f"https://github.com/{repo}/commit"
    baseline_label = f"[`{baseline_name}`]({base_url}/{baseline_ref})"
    feature_label = f"[`{feature_name}`]({base_url}/{feature_sha})"

    lines = [
        f"| Headline metric | {baseline_label} | {feature_label} | Change |",
        "|--------|------|--------|--------|",
        f"| Mean newPayload | {fmt_ms(run1['mean_ms'])} | {fmt_ms(run2['mean_ms'])} | {change_str(pct(run1['mean_ms'], run2['mean_ms']), paired['ci_ms'] / run1['mean_ms'] * 100.0 if run1['mean_ms'] > 0 else 0.0, lower_is_better=True, directions_agree=lat_agree)} |",
        f"| P50 newPayload | {fmt_ms(run1['p50_ms'])} | {fmt_ms(run2['p50_ms'])} | {change_str(p50_pct, p50_ci_pct, lower_is_better=True, directions_agree=lat_agree)} |",
        f"| P90 newPayload | {fmt_ms(run1['p90_ms'])} | {fmt_ms(run2['p90_ms'])} | {change_str(p90_pct, p90_ci_pct, lower_is_better=True, directions_agree=lat_agree)} |",
        f"| P99 newPayload | {fmt_ms(run1['p99_ms'])} | {fmt_ms(run2['p99_ms'])} | {change_str(p99_pct, p99_ci_pct, lower_is_better=True, directions_agree=lat_agree)} |",
        f"| Throughput | {fmt_mgas(run1['mean_mgas_s'])} Mgas/s | {fmt_mgas(run2['mean_mgas_s'])} Mgas/s | {change_str(gas_pct, mgas_ci_pct, lower_is_better=False, directions_agree=mgas_agree)} |",
        f"| Wall clock | {fmt_s(run1['wall_clock_s'])} | {fmt_s(run2['wall_clock_s'])} | {change_str(wall_pct, wall_ci_pct, lower_is_better=True, directions_agree=total_agree)} |",
    ]
    prom_by_key = {r["key"]: r for r in (prometheus or {}).get("phases", [])}
    for key, label in (("evm_execution", "EVM execution p95"), ("state_root", "State root p95"), ("bal_validation", "BAL validation p95")):
        row = prom_by_key.get(key)
        if row and (key != "bal_validation" or display_bal_mode(bal_mode)):
            lines.append(f"| {label} | {row['baseline_fmt']} | {row['feature_fmt']} | {row['change']} |")
    lines.extend([
        "",
        "<!-- BENCH_CHART:latency_throughput.png:Latency, throughput & diff:open -->",
        "",
        "> Noise: `±` is the 95% CI half-width. ⚪ means the change is within noise or ABBA cross-pairings disagree. Detailed rows show absolute deltas first so tiny baselines do not produce misleading percentages.",
        "",
    ])
    meta_parts = [f"{n} {'big blocks' if big_blocks else 'blocks'}"]
    if warmup_blocks:
        meta_parts.append(f"{warmup_blocks} warmup")
    if wait_time:
        meta_parts.append(f"wait time: {wait_time}")
    display_mode = display_bal_mode(bal_mode)
    if big_blocks and display_mode:
        meta_parts.append(f"BAL: {display_mode}")
    lines.append(f"*{', '.join(meta_parts)}*")
    return "\n".join(lines)


def generate_wait_time_table(
    title: str,
    baseline_stats: dict,
    feature_stats: dict,
    baseline_label: str,
    feature_label: str,
    mean_change: str = "",
) -> str:
    """Generate a markdown table for a wait time metric."""
    if not baseline_stats or not feature_stats:
        return ""
    lines = [
        f"### {title}",
        "",
        f"| Metric | {baseline_label} | {feature_label} | Change |",
        "|--------|------|--------|--------|",
        f"| Mean | {fmt_ms(baseline_stats['mean_ms'])} | {fmt_ms(feature_stats['mean_ms'])} | {mean_change} |",
        f"| P50 | {fmt_ms(baseline_stats['p50_ms'])} | {fmt_ms(feature_stats['p50_ms'])} |  |",
        f"| P95 | {fmt_ms(baseline_stats['p95_ms'])} | {fmt_ms(feature_stats['p95_ms'])} |  |",
    ]
    return "\n".join(lines)


def _markdown_rows(title: str, rows: list[dict], chart: str | None = None) -> list[str]:
    if not rows:
        return []
    lines = ["", "<details>", f"<summary>{title}</summary>", ""]
    if rows:
        lines.extend([
            "| Metric | Baseline | Feature | Change |",
            "|---|---:|---:|---:|",
        ])
        for row in rows:
            lines.append(f"| {row['label']} | {row['baseline_fmt']} | {row['feature_fmt']} | {row['change']} |")
        lines.append("")
    if chart:
        lines.append(chart)
        lines.append("")
    lines.append("</details>")
    return lines


def _workload_section(summary: dict) -> list[str]:
    workload = summary.get("workload") or {}
    lines = [
        "",
        "<details>",
        "<summary>Workload and gas</summary>",
        "",
        "| Workload metric | Value |",
        "|---|---:|",
        f"| Measured payloads | {summary['blocks']} |",
    ]
    warmup = summary.get("warmup_blocks")
    if warmup:
        lines.append(f"| Warmup payloads | {warmup} |")
    lines.extend([
        f"| Total gas | {format_gas(int(workload.get('total_gas', 0))) if workload.get('total_gas') else '—'} |",
        f"| Total txs | {int(workload.get('total_txs', 0)):,} |" if workload.get("total_txs") else "| Total txs | — |",
    ])
    if summary.get("big_blocks"):
        lines.append(f"| Target gas / big block | {summary.get('big_block_target_gas') or '1G'} |")
    lines.append("")
    lines.append("<!-- BENCH_CHART:gas_vs_latency.png:Gas vs latency -->")
    lines.append("")
    lines.append("</details>")
    return lines


def generate_markdown(
    summary: dict, comparison_table: str,
    wait_time_tables: list[str] | None = None,
    behind_baseline: int = 0, repo: str = "", baseline_ref: str = "", baseline_name: str = "",
    grafana_url: str | None = None,
) -> str:
    """Generate a markdown comment body."""
    lines = ["## Benchmark Results", ""]
    if behind_baseline > 0:
        s = "s" if behind_baseline > 1 else ""
        diff_link = f"https://github.com/{repo}/compare/{baseline_ref[:12]}...{baseline_name}"
        lines.append(f"> ⚠️ Feature is [**{behind_baseline} commit{s} behind `{baseline_name}`**]({diff_link}). Consider rebasing for accurate results.")
        lines.append("")
    lines.append(comparison_table)

    prom = summary.get("prometheus") or {}
    if summary.get("bal_mode") and prom.get("bal"):
        lines.extend(_markdown_rows("BAL metrics", prom["bal"], "<!-- BENCH_CHART:bal.png:BAL metrics -->"))
    lines.extend(_workload_section(summary))
    phase_rows = [r for r in prom.get("phases", []) if summary.get("bal_mode") or r.get("key") != "bal_validation"]
    lines.extend(_markdown_rows("Execution phase breakdown", phase_rows, "<!-- BENCH_CHART:prometheus_phases.png:Prometheus execution phases -->"))
    lines.extend(_markdown_rows("Cache and trie metrics", prom.get("cache_trie", []), "<!-- BENCH_CHART:cache_trie.png:Cache and trie -->"))

    if wait_time_tables:
        lines.append("")
        lines.append("<details>")
        lines.append("<summary>Wait Time Breakdown</summary>")
        lines.append("")
        for table in wait_time_tables:
            if table:
                lines.append(table)
                lines.append("")
        lines.append("<!-- BENCH_CHART:wait_breakdown.png:Wait time breakdown -->")
        lines.append("")
        lines.append("</details>")
    if grafana_url:
        lines.append("")
        lines.append(f"**[Grafana Dashboard]({grafana_url})**")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Parse benchmark ABBA results")
    parser.add_argument(
        "--baseline-csv", nargs="+", required=True,
        help="Baseline combined_latency.csv files (A1, A2)",
    )
    parser.add_argument(
        "--feature-csv", "--branch-csv", nargs="+", required=True,
        help="Feature combined_latency.csv files (B1, B2)",
    )
    parser.add_argument("--gas-csv", required=True, help="Path to total_gas.csv")
    parser.add_argument(
        "--baseline-report", nargs="+", help="Baseline txgen JSON report files"
    )
    parser.add_argument(
        "--feature-report", nargs="+", help="Feature txgen JSON report files"
    )
    parser.add_argument(
        "--output-summary", required=True, help="Output JSON summary path"
    )
    parser.add_argument("--output-markdown", required=True, help="Output markdown path")
    parser.add_argument(
        "--repo", default="paradigmxyz/reth", help="GitHub repo (owner/name)"
    )
    parser.add_argument("--baseline-ref", default=None, help="Baseline commit SHA")
    parser.add_argument("--baseline-name", default=None, help="Baseline display name")
    parser.add_argument("--feature-name", "--branch-name", default=None, help="Feature branch name")
    parser.add_argument("--feature-ref", "--branch-sha", "--feature-sha", default=None, help="Feature commit SHA")
    parser.add_argument("--behind-baseline", "--behind-main", type=int, default=0, help="Commits behind baseline")
    parser.add_argument("--big-blocks", action="store_true", default=False, help="Big blocks mode")
    parser.add_argument("--warmup-blocks", default=None, help="Number of warmup blocks")
    parser.add_argument("--wait-time", default=None, help="Wait time interval used between blocks")
    parser.add_argument("--bal-mode", default=None, help="BAL mode (true, feature, baseline)")
    parser.add_argument("--grafana-url", default=None, help="Grafana dashboard URL for this benchmark run")
    args = parser.parse_args()

    if len(args.baseline_csv) != len(args.feature_csv):
        print("Must provide equal number of baseline and feature CSVs", file=sys.stderr)
        sys.exit(1)

    baseline_runs = []
    feature_runs = []
    for path in args.baseline_csv:
        data = parse_combined_csv(path)
        if not data:
            print(f"No results in {path}", file=sys.stderr)
            sys.exit(1)
        baseline_runs.append(data)
    for path in args.feature_csv:
        data = parse_combined_csv(path)
        if not data:
            print(f"No results in {path}", file=sys.stderr)
            sys.exit(1)
        feature_runs.append(data)

    gas = parse_gas_csv(args.gas_csv)

    all_baseline = [r for run in baseline_runs for r in run]
    all_feature = [r for run in feature_runs for r in run]

    baseline_stats = compute_stats(all_baseline)
    feature_stats = compute_stats(all_feature)
    paired_stats = compute_paired_stats(baseline_runs, feature_runs)

    if not paired_stats:
        print("No common blocks between baseline and feature runs", file=sys.stderr)
        sys.exit(1)

    baseline_ref = args.baseline_ref or "main"
    baseline_name = args.baseline_name or "baseline"
    feature_name = args.feature_name or "feature"
    feature_sha = args.feature_ref or "unknown"
    bal_mode = display_bal_mode(args.bal_mode)
    prometheus = summarize_prometheus_metrics(
        load_reports(args.baseline_report),
        load_reports(args.feature_report),
    )

    comparison_table = generate_comparison_table(
        baseline_stats,
        feature_stats,
        paired_stats,
        repo=args.repo,
        baseline_ref=baseline_ref,
        baseline_name=baseline_name,
        feature_name=feature_name,
        feature_sha=feature_sha,
        big_blocks=args.big_blocks,
        warmup_blocks=args.warmup_blocks,
        wait_time=args.wait_time,
        bal_mode=bal_mode,
        prometheus=prometheus,
    )
    print(f"Generated comparison ({paired_stats['n']} paired blocks, "
          f"mean diff {paired_stats['mean_diff_ms']:+.3f}ms ± {paired_stats['ci_ms']:.3f}ms)")

    base_url = f"https://github.com/{args.repo}/commit"
    baseline_label = f"[`{baseline_name}`]({base_url}/{baseline_ref})"
    feature_label = f"[`{feature_name}`]({base_url}/{feature_sha})"

    wait_fields = [
        ("persistence_wait_us", "Persistence Wait"),
        ("sparse_trie_wait_us", "Trie Cache Update Wait"),
        ("execution_cache_wait_us", "Execution Cache Update Wait"),
    ]
    changes = compute_changes(baseline_stats, feature_stats, paired_stats)
    wait_time_tables = []
    wait_time_data = {}
    for field, title in wait_fields:
        b_stats = compute_wait_stats(all_baseline, field)
        f_stats = compute_wait_stats(all_feature, field)
        mean_change = ""
        if b_stats and f_stats:
            b_values = [s["mean_ms"] for run in baseline_runs if (s := compute_wait_stats(run, field))]
            f_values = [s["mean_ms"] for run in feature_runs if (s := compute_wait_stats(run, field))]
            mean_change = _format_change_with_ci(
                b_stats["mean_ms"],
                f_stats["mean_ms"],
                _metric_ci(b_values, f_values),
                b_values,
                f_values,
                "ms",
                True,
            )
        if b_stats and f_stats:
            wait_time_data[field] = {
                "title": title,
                "baseline": b_stats,
                "feature": f_stats,
                "change": mean_change,
            }
        table = generate_wait_time_table(title, b_stats, f_stats, baseline_label, feature_label, mean_change)
        if table:
            wait_time_tables.append(table)

    first_feature_run = feature_runs[0]
    summary = {
        "blocks": paired_stats["blocks"],
        "workload": {
            "total_gas": sum(r["gas_used"] for r in first_feature_run),
            "total_txs": sum(r["transaction_count"] for r in first_feature_run),
        },
        "big_blocks": args.big_blocks,
        "warmup_blocks": args.warmup_blocks,
        "wait_time": args.wait_time,
        "bal_mode": bal_mode,
        "baseline": {
            "name": baseline_name,
            "ref": baseline_ref,
            "stats": baseline_stats,
        },
        "feature": {
            "name": feature_name,
            "ref": feature_sha,
            "stats": feature_stats,
        },
        "paired": paired_stats,
        "changes": changes,
        "wait_times": wait_time_data,
        "prometheus": prometheus,
        "big_block_target_gas": "1G" if args.big_blocks else None,
    }
    with open(args.output_summary, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"Summary written to {args.output_summary}")

    markdown = generate_markdown(
        summary, comparison_table,
        wait_time_tables=wait_time_tables,
        behind_baseline=args.behind_baseline,
        repo=args.repo,
        baseline_ref=baseline_ref,
        baseline_name=baseline_name,
        grafana_url=args.grafana_url,
    )

    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")


if __name__ == "__main__":
    main()
