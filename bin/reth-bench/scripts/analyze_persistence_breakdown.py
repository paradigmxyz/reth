#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.8"
# dependencies = []
# ///

"""
Generate detailed breakdown for each save_blocks + commit pair.

Usage:
    uv run analyze_persistence_breakdown.py <log_file> [output_file]
"""

import re
import sys
from datetime import datetime


def parse_timestamp(ts_str: str) -> datetime:
    return datetime.fromisoformat(ts_str.replace('Z', '+00:00'))


def parse_duration(text: str, field: str) -> float:
    """Extract duration in ms. Handles both 'field=VAL' and 'field=Some(VAL)'."""
    # Milliseconds
    m = re.search(rf'{field}=(?:Some\()?([\d.]+)ms\)?', text)
    if m:
        return float(m.group(1))
    # Seconds
    m = re.search(rf'{field}=(?:Some\()?([\d.]+)s(?![\w])\)?', text)
    if m:
        return float(m.group(1)) * 1000
    # Microseconds
    m = re.search(rf'{field}=(?:Some\()?([\d.]+)[µu]s\)?', text)
    if m:
        return float(m.group(1)) / 1000
    # Nanoseconds
    m = re.search(rf'{field}=(?:Some\()?([\d.]+)ns\)?', text)
    if m:
        return float(m.group(1)) / 1000000
    return 0


def format_duration(ms: float) -> str:
    """Format duration nicely."""
    if ms >= 1000:
        return f"{ms/1000:.2f}s"
    elif ms >= 1:
        return f"{ms:.2f}ms"
    elif ms >= 0.001:
        return f"{ms*1000:.2f}µs"
    else:
        return f"{ms*1000000:.2f}ns"


def parse_log(log_file: str) -> list:
    """Parse log file and extract save_blocks + commit pairs."""
    events = []

    with open(log_file) as f:
        lines = f.readlines()

    i = 0
    while i < len(lines):
        line = lines[i]

        if 'save_blocks breakdown' in line:
            ts = parse_timestamp(line.split()[0])
            bc = int(re.search(r'block_count=(\d+)', line).group(1))

            save_data = {
                'ts': ts,
                'block_count': bc,
                'mdbx_elapsed': parse_duration(line, 'mdbx_elapsed'),
                'sf_elapsed': parse_duration(line, 'sf_elapsed'),
                'sf_join_elapsed': parse_duration(line, 'sf_join_elapsed'),
                'rocksdb_elapsed': parse_duration(line, 'rocksdb_elapsed'),
                'rocksdb_join_elapsed': parse_duration(line, 'rocksdb_join_elapsed'),
                'insert_block': parse_duration(line, 'total_insert_block'),
                'write_state': parse_duration(line, 'total_write_state'),
                'write_hashed_state': parse_duration(line, 'total_write_hashed_state'),
                'write_trie_changesets': parse_duration(line, 'total_write_trie_changesets'),
                'write_trie_updates': parse_duration(line, 'total_write_trie_updates'),
                'update_history': parse_duration(line, 'duration_update_history_indices'),
            }

            # Look for the significant commit that follows (> 10ms mdbx time)
            # There are many small commits from other operations, we want the big one
            commit_data = None
            for j in range(i + 1, min(i + 200, len(lines))):
                if 'commit breakdown' in lines[j]:
                    mdbx_elapsed = parse_duration(lines[j], 'mdbx_elapsed')
                    # Look for significant commit (> 10ms) - the actual save_blocks commit
                    if mdbx_elapsed > 10:
                        commit_ts = parse_timestamp(lines[j].split()[0])
                        commit_data = {
                            'ts': commit_ts,
                            'sf_elapsed': parse_duration(lines[j], 'sf_elapsed'),
                            'mdbx_elapsed': mdbx_elapsed,
                            'is_unwind': 'unwind' in lines[j],
                        }
                        break
                elif 'save_blocks breakdown' in lines[j]:
                    # Hit next save_blocks before finding commit
                    break

            # Look for block range
            block_range = None
            for j in range(i + 1, min(i + 10, len(lines))):
                if 'Appended block data' in lines[j]:
                    m = re.search(r'range=(\d+)\.\.=(\d+)', lines[j])
                    if m:
                        block_range = (int(m.group(1)), int(m.group(2)))
                    break

            events.append({
                'save': save_data,
                'commit': commit_data,
                'block_range': block_range,
            })

        i += 1

    return events


