#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.8"
# dependencies = [
#     "pandas",
#     "matplotlib",
# ]
# ///

"""
Deep analysis of reth persistence logs - correlations, bottlenecks, and breakdowns.

Usage:
    uv run analyze_persistence_deep.py <log_file> [-o output_dir]

Analyzes:
1. Per save_blocks call breakdown (SF vs MDBX parallel work + commit)
2. Bottleneck analysis - which component is slowing things down
3. Correlation between SF and MDBX times
4. Time distribution charts
"""

import re
import sys
import subprocess
import argparse
from datetime import datetime
from pathlib import Path

import pandas as pd
import matplotlib.pyplot as plt


def grep_lines(log_file: str, pattern: str) -> list[str]:
    """Use grep to efficiently filter log lines."""
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


def parse_duration(text: str, field: str) -> float | None:
    """Extract duration in ms from field like 'field=123.456ms'."""
    # Milliseconds
    match = re.search(rf'{field}=(\d+\.?\d*)ms', text)
    if match:
        return float(match.group(1))
    # Seconds
    match = re.search(rf'{field}=(\d+\.?\d*)s(?![\w])', text)
    if match:
        return float(match.group(1)) * 1000
    # Microseconds
    match = re.search(rf'{field}=(\d+\.?\d*)[µu]s', text)
    if match:
        return float(match.group(1)) / 1000
    # Nanoseconds
    match = re.search(rf'{field}=(\d+\.?\d*)ns', text)
    if match:
        return float(match.group(1)) / 1_000_000
    return None


def parse_int(text: str, field: str) -> int | None:
    """Extract integer from field like 'field=123'."""
    match = re.search(rf'{field}=(\d+)', text)
    return int(match.group(1)) if match else None


def parse_save_blocks_entries(log_file: str) -> pd.DataFrame:
    """Parse save_blocks breakdown DEBUG logs into a DataFrame."""
    lines = grep_lines(log_file, "save_blocks breakdown")

    entries = []
    for line in lines:
        if not line.strip():
            continue
        entry = {
            "block_count": parse_int(line, "block_count"),
            "mdbx_ms": parse_duration(line, "mdbx_elapsed"),
            "sf_ms": parse_duration(line, "sf_elapsed"),
            "join_ms": parse_duration(line, "join_elapsed"),
            "insert_block_ms": parse_duration(line, "total_insert_block"),
            "write_state_ms": parse_duration(line, "total_write_state"),
            "write_hashed_state_ms": parse_duration(line, "total_write_hashed_state"),
            "write_trie_changesets_ms": parse_duration(line, "total_write_trie_changesets"),
            "write_trie_updates_ms": parse_duration(line, "total_write_trie_updates"),
            "update_history_ms": parse_duration(line, "duration_update_history_indices"),
        }
        if entry["block_count"]:
            entries.append(entry)

    return pd.DataFrame(entries)


def parse_commit_entries(log_file: str) -> pd.DataFrame:
    """Parse commit breakdown DEBUG logs into a DataFrame."""
    lines = grep_lines(log_file, "commit breakdown")

    entries = []
    for line in lines:
        if not line.strip():
            continue
        entry = {
            "sf_ms": parse_duration(line, "sf_elapsed"),
            "mdbx_ms": parse_duration(line, "mdbx_elapsed"),
            "is_unwind": "unwind path" in line,
        }
        if entry["sf_ms"] is not None or entry["mdbx_ms"] is not None:
            entries.append(entry)

    return pd.DataFrame(entries)


def calc_percentiles(series: pd.Series) -> dict:
    """Calculate statistics for a series."""
    s = series.dropna()
    if len(s) == 0:
        return {}
    return {
        "count": len(s),
        "avg": s.mean(),
        "p50": s.quantile(0.50),
        "p90": s.quantile(0.90),
        "p99": s.quantile(0.99),
        "min": s.min(),
        "max": s.max(),
        "std": s.std(),
    }


def print_section(title: str):
    """Print a section header."""
    print(f"\n{'=' * 70}")
    print(f" {title}")
    print('=' * 70)


def print_subsection(title: str):
    """Print a subsection header."""
    print(f"\n{'-' * 50}")
    print(f" {title}")
    print('-' * 50)


def print_stats_row(name: str, stats: dict, indent: int = 2):
    """Print a row of statistics."""
    if not stats:
        print(f"{' ' * indent}{name:28s} (no data)")
        return
    print(f"{' ' * indent}{name:28s} avg={stats['avg']:8.2f}ms  p50={stats['p50']:8.2f}ms  p90={stats['p90']:8.2f}ms  p99={stats['p99']:8.2f}ms  max={stats['max']:8.2f}ms")


