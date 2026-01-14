#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.8"
# dependencies = []
# ///

"""
Analyze reth persistence logs to extract average block save times.

Usage:
    uv run analyze_persistence_logs.py <baseline_log> <feature_log>

Parses:
1. "Saving range of blocks" / "Saved range of blocks" pairs (DEBUG level)
2. "save_blocks breakdown" entries (DEBUG level) for SF vs MDBX timing
"""

import re
import sys
import subprocess
from datetime import datetime
from collections import defaultdict


def grep_lines(log_file: str, pattern: str) -> list[str]:
    """Use grep to efficiently filter log lines from large files."""
    try:
        result = subprocess.run(
            ["grep", "-E", pattern, log_file],
            capture_output=True,
            text=True,
            check=False
        )
        return result.stdout.strip().split('\n') if result.stdout.strip() else []
    except Exception as e:
        print(f"Error grepping {log_file}: {e}", file=sys.stderr)
        return []


def parse_timestamp(ts_str: str) -> datetime:
    """Parse ISO timestamp from log line."""
    return datetime.fromisoformat(ts_str.replace('Z', '+00:00'))


def parse_block_number(text: str, field: str) -> int | None:
    """Extract block number from first= or last= field."""
    pattern = rf'{field}=Some\(NumHash \{{ number: (\d+)'
    match = re.search(pattern, text)
    if match:
        return int(match.group(1))
    return None


def parse_block_count(text: str) -> int | None:
    """Extract block_count from log line."""
    match = re.search(r'block_count=(\d+)', text)
    if match:
        return int(match.group(1))
    return None


def parse_duration_field(text: str, field: str) -> float | None:
    """Extract duration in ms from field like 'field=123.456ms' or 'field=Some(123.456ms)'."""
    # Handle both direct values and Some(...) wrapped values
    # Try milliseconds first: field=123.456ms or field=Some(123.456ms)
    pattern = rf'{field}=(?:Some\()?(\d+\.?\d*)ms\)?'
    match = re.search(pattern, text)
    if match:
        return float(match.group(1))

    # Try seconds: field=1.23s or field=Some(1.23s)
    pattern = rf'{field}=(?:Some\()?(\d+\.?\d*)s(?![\w])\)?'
    match = re.search(pattern, text)
    if match:
        return float(match.group(1)) * 1000

    # Try microseconds: field=123.456µs or field=Some(123.456µs)
    pattern = rf'{field}=(?:Some\()?(\d+\.?\d*)[µu]s\)?'
    match = re.search(pattern, text)
    if match:
        return float(match.group(1)) / 1000

    # Try nanoseconds: field=123ns or field=Some(123ns)
    pattern = rf'{field}=(?:Some\()?(\d+\.?\d*)ns\)?'
    match = re.search(pattern, text)
    if match:
        return float(match.group(1)) / 1_000_000

    return None


def analyze_high_level(log_file: str) -> dict:
    """Analyze Saving/Saved pairs for high-level timing."""
    lines = grep_lines(log_file, "engine::persistence.*range of blocks")

    if not lines:
        return {"error": f"No persistence log lines found in {log_file}"}

    saving_entries = {}
    per_block_times_ms = []
    total_blocks = 0
    total_time_ms = 0

    for line in lines:
        if not line.strip():
            continue

        try:
            parts = line.split(' ', 1)
            if len(parts) < 2:
                continue

            ts = parse_timestamp(parts[0])
            first_block = parse_block_number(line, 'first')
            last_block = parse_block_number(line, 'last')

            if first_block is None or last_block is None:
                continue

            key = (first_block, last_block)

            if "Saving range of blocks" in line:
                block_count = parse_block_count(line)
                if block_count:
                    saving_entries[key] = (ts, block_count)

            elif "Saved range of blocks" in line:
                if key in saving_entries:
                    start_ts, block_count = saving_entries[key]
                    duration = (ts - start_ts).total_seconds() * 1000

                    per_block_time = duration / block_count
                    per_block_times_ms.append(per_block_time)
                    total_blocks += block_count
                    total_time_ms += duration

                    del saving_entries[key]

        except Exception:
            continue

    if not per_block_times_ms:
        return {"error": f"No matched Saving/Saved pairs found in {log_file}"}

    per_block_times_ms.sort()
    n = len(per_block_times_ms)

    return {
        "file": log_file,
        "matched_pairs": n,
        "total_blocks": total_blocks,
        "total_time_ms": total_time_ms,
        "avg_per_block_ms": sum(per_block_times_ms) / n,
        "p50_per_block_ms": per_block_times_ms[int(n * 0.50)],
        "p90_per_block_ms": per_block_times_ms[int(n * 0.90)] if n >= 10 else per_block_times_ms[-1],
        "min_per_block_ms": min(per_block_times_ms),
        "max_per_block_ms": max(per_block_times_ms),
    }