def render_breakdown(event: dict, index: int) -> str:
    """Render a single save_blocks + commit breakdown."""
    save = event['save']
    commit = event['commit']
    block_range = event['block_range']

    bc = save['block_count']
    mdbx = save['mdbx_elapsed']

    lines = []

    # Header
    lines.append("=" * 80)
    range_str = f"{block_range[0]}..={block_range[1]}" if block_range else "unknown"
    lines.append(f" #{index + 1}  save_blocks({bc}) @ {save['ts'].strftime('%H:%M:%S.%f')[:-3]}  blocks {range_str}")
    lines.append("=" * 80)
    lines.append("")

    # MDBX breakdown
    lines.append(f"├─ MDBX work (parallel):           {format_duration(mdbx):>12}")

    components = [
        ('update_history', 'update_history_indices'),
        ('write_trie_updates', 'write_trie_updates'),
        ('write_state', 'write_state'),
        ('write_hashed_state', 'write_hashed_state'),
        ('write_trie_changesets', 'write_trie_changesets'),
        ('insert_block', 'insert_block'),
    ]

    # Sort by time descending
    sorted_components = sorted(components, key=lambda x: save[x[0]], reverse=True)

    for i, (key, label) in enumerate(sorted_components):
        val = save[key]
        pct = (val / mdbx * 100) if mdbx > 0 else 0
        prefix = "│  └─" if i == len(sorted_components) - 1 else "│  ├─"
        lines.append(f"{prefix} {label:28s} {format_duration(val):>12}  ({pct:5.1f}%)")

    lines.append("│")

    # SF work
    sf = save['sf_elapsed']
    sf_join = save['sf_join_elapsed']
    has_rocksdb = save['rocksdb_elapsed'] > 0

    if has_rocksdb:
        lines.append(f"├─ SF work (parallel):             {format_duration(sf):>12}")
        lines.append(f"│  └─ sf_join_elapsed:              {format_duration(sf_join):>12}")
        lines.append("│")

        # RocksDB work
        rocksdb = save['rocksdb_elapsed']
        rocksdb_join = save['rocksdb_join_elapsed']
        lines.append(f"└─ RocksDB work (parallel):        {format_duration(rocksdb):>12}")
        lines.append(f"   └─ rocksdb_join_elapsed:        {format_duration(rocksdb_join):>12}")
    else:
        lines.append(f"└─ SF work (parallel):             {format_duration(sf):>12}")
        lines.append(f"   └─ sf_join_elapsed:             {format_duration(sf_join):>12}")

    lines.append("")

    # Bottleneck analysis
    parallel_times = [('MDBX', mdbx), ('SF', sf)]
    if has_rocksdb:
        parallel_times.append(('RocksDB', save['rocksdb_elapsed']))
    bottleneck = max(parallel_times, key=lambda x: x[1])
    lines.append(f"Bottleneck: {bottleneck[0]} ({format_duration(bottleneck[1])})")
    lines.append("")

    # Commit
    if commit:
        commit_total = commit['sf_elapsed'] + commit['mdbx_elapsed']
        path = "unwind" if commit['is_unwind'] else "normal"
        lines.append(f"commit ({path} path) @ {commit['ts'].strftime('%H:%M:%S.%f')[:-3]}")
        lines.append(f"├─ sf_elapsed:                     {format_duration(commit['sf_elapsed']):>12}")
        lines.append(f"└─ mdbx_elapsed:                   {format_duration(commit['mdbx_elapsed']):>12}")
        lines.append("")

        # Total
        total = mdbx + commit['mdbx_elapsed']
        lines.append("─" * 50)
        lines.append(f"TOTAL:                             {format_duration(total):>12}  for {bc} blocks")
        lines.append(f"PER BLOCK:                         {format_duration(total/bc):>12}")
    else:
        lines.append("(no commit found)")
        lines.append("")
        lines.append("─" * 50)
        lines.append(f"TOTAL (save only):                 {format_duration(mdbx):>12}  for {bc} blocks")
        lines.append(f"PER BLOCK:                         {format_duration(mdbx/bc):>12}")

    lines.append("")

    # Percentage breakdown bar
    if commit:
        total = mdbx + commit['mdbx_elapsed']
        save_pct = mdbx / total * 100
        commit_pct = commit['mdbx_elapsed'] / total * 100

        bar_width = 50
        save_bars = int(save_pct / 100 * bar_width)
        commit_bars = bar_width - save_bars

        lines.append(f"[{'█' * save_bars}{'░' * commit_bars}]")
        lines.append(f" save_blocks: {save_pct:.1f}%          commit: {commit_pct:.1f}%")

    lines.append("")
    lines.append("")

    return "\n".join(lines)


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <log_file> [output_file]", file=sys.stderr)
        sys.exit(1)

    log_file = sys.argv[1]
    output_file = sys.argv[2] if len(sys.argv) > 2 else None

    events = parse_log(log_file)

    if not events:
        print("No save_blocks entries found", file=sys.stderr)
        sys.exit(1)

    output_lines = []

    # Summary header
    output_lines.append("=" * 80)
    output_lines.append(" SAVE_BLOCKS BREAKDOWN REPORT")
    output_lines.append(f" Log file: {log_file}")
    output_lines.append(f" Total save_blocks calls: {len(events)}")
    total_blocks = sum(e['save']['block_count'] for e in events)
    output_lines.append(f" Total blocks: {total_blocks}")
    output_lines.append("=" * 80)
    output_lines.append("")
    output_lines.append("")

    # Render each breakdown
    for i, event in enumerate(events):
        output_lines.append(render_breakdown(event, i))

    # Summary at end
    output_lines.append("=" * 80)
    output_lines.append(" SUMMARY")
    output_lines.append("=" * 80)
    output_lines.append("")

    # Per-block stats
    per_block_times = []
    for e in events:
        bc = e['save']['block_count']
        total = e['save']['mdbx_elapsed']
        if e['commit']:
            total += e['commit']['mdbx_elapsed']
        per_block_times.append((bc, total / bc, total))

    output_lines.append(f"{'Batch':>6} {'Per-Block':>12} {'Total':>12} {'Histogram'}")
    output_lines.append("-" * 60)

    max_per_block = max(t[1] for t in per_block_times)
    for bc, per_block, total in per_block_times:
        bar_len = int(per_block / max_per_block * 30)
        output_lines.append(f"{bc:>6} {format_duration(per_block):>12} {format_duration(total):>12} {'█' * bar_len}")

    output_lines.append("")

    output = "\n".join(output_lines)

    if output_file:
        with open(output_file, 'w') as f:
            f.write(output)
        print(f"Written to {output_file}")
    else:
        print(output)


if __name__ == "__main__":
    main()