def analyze_bottlenecks(df: pd.DataFrame) -> dict:
    """Analyze which component is the bottleneck in save_blocks."""
    if df.empty or 'mdbx_ms' not in df or 'sf_ms' not in df:
        return {}

    valid = df.dropna(subset=['mdbx_ms', 'sf_ms'])
    if valid.empty:
        return {}

    # Who finished last? (higher time = bottleneck)
    sf_slower = (valid['sf_ms'] > valid['mdbx_ms']).sum()
    mdbx_slower = (valid['mdbx_ms'] > valid['sf_ms']).sum()
    equal = (valid['mdbx_ms'] == valid['sf_ms']).sum()

    # Join time indicates wait - if join > 0, we waited for SF
    # (SF is spawned first, MDBX runs in main thread, join waits for SF)
    if 'join_ms' in valid:
        significant_wait = (valid['join_ms'] > 1.0).sum()  # > 1ms wait
    else:
        significant_wait = 0

    # Calculate how much time was "wasted" waiting
    total_parallel_time = valid['mdbx_ms'].sum() + valid['sf_ms'].sum()
    actual_wall_time = valid[['mdbx_ms', 'sf_ms']].max(axis=1).sum()
    parallelism_efficiency = (total_parallel_time / actual_wall_time / 2) * 100 if actual_wall_time > 0 else 0

    return {
        "sf_slower_count": sf_slower,
        "mdbx_slower_count": mdbx_slower,
        "equal_count": equal,
        "total": len(valid),
        "sf_slower_pct": sf_slower / len(valid) * 100,
        "mdbx_slower_pct": mdbx_slower / len(valid) * 100,
        "significant_wait_count": significant_wait,
        "parallelism_efficiency": parallelism_efficiency,
    }


def analyze_mdbx_breakdown(df: pd.DataFrame) -> dict:
    """Analyze the breakdown of MDBX operations."""
    components = [
        ("insert_block_ms", "insert_block"),
        ("write_state_ms", "write_state"),
        ("write_hashed_state_ms", "write_hashed_state"),
        ("write_trie_changesets_ms", "write_trie_changesets"),
        ("write_trie_updates_ms", "write_trie_updates"),
        ("update_history_ms", "update_history"),
    ]

    result = {}
    for col, name in components:
        if col in df:
            result[name] = calc_percentiles(df[col])

    # Calculate percentage contribution of each component
    if 'mdbx_ms' in df:
        total_mdbx = df['mdbx_ms'].sum()
        if total_mdbx > 0:
            for col, name in components:
                if col in df and name in result:
                    component_total = df[col].sum()
                    result[name]['pct_of_mdbx'] = (component_total / total_mdbx) * 100

    return result


def analyze_correlation(df: pd.DataFrame) -> dict:
    """Analyze correlation between SF and MDBX times."""
    if df.empty:
        return {}

    result = {}

    # SF vs MDBX correlation
    if 'sf_ms' in df and 'mdbx_ms' in df:
        valid = df[['sf_ms', 'mdbx_ms']].dropna()
        if len(valid) > 10:
            result['sf_mdbx_correlation'] = valid['sf_ms'].corr(valid['mdbx_ms'])

    # Block count vs time correlations
    if 'block_count' in df and 'mdbx_ms' in df:
        valid = df[['block_count', 'mdbx_ms']].dropna()
        if len(valid) > 10:
            result['block_count_mdbx_correlation'] = valid['block_count'].corr(valid['mdbx_ms'])

    if 'block_count' in df and 'sf_ms' in df:
        valid = df[['block_count', 'sf_ms']].dropna()
        if len(valid) > 10:
            result['block_count_sf_correlation'] = valid['block_count'].corr(valid['sf_ms'])

    return result


