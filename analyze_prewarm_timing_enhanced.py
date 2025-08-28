#!/usr/bin/env python3
"""
Enhanced analysis script for prewarm timing logs with atomic counter implementation.
Parses logs with format: global_tx_position, worker_id, local_tx_index
"""

import re
import sys
from collections import defaultdict
from datetime import datetime
import statistics

def parse_log_file(filename):
    """Parse enhanced prewarm timing logs."""
    
    # Patterns for the new log format
    prewarm_start_pattern = re.compile(
        r'(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z).*PREWARM_START.*'
        r'global_tx_position=(\d+).*worker_id=(\d+).*local_tx_index=(\d+).*tx_hash=(\w+)'
    )
    
    prewarm_end_pattern = re.compile(
        r'(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z).*PREWARM_END.*'
        r'global_tx_position=(\d+).*worker_id=(\d+).*local_tx_index=(\d+).*'
        r'tx_hash=(\w+).*duration_ms=(\d+)'
    )
    
    skipped_pattern = re.compile(
        r'.*SKIPPED_IN_PREWARM.*tx_index=(\d+)'
    )
    
    main_exec_pattern = re.compile(
        r'(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z).*MAIN_EXEC.*tx_index=(\d+)'
    )
    
    transactions = {}
    worker_stats = defaultdict(lambda: {'count': 0, 'total_time': 0, 'transactions': []})
    skipped_txs = set()
    
    with open(filename, 'r') as f:
        for line in f:
            # Check for skipped transactions
            skip_match = skipped_pattern.match(line)
            if skip_match:
                tx_idx = int(skip_match.group(1))
                skipped_txs.add(tx_idx)
                continue
            
            # Parse prewarm start
            start_match = prewarm_start_pattern.match(line)
            if start_match:
                timestamp_str = start_match.group(1)
                global_pos = int(start_match.group(2))
                worker_id = int(start_match.group(3))
                local_idx = int(start_match.group(4))
                tx_hash = start_match.group(5)
                
                timestamp = datetime.fromisoformat(timestamp_str.replace('Z', '+00:00'))
                
                if global_pos not in transactions:
                    transactions[global_pos] = {}
                
                transactions[global_pos]['prewarm_start'] = timestamp
                transactions[global_pos]['worker_id'] = worker_id
                transactions[global_pos]['local_idx'] = local_idx
                transactions[global_pos]['tx_hash'] = tx_hash
                continue
            
            # Parse prewarm end
            end_match = prewarm_end_pattern.match(line)
            if end_match:
                timestamp_str = end_match.group(1)
                global_pos = int(end_match.group(2))
                worker_id = int(end_match.group(3))
                local_idx = int(end_match.group(4))
                tx_hash = end_match.group(5)
                duration_ms = int(end_match.group(6))
                
                timestamp = datetime.fromisoformat(timestamp_str.replace('Z', '+00:00'))
                
                if global_pos in transactions:
                    transactions[global_pos]['prewarm_end'] = timestamp
                    transactions[global_pos]['duration_ms'] = duration_ms
                    
                    # Update worker statistics
                    worker_stats[worker_id]['count'] += 1
                    worker_stats[worker_id]['total_time'] += duration_ms
                    worker_stats[worker_id]['transactions'].append(global_pos)
                continue
            
            # Parse main execution
            main_match = main_exec_pattern.match(line)
            if main_match:
                timestamp_str = main_match.group(1)
                tx_idx = int(main_match.group(2))
                timestamp = datetime.fromisoformat(timestamp_str.replace('Z', '+00:00'))
                
                if tx_idx in transactions:
                    transactions[tx_idx]['main_exec'] = timestamp
    
    return transactions, worker_stats, skipped_txs