def analyze_commit_breakdown(log_file: str) -> dict:
    """Analyze commit breakdown DEBUG logs for SF finalize vs MDBX commit timing."""
    lines = grep_lines(log_file, "commit breakdown")

    if not lines:
        return {"error": "No commit breakdown logs found (need DEBUG level)"}

    entries = []
    for line in lines:
        if not line.strip():
            continue
        try:
            entry = {
                "sf_ms": parse_duration_field(line, "sf_elapsed"),
                "mdbx_ms": parse_duration_field(line, "mdbx_elapsed"),
                "is_unwind": "unwind path" in line,
            }
            if entry["sf_ms"] is not None or entry["mdbx_ms"] is not None:
                entries.append(entry)
        except Exception:
            continue

    if not entries:
        return {"error": "No valid commit breakdown entries parsed"}

    # Aggregate stats helper
    def calc_stats(values):
        valid = [v for v in values if v is not None]
        if not valid:
            return None
        valid.sort()
        n = len(valid)
        return {
            "avg": sum(valid) / n,
            "p50": valid[int(n * 0.50)],
            "p90": valid[int(n * 0.90)] if n >= 10 else valid[-1],
            "min": min(valid),
            "max": max(valid),
            "total": sum(valid),
        }

    normal_entries = [e for e in entries if not e["is_unwind"]]
    unwind_entries = [e for e in entries if e["is_unwind"]]

    return {
        "file": log_file,
        "total_commits": len(entries),
        "normal_commits": len(normal_entries),
        "unwind_commits": len(unwind_entries),
        "sf_finalize": calc_stats([e["sf_ms"] for e in normal_entries]),
        "mdbx_commit": calc_stats([e["mdbx_ms"] for e in normal_entries]),
    }