def plot_analysis(df_save: pd.DataFrame, df_commit: pd.DataFrame, output_dir: Path):
    """Generate analysis charts."""
    output_dir.mkdir(parents=True, exist_ok=True)

    # Figure 1: SF vs MDBX scatter + histograms
    if not df_save.empty and 'sf_ms' in df_save and 'mdbx_ms' in df_save:
        fig, axes = plt.subplots(2, 2, figsize=(14, 12))

        valid = df_save[['sf_ms', 'mdbx_ms', 'join_ms']].dropna()

        # Scatter: SF vs MDBX
        ax = axes[0, 0]
        ax.scatter(valid['mdbx_ms'], valid['sf_ms'], alpha=0.5, s=10)
        max_val = max(valid['mdbx_ms'].max(), valid['sf_ms'].max())
        ax.plot([0, max_val], [0, max_val], 'r--', label='SF = MDBX')
        ax.set_xlabel('MDBX time (ms)')
        ax.set_ylabel('SF time (ms)')
        ax.set_title('SF vs MDBX Time (points above line = SF slower)')
        ax.legend()
        ax.grid(True, alpha=0.3)

        # Histogram: MDBX
        ax = axes[0, 1]
        ax.hist(valid['mdbx_ms'], bins=50, alpha=0.7, edgecolor='black')
        ax.axvline(valid['mdbx_ms'].median(), color='r', linestyle='--', label=f"p50={valid['mdbx_ms'].median():.1f}ms")
        ax.axvline(valid['mdbx_ms'].quantile(0.90), color='orange', linestyle='--', label=f"p90={valid['mdbx_ms'].quantile(0.90):.1f}ms")
        ax.set_xlabel('MDBX time (ms)')
        ax.set_ylabel('Count')
        ax.set_title('MDBX Time Distribution')
        ax.legend()
        ax.grid(True, alpha=0.3)

        # Histogram: SF
        ax = axes[1, 0]
        ax.hist(valid['sf_ms'], bins=50, alpha=0.7, edgecolor='black', color='green')
        ax.axvline(valid['sf_ms'].median(), color='r', linestyle='--', label=f"p50={valid['sf_ms'].median():.1f}ms")
        ax.axvline(valid['sf_ms'].quantile(0.90), color='orange', linestyle='--', label=f"p90={valid['sf_ms'].quantile(0.90):.1f}ms")
        ax.set_xlabel('SF time (ms)')
        ax.set_ylabel('Count')
        ax.set_title('SF Time Distribution')
        ax.legend()
        ax.grid(True, alpha=0.3)

        # Histogram: Join wait
        ax = axes[1, 1]
        if 'join_ms' in valid and valid['join_ms'].sum() > 0:
            ax.hist(valid['join_ms'], bins=50, alpha=0.7, edgecolor='black', color='purple')
            ax.axvline(valid['join_ms'].median(), color='r', linestyle='--', label=f"p50={valid['join_ms'].median():.2f}ms")
            ax.set_xlabel('Join wait time (ms)')
            ax.set_ylabel('Count')
            ax.set_title('Join Wait Distribution (time waiting for SF after MDBX done)')
            ax.legend()
        else:
            ax.text(0.5, 0.5, 'No significant join wait times', ha='center', va='center', transform=ax.transAxes)
            ax.set_title('Join Wait Distribution')
        ax.grid(True, alpha=0.3)

        plt.tight_layout()
        plt.savefig(output_dir / 'sf_vs_mdbx.png', dpi=150)
        plt.close()
        print(f"  Saved: {output_dir / 'sf_vs_mdbx.png'}")

    # Figure 2: MDBX breakdown stacked
    if not df_save.empty:
        components = ['insert_block_ms', 'write_state_ms', 'write_hashed_state_ms',
                      'write_trie_changesets_ms', 'write_trie_updates_ms', 'update_history_ms']
        available = [c for c in components if c in df_save and df_save[c].notna().any()]

        if available:
            fig, axes = plt.subplots(1, 2, figsize=(14, 6))

            # Pie chart of total time
            ax = axes[0]
            totals = [df_save[c].sum() for c in available]
            labels = [c.replace('_ms', '').replace('_', ' ') for c in available]
            ax.pie(totals, labels=labels, autopct='%1.1f%%', startangle=90)
            ax.set_title('MDBX Time Breakdown (% of total)')

            # Box plot
            ax = axes[1]
            data = [df_save[c].dropna() for c in available]
            ax.boxplot(data, labels=[c.replace('_ms', '').replace('_', '\n') for c in available])
            ax.set_ylabel('Time (ms)')
            ax.set_title('MDBX Component Time Distribution')
            ax.grid(True, alpha=0.3)

            plt.tight_layout()
            plt.savefig(output_dir / 'mdbx_breakdown.png', dpi=150)
            plt.close()
            print(f"  Saved: {output_dir / 'mdbx_breakdown.png'}")

    # Figure 3: Commit breakdown
    if not df_commit.empty and 'sf_ms' in df_commit and 'mdbx_ms' in df_commit:
        normal = df_commit[~df_commit['is_unwind']]
        if not normal.empty:
            fig, axes = plt.subplots(1, 2, figsize=(12, 5))

            ax = axes[0]
            ax.hist(normal['sf_ms'].dropna(), bins=30, alpha=0.7, label='SF finalize', color='green')
            ax.hist(normal['mdbx_ms'].dropna(), bins=30, alpha=0.7, label='MDBX commit', color='blue')
            ax.set_xlabel('Time (ms)')
            ax.set_ylabel('Count')
            ax.set_title('Commit Time Distribution')
            ax.legend()
            ax.grid(True, alpha=0.3)

            ax = axes[1]
            sf_total = normal['sf_ms'].sum()
            mdbx_total = normal['mdbx_ms'].sum()
            ax.bar(['SF finalize', 'MDBX commit'], [sf_total, mdbx_total], color=['green', 'blue'])
            ax.set_ylabel('Total time (ms)')
            ax.set_title('Total Commit Time by Component')
            ax.grid(True, alpha=0.3)

            plt.tight_layout()
            plt.savefig(output_dir / 'commit_breakdown.png', dpi=150)
            plt.close()
            print(f"  Saved: {output_dir / 'commit_breakdown.png'}")


