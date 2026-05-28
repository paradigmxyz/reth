#!/usr/bin/env python3
"""Parse benchmark CSV output and generate a summary JSON + markdown comparison.

Usage:
    bench-reth-summary.py \
        --output-summary <summary.json> \
        --output-markdown <comment.md> \
        --baseline-csv <baseline_combined.csv> [<baseline_combined.csv> ...] \
        [--repo <owner/repo>] \
        [--baseline-ref <sha>] \
        [--feature-name <name>] \
        [--feature-sha <sha>]

Generates a statistical comparison between baseline and feature. Point estimates
use pooled baseline and feature rows. Confidence intervals use whole-run cluster
bootstrapping when multiple runs are available. Fails if baseline or feature CSV
is missing or empty.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
from pathlib import Path
import random
import re
import sys

BOOTSTRAP_ITERATIONS = 10_000
EPSILON = 1e-9
P99_MIN_VERDICT_BLOCKS = 125
PERSIST_WAIT_MIN_VERDICT_MS = 0.5
PERSIST_WAIT_MIN_TOTAL_PCT = 0.1
TARGET_METRIC_BLOCK_HEIGHT_QUERY = "reth_blockchain_tree_canonical_chain_height"
TARGET_METRIC_COUNTER_STATS = ("mean",)
TARGET_METRIC_MIN_PAIRED_OBSERVATIONS = 30
SELECTOR_RE = re.compile(
    r"^(?P<name>[a-zA-Z_:][a-zA-Z0-9_:]*)(?:\{(?P<labels>[^}]*)\})?$"
)
PRACTICAL_FLOOR_PCT = {
    "mean": 1.20,
    "p50": 1.20,
    "p90": 1.35,
    "p99": 5.0,
    "mgas_s": 1.20,
    "wall_clock": 0.70,
    "persist_wait": 5.0,
}


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
                    "new_payload_latency_us": int(row["new_payload_latency"]),
                    "total_latency_us": int(row["total_latency"]),
                    "persistence_wait_us": _opt_int(row, "persistence_wait"),
                    "execution_cache_wait_us": _opt_int(row, "execution_cache_wait"),
                    "sparse_trie_wait_us": _opt_int(row, "sparse_trie_wait"),
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


def parse_label_string(text: str | None) -> dict[str, str]:
    if not text:
        return {}

    labels = {}
    parts = []
    current = []
    in_quotes = False
    escaped = False
    for ch in text:
        if escaped:
            current.append(ch)
            escaped = False
            continue
        if ch == "\\":
            current.append(ch)
            escaped = True
            continue
        if ch == '"':
            current.append(ch)
            in_quotes = not in_quotes
            continue
        if ch == "," and not in_quotes:
            parts.append("".join(current).strip())
            current = []
            continue
        current.append(ch)
    if current:
        parts.append("".join(current).strip())

    for part in parts:
        if not part:
            continue
        key, value = part.split("=", 1)
        labels[key.strip()] = bytes(value.strip()[1:-1], "utf-8").decode("unicode_escape")
    return labels


def parse_target_metric_query(query: str) -> tuple[str, str, dict[str, str]]:
    query = query.strip()
    aggregate = "single"
    inner = query
    if query.startswith("sum(") and query.endswith(")"):
        aggregate = "sum"
        inner = query[4:-1].strip()

    match = SELECTOR_RE.match(inner)
    if not match:
        raise ValueError(f"Unsupported target metric query: {query}")
    return aggregate, match.group("name"), parse_label_string(match.group("labels"))


def format_label_value(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"')


def format_target_metric_query(metric_name: str, labels: dict[str, str]) -> str:
    if not labels:
        return metric_name
    encoded_labels = ",".join(
        f'{key}="{format_label_value(value)}"' for key, value in sorted(labels.items())
    )
    return f"{metric_name}{{{encoded_labels}}}"


def histogram_counter_query(query: str, suffix: str) -> str:
    aggregate, metric_name, label_filters = parse_target_metric_query(query)
    if aggregate != "single":
        raise ValueError(f"Histogram target metric queries must not use sum(...): {query}")
    return format_target_metric_query(f"{metric_name}_{suffix}", label_filters)


def target_metric_query_encodes_seconds(query: str) -> bool:
    _aggregate, metric_name, _label_filters = parse_target_metric_query(query)
    return "_seconds" in metric_name


def target_metric_counter_encodes_seconds(query: str) -> bool:
    return target_metric_query_encodes_seconds(query)


def target_metric_min_value_pct_total_latency(config: dict) -> float | None:
    value = config.get("min_value_pct_total_latency")
    if value is None:
        return None
    value = float(value)
    if value < 0.0:
        raise ValueError("min_value_pct_total_latency must be non-negative")
    return value


def query_matches_sample(sample: dict, metric_name: str, label_filters: dict[str, str]) -> bool:
    return sample["name"] == metric_name and all(
        sample["labels"].get(key) == value for key, value in label_filters.items()
    )


def query_samples(samples: list[dict], query: str) -> tuple[str, list[dict]]:
    aggregate, metric_name, label_filters = parse_target_metric_query(query)
    matches = [
        sample
        for sample in samples
        if query_matches_sample(sample, metric_name, label_filters)
    ]
    return aggregate, matches


def evaluate_query(samples: list[dict], query: str, allow_missing: bool = False) -> float:
    aggregate, matched_samples = query_samples(samples, query)
    matches = [sample["value"] for sample in matched_samples]

    if not matches:
        if allow_missing:
            return 0.0
        raise ValueError(f"Query matched no samples: {query}")

    if aggregate == "sum":
        return float(sum(matches))
    if len(matches) > 1:
        raise ValueError(
            f"Query matched {len(matches)} samples; use sum(...) or label filters: {query}"
        )
    return float(matches[0])


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


def compute_point_stats(runs: list[list[dict]]) -> dict:
    """Compute displayed point estimates for one side of a comparison.

    Cluster CIs estimate percentile changes from one percentile value per run.
    Match that estimator for displayed percentile point estimates instead of
    pooling repeated block rows into one percentile sample.
    """
    combined = [row for run in runs for row in run]
    stats = compute_stats(combined)
    if len(runs) < 2:
        return stats

    per_run_stats = [compute_stats(run) for run in runs if run]
    for key in ("p50_ms", "p90_ms", "p99_ms"):
        stats[key] = _mean([run_stats[key] for run_stats in per_run_stats])
    return stats


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


def _ci_half_width(samples: list[float]) -> float:
    """Return the 95% CI half-width from sorted bootstrap samples."""
    if len(samples) < 2:
        return 0.0
    samples.sort()
    n = len(samples)
    lo = int(n * 0.025)
    hi = int(n * 0.975)
    return (samples[hi] - samples[lo]) / 2


def _mean(values: list[float]) -> float:
    return sum(values) / len(values) if values else 0.0


def _per_run_metric_values(runs: list[list[dict]]) -> dict[str, list[float]]:
    """Compute one metric value per benchmark run for cluster bootstrap."""
    values = {
        "mean_ms": [],
        "p50_ms": [],
        "p90_ms": [],
        "p99_ms": [],
        "mgas": [],
        "wall_clock_ms": [],
        "persist_ms": [],
    }
    for run in runs:
        stats = compute_stats(run)
        values["mean_ms"].append(stats["mean_ms"])
        values["p50_ms"].append(stats["p50_ms"])
        values["p90_ms"].append(stats["p90_ms"])
        values["p99_ms"].append(stats["p99_ms"])
        values["mgas"].append(stats["mean_mgas_s"])
        values["wall_clock_ms"].append(stats["mean_total_lat_ms"])
        values["persist_ms"].append(stats["mean_persist_ms"])
    return values


def _cluster_bootstrap_ci(
    rng: random.Random,
    baseline_runs: list[list[dict]],
    feature_runs: list[list[dict]],
    n_iter: int = BOOTSTRAP_ITERATIONS,
) -> dict[str, float]:
    """Compute run-cluster bootstrap CIs.

    Each bootstrap sample resamples whole baseline and feature runs with
    replacement. This estimates run-to-run noise without expanding reused
    baseline/feature runs into independent block-level datapoints.
    """
    metrics = ("mean_ms", "p50_ms", "p90_ms", "p99_ms", "mgas", "wall_clock_ms", "persist_ms")
    empty = {metric: 0.0 for metric in metrics}
    if len(baseline_runs) < 2 or len(feature_runs) < 2:
        return empty

    baseline_values = _per_run_metric_values(baseline_runs)
    feature_values = _per_run_metric_values(feature_runs)
    samples = {metric: [] for metric in metrics}
    baseline_count = len(baseline_runs)
    feature_count = len(feature_runs)

    for _ in range(n_iter):
        baseline_indexes = [rng.randrange(baseline_count) for _ in range(baseline_count)]
        feature_indexes = [rng.randrange(feature_count) for _ in range(feature_count)]
        for metric in metrics:
            baseline_sample = [baseline_values[metric][i] for i in baseline_indexes]
            feature_sample = [feature_values[metric][i] for i in feature_indexes]
            samples[metric].append(_mean(feature_sample) - _mean(baseline_sample))

    return {metric: _ci_half_width(samples[metric]) for metric in metrics}


def compute_ci_stats(
    baseline_runs: list[list[dict]],
    feature_runs: list[list[dict]],
) -> dict:
    """Compute confidence interval half-widths for displayed changes.

    Multiple-run comparisons resample whole runs so reused blocks are not
    treated as independent observations. Single-run comparisons fall back to
    block-level bootstrap over matched block numbers.
    """
    if not baseline_runs or not feature_runs:
        return {}

    blocks = max(len(run) for run in baseline_runs + feature_runs)
    rng = random.Random(42)

    if len(baseline_runs) >= 2 and len(feature_runs) >= 2:
        cluster_ci = _cluster_bootstrap_ci(rng, baseline_runs, feature_runs)
        ci = cluster_ci["mean_ms"]
        p50_ci = cluster_ci["p50_ms"]
        p90_ci = cluster_ci["p90_ms"]
        p99_ci = cluster_ci["p99_ms"]
        mgas_ci = cluster_ci["mgas"]
        wall_clock_ci_ms = cluster_ci["wall_clock_ms"]
        persist_ci_ms = cluster_ci["persist_ms"]
    else:
        pairs, all_lat_diffs, all_mgas_diffs, all_total_lat_diffs, all_persist_diffs = (
            _paired_data(baseline_runs[0], feature_runs[0])
        )
        if not all_lat_diffs:
            return {}
        ci = _bootstrap_ci(rng, all_lat_diffs)
        p50_ci = _bootstrap_percentile_ci(rng, pairs, 50)
        p90_ci = _bootstrap_percentile_ci(rng, pairs, 90)
        p99_ci = _bootstrap_percentile_ci(rng, pairs, 99)
        mgas_ci = _bootstrap_ci(rng, all_mgas_diffs) if all_mgas_diffs else 0.0
        wall_clock_ci_ms = _bootstrap_ci(rng, all_total_lat_diffs) if all_total_lat_diffs else 0.0
        persist_ci_ms = _bootstrap_ci(rng, all_persist_diffs) if all_persist_diffs else 0.0

    return {
        "ci_ms": ci,
        "p50_ci_ms": p50_ci,
        "p90_ci_ms": p90_ci,
        "p99_ci_ms": p99_ci,
        "mgas_ci": mgas_ci,
        "wall_clock_ci_ms": wall_clock_ci_ms,
        "persist_ci_ms": persist_ci_ms,
        "blocks": blocks,
    }


def fmt_ms(v: float) -> str:
    return f"{v:.2f}ms"


def fmt_mgas(v: float) -> str:
    return f"{v:.2f}"


def fmt_s(v: float) -> str:
    return f"{v:.2f}s"


def fmt_metric_value(v: float) -> str:
    abs_v = abs(v)
    if abs_v == 0:
        return "0"
    if abs_v < 0.001:
        return f"{v:.4g}"
    if abs_v >= 1 and abs(v - round(v)) <= 0.00005:
        return f"{round(v):.0f}"
    return f"{v:.4f}".rstrip("0").rstrip(".")


def fmt_target_metric_value(metric: dict, v: float) -> str:
    if metric.get("unit") == "seconds":
        return fmt_ms(v * 1_000.0)
    return fmt_metric_value(v)


def display_bal_mode(bal_mode: str | None) -> str | None:
    if not bal_mode or bal_mode == "false":
        return None
    if bal_mode == "both":
        return "true"
    return bal_mode


def practical_floor_pct(metric: str, _baseline_value: float) -> float:
    """Return the practical significance floor as a percent of baseline."""
    return PRACTICAL_FLOOR_PCT.get(metric, 0.0)


def significance(pct: float, ci_pct: float, floor_pct: float, lower_is_better: bool) -> str:
    """Return significance label: 'good', 'bad', or 'neutral'.

    A result is only significant if the whole confidence interval clears a
    practical significance floor. The floor is the same for every run shape;
    higher run counts only tighten the CI.
    """
    improvement_pct = -pct if lower_is_better else pct
    if improvement_pct - ci_pct > floor_pct:
        return "good"
    if improvement_pct + ci_pct < -floor_pct:
        return "bad"
    return "neutral"


def persist_wait_is_material(stats: dict) -> bool:
    threshold = max(
        PERSIST_WAIT_MIN_VERDICT_MS,
        stats["mean_total_lat_ms"] * PERSIST_WAIT_MIN_TOTAL_PCT / 100.0,
    )
    return stats["mean_persist_ms"] >= threshold


def informational_reason(
    metric: str,
    ci_stats: dict,
    baseline_stats: dict,
    feature_stats: dict,
) -> str | None:
    if metric == "p99" and ci_stats["blocks"] < P99_MIN_VERDICT_BLOCKS:
        return f"informational <{P99_MIN_VERDICT_BLOCKS} blocks"
    if (
        metric == "persist_wait" and
        not persist_wait_is_material(baseline_stats) and
        not persist_wait_is_material(feature_stats)
    ):
        return (
            f"informational <max({PERSIST_WAIT_MIN_VERDICT_MS:.1f}ms, "
            f"{PERSIST_WAIT_MIN_TOTAL_PCT:.1f}% total latency)"
        )
    return None


def change_str(
    pct: float,
    ci_pct: float,
    floor_pct: float,
    lower_is_better: bool,
    informational: str | None = None,
) -> str:
    """Format change% with CI significance.

    Significant if the confidence interval clears the practical floor.
    """
    sig = "neutral" if informational else significance(pct, ci_pct, floor_pct, lower_is_better)
    emoji = {"good": "✅", "bad": "❌", "neutral": "⚪"}[sig]
    details = [f"±{ci_pct:.2f}%", f"floor {floor_pct:.2f}%"]
    if informational:
        details.append("informational")
    return f"{pct:+.2f}% {emoji} ({', '.join(details)})"


def compute_changes(
    baseline_stats: dict, feature_stats: dict, ci_stats: dict
) -> dict:
    """Pre-compute change percentages and significance for each metric."""
    def pct(base: float, feat: float) -> float:
        return (feat - base) / base * 100.0 if base > 0 else 0.0

    def ci_pct(ci_ms: float, base_ms: float) -> float:
        return ci_ms / base_ms * 100.0 if base_ms > 0 else 0.0

    metrics = [
        ("mean", "mean_ms", "ci_ms", "mean_ms", True),
        ("p50", "p50_ms", "p50_ci_ms", "p50_ms", True),
        ("p90", "p90_ms", "p90_ci_ms", "p90_ms", True),
        ("p99", "p99_ms", "p99_ci_ms", "p99_ms", True),
        ("mgas_s", "mean_mgas_s", "mgas_ci", "mean_mgas_s", False),
        ("wall_clock", "wall_clock_s", "wall_clock_ci_ms", "mean_total_lat_ms", True),
        ("persist_wait", "mean_persist_ms", "persist_ci_ms", "mean_persist_ms", True),
    ]
    changes = {}
    for name, stat_key, ci_key, base_key, lower_is_better in metrics:
        p = pct(baseline_stats[stat_key], feature_stats[stat_key])
        c = ci_pct(ci_stats[ci_key], baseline_stats[base_key])
        floor = practical_floor_pct(name, baseline_stats[base_key])
        informational = informational_reason(name, ci_stats, baseline_stats, feature_stats)
        sig = significance(p, c, floor, lower_is_better)
        changes[name] = {
            "pct": round(p, 4),
            "ci_pct": round(c, 4),
            "floor_pct": round(floor, 4),
            "sig": "neutral" if informational else sig,
        }
        if informational:
            changes[name]["informational"] = True
            changes[name]["informational_reason"] = informational
            changes[name]["raw_sig"] = sig
    return changes


def target_metric_identity_key(labels: dict[str, str]) -> tuple[tuple[str, str], ...]:
    return tuple(sorted(labels.items()))


def target_metric_candidate_identity_labels(
    query: str,
    sample_labels: dict[str, str],
) -> dict[str, str]:
    _aggregate, _metric_name, label_filters = parse_target_metric_query(query)
    return {
        key: value
        for key, value in sorted(sample_labels.items())
        if key not in label_filters
    }


def target_metric_variable_identity_label_keys(
    scrapes: list[dict],
    query: str,
) -> set[str]:
    aggregate, _metric_name, _label_filters = parse_target_metric_query(query)
    if aggregate == "sum":
        return set()

    matched_labels = []
    candidate_keys = set()
    for scrape in scrapes:
        _aggregate, matched_samples = query_samples(scrape["samples"], query)
        for sample in matched_samples:
            labels = target_metric_candidate_identity_labels(query, sample["labels"])
            matched_labels.append(labels)
            candidate_keys.update(labels)

    return {
        key
        for key in candidate_keys
        if len({labels.get(key) for labels in matched_labels}) > 1
    }


def target_metric_identity_labels(
    query: str,
    sample_labels: dict[str, str],
    identity_label_keys: set[str],
) -> dict[str, str]:
    candidate_labels = target_metric_candidate_identity_labels(query, sample_labels)
    return {
        key: value
        for key, value in candidate_labels.items()
        if key in identity_label_keys
    }


def target_metric_display_query(query: str, identity_labels: dict[str, str] | None = None) -> str:
    _, metric_name, label_filters = parse_target_metric_query(query)
    display_labels = dict(label_filters)
    if identity_labels:
        display_labels.update(identity_labels)
    return re.sub(r"\s+", "", format_target_metric_query(metric_name, display_labels))


def group_query_samples_by_identity(
    samples: list[dict],
    query: str,
    identity_label_keys: set[str],
) -> tuple[str, dict]:
    aggregate, matched_samples = query_samples(samples, query)
    groups = {}
    if aggregate == "sum":
        if matched_samples:
            groups[()] = {"identity_labels": {}, "samples": matched_samples}
        return aggregate, groups

    for sample in matched_samples:
        identity_labels = target_metric_identity_labels(
            query,
            sample["labels"],
            identity_label_keys,
        )
        key = target_metric_identity_key(identity_labels)
        group = groups.setdefault(key, {"identity_labels": identity_labels, "samples": []})
        group["samples"].append(sample)
    return aggregate, groups


def grouped_sample_value(group: dict | None, aggregate: str, allow_missing: bool = False) -> float:
    if not group or not group["samples"]:
        if allow_missing:
            return 0.0
        raise ValueError("Target metric sample group was empty")

    values = [float(sample["value"]) for sample in group["samples"]]
    if aggregate == "sum":
        return sum(values)
    return sum(values) / len(values)


def collect_metric_identities(grouped_scrapes: list[dict]) -> dict[tuple[tuple[str, str], ...], dict[str, str]]:
    identities = {}
    for grouped in grouped_scrapes:
        for key, group in grouped.items():
            identities.setdefault(key, group["identity_labels"])
    return identities


def format_target_metric_identity(query: str, identity_labels: dict[str, str]) -> str:
    return target_metric_display_query(query, identity_labels)


def load_target_metric_range(path: str) -> dict:
    range_path = Path(path).with_name("target-metrics-range.json")
    with open(range_path) as f:
        metadata = json.load(f)
    if not metadata.get("benchmark_id"):
        raise ValueError(f"Missing benchmark_id in {range_path}")
    metadata["benchmark_run"] = run_label_from_path(path)
    if metadata.get("duration_ms", 0) <= 0:
        raise ValueError(f"Non-positive target metric scrape range in {range_path}")
    return metadata


def load_target_metric_scrapes(path: str) -> list[dict]:
    scrape_path = Path(path).with_name("target-metrics-scrapes.jsonl")
    scrapes_by_unix_ms = {}
    with open(scrape_path) as f:
        for line_number, line in enumerate(f, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                sample = json.loads(line)
            except json.JSONDecodeError as err:
                raise ValueError(f"Invalid target metric scrape JSON in {scrape_path}:{line_number}: {err}") from err
            if not isinstance(sample, dict):
                raise ValueError(f"Invalid target metric sample in {scrape_path}:{line_number}")
            if not all(key in sample for key in ("name", "labels", "value", "offset_ms", "unix_ms")):
                raise ValueError(f"Incomplete target metric scrape record in {scrape_path}:{line_number}")
            if not isinstance(sample["name"], str) or not isinstance(sample["labels"], dict):
                raise ValueError(f"Invalid target metric scrape record in {scrape_path}:{line_number}")

            unix_ms = int(sample["unix_ms"])
            offset_ms = int(sample["offset_ms"])

            scrape = scrapes_by_unix_ms.setdefault(unix_ms, {"unix_ms": unix_ms, "_offset_ms": offset_ms, "samples": []})
            if scrape["_offset_ms"] != offset_ms:
                raise ValueError(
                    f"Mismatched target metric sample offsets for scrape {unix_ms} in {scrape_path}"
                )
            scrape["samples"].append(
                {
                    "name": sample["name"],
                    "labels": dict(sorted(sample["labels"].items())),
                    "value": float(sample["value"]),
                }
            )
    scrapes = sorted(scrapes_by_unix_ms.values(), key=lambda scrape: int(scrape["unix_ms"]))
    for scrape in scrapes:
        del scrape["_offset_ms"]
    if not scrapes:
        raise ValueError(f"No target metric scrapes found in {scrape_path}")
    return scrapes


def compute_target_metric_series_stats(values: list[float]) -> dict[str, float]:
    if not values:
        raise ValueError("Target metric series was empty")
    sorted_values = sorted(values)
    return {
        "mean": sum(values) / len(values),
        "p50": percentile(sorted_values, 50),
        "p90": percentile(sorted_values, 90),
        "p99": percentile(sorted_values, 99),
    }


def counter_target_metric_stat_value(values: list[float], stat_name: str) -> float:
    if stat_name == "mean":
        return sum(values) / len(values)
    if stat_name == "p50":
        return percentile(sorted(values), 50)
    if stat_name == "p90":
        return percentile(sorted(values), 90)
    if stat_name == "p99":
        return percentile(sorted(values), 99)
    raise ValueError(f"Unsupported target metric statistic: {stat_name}")


def paired_target_metric_observations(
    baseline_run: dict, feature_run: dict
) -> list[tuple[float, float]]:
    pairs = []
    baseline_by_block = {
        observation["block_height"]: observation["value"]
        for observation in baseline_run["_observations"]
    }
    feature_by_block = {
        observation["block_height"]: observation["value"]
        for observation in feature_run["_observations"]
    }
    for block_height in sorted(set(baseline_by_block) & set(feature_by_block)):
        pairs.append((baseline_by_block[block_height], feature_by_block[block_height]))
    return pairs


def target_metric_stat_diff(pairs: list[tuple[float, float]], stat_name: str) -> float:
    baseline_values = [baseline for baseline, _feature in pairs]
    feature_values = [feature for _baseline, feature in pairs]
    return (
        counter_target_metric_stat_value(feature_values, stat_name)
        - counter_target_metric_stat_value(baseline_values, stat_name)
    )


def target_metric_run_stat_value(run: dict, stat_name: str) -> float:
    value = run.get(stat_name)
    if isinstance(value, dict):
        return float(value["value"])
    if value is not None:
        return float(value)
    if "value" in run:
        return float(run["value"])
    if "_values" in run:
        return counter_target_metric_stat_value([float(v) for v in run["_values"]], stat_name)
    raise ValueError(f"Unsupported target metric run statistic: {stat_name}")


def _target_metric_cluster_ci(
    rng: random.Random,
    baseline_values: list[float],
    feature_values: list[float],
    n_iter: int = BOOTSTRAP_ITERATIONS,
) -> float:
    if len(baseline_values) < 2 or len(feature_values) < 2:
        return 0.0

    samples = []
    baseline_count = len(baseline_values)
    feature_count = len(feature_values)
    for _ in range(n_iter):
        baseline_sample = [
            baseline_values[rng.randrange(baseline_count)]
            for _ in range(baseline_count)
        ]
        feature_sample = [
            feature_values[rng.randrange(feature_count)]
            for _ in range(feature_count)
        ]
        samples.append(_mean(feature_sample) - _mean(baseline_sample))
    return _ci_half_width(samples)


def _target_metric_single_run_ci(
    rng: random.Random,
    baseline_run: dict,
    feature_run: dict,
    stat_name: str,
    n_iter: int = BOOTSTRAP_ITERATIONS,
) -> tuple[float, int]:
    pairs = paired_target_metric_observations(baseline_run, feature_run)
    if not pairs:
        return 0.0, 0

    boot_diffs = []
    for _ in range(n_iter):
        sample = rng.choices(pairs, k=len(pairs))
        boot_diffs.append(target_metric_stat_diff(sample, stat_name))
    return _ci_half_width(boot_diffs), len(pairs)


def compute_target_metric_change(
    baseline_runs: list[dict],
    feature_runs: list[dict],
    query: str,
    target: str,
    stat_name: str,
    floor_pct: float,
    min_value_pct_total_latency: float | None,
    block_latency_ms: float,
) -> dict:
    if not baseline_runs or not feature_runs:
        return {
            "baseline": None,
            "feature": None,
            "diff": 0.0,
            "pct": 0.0,
            "ci": 0.0,
            "ci_pct": 0.0,
            "floor_pct": floor_pct,
            "sig": "neutral",
            "significance_reason": "requires baseline and feature runs",
            "informational": True,
            "informational_reason": "requires baseline and feature runs",
        }

    baseline_values = [
        target_metric_run_stat_value(run, stat_name) for run in baseline_runs
    ]
    feature_values = [
        target_metric_run_stat_value(run, stat_name) for run in feature_runs
    ]
    baseline_value = _mean(baseline_values)
    feature_value = _mean(feature_values)
    diff = feature_value - baseline_value

    rng = random.Random(f"{query}:{stat_name}")
    paired_observations = 0
    significance_reason = None
    if len(baseline_values) >= 2 and len(feature_values) >= 2:
        ci = _target_metric_cluster_ci(rng, baseline_values, feature_values)
        ci_method = "run_cluster_bootstrap"
    else:
        ci, paired_observations = _target_metric_single_run_ci(
            rng, baseline_runs[0], feature_runs[0], stat_name
        )
        ci_method = "paired_scrape_bootstrap"
        if paired_observations < TARGET_METRIC_MIN_PAIRED_OBSERVATIONS:
            significance_reason = (
                f"requires at least {TARGET_METRIC_MIN_PAIRED_OBSERVATIONS} paired observations"
            )

    pct = (diff / baseline_value * 100.0) if abs(baseline_value) > EPSILON else 0.0
    ci_pct = (ci / abs(baseline_value) * 100.0) if abs(baseline_value) > EPSILON else 0.0
    raw_sig = significance(
        pct,
        ci_pct,
        floor_pct,
        lower_is_better=target == "decrease",
    )
    sig = raw_sig
    informational_reason = None
    materiality_details = None
    if min_value_pct_total_latency is not None and min_value_pct_total_latency > 0.0:
        threshold_ms = block_latency_ms * min_value_pct_total_latency / 100.0
        baseline_ms = abs(baseline_value) * 1_000.0
        feature_ms = abs(feature_value) * 1_000.0
        materiality_details = {
            "min_value_pct_total_latency": round(min_value_pct_total_latency, 6),
            "threshold_ms": round(threshold_ms, 6),
            "baseline_ms": round(baseline_ms, 6),
            "feature_ms": round(feature_ms, 6),
            "block_latency_ms": round(block_latency_ms, 6),
        }
        if raw_sig != "neutral" and max(baseline_ms, feature_ms) < threshold_ms:
            informational_reason = (
                f"requires value >= {min_value_pct_total_latency:.3g}% total latency"
            )
            sig = "neutral"
    if significance_reason:
        sig = "neutral"
    result = {
        "baseline": baseline_value,
        "feature": feature_value,
        "diff": round(diff, 6),
        "pct": round(pct, 4),
        "ci": round(ci, 6),
        "ci_pct": round(ci_pct, 4),
        "floor_pct": round(floor_pct, 4),
        "sig": sig,
        "ci_method": ci_method,
        "runs": {
            "baseline": len(baseline_values),
            "feature": len(feature_values),
        },
    }
    if paired_observations:
        result["paired_observations"] = paired_observations
    if materiality_details:
        result["materiality"] = materiality_details
    if informational_reason:
        result["informational"] = True
        result["informational_reason"] = informational_reason
        result["raw_sig"] = raw_sig
    if significance_reason:
        result["significance_reason"] = significance_reason
        result["informational"] = True
        result["informational_reason"] = significance_reason
        result["raw_sig"] = raw_sig
    return result


def query_counter_target_metric_run(
    relevant_scrapes: list[dict],
    counter: dict,
    run_label: str,
    metadata: dict,
    identity_label_keys: set[str],
) -> dict[tuple[tuple[str, str], ...], dict]:
    query = counter["query"]
    aggregate, _metric_name, _label_filters = parse_target_metric_query(query)
    grouped_scrapes = [
        group_query_samples_by_identity(scrape["samples"], query, identity_label_keys)[1]
        for scrape in relevant_scrapes
    ]
    identities = collect_metric_identities(grouped_scrapes)
    if not identities:
        raise ValueError(f"Target metric '{query}' in run '{run_label}' had no sampled values")

    results = {}
    for identity_key, identity_labels in sorted(identities.items()):
        interval_observations = []
        counter_increase = 0.0
        block_height_delta = 0.0
        display_query = format_target_metric_identity(query, identity_labels)
        for previous_scrape, current_scrape, previous_groups, current_groups in zip(
            relevant_scrapes,
            relevant_scrapes[1:],
            grouped_scrapes,
            grouped_scrapes[1:],
        ):
            current_counter_value = grouped_sample_value(
                current_groups.get(identity_key), aggregate, allow_missing=True
            )
            previous_counter_value = grouped_sample_value(
                previous_groups.get(identity_key), aggregate, allow_missing=True
            )
            current_block_height = float(current_scrape["block_height"])
            previous_block_height = float(previous_scrape["block_height"])
            interval_block_height_delta = current_block_height - previous_block_height
            if interval_block_height_delta <= EPSILON:
                continue

            interval_counter_delta = current_counter_value - previous_counter_value
            if interval_counter_delta < -EPSILON:
                raise ValueError(
                    f"Target metric '{display_query}' decreased within run '{run_label}', which is not valid for counters"
                )

            counter_increase += interval_counter_delta
            block_height_delta += interval_block_height_delta
            interval_observations.append(
                {
                    "block_height": current_block_height,
                    "value": interval_counter_delta / interval_block_height_delta,
                }
            )

        if not interval_observations:
            raise ValueError(
                f"Target metric '{display_query}' in run '{run_label}' had no positive block-height scrape intervals"
            )

        stats = compute_target_metric_series_stats(
            [observation["value"] for observation in interval_observations]
        )
        results[identity_key] = {
            "query": query,
            "display_query": display_query,
            "identity_labels": identity_labels,
            "target": counter["target"],
            "counter_increase": counter_increase,
            "block_height_delta": block_height_delta,
            "mean": stats["mean"],
            "p50": stats["p50"],
            "p90": stats["p90"],
            "p99": stats["p99"],
            "intervals": len(interval_observations),
            "scrapes": len(relevant_scrapes),
            "duration_ms": int(metadata["duration_ms"]),
            "range_start_ms": int(metadata["range_start_ms"]),
            "range_end_ms": int(metadata["range_end_ms"]),
            "benchmark_id": metadata["benchmark_id"],
            "benchmark_run": metadata["benchmark_run"],
            "_values": [observation["value"] for observation in interval_observations],
            "_observations": interval_observations,
        }
    return results


def query_histogram_target_metric_run(
    relevant_scrapes: list[dict],
    histogram: dict,
    run_label: str,
    metadata: dict,
    identity_label_keys: set[str],
) -> dict[tuple[tuple[str, str], ...], dict]:
    sum_query = histogram_counter_query(histogram["query"], "sum")
    sum_grouped_scrapes = [
        group_query_samples_by_identity(scrape["samples"], sum_query, identity_label_keys)[1]
        for scrape in relevant_scrapes
    ]
    mean_identities = collect_metric_identities(sum_grouped_scrapes)

    metrics_by_identity = {}
    for identity_key, identity_labels in sorted(mean_identities.items()):
        per_block_observations = []
        has_histogram_activity = False
        pending_sum_delta = 0.0
        total_sum_delta = 0.0
        total_block_delta = 0.0
        display_query = format_target_metric_identity(histogram["query"], identity_labels)
        for previous_scrape, current_scrape, previous_sum_groups, current_sum_groups in zip(
            relevant_scrapes,
            relevant_scrapes[1:],
            sum_grouped_scrapes,
            sum_grouped_scrapes[1:],
        ):
            current_sum = grouped_sample_value(current_sum_groups.get(identity_key), "single", allow_missing=True)
            previous_sum = grouped_sample_value(previous_sum_groups.get(identity_key), "single", allow_missing=True)
            current_block_height = float(current_scrape["block_height"])
            previous_block_height = float(previous_scrape["block_height"])
            block_delta = current_block_height - previous_block_height
            sum_delta = current_sum - previous_sum
            if sum_delta < -EPSILON:
                raise ValueError(
                    f"Histogram target metric '{display_query}' sum decreased within run '{run_label}'"
                )
            if sum_delta > EPSILON:
                has_histogram_activity = True
                pending_sum_delta += sum_delta
                total_sum_delta += sum_delta

            if block_delta <= EPSILON:
                continue

            total_block_delta += block_delta
            per_block_observations.append(
                {
                    "block_height": current_block_height,
                    "sum_delta": pending_sum_delta,
                    "block_delta": block_delta,
                    "value": pending_sum_delta / block_delta,
                }
            )
            pending_sum_delta = 0.0

        if pending_sum_delta > EPSILON and per_block_observations:
            last_observation = per_block_observations[-1]
            last_observation["sum_delta"] += pending_sum_delta
            last_observation["value"] = (
                last_observation["sum_delta"] / last_observation["block_delta"]
            )

        if not has_histogram_activity or not per_block_observations or total_block_delta <= EPSILON:
            continue

        stats = compute_target_metric_series_stats(
            [observation["value"] for observation in per_block_observations]
        )
        stats["mean"] = total_sum_delta / total_block_delta
        metrics_by_identity[identity_key] = {
            "query": histogram["query"],
            "display_query": display_query,
            "identity_labels": identity_labels,
            "target": histogram["target"],
            "mean": {
                "query": f"delta({sum_query}) / delta({TARGET_METRIC_BLOCK_HEIGHT_QUERY})",
                "value": stats["mean"],
                "samples": len(per_block_observations),
                "scrapes": len(relevant_scrapes),
                "duration_ms": int(metadata["duration_ms"]),
                "range_start_ms": int(metadata["range_start_ms"]),
                "range_end_ms": int(metadata["range_end_ms"]),
                "benchmark_id": metadata["benchmark_id"],
                "benchmark_run": metadata["benchmark_run"],
                "_values": [observation["value"] for observation in per_block_observations],
                "_observations": per_block_observations,
            },
        }

    return metrics_by_identity


def load_target_metric_run_context(path: str) -> dict:
    run_label = run_label_from_path(path)
    metadata = load_target_metric_range(path)
    scrapes = load_target_metric_scrapes(path)
    range_start_ms = int(metadata["range_start_ms"])
    range_end_ms = int(metadata["range_end_ms"])
    relevant_scrapes = [
        scrape
        for scrape in scrapes
        if range_start_ms <= int(scrape["unix_ms"]) <= range_end_ms
    ]
    if len(relevant_scrapes) < 2:
        raise ValueError(
            f"Target metric scrapes for run '{run_label}' only had {len(relevant_scrapes)} samples inside the benchmark window"
        )
    for scrape in relevant_scrapes:
        scrape["block_height"] = evaluate_query(scrape["samples"], TARGET_METRIC_BLOCK_HEIGHT_QUERY)
    return {
        "run_label": run_label,
        "metadata": metadata,
        "relevant_scrapes": relevant_scrapes,
    }


def collect_target_metric_identity_label_keys(
    run_contexts: list[dict],
    config: dict,
) -> dict[str, set[str]]:
    identity_label_keys_by_query = {}

    for counter in config.get("counters", []):
        query = counter["query"]
        identity_label_keys_by_query[query] = set().union(
            *(
                target_metric_variable_identity_label_keys(
                    context["relevant_scrapes"],
                    query,
                )
                for context in run_contexts
            )
        )

    for histogram in config.get("histograms", []):
        query = histogram_counter_query(histogram["query"], "sum")
        identity_label_keys_by_query[query] = set().union(
            *(
                target_metric_variable_identity_label_keys(
                    context["relevant_scrapes"],
                    query,
                )
                for context in run_contexts
            )
        )

    return identity_label_keys_by_query


def query_target_metric_run(
    run_context: dict,
    config: dict,
    identity_label_keys_by_query: dict[str, set[str]],
) -> tuple[str, dict[str, dict[str, dict]]]:
    run_label = run_context["run_label"]
    metadata = run_context["metadata"]
    relevant_scrapes = run_context["relevant_scrapes"]

    counters = {}
    for counter in config.get("counters", []):
        query = counter["query"]
        counters[counter["query"]] = query_counter_target_metric_run(
            relevant_scrapes,
            counter,
            run_label,
            metadata,
            identity_label_keys_by_query[query],
        )

    histograms = {}
    for histogram in config.get("histograms", []):
        query = histogram_counter_query(histogram["query"], "sum")
        histograms[histogram["query"]] = query_histogram_target_metric_run(
            relevant_scrapes,
            histogram,
            run_label,
            metadata,
            identity_label_keys_by_query[query],
        )

    return run_label, {"counters": counters, "histograms": histograms}


def run_label_from_path(path: str) -> str:
    return Path(path).parent.name or Path(path).stem


def summarize_target_metric_runs(run_items: list[dict], fields: tuple[str, ...]) -> dict:
    summary = {field: sum(item[field] for item in run_items) / len(run_items) for field in fields}
    summary["runs"] = run_items
    return summary


def summarize_target_metric_change(changes: dict[str, dict], display_stats: tuple[str, ...]) -> dict:
    significant = [name for name in display_stats if changes[name]["sig"] != "neutral"]
    if not significant:
        sig = "neutral"
    elif any(changes[name]["sig"] == "bad" for name in significant):
        sig = "bad"
    else:
        sig = "good"
    return {
        "sig": sig,
        "significant_stats": significant,
    }


def collect_query_metric_identities(
    runs: list[tuple[str, dict[str, dict[str, dict]]]], kind: str, query: str
) -> dict[tuple[tuple[str, str], ...], dict[str, str]]:
    identities = {}
    for run_label, run_data in runs:
        if query not in run_data[kind]:
            raise ValueError(f"Missing target metric '{query}' in run '{run_label}'")
        for identity_key, metric in run_data[kind][query].items():
            identities.setdefault(identity_key, metric["identity_labels"])
    return identities


def target_metric_pair_rows(
    baseline_values: list[dict],
    feature_values: list[dict],
    pair_stats: tuple[str, ...],
    include_mean_values: bool,
) -> list[dict] | None:
    if len(baseline_values) <= 1 or len(feature_values) <= 1:
        return None

    rows = []
    for baseline_item, feature_item in zip(baseline_values, feature_values):
        row = {
            "baseline_run": baseline_item["run"],
            "feature_run": feature_item["run"],
        }
        if include_mean_values:
            row["baseline_mean"] = baseline_item["mean"]
            row["feature_mean"] = feature_item["mean"]
        row.update(
            {
                f"{stat_name}_diff": feature_item[stat_name] - baseline_item[stat_name]
                for stat_name in pair_stats
            }
        )
        rows.append(row)
    return rows


def build_target_metric_entry(
    kind: str,
    configured_query: str,
    display_query: str,
    identity_labels: dict[str, str],
    target: str,
    display_stats: tuple[str, ...],
    summary_fields: tuple[str, ...],
    baseline_values: list[dict],
    feature_values: list[dict],
    baseline_runs_by_stat: dict[str, list[dict]],
    feature_runs_by_stat: dict[str, list[dict]],
    pair_stats: tuple[str, ...],
    floor_pct: float,
    min_value_pct_total_latency: float | None,
    block_latency_ms: float,
    include_pair_mean_values: bool = False,
    unit: str | None = None,
) -> dict:
    changes = {
        stat_name: compute_target_metric_change(
            baseline_runs_by_stat[stat_name],
            feature_runs_by_stat[stat_name],
            display_query,
            target,
            stat_name,
            floor_pct,
            min_value_pct_total_latency,
            block_latency_ms,
        )
        for stat_name in display_stats
    }
    metric_unit = unit or (
        "seconds" if target_metric_query_encodes_seconds(configured_query) else None
    )
    entry = {
        "kind": kind,
        "name": display_query,
        "query": display_query,
        "configured_query": configured_query,
        "identity_labels": identity_labels,
        "target": target,
        "floor_pct": floor_pct,
        "display_stats": list(display_stats),
        "baseline": summarize_target_metric_runs(baseline_values, summary_fields),
        "feature": summarize_target_metric_runs(feature_values, summary_fields),
        "changes": changes,
        "change": summarize_target_metric_change(changes, display_stats),
    }
    if metric_unit:
        entry["unit"] = metric_unit
    if min_value_pct_total_latency is not None:
        entry["materiality"] = {
            "min_value_pct_total_latency": min_value_pct_total_latency,
        }

    pairs = target_metric_pair_rows(
        baseline_values, feature_values, pair_stats, include_pair_mean_values
    )
    if pairs:
        entry["pairs"] = pairs
    return entry


def compute_target_metric_summary(
    config_path: str,
    baseline_csv_paths: list[str],
    feature_csv_paths: list[str],
    baseline_stats: dict,
    feature_stats: dict,
) -> dict:
    with open(config_path) as f:
        config = json.load(f)

    baseline_run_contexts = [
        load_target_metric_run_context(path) for path in baseline_csv_paths
    ]
    feature_run_contexts = [
        load_target_metric_run_context(path) for path in feature_csv_paths
    ]
    identity_label_keys_by_query = collect_target_metric_identity_label_keys(
        baseline_run_contexts + feature_run_contexts,
        config,
    )
    baseline_runs = [
        query_target_metric_run(context, config, identity_label_keys_by_query)
        for context in baseline_run_contexts
    ]
    feature_runs = [
        query_target_metric_run(context, config, identity_label_keys_by_query)
        for context in feature_run_contexts
    ]
    block_latency_ms = max(
        float(baseline_stats["mean_total_lat_ms"]),
        float(feature_stats["mean_total_lat_ms"]),
    )

    metrics = []
    for counter in config.get("counters", []):
        query = counter["query"]
        target = counter["target"]
        floor_pct = float(counter.get("floor_pct", 0.0))
        counter_min_value_pct_total_latency = target_metric_min_value_pct_total_latency(counter)
        counter_encodes_seconds = target_metric_counter_encodes_seconds(query)
        if (
            counter_min_value_pct_total_latency is not None and
            counter_min_value_pct_total_latency > 0.0 and
            not counter_encodes_seconds
        ):
            raise ValueError(
                f"Counter target metric '{query}' sets min_value_pct_total_latency "
                "but does not encode seconds"
            )
        if not counter_encodes_seconds:
            counter_min_value_pct_total_latency = None
        display_stats = TARGET_METRIC_COUNTER_STATS
        summary_fields = ("mean", "p50", "p90", "p99")
        identities = collect_query_metric_identities(
            baseline_runs + feature_runs, "counters", query
        )

        for identity_key, identity_labels in sorted(identities.items()):
            baseline_values = []
            feature_values = []
            baseline_runs_for_stats = []
            feature_runs_for_stats = []
            display_query = format_target_metric_identity(query, identity_labels)

            for run_label, run_data in baseline_runs:
                if identity_key not in run_data["counters"][query]:
                    raise ValueError(
                        f"Missing target metric '{display_query}' in baseline run '{run_label}'"
                    )
                run_metric = run_data["counters"][query][identity_key]
                baseline_runs_for_stats.append(run_metric)
                baseline_values.append(
                    {
                        "run": run_label,
                        "mean": float(run_metric["mean"]),
                        "p50": float(run_metric["p50"]),
                        "p90": float(run_metric["p90"]),
                        "p99": float(run_metric["p99"]),
                        "counter_increase": float(run_metric["counter_increase"]),
                        "block_height_delta": float(run_metric["block_height_delta"]),
                        "intervals": int(run_metric["intervals"]),
                        "scrapes": int(run_metric["scrapes"]),
                        "duration_ms": int(run_metric["duration_ms"]),
                    }
                )
            for run_label, run_data in feature_runs:
                if identity_key not in run_data["counters"][query]:
                    raise ValueError(
                        f"Missing target metric '{display_query}' in feature run '{run_label}'"
                    )
                run_metric = run_data["counters"][query][identity_key]
                feature_runs_for_stats.append(run_metric)
                feature_values.append(
                    {
                        "run": run_label,
                        "mean": float(run_metric["mean"]),
                        "p50": float(run_metric["p50"]),
                        "p90": float(run_metric["p90"]),
                        "p99": float(run_metric["p99"]),
                        "counter_increase": float(run_metric["counter_increase"]),
                        "block_height_delta": float(run_metric["block_height_delta"]),
                        "intervals": int(run_metric["intervals"]),
                        "scrapes": int(run_metric["scrapes"]),
                        "duration_ms": int(run_metric["duration_ms"]),
                    }
                )

            metrics.append(
                build_target_metric_entry(
                    kind="counter",
                    configured_query=query,
                    display_query=display_query,
                    identity_labels=identity_labels,
                    target=target,
                    display_stats=display_stats,
                    summary_fields=summary_fields,
                    baseline_values=baseline_values,
                    feature_values=feature_values,
                    baseline_runs_by_stat={
                        stat_name: baseline_runs_for_stats for stat_name in display_stats
                    },
                    feature_runs_by_stat={
                        stat_name: feature_runs_for_stats for stat_name in display_stats
                    },
                    pair_stats=summary_fields,
                    floor_pct=floor_pct,
                    min_value_pct_total_latency=counter_min_value_pct_total_latency,
                    block_latency_ms=block_latency_ms,
                    include_pair_mean_values=True,
                    unit=counter.get("unit"),
                )
            )

    for histogram in config.get("histograms", []):
        query = histogram["query"]
        target = histogram["target"]
        floor_pct = float(histogram.get("floor_pct", 0.0))
        histogram_min_value_pct_total_latency = target_metric_min_value_pct_total_latency(
            histogram
        )
        display_stats = ("mean",)
        identities = collect_query_metric_identities(
            baseline_runs + feature_runs, "histograms", query
        )

        for identity_key, identity_labels in sorted(identities.items()):
            baseline_values = []
            feature_values = []
            display_query = format_target_metric_identity(query, identity_labels)
            baseline_run_metrics = []
            feature_run_metrics = []
            if not all(
                identity_key in run_data["histograms"][query]
                for _run_label, run_data in baseline_runs + feature_runs
            ):
                continue

            for run_label, run_data in baseline_runs:
                run_metric = run_data["histograms"][query][identity_key]
                baseline_run_metrics.append((run_label, run_metric))

            for run_label, run_data in feature_runs:
                run_metric = run_data["histograms"][query][identity_key]
                feature_run_metrics.append((run_label, run_metric))

            baseline_runs_for_stats = {stat_name: [] for stat_name in display_stats}
            feature_runs_for_stats = {stat_name: [] for stat_name in display_stats}

            for run_label, run_metric in baseline_run_metrics:
                run_values = {"run": run_label}
                for stat_name in display_stats:
                    run_stat = run_metric[stat_name]
                    baseline_runs_for_stats[stat_name].append(run_stat)
                    run_values[stat_name] = float(run_stat["value"])
                baseline_values.append(run_values)

            for run_label, run_metric in feature_run_metrics:
                run_values = {"run": run_label}
                for stat_name in display_stats:
                    run_stat = run_metric[stat_name]
                    feature_runs_for_stats[stat_name].append(run_stat)
                    run_values[stat_name] = float(run_stat["value"])
                feature_values.append(run_values)

            metrics.append(
                build_target_metric_entry(
                    kind="histogram",
                    configured_query=query,
                    display_query=display_query,
                    identity_labels=identity_labels,
                    target=target,
                    display_stats=display_stats,
                    summary_fields=display_stats,
                    baseline_values=baseline_values,
                    feature_values=feature_values,
                    baseline_runs_by_stat=baseline_runs_for_stats,
                    feature_runs_by_stat=feature_runs_for_stats,
                    pair_stats=display_stats,
                    floor_pct=floor_pct,
                    min_value_pct_total_latency=histogram_min_value_pct_total_latency,
                    block_latency_ms=block_latency_ms,
                    unit=histogram.get("unit"),
                )
            )

    changed = [metric for metric in metrics if metric["change"]["significant_stats"]]
    return {
        "config": config_path,
        "block_latency_ms": round(block_latency_ms, 6),
        "metrics": metrics,
        "changed": changed,
        "improvements": [metric["name"] for metric in changed if metric["change"]["sig"] == "good"],
        "regressions": [metric["name"] for metric in changed if metric["change"]["sig"] == "bad"],
    }


def generate_comparison_table(
    run1: dict,
    run2: dict,
    ci_stats: dict,
    repo: str,
    baseline_ref: str,
    baseline_name: str,
    feature_name: str,
    feature_sha: str,
    big_blocks: bool = False,
    warmup_blocks: str | None = None,
    wait_time: str | None = None,
    bal_mode: str | None = None,
    run_pairs: int | None = None,
) -> str:
    """Generate a markdown comparison table between baseline and feature."""
    n = ci_stats["blocks"]

    def pct(base: float, feat: float) -> float:
        return (feat - base) / base * 100.0 if base > 0 else 0.0

    gas_pct = pct(run1["mean_mgas_s"], run2["mean_mgas_s"])
    wall_pct = pct(run1["wall_clock_s"], run2["wall_clock_s"])

    mean_pct = pct(run1["mean_ms"], run2["mean_ms"])
    p50_pct = pct(run1["p50_ms"], run2["p50_ms"])
    p90_pct = pct(run1["p90_ms"], run2["p90_ms"])
    p99_pct = pct(run1["p99_ms"], run2["p99_ms"])

    persist_pct = pct(run1["mean_persist_ms"], run2["mean_persist_ms"])

    # Bootstrap CIs as % of baseline percentile
    mean_ci_pct = ci_stats["ci_ms"] / run1["mean_ms"] * 100.0 if run1["mean_ms"] > 0 else 0.0
    p50_ci_pct = ci_stats["p50_ci_ms"] / run1["p50_ms"] * 100.0 if run1["p50_ms"] > 0 else 0.0
    p90_ci_pct = ci_stats["p90_ci_ms"] / run1["p90_ms"] * 100.0 if run1["p90_ms"] > 0 else 0.0
    p99_ci_pct = ci_stats["p99_ci_ms"] / run1["p99_ms"] * 100.0 if run1["p99_ms"] > 0 else 0.0

    # CI as a percentage of baseline
    mgas_ci_pct = ci_stats["mgas_ci"] / run1["mean_mgas_s"] * 100.0 if run1["mean_mgas_s"] > 0 else 0.0
    wall_ci_pct = ci_stats["wall_clock_ci_ms"] / run1["mean_total_lat_ms"] * 100.0 if run1["mean_total_lat_ms"] > 0 else 0.0
    persist_ci_pct = ci_stats["persist_ci_ms"] / run1["mean_persist_ms"] * 100.0 if run1["mean_persist_ms"] > 0 else 0.0

    mean_floor = practical_floor_pct("mean", run1["mean_ms"])
    p50_floor = practical_floor_pct("p50", run1["p50_ms"])
    p90_floor = practical_floor_pct("p90", run1["p90_ms"])
    p99_floor = practical_floor_pct("p99", run1["p99_ms"])
    mgas_floor = practical_floor_pct("mgas_s", run1["mean_mgas_s"])
    wall_floor = practical_floor_pct("wall_clock", run1["mean_total_lat_ms"])
    persist_floor = practical_floor_pct("persist_wait", run1["mean_persist_ms"])
    p99_informational = informational_reason("p99", ci_stats, run1, run2)
    persist_informational = informational_reason("persist_wait", ci_stats, run1, run2)

    base_url = f"https://github.com/{repo}/commit"
    baseline_label = f"[`{baseline_name}`]({base_url}/{baseline_ref})"
    feature_label = f"[`{feature_name}`]({base_url}/{feature_sha})"

    lines = [
        f"| Metric | {baseline_label} | {feature_label} | Change |",
        "|--------|------|--------|--------|",
        f"| Mean | {fmt_ms(run1['mean_ms'])} | {fmt_ms(run2['mean_ms'])} | {change_str(mean_pct, mean_ci_pct, mean_floor, lower_is_better=True)} |",
        f"| P50 | {fmt_ms(run1['p50_ms'])} | {fmt_ms(run2['p50_ms'])} | {change_str(p50_pct, p50_ci_pct, p50_floor, lower_is_better=True)} |",
        f"| P90 | {fmt_ms(run1['p90_ms'])} | {fmt_ms(run2['p90_ms'])} | {change_str(p90_pct, p90_ci_pct, p90_floor, lower_is_better=True)} |",
        f"| P99 | {fmt_ms(run1['p99_ms'])} | {fmt_ms(run2['p99_ms'])} | {change_str(p99_pct, p99_ci_pct, p99_floor, lower_is_better=True, informational=p99_informational)} |",
        f"| Mgas/s | {fmt_mgas(run1['mean_mgas_s'])} | {fmt_mgas(run2['mean_mgas_s'])} | {change_str(gas_pct, mgas_ci_pct, mgas_floor, lower_is_better=False)} |",
        f"| Wall Clock | {fmt_s(run1['wall_clock_s'])} | {fmt_s(run2['wall_clock_s'])} | {change_str(wall_pct, wall_ci_pct, wall_floor, lower_is_better=True)} |",
        f"| Persist Wait | {fmt_ms(run1['mean_persist_ms'])} | {fmt_ms(run2['mean_persist_ms'])} | {change_str(persist_pct, persist_ci_pct, persist_floor, lower_is_better=True, informational=persist_informational)} |",
        "",
    ]
    meta_parts = [f"{n} {'big blocks' if big_blocks else 'blocks'}"]
    if warmup_blocks:
        meta_parts.append(f"{warmup_blocks} warmup")
    if run_pairs:
        meta_parts.append(f"{run_pairs} run pairs")
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
) -> str:
    """Generate a markdown table for a wait time metric."""
    if not baseline_stats or not feature_stats:
        return ""
    lines = [
        f"### {title}",
        "",
        f"| Metric | {baseline_label} | {feature_label} |",
        "|--------|------|--------|",
        f"| Mean | {fmt_ms(baseline_stats['mean_ms'])} | {fmt_ms(feature_stats['mean_ms'])} |",
        f"| P50 | {fmt_ms(baseline_stats['p50_ms'])} | {fmt_ms(feature_stats['p50_ms'])} |",
        f"| P95 | {fmt_ms(baseline_stats['p95_ms'])} | {fmt_ms(feature_stats['p95_ms'])} |",
    ]
    return "\n".join(lines)


def generate_target_metric_table(target_metrics: dict | None) -> str:
    if not target_metrics:
        return ""

    changed = target_metrics.get("changed", [])
    if not changed:
        return ""

    lines = [
        "### Target Metrics",
        "",
        "| Metric | Baseline | Feature | Change |",
        "|--------|----------|---------|--------|",
    ]
    row_count = 0
    for metric in changed:
        for stat_name in metric.get("display_stats", []):
            change = metric["changes"][stat_name]
            if change["sig"] == "neutral":
                continue
            row_count += 1
            lines.append(
                "| `{}` | {} | {} | {} |".format(
                    f"{metric['name']} {stat_name}",
                    fmt_target_metric_value(metric, change["baseline"]),
                    fmt_target_metric_value(metric, change["feature"]),
                    target_metric_change_str(change),
                )
            )

    if row_count == 0:
        return ""

    return "\n".join(lines)


def target_metric_change_str(change: dict) -> str:
    sig = "neutral" if change.get("informational") else change.get("sig", "neutral")
    emoji = {"good": "✅", "bad": "❌", "neutral": "⚪"}[sig]
    details = [f"±{change['ci_pct']:.2f}%", f"floor {change['floor_pct']:.2f}%"]
    if materiality := change.get("materiality"):
        details.append(f"materiality {materiality['threshold_ms']:.2f}ms")
    if change.get("informational"):
        details.append("informational")
    return (
        f"{change['pct']:+.2f}% {emoji} "
        f"({', '.join(details)})"
    )


def generate_observability_section(summary: dict) -> list[str]:
    observability = summary.get("observability") or {}
    benchmark_id = summary.get("benchmark_id") or observability.get("benchmark_id")
    links = [
        ("Metrics dashboard", observability.get("grafana_url")),
        ("Logs", observability.get("logs_url")),
        ("Traces", observability.get("traces_url")),
    ]
    links = [(label, url) for label, url in links if url]

    if not benchmark_id and not links:
        return []

    lines = ["", "### Observability", ""]
    if benchmark_id:
        lines.append(f"- Benchmark ID: `{benchmark_id}`")
    for label, url in links:
        lines.append(f"- [{label}]({url})")
    return lines


def generate_markdown(
    summary: dict, comparison_table: str,
    wait_time_tables: list[str] | None = None,
    target_metric_table: str = "",
    behind_baseline: int = 0, repo: str = "", baseline_ref: str = "", baseline_name: str = "",
    derek_command: str | None = None,
) -> str:
    """Generate a markdown comment body."""
    lines = ["## Benchmark Results", "", "## Configuration"]
    if derek_command:
        lines.append(f"- Derek command: `{derek_command}`")
    lines.append("")
    if behind_baseline > 0:
        s = "s" if behind_baseline > 1 else ""
        diff_link = f"https://github.com/{repo}/compare/{baseline_ref[:12]}...{baseline_name}"
        lines.append(f"> ⚠️ Feature is [**{behind_baseline} commit{s} behind `{baseline_name}`**]({diff_link}). Consider rebasing for accurate results.")
        lines.append("")
    lines.append(comparison_table)
    if wait_time_tables:
        lines.append("")
        lines.append("<details>")
        lines.append("<summary>Wait Time Breakdown</summary>")
        lines.append("")
        for table in wait_time_tables:
            if table:
                lines.append(table)
                lines.append("")
        lines.append("</details>")
    if target_metric_table:
        lines.append("")
        lines.append(target_metric_table)
    lines.extend(generate_observability_section(summary))
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Parse benchmark run-pair results")
    parser.add_argument(
        "--baseline-csv", nargs="+", required=True,
        help="Baseline combined_latency.csv files",
    )
    parser.add_argument(
        "--feature-csv", "--branch-csv", nargs="+", required=True,
        help="Feature combined_latency.csv files",
    )
    parser.add_argument("--gas-csv", default=None, help=argparse.SUPPRESS)
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
    parser.add_argument("--benchmark-id", default=os.environ.get("BENCH_ID"), help="Benchmark ID used for OTLP labels")
    parser.add_argument("--grafana-url", default=None, help="Grafana dashboard URL for this benchmark run")
    parser.add_argument("--logs-url", default=None, help="Grafana Explore URL for benchmark logs")
    parser.add_argument("--traces-url", default=None, help="Grafana Explore URL for benchmark traces")
    parser.add_argument("--target-metrics-config", default=None, help="Target metrics config path")
    parser.add_argument(
        "--derek-command",
        default=os.environ.get("DEREK_BENCH_COMMAND"),
        help="Full derek bench command",
    )
    parser.add_argument("--run-pairs", type=int, default=None, help="Configured number of benchmark run pairs")
    args = parser.parse_args()

    if args.run_pairs is not None and args.run_pairs < 1:
        print("--run-pairs must be greater than zero", file=sys.stderr)
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

    all_baseline = [r for run in baseline_runs for r in run]
    all_feature = [r for run in feature_runs for r in run]

    baseline_stats = compute_point_stats(baseline_runs)
    feature_stats = compute_point_stats(feature_runs)
    ci_stats = compute_ci_stats(baseline_runs, feature_runs)

    if not ci_stats:
        print("No comparable baseline and feature results", file=sys.stderr)
        sys.exit(1)

    baseline_ref = args.baseline_ref or "main"
    baseline_name = args.baseline_name or "baseline"
    feature_name = args.feature_name or "feature"
    feature_sha = args.feature_ref or "unknown"
    bal_mode = display_bal_mode(args.bal_mode)

    comparison_table = generate_comparison_table(
        baseline_stats,
        feature_stats,
        ci_stats,
        repo=args.repo,
        baseline_ref=baseline_ref,
        baseline_name=baseline_name,
        feature_name=feature_name,
        feature_sha=feature_sha,
        big_blocks=args.big_blocks,
        warmup_blocks=args.warmup_blocks,
        wait_time=args.wait_time,
        bal_mode=bal_mode,
        run_pairs=args.run_pairs,
    )
    print(
        f"Generated comparison ({ci_stats['blocks']} blocks, "
        f"mean CI ± {ci_stats['ci_ms']:.3f}ms)"
    )

    base_url = f"https://github.com/{args.repo}/commit"
    baseline_label = f"[`{baseline_name}`]({base_url}/{baseline_ref})"
    feature_label = f"[`{feature_name}`]({base_url}/{feature_sha})"

    wait_fields = [
        ("persistence_wait_us", "Persistence Wait"),
        ("sparse_trie_wait_us", "Trie Cache Update Wait"),
        ("execution_cache_wait_us", "Execution Cache Update Wait"),
    ]
    wait_time_tables = []
    wait_time_data = {}
    for field, title in wait_fields:
        b_stats = compute_wait_stats(all_baseline, field)
        f_stats = compute_wait_stats(all_feature, field)
        if b_stats and f_stats:
            wait_time_data[field] = {
                "title": title,
                "baseline": b_stats,
                "feature": f_stats,
            }
        table = generate_wait_time_table(title, b_stats, f_stats, baseline_label, feature_label)
        if table:
            wait_time_tables.append(table)


    target_metric_summary = None
    target_metric_table = ""
    if args.target_metrics_config:
        target_metric_summary = compute_target_metric_summary(
            args.target_metrics_config,
            args.baseline_csv,
            args.feature_csv,
            baseline_stats,
            feature_stats,
        )
        target_metric_table = generate_target_metric_table(target_metric_summary)

    summary = {
        "blocks": ci_stats["blocks"],
        "big_blocks": args.big_blocks,
        "warmup_blocks": args.warmup_blocks,
        "run_pairs": args.run_pairs,
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
        "changes": compute_changes(baseline_stats, feature_stats, ci_stats),
        "wait_times": wait_time_data,
    }
    if args.benchmark_id:
        summary["benchmark_id"] = args.benchmark_id
    observability = {
        key: value for key, value in {
            "benchmark_id": args.benchmark_id,
            "grafana_url": args.grafana_url,
            "logs_url": args.logs_url,
            "traces_url": args.traces_url,
        }.items() if value
    }
    if observability:
        summary["observability"] = observability
    if target_metric_summary:
        summary["target_metrics"] = target_metric_summary
    with open(args.output_summary, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"Summary written to {args.output_summary}")

    markdown = generate_markdown(
        summary, comparison_table,
        wait_time_tables=wait_time_tables,
        target_metric_table=target_metric_table,
        behind_baseline=args.behind_baseline,
        repo=args.repo,
        baseline_ref=baseline_ref,
        baseline_name=baseline_name,
        derek_command=args.derek_command,
    )

    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")


if __name__ == "__main__":
    main()