def analyze_breakdown(log_file: str) -> dict:
    """Analyze save_blocks breakdown DEBUG logs for SF vs MDBX timing."""
    lines = grep_lines(log_file, "save_blocks breakdown")

    if not lines:
        return {"error": "No breakdown logs found (need DEBUG level)"}

    # Collect per-entry data
    entries = []

    for line in lines:
        if not line.strip():
            continue

        try:
            block_count = parse_block_count(line)
            if not block_count:
                continue

            entry = {
                "block_count": block_count,
                "mdbx_ms": parse_duration_field(line, "mdbx_elapsed"),
                "sf_ms": parse_duration_field(line, "sf_elapsed"),
                "sf_join_ms": parse_duration_field(line, "sf_join_elapsed"),
                "rocksdb_ms": parse_duration_field(line, "rocksdb_elapsed"),
                "rocksdb_join_ms": parse_duration_field(line, "rocksdb_join_elapsed"),
                "insert_block_ms": parse_duration_field(line, "total_insert_block"),
                "write_state_ms": parse_duration_field(line, "total_write_state"),
                "write_hashed_state_ms": parse_duration_field(line, "total_write_hashed_state"),
                "write_trie_changesets_ms": parse_duration_field(line, "total_write_trie_changesets"),
                "write_trie_updates_ms": parse_duration_field(line, "total_write_trie_updates"),
                "update_history_ms": parse_duration_field(line, "duration_update_history_indices"),
            }
            entries.append(entry)

        except Exception:
            continue

    if not entries:
        return {"error": "No valid breakdown entries parsed"}

    # Aggregate stats
    def calc_stats(values):
        valid = [v for v in values if v is not None]
        if not valid:
            return None
        valid.sort()
        n = len(valid)
        return {
            "avg": sum(valid) / n,
            "p50": valid[int(n * 0.50)],
            "p90": valid[int(n * 0.90)] if n >= 10 else valid[-1],
            "min": min(valid),
            "max": max(valid),
            "total": sum(valid),
        }

    # Per-block normalization
    mdbx_per_block = [e["mdbx_ms"] / e["block_count"] for e in entries if e["mdbx_ms"]]
    sf_per_block = [e["sf_ms"] / e["block_count"] for e in entries if e["sf_ms"]]
    sf_join_per_block = [e["sf_join_ms"] / e["block_count"] for e in entries if e["sf_join_ms"]]
    rocksdb_per_block = [e["rocksdb_ms"] / e["block_count"] for e in entries if e["rocksdb_ms"]]
    rocksdb_join_per_block = [e["rocksdb_join_ms"] / e["block_count"] for e in entries if e["rocksdb_join_ms"]]

    # Count how often SF vs MDBX was the bottleneck
    sf_slower = sum(1 for e in entries if e["sf_ms"] and e["mdbx_ms"] and e["sf_ms"] > e["mdbx_ms"])
    mdbx_slower = sum(1 for e in entries if e["sf_ms"] and e["mdbx_ms"] and e["mdbx_ms"] > e["sf_ms"])

    return {
        "file": log_file,
        "entries": len(entries),
        "total_blocks": sum(e["block_count"] for e in entries),
        "mdbx_per_block": calc_stats(mdbx_per_block),
        "sf_per_block": calc_stats(sf_per_block),
        "sf_join_per_block": calc_stats(sf_join_per_block),
        "rocksdb_per_block": calc_stats(rocksdb_per_block),
        "rocksdb_join_per_block": calc_stats(rocksdb_join_per_block),
        "sf_slower_count": sf_slower,
        "mdbx_slower_count": mdbx_slower,
        # Raw totals for MDBX breakdown
        "insert_block": calc_stats([e["insert_block_ms"] for e in entries]),
        "write_state": calc_stats([e["write_state_ms"] for e in entries]),
        "write_hashed_state": calc_stats([e["write_hashed_state_ms"] for e in entries]),
        "write_trie_changesets": calc_stats([e["write_trie_changesets_ms"] for e in entries]),
        "write_trie_updates": calc_stats([e["write_trie_updates_ms"] for e in entries]),
        "update_history": calc_stats([e["update_history_ms"] for e in entries]),
    }


def print_high_level_stats(label: str, stats: dict):
    """Print high-level statistics."""
    print(f"\n{'=' * 60}")
    print(f"{label}: {stats.get('file', 'N/A')}")
    print('=' * 60)

    if "error" in stats:
        print(f"  ERROR: {stats['error']}")
        return

    print(f"  Matched save pairs:    {stats['matched_pairs']}")
    print(f"  Total blocks saved:    {stats['total_blocks']}")
    print(f"  Total time:            {stats['total_time_ms']:.2f} ms")
    print(f"  ")
    print(f"  Avg per block:         {stats['avg_per_block_ms']:.3f} ms")
    print(f"  P50 per block:         {stats['p50_per_block_ms']:.3f} ms")
    print(f"  P90 per block:         {stats['p90_per_block_ms']:.3f} ms")
    print(f"  Min per block:         {stats['min_per_block_ms']:.3f} ms")
    print(f"  Max per block:         {stats['max_per_block_ms']:.3f} ms")