def main():
    parser = argparse.ArgumentParser(description='Deep analysis of reth persistence logs')
    parser.add_argument('log_file', help='Path to reth log file with DEBUG level')
    parser.add_argument('-o', '--output', default='./persistence_analysis', help='Output directory for charts')
    parser.add_argument('--no-charts', action='store_true', help='Skip chart generation')
    args = parser.parse_args()

    print(f"Analyzing: {args.log_file}")

    # Parse data
    print("\nParsing log entries...")
    df_save = parse_save_blocks_entries(args.log_file)
    df_commit = parse_commit_entries(args.log_file)

    print(f"  save_blocks entries: {len(df_save)}")
    print(f"  commit entries:      {len(df_commit)}")

    if df_save.empty and df_commit.empty:
        print("\nNo DEBUG data found. Make sure logging includes: --log.file.filter 'providers::db=debug'")
        sys.exit(1)

    # ==================== SAVE_BLOCKS ANALYSIS ====================
    if not df_save.empty:
        print_section("SAVE_BLOCKS ANALYSIS")

        # Block count stats
        print_subsection("Batch Size Statistics")
        bc_stats = calc_percentiles(df_save['block_count'])
        print(f"  Total save_blocks calls: {len(df_save)}")
        print(f"  Total blocks:            {df_save['block_count'].sum():.0f}")
        print(f"  Blocks per call:         avg={bc_stats['avg']:6.1f}  p50={bc_stats['p50']:6.1f}  p90={bc_stats['p90']:6.1f}  min={bc_stats['min']:6.0f}  max={bc_stats['max']:6.0f}")

        # Normalize to per-block
        df_save['mdbx_per_block'] = df_save['mdbx_ms'] / df_save['block_count']
        df_save['sf_per_block'] = df_save['sf_ms'] / df_save['block_count']
        df_save['join_per_block'] = df_save['join_ms'] / df_save['block_count']

        # Overall stats
        print_subsection("Overall Timing (per save_blocks call)")
        print_stats_row("MDBX total", calc_percentiles(df_save['mdbx_ms']))
        print_stats_row("SF total", calc_percentiles(df_save['sf_ms']))
        print_stats_row("Join wait", calc_percentiles(df_save['join_ms']))

        print_subsection("Per-Block Timing")
        print_stats_row("MDBX per block", calc_percentiles(df_save['mdbx_per_block']))
        print_stats_row("SF per block", calc_percentiles(df_save['sf_per_block']))
        print_stats_row("Join per block", calc_percentiles(df_save['join_per_block']))

        # Bottleneck analysis
        print_subsection("Bottleneck Analysis")
        bottleneck = analyze_bottlenecks(df_save)
        if bottleneck:
            total = bottleneck['total']
            print(f"  Total save_blocks calls: {total}")
            print(f"  SF was slower:           {bottleneck['sf_slower_count']:5d} ({bottleneck['sf_slower_pct']:5.1f}%)")
            print(f"  MDBX was slower:         {bottleneck['mdbx_slower_count']:5d} ({bottleneck['mdbx_slower_pct']:5.1f}%)")
            print(f"  Significant waits (>1ms):{bottleneck['significant_wait_count']:5d}")
            print(f"  Parallelism efficiency:  {bottleneck['parallelism_efficiency']:5.1f}% (100% = perfect overlap)")

            if bottleneck['sf_slower_pct'] > 60:
                print("\n  >> SF is the bottleneck - MDBX finishes first and waits")
            elif bottleneck['mdbx_slower_pct'] > 60:
                print("\n  >> MDBX is the bottleneck - SF finishes first")
            else:
                print("\n  >> Mixed bottleneck - neither consistently slower")

        # MDBX breakdown
        print_subsection("MDBX Breakdown (per save_blocks call)")
        mdbx_breakdown = analyze_mdbx_breakdown(df_save)
        for name, stats in mdbx_breakdown.items():
            pct = stats.get('pct_of_mdbx', 0)
            print(f"  {name:28s} avg={stats['avg']:7.2f}ms  p50={stats['p50']:7.2f}ms  p90={stats['p90']:7.2f}ms  ({pct:5.1f}% of MDBX)")

        # Correlation
        print_subsection("Correlations")
        corr = analyze_correlation(df_save)
        if 'sf_mdbx_correlation' in corr:
            c = corr['sf_mdbx_correlation']
            print(f"  SF vs MDBX time:         {c:+.3f}", end="")
            if c > 0.7:
                print("  (strong positive - they scale together)")
            elif c < -0.3:
                print("  (negative - one speeds up when other slows)")
            else:
                print("  (weak - mostly independent)")
        if 'block_count_mdbx_correlation' in corr:
            print(f"  Block count vs MDBX:     {corr['block_count_mdbx_correlation']:+.3f}")
        if 'block_count_sf_correlation' in corr:
            print(f"  Block count vs SF:       {corr['block_count_sf_correlation']:+.3f}")

    # ==================== COMMIT ANALYSIS ====================
    if not df_commit.empty:
        print_section("COMMIT ANALYSIS")

        normal = df_commit[~df_commit['is_unwind']]
        unwind = df_commit[df_commit['is_unwind']]

        print(f"  Normal path commits: {len(normal)}")
        print(f"  Unwind path commits: {len(unwind)}")

        if not normal.empty:
            print_subsection("Normal Path Commit Timing")
            print_stats_row("SF finalize", calc_percentiles(normal['sf_ms']))
            print_stats_row("MDBX commit", calc_percentiles(normal['mdbx_ms']))

            # Total time in commits
            sf_total = normal['sf_ms'].sum()
            mdbx_total = normal['mdbx_ms'].sum()
            total = sf_total + mdbx_total
            if total > 0:
                print(f"\n  Total commit time:       {total:.1f}ms")
                print(f"    SF finalize:           {sf_total:.1f}ms ({sf_total/total*100:.1f}%)")
                print(f"    MDBX commit:           {mdbx_total:.1f}ms ({mdbx_total/total*100:.1f}%)")

    # ==================== AGGREGATE SUMMARY ====================
    print_section("AGGREGATE SUMMARY")

    if not df_save.empty:
        total_mdbx = df_save['mdbx_ms'].sum()
        total_sf = df_save['sf_ms'].sum()
        total_join = df_save['join_ms'].sum()
        total_blocks = df_save['block_count'].sum()

        # Wall clock estimate (max of parallel work + join wait)
        wall_time = df_save[['mdbx_ms', 'sf_ms']].max(axis=1).sum()

        print(f"  Total blocks processed:  {total_blocks:.0f}")
        print(f"  Total save_blocks calls: {len(df_save)}")
        print(f"")
        print(f"  Aggregate MDBX time:     {total_mdbx:,.1f}ms ({total_mdbx/1000:.1f}s)")
        print(f"  Aggregate SF time:       {total_sf:,.1f}ms ({total_sf/1000:.1f}s)")
        print(f"  Aggregate join wait:     {total_join:,.1f}ms ({total_join/1000:.1f}s)")
        print(f"  Est. wall clock time:    {wall_time:,.1f}ms ({wall_time/1000:.1f}s)")
        print(f"")
        print(f"  Avg per block (MDBX):    {total_mdbx/total_blocks:.3f}ms")
        print(f"  Avg per block (SF):      {total_sf/total_blocks:.3f}ms")

    if not df_commit.empty:
        normal = df_commit[~df_commit['is_unwind']]
        if not normal.empty:
            total_sf_commit = normal['sf_ms'].sum()
            total_mdbx_commit = normal['mdbx_ms'].sum()
            print(f"")
            print(f"  Total SF finalize:       {total_sf_commit:,.1f}ms ({total_sf_commit/1000:.1f}s)")
            print(f"  Total MDBX commit:       {total_mdbx_commit:,.1f}ms ({total_mdbx_commit/1000:.1f}s)")

    # Generate charts
    if not args.no_charts:
        print_section("GENERATING CHARTS")
        output_dir = Path(args.output)
        plot_analysis(df_save, df_commit, output_dir)

    print("\nDone.")


if __name__ == "__main__":
    main()
