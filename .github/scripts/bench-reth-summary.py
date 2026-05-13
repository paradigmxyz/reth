#!/usr/bin/env python3
"""Parse reth-bench CSV output and generate a summary JSON + markdown comparison.

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

import argparse
import csv
import json
import math
from pathlib import Path
import random
import re
import sys

GIGAGAS = 1_000_000_000
BOOTSTRAP_ITERATIONS = 10_000
EPSILON = 1e-9
TARGET_METRIC_BLOCK_HEIGHT_QUERY = "reth_blockchain_tree_canonical_chain_height"
TARGET_METRIC_COUNTER_STATS = ("p50", "p90")
TARGET_METRIC_MIN_PAIRED_OBSERVATIONS = 30
TARGET_METRIC_IGNORED_CARDINALITY_LABELS = frozenset(("quantile", "run_type"))
SELECTOR_RE = re.compile(
    r"^(?P<name>[a-zA-Z_:][a-zA-Z0-9_:]*)(?:\{(?P<labels>[^}]*)\})?$"
)
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


def fmt_metric_value(v: float) -> str:
    abs_v = abs(v)
    if abs_v == 0:
        return "0"
    if abs_v < 0.001:
        return f"{v:.4g}"
    if abs_v >= 1 and abs(v - round(v)) <= 0.00005:
        return f"{round(v):.0f}"
    return f"{v:.4f}".rstrip("0").rstrip(".")


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


def target_metric_identity_key(labels: dict[str, str]) -> tuple[tuple[str, str], ...]:
    return tuple(sorted(labels.items()))


def target_metric_identity_labels(query: str, sample_labels: dict[str, str]) -> dict[str, str]:
    _, _, label_filters = parse_target_metric_query(query)
    return {
        key: value
        for key, value in sorted(sample_labels.items())
        if key not in label_filters and key not in TARGET_METRIC_IGNORED_CARDINALITY_LABELS
    }


def target_metric_display_query(query: str, identity_labels: dict[str, str] | None = None) -> str:
    _, metric_name, label_filters = parse_target_metric_query(query)
    display_labels = dict(label_filters)
    if identity_labels:
        display_labels.update(identity_labels)
    return re.sub(r"\s+", "", format_target_metric_query(metric_name, display_labels))


def group_query_samples_by_identity(samples: list[dict], query: str) -> tuple[str, dict]:
    aggregate, matched_samples = query_samples(samples, query)
    groups = {}
    if aggregate == "sum":
        if matched_samples:
            groups[()] = {"identity_labels": {}, "samples": matched_samples}
        return aggregate, groups

    for sample in matched_samples:
        identity_labels = target_metric_identity_labels(query, sample["labels"])
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


def target_metric_directions_agree(diffs: list[float]) -> bool:
    if len(diffs) < 2:
        return True
    return (
        all(diff > 0 for diff in diffs)
        or all(diff < 0 for diff in diffs)
        or all(diff == 0 for diff in diffs)
    )


def compute_paired_target_metric_change(
    baseline_runs: list[dict],
    feature_runs: list[dict],
    query: str,
    target: str,
    stat_name: str,
) -> dict:
    all_pairs = []
    per_pairing_diffs = []
    for baseline_run in baseline_runs:
        for feature_run in feature_runs:
            pairs = paired_target_metric_observations(baseline_run, feature_run)
            if not pairs:
                continue
            all_pairs.extend(pairs)
            per_pairing_diffs.append(target_metric_stat_diff(pairs, stat_name))

    if not all_pairs:
        return {
            "baseline": None,
            "feature": None,
            "diff": 0.0,
            "pct": 0.0,
            "ci": 0.0,
            "ci_pct": 0.0,
            "sig": "neutral",
            "paired_observations": 0,
            "directions_agree": True,
            "significance_reason": "requires paired observations",
        }

    baseline_values = [baseline for baseline, _feature in all_pairs]
    feature_values = [feature for _baseline, feature in all_pairs]
    baseline_value = counter_target_metric_stat_value(baseline_values, stat_name)
    feature_value = counter_target_metric_stat_value(feature_values, stat_name)
    diff = feature_value - baseline_value

    rng = random.Random(f"{query}:{stat_name}")
    boot_diffs = []
    for _ in range(BOOTSTRAP_ITERATIONS):
        sample = rng.choices(all_pairs, k=len(all_pairs))
        boot_diffs.append(target_metric_stat_diff(sample, stat_name))

    boot_diffs.sort()
    lo = int(BOOTSTRAP_ITERATIONS * 0.025)
    hi = int(BOOTSTRAP_ITERATIONS * 0.975)
    ci_within = (boot_diffs[hi] - boot_diffs[lo]) / 2
    ci_between = _between_pairing_ci(per_pairing_diffs)
    ci = max(ci_within, ci_between)

    pct = (diff / baseline_value * 100.0) if abs(baseline_value) > EPSILON else 0.0
    ci_pct = (ci / abs(baseline_value) * 100.0) if abs(baseline_value) > EPSILON else 0.0
    directions_agree = target_metric_directions_agree(per_pairing_diffs)
    sig = significance(
        pct,
        ci_pct,
        lower_is_better=target == "decrease",
        directions_agree=directions_agree,
    )
    significance_reason = None
    if len(all_pairs) < TARGET_METRIC_MIN_PAIRED_OBSERVATIONS:
        sig = "neutral"
        significance_reason = (
            f"requires at least {TARGET_METRIC_MIN_PAIRED_OBSERVATIONS} paired observations"
        )
    result = {
        "baseline": baseline_value,
        "feature": feature_value,
        "diff": round(diff, 6),
        "pct": round(pct, 4),
        "ci": round(ci, 6),
        "ci_pct": round(ci_pct, 4),
        "sig": sig,
        "paired_observations": len(all_pairs),
        "directions_agree": directions_agree,
    }
    if significance_reason:
        result["significance_reason"] = significance_reason
    return result


def query_counter_target_metric_run(
    relevant_scrapes: list[dict],
    counter: dict,
    run_label: str,
    metadata: dict,
) -> dict[tuple[tuple[str, str], ...], dict]:
    query = counter["query"]
    aggregate, _metric_name, _label_filters = parse_target_metric_query(query)
    grouped_scrapes = [group_query_samples_by_identity(scrape["samples"], query)[1] for scrape in relevant_scrapes]
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
) -> dict[tuple[tuple[str, str], ...], dict]:
    sum_query = histogram_counter_query(histogram["query"], "sum")
    count_query = histogram_counter_query(histogram["query"], "count")
    sum_grouped_scrapes = [group_query_samples_by_identity(scrape["samples"], sum_query)[1] for scrape in relevant_scrapes]
    count_grouped_scrapes = [group_query_samples_by_identity(scrape["samples"], count_query)[1] for scrape in relevant_scrapes]
    mean_identities = collect_metric_identities(sum_grouped_scrapes)
    mean_identities.update(collect_metric_identities(count_grouped_scrapes))

    metrics_by_identity = {}
    for identity_key, identity_labels in sorted(mean_identities.items()):
        mean_observations = []
        display_query = format_target_metric_identity(histogram["query"], identity_labels)
        for previous_scrape, current_scrape, previous_sum_groups, current_sum_groups, previous_count_groups, current_count_groups in zip(
            relevant_scrapes,
            relevant_scrapes[1:],
            sum_grouped_scrapes,
            sum_grouped_scrapes[1:],
            count_grouped_scrapes,
            count_grouped_scrapes[1:],
        ):
            current_sum = grouped_sample_value(current_sum_groups.get(identity_key), "single", allow_missing=True)
            previous_sum = grouped_sample_value(previous_sum_groups.get(identity_key), "single", allow_missing=True)
            current_count = grouped_sample_value(current_count_groups.get(identity_key), "single", allow_missing=True)
            previous_count = grouped_sample_value(previous_count_groups.get(identity_key), "single", allow_missing=True)
            current_block_height = float(current_scrape["block_height"])
            previous_block_height = float(previous_scrape["block_height"])
            if current_block_height - previous_block_height <= EPSILON:
                continue

            sum_delta = current_sum - previous_sum
            count_delta = current_count - previous_count
            if sum_delta < -EPSILON or count_delta < -EPSILON:
                raise ValueError(
                    f"Histogram target metric '{display_query}' sum/count decreased within run '{run_label}'"
                )
            if count_delta <= EPSILON:
                continue
            mean_observations.append(
                {
                    "block_height": current_block_height,
                    "value": sum_delta / count_delta,
                }
            )

        if not mean_observations:
            continue

        stats = compute_target_metric_series_stats(
            [observation["value"] for observation in mean_observations]
        )
        metrics_by_identity[identity_key] = {
            "query": histogram["query"],
            "display_query": display_query,
            "identity_labels": identity_labels,
            "target": histogram["target"],
            "mean": {
                "query": f"{sum_query} / {count_query}",
                "value": stats["mean"],
                "samples": len(mean_observations),
                "scrapes": len(relevant_scrapes),
                "duration_ms": int(metadata["duration_ms"]),
                "range_start_ms": int(metadata["range_start_ms"]),
                "range_end_ms": int(metadata["range_end_ms"]),
                "benchmark_id": metadata["benchmark_id"],
                "benchmark_run": metadata["benchmark_run"],
                "_values": [observation["value"] for observation in mean_observations],
                "_observations": mean_observations,
            },
        }

    return metrics_by_identity


def query_target_metric_run(path: str, config: dict) -> tuple[str, dict[str, dict[str, dict]]]:
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

    counters = {}
    for counter in config.get("counters", []):
        counters[counter["query"]] = query_counter_target_metric_run(
            relevant_scrapes, counter, run_label, metadata
        )

    histograms = {}
    for histogram in config.get("histograms", []):
        histograms[histogram["query"]] = query_histogram_target_metric_run(
            relevant_scrapes, histogram, run_label, metadata
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
    include_pair_mean_values: bool = False,
) -> dict:
    changes = {
        stat_name: compute_paired_target_metric_change(
            baseline_runs_by_stat[stat_name],
            feature_runs_by_stat[stat_name],
            display_query,
            target,
            stat_name,
        )
        for stat_name in display_stats
    }
    entry = {
        "kind": kind,
        "name": display_query,
        "query": display_query,
        "configured_query": configured_query,
        "identity_labels": identity_labels,
        "target": target,
        "display_stats": list(display_stats),
        "baseline": summarize_target_metric_runs(baseline_values, summary_fields),
        "feature": summarize_target_metric_runs(feature_values, summary_fields),
        "changes": changes,
        "change": summarize_target_metric_change(changes, display_stats),
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
) -> dict:
    with open(config_path) as f:
        config = json.load(f)

    baseline_runs = [query_target_metric_run(path, config) for path in baseline_csv_paths]
    feature_runs = [query_target_metric_run(path, config) for path in feature_csv_paths]

    metrics = []
    for counter in config.get("counters", []):
        query = counter["query"]
        target = counter["target"]
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
                    include_pair_mean_values=True,
                )
            )

    for histogram in config.get("histograms", []):
        query = histogram["query"]
        target = histogram["target"]
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
                )
            )

    changed = [metric for metric in metrics if metric["change"]["significant_stats"]]
    return {
        "config": config_path,
        "metrics": metrics,
        "changed": changed,
        "improvements": [metric["name"] for metric in changed if metric["change"]["sig"] == "good"],
        "regressions": [metric["name"] for metric in changed if metric["change"]["sig"] == "bad"],
    }


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
    driver: str | None = None,
    driver_reason: str | None = None,
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
        f"| Metric | {baseline_label} | {feature_label} | Change |",
        "|--------|------|--------|--------|",
        f"| P50 | {fmt_ms(run1['p50_ms'])} | {fmt_ms(run2['p50_ms'])} | {change_str(p50_pct, p50_ci_pct, lower_is_better=True, directions_agree=lat_agree)} |",
        f"| P90 | {fmt_ms(run1['p90_ms'])} | {fmt_ms(run2['p90_ms'])} | {change_str(p90_pct, p90_ci_pct, lower_is_better=True, directions_agree=lat_agree)} |",
        f"| P99 | {fmt_ms(run1['p99_ms'])} | {fmt_ms(run2['p99_ms'])} | {change_str(p99_pct, p99_ci_pct, lower_is_better=True, directions_agree=lat_agree)} |",
        f"| Mgas/s | {fmt_mgas(run1['mean_mgas_s'])} | {fmt_mgas(run2['mean_mgas_s'])} | {change_str(gas_pct, mgas_ci_pct, lower_is_better=False, directions_agree=mgas_agree)} |",
        f"| Wall Clock | {fmt_s(run1['wall_clock_s'])} | {fmt_s(run2['wall_clock_s'])} | {change_str(wall_pct, wall_ci_pct, lower_is_better=True, directions_agree=total_agree)} |",
        f"| Persist Wait | {fmt_ms(run1['mean_persist_ms'])} | {fmt_ms(run2['mean_persist_ms'])} | {change_str(persist_pct, persist_ci_pct, lower_is_better=True, directions_agree=persist_agree)} |",
        "",
    ]
    meta_parts = [f"{n} {'big blocks' if big_blocks else 'blocks'}"]
    if driver:
        driver_label = driver
        if driver_reason:
            driver_label += f" (fallback: {driver_reason})"
        meta_parts.append(f"driver: {driver_label}")
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
                    fmt_metric_value(change["baseline"]),
                    fmt_metric_value(change["feature"]),
                    change_str(
                        change["pct"],
                        change["ci_pct"],
                        metric["target"] == "decrease",
                        directions_agree=change.get("directions_agree", True),
                    ),
                )
            )

    if row_count == 0:
        return ""

    return "\n".join(lines)


def generate_markdown(
    summary: dict, comparison_table: str,
    wait_time_tables: list[str] | None = None,
    target_metric_table: str = "",
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
    if grafana_url:
        lines.append("")
        lines.append(f"**[Grafana Dashboard]({grafana_url})**")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Parse reth-bench ABBA results")
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
    parser.add_argument("--driver", default=None, help="Benchmark driver used for this run")
    parser.add_argument("--driver-reason", default=None, help="Why the benchmark fell back to this driver")
    parser.add_argument("--grafana-url", default=None, help="Grafana dashboard URL for this benchmark run")
    parser.add_argument("--target-metrics-config", default=None, help="Target metrics config path")
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
        driver=args.driver,
        driver_reason=args.driver_reason,
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
        )
        target_metric_table = generate_target_metric_table(target_metric_summary)

    summary = {
        "blocks": paired_stats["blocks"],
        "driver": args.driver,
        "driver_reason": args.driver_reason,
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
        "changes": compute_changes(baseline_stats, feature_stats, paired_stats),
        "wait_times": wait_time_data,
    }
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
        grafana_url=args.grafana_url,
    )

    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")


if __name__ == "__main__":
    main()