def print_breakdown_stats(label: str, stats: dict):
    """Print breakdown statistics."""
    print(f"\n{'-' * 60}")
    print(f"{label} BREAKDOWN (DEBUG)")
    print('-' * 60)

    if "error" in stats:
        print(f"  (not available - {stats['error']})")
        return

    print(f"  Entries analyzed:      {stats['entries']}")
    print(f"  Total blocks:          {stats['total_blocks']}")
    print()

    # Bottleneck analysis
    total = stats['sf_slower_count'] + stats['mdbx_slower_count']
    if total > 0:
        sf_pct = stats['sf_slower_count'] / total * 100
        mdbx_pct = stats['mdbx_slower_count'] / total * 100
        print(f"  Bottleneck analysis:")
        print(f"    SF slower:           {stats['sf_slower_count']} ({sf_pct:.1f}%)")
        print(f"    MDBX slower:         {stats['mdbx_slower_count']} ({mdbx_pct:.1f}%)")
        print()

    # Per-block times
    if stats['mdbx_per_block']:
        s = stats['mdbx_per_block']
        print(f"  MDBX per block:        avg={s['avg']:.3f}ms  p50={s['p50']:.3f}ms  p90={s['p90']:.3f}ms  max={s['max']:.3f}ms")
    if stats['sf_per_block']:
        s = stats['sf_per_block']
        print(f"  SF per block:          avg={s['avg']:.3f}ms  p50={s['p50']:.3f}ms  p90={s['p90']:.3f}ms  max={s['max']:.3f}ms")
    if stats.get('sf_join_per_block'):
        s = stats['sf_join_per_block']
        print(f"  SF join per block:     avg={s['avg']:.3f}ms  p50={s['p50']:.3f}ms  p90={s['p90']:.3f}ms  max={s['max']:.3f}ms")
    if stats.get('rocksdb_per_block'):
        s = stats['rocksdb_per_block']
        print(f"  RocksDB per block:     avg={s['avg']:.3f}ms  p50={s['p50']:.3f}ms  p90={s['p90']:.3f}ms  max={s['max']:.3f}ms")
    if stats.get('rocksdb_join_per_block'):
        s = stats['rocksdb_join_per_block']
        print(f"  RocksDB join/block:    avg={s['avg']:.3f}ms  p50={s['p50']:.3f}ms  p90={s['p90']:.3f}ms  max={s['max']:.3f}ms")

    # MDBX breakdown
    print()
    print("  MDBX breakdown (per save_blocks call):")
    for name, key in [
        ("insert_block", "insert_block"),
        ("write_state", "write_state"),
        ("write_hashed_state", "write_hashed_state"),
        ("write_trie_changesets", "write_trie_changesets"),
        ("write_trie_updates", "write_trie_updates"),
        ("update_history", "update_history"),
    ]:
        if stats.get(key):
            s = stats[key]
            print(f"    {name:24s} avg={s['avg']:7.2f}ms  p50={s['p50']:7.2f}ms  p90={s['p90']:7.2f}ms")


def print_commit_breakdown_stats(label: str, stats: dict):
    """Print commit breakdown statistics."""
    print(f"\n{'-' * 60}")
    print(f"{label} COMMIT BREAKDOWN (DEBUG)")
    print('-' * 60)

    if "error" in stats:
        print(f"  (not available - {stats['error']})")
        return

    print(f"  Total commits:         {stats['total_commits']}")
    print(f"  Normal path:           {stats['normal_commits']}")
    print(f"  Unwind path:           {stats['unwind_commits']}")
    print()

    if stats['sf_finalize']:
        s = stats['sf_finalize']
        print(f"  SF finalize:           avg={s['avg']:.3f}ms  p50={s['p50']:.3f}ms  p90={s['p90']:.3f}ms  max={s['max']:.3f}ms")
    if stats['mdbx_commit']:
        s = stats['mdbx_commit']
        print(f"  MDBX commit:           avg={s['avg']:.3f}ms  p50={s['p50']:.3f}ms  p90={s['p90']:.3f}ms  max={s['max']:.3f}ms")