def analyze_timing(transactions, worker_stats, skipped_txs):
    """Analyze timing relationships between prewarm and main execution."""
    
    print("\n=== PREWARM TIMING ANALYSIS (ENHANCED) ===\n")
    
    # Overall statistics
    total_txs = len(transactions)
    completed_prewarms = sum(1 for tx in transactions.values() if 'duration_ms' in tx)
    
    print(f"Total transactions processed: {total_txs}")
    print(f"Skipped transactions: {len(skipped_txs)} {list(sorted(skipped_txs))[:10] if skipped_txs else []}")
    print(f"Completed prewarms: {completed_prewarms}")
    print(f"Success rate: {completed_prewarms/total_txs*100:.1f}%\n")
    
    # Worker distribution analysis
    print("=== WORKER DISTRIBUTION ===")
    active_workers = [wid for wid, stats in worker_stats.items() if stats['count'] > 0]
    print(f"Active workers: {len(active_workers)} out of 64")
    
    if worker_stats:
        # Show top 5 busiest workers
        sorted_workers = sorted(worker_stats.items(), key=lambda x: x[1]['count'], reverse=True)[:5]
        print("\nTop 5 busiest workers:")
        for worker_id, stats in sorted_workers:
            avg_time = stats['total_time'] / stats['count'] if stats['count'] > 0 else 0
            print(f"  Worker {worker_id}: {stats['count']} txs, avg {avg_time:.1f}ms")
    
    # Timing analysis for transactions with both prewarm and main execution
    race_conditions = []
    successful_prewarms = []
    
    for tx_idx, tx_data in transactions.items():
        if 'prewarm_end' in tx_data and 'main_exec' in tx_data:
            prewarm_end = tx_data['prewarm_end']
            main_exec = tx_data['main_exec']
            
            if main_exec < prewarm_end:
                race_conditions.append(tx_idx)
            else:
                successful_prewarms.append(tx_idx)
    
    print(f"\n=== RACE CONDITIONS ===")
    print(f"Transactions with race conditions: {len(race_conditions)}")
    if race_conditions:
        print(f"Affected positions: {sorted(race_conditions)[:20]}...")
    
    print(f"\nSuccessful prewarms (finished before main): {len(successful_prewarms)}")
    
    # Calculate timing gaps
    if successful_prewarms:
        gaps = []
        for tx_idx in successful_prewarms:
            tx = transactions[tx_idx]
            gap = (tx['main_exec'] - tx['prewarm_end']).total_seconds() * 1000
            gaps.append(gap)
        
        if gaps:
            print(f"\n=== TIMING GAPS (Prewarm completion to main execution) ===")
            print(f"Min gap: {min(gaps):.1f}ms")
            print(f"Max gap: {max(gaps):.1f}ms")
            print(f"Avg gap: {statistics.mean(gaps):.1f}ms")
            print(f"Median gap: {statistics.median(gaps):.1f}ms")
    
    # Analyze transaction positions with issues
    print(f"\n=== POSITION ANALYSIS ===")
    problem_positions = defaultdict(list)
    
    for tx_idx in range(min(50, total_txs)):  # Analyze first 50 positions
        if tx_idx in skipped_txs:
            problem_positions['skipped'].append(tx_idx)
        elif tx_idx in race_conditions:
            problem_positions['race_condition'].append(tx_idx)
        elif tx_idx not in transactions:
            problem_positions['missing'].append(tx_idx)
        elif tx_idx in successful_prewarms:
            problem_positions['successful'].append(tx_idx)
    
    for category, positions in problem_positions.items():
        if positions:
            print(f"{category}: positions {positions[:10]}{'...' if len(positions) > 10 else ''}")
    
    # Recommendations based on analysis
    print(f"\n=== RECOMMENDATIONS ===")
    if race_conditions:
        first_safe = max(race_conditions) + 1 if race_conditions else 0
        print(f"Consider skipping first {first_safe} transactions to avoid race conditions")
    
    if worker_stats:
        utilization = len(active_workers) / 64 * 100
        print(f"Worker utilization: {utilization:.1f}%")
        if utilization < 50:
            print("Low worker utilization - consider adjusting concurrency")
    
    return {
        'total_txs': total_txs,
        'skipped': len(skipped_txs),
        'race_conditions': len(race_conditions),
        'successful': len(successful_prewarms),
        'worker_utilization': len(active_workers) / 64 * 100 if worker_stats else 0
    }

def main():
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <log_file>")
        sys.exit(1)
    
    log_file = sys.argv[1]
    print(f"Analyzing enhanced prewarm timing logs from: {log_file}")
    
    transactions, worker_stats, skipped_txs = parse_log_file(log_file)
    stats = analyze_timing(transactions, worker_stats, skipped_txs)
    
    # Output summary for automated collection
    print(f"\n=== SUMMARY FOR AUTOMATION ===")
    print(f"STATS:{stats}")

if __name__ == "__main__":
    main()