def print_comparison(baseline: dict, feature: dict, baseline_bd: dict, feature_bd: dict, baseline_commit: dict, feature_commit: dict):
    """Print comparison between baseline and feature."""
    print(f"\n{'=' * 60}")
    print("COMPARISON")
    print('=' * 60)

    def pct(b, f):
        return ((f - b) / b) * 100 if b > 0 else 0

    if "error" not in baseline and "error" not in feature:
        print(f"\n  High-level (Saving→Saved):")
        print(f"    avg:  baseline={baseline['avg_per_block_ms']:7.3f}ms  feature={feature['avg_per_block_ms']:7.3f}ms  change={pct(baseline['avg_per_block_ms'], feature['avg_per_block_ms']):+.2f}%")
        print(f"    p50:  baseline={baseline['p50_per_block_ms']:7.3f}ms  feature={feature['p50_per_block_ms']:7.3f}ms  change={pct(baseline['p50_per_block_ms'], feature['p50_per_block_ms']):+.2f}%")
        print(f"    p90:  baseline={baseline['p90_per_block_ms']:7.3f}ms  feature={feature['p90_per_block_ms']:7.3f}ms  change={pct(baseline['p90_per_block_ms'], feature['p90_per_block_ms']):+.2f}%")

    # Breakdown comparison (only if feature has data - baseline may not have TRACE logs)
    if "error" not in feature_bd:
        print(f"\n  save_blocks breakdown (per block):")

        has_baseline = "error" not in baseline_bd

        for name, key in [("MDBX", "mdbx_per_block"), ("SF", "sf_per_block"), ("Join", "join_per_block")]:
            f = feature_bd.get(key)
            b = baseline_bd.get(key) if has_baseline else None

            if f:
                print(f"    {name}:")
                if b:
                    print(f"      avg:  baseline={b['avg']:7.3f}ms  feature={f['avg']:7.3f}ms  change={pct(b['avg'], f['avg']):+.2f}%")
                    print(f"      p50:  baseline={b['p50']:7.3f}ms  feature={f['p50']:7.3f}ms  change={pct(b['p50'], f['p50']):+.2f}%")
                    print(f"      p90:  baseline={b['p90']:7.3f}ms  feature={f['p90']:7.3f}ms  change={pct(b['p90'], f['p90']):+.2f}%")
                else:
                    print(f"      avg:  feature={f['avg']:7.3f}ms  (no baseline data)")
                    print(f"      p50:  feature={f['p50']:7.3f}ms")
                    print(f"      p90:  feature={f['p90']:7.3f}ms")

    # Commit breakdown comparison (only if feature has data)
    if "error" not in feature_commit:
        print(f"\n  commit breakdown:")

        has_baseline = "error" not in baseline_commit

        for name, key in [("SF finalize", "sf_finalize"), ("MDBX commit", "mdbx_commit")]:
            f = feature_commit.get(key)
            b = baseline_commit.get(key) if has_baseline else None

            if f:
                print(f"    {name}:")
                if b:
                    print(f"      avg:  baseline={b['avg']:7.3f}ms  feature={f['avg']:7.3f}ms  change={pct(b['avg'], f['avg']):+.2f}%")
                    print(f"      p50:  baseline={b['p50']:7.3f}ms  feature={f['p50']:7.3f}ms  change={pct(b['p50'], f['p50']):+.2f}%")
                    print(f"      p90:  baseline={b['p90']:7.3f}ms  feature={f['p90']:7.3f}ms  change={pct(b['p90'], f['p90']):+.2f}%")
                else:
                    print(f"      avg:  feature={f['avg']:7.3f}ms  (no baseline data)")
                    print(f"      p50:  feature={f['p50']:7.3f}ms")
                    print(f"      p90:  feature={f['p90']:7.3f}ms")


def main():
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <baseline_log> <feature_log>", file=sys.stderr)
        sys.exit(1)

    baseline_log = sys.argv[1]
    feature_log = sys.argv[2]

    print("Analyzing persistence logs...")
    print(f"  Baseline: {baseline_log}")
    print(f"  Feature:  {feature_log}")

    # High-level analysis (DEBUG logs)
    baseline_stats = analyze_high_level(baseline_log)
    feature_stats = analyze_high_level(feature_log)

    print_high_level_stats("BASELINE", baseline_stats)
    print_high_level_stats("FEATURE", feature_stats)

    # Breakdown analysis (TRACE logs)
    baseline_breakdown = analyze_breakdown(baseline_log)
    feature_breakdown = analyze_breakdown(feature_log)

    print_breakdown_stats("BASELINE", baseline_breakdown)
    print_breakdown_stats("FEATURE", feature_breakdown)

    # Commit breakdown analysis (TRACE logs)
    baseline_commit = analyze_commit_breakdown(baseline_log)
    feature_commit = analyze_commit_breakdown(feature_log)

    print_commit_breakdown_stats("BASELINE", baseline_commit)
    print_commit_breakdown_stats("FEATURE", feature_commit)

    # Comparison
    print_comparison(baseline_stats, feature_stats, baseline_breakdown, feature_breakdown, baseline_commit, feature_commit)


if __name__ == "__main__":
    main()
