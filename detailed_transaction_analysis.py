#!/usr/bin/env python3
"""
Enhanced Transaction Timing Analysis Script
Analyzes detailed per-transaction timing logs to provide comprehensive performance insights
"""
import re
import json
import statistics
import matplotlib.pyplot as plt
import pandas as pd
from collections import defaultdict, namedtuple
from typing import List, Dict, Optional

# Data structures for timing analysis
TransactionTiming = namedtuple('TransactionTiming', [
    'tx_hash', 'tx_index', 'gas_limit', 'gas_used', 'success', 'cache_hit',
    'cache_lookup_us', 'cache_validation_us', 'evm_setup_us', 'evm_execution_us', 
    'evm_cleanup_us', 'db_reads_us', 'db_writes_us', 'db_read_count', 
    'db_write_count', 'memory_allocations_us', 'total_us'
])

BlockSummary = namedtuple('BlockSummary', [
    'block_number', 'total_transactions', 'successful_transactions', 
    'cache_hits', 'cache_misses', 'cache_hit_rate', 'gas_skipped',
    'total_cache_lookup_us', 'avg_cache_lookup_us', 'total_evm_execution_us',
    'avg_evm_execution_us', 'total_evm_setup_us', 'total_evm_cleanup_us',
    'total_db_reads_us', 'total_db_writes_us', 'total_db_operations'
])

def parse_transaction_timing_logs(log_file: str) -> List[TransactionTiming]:
    """Parse detailed transaction timing logs"""
    transactions = []
    
    with open(log_file, 'r') as f:
        for line in f:
            if 'TRANSACTION_TIMING_BREAKDOWN' in line:
                # Extract transaction timing data
                match = re.search(
                    r'tx_hash=([a-fA-F0-9x]+).*?'
                    r'tx_index=(\d+).*?'
                    r'gas_limit=(\d+).*?'
                    r'gas_used=(\d+).*?'
                    r'success=(\w+).*?'
                    r'cache_hit=(\w+).*?'
                    r'cache_lookup_us=(\d+).*?'
                    r'cache_validation_us=(\d+).*?'
                    r'evm_setup_us=(\d+).*?'
                    r'evm_execution_us=(\d+).*?'
                    r'evm_cleanup_us=(\d+).*?'
                    r'db_reads_us=(\d+).*?'
                    r'db_writes_us=(\d+).*?'
                    r'db_read_count=(\d+).*?'
                    r'db_write_count=(\d+).*?'
                    r'memory_allocations_us=(\d+).*?'
                    r'total_us=(\d+)',
                    line
                )
                
                if match:
                    tx_timing = TransactionTiming(
                        tx_hash=match.group(1),
                        tx_index=int(match.group(2)),
                        gas_limit=int(match.group(3)),
                        gas_used=int(match.group(4)),
                        success=match.group(5) == 'true',
                        cache_hit=match.group(6) == 'true',
                        cache_lookup_us=int(match.group(7)),
                        cache_validation_us=int(match.group(8)),
                        evm_setup_us=int(match.group(9)),
                        evm_execution_us=int(match.group(10)),
                        evm_cleanup_us=int(match.group(11)),
                        db_reads_us=int(match.group(12)),
                        db_writes_us=int(match.group(13)),
                        db_read_count=int(match.group(14)),
                        db_write_count=int(match.group(15)),
                        memory_allocations_us=int(match.group(16)),
                        total_us=int(match.group(17))
                    )
                    transactions.append(tx_timing)
    
    return transactions

def parse_block_summaries(log_file: str) -> List[BlockSummary]:
    """Parse block-level transaction summaries"""
    summaries = []
    
    with open(log_file, 'r') as f:
        for line in f:
            if 'BLOCK_TRANSACTION_SUMMARY' in line:
                # Extract block summary data
                match = re.search(
                    r'total_transactions=(\d+).*?'
                    r'successful_transactions=(\d+).*?'
                    r'cache_hits=(\d+).*?'
                    r'cache_misses=(\d+).*?'
                    r'cache_hit_rate=([0-9.]+%).*?'
                    r'gas_skipped=(\d+).*?'
                    r'total_cache_lookup_us=(\d+).*?'
                    r'avg_cache_lookup_us=(\d+).*?'
                    r'total_evm_execution_us=(\d+).*?'
                    r'avg_evm_execution_us=(\d+).*?'
                    r'total_evm_setup_us=(\d+).*?'
                    r'total_evm_cleanup_us=(\d+).*?'
                    r'total_db_reads_us=(\d+).*?'
                    r'total_db_writes_us=(\d+).*?'
                    r'total_db_operations=(\d+)',
                    line
                )
                
                if match:
                    # Extract block number from the line
                    block_match = re.search(r'block_number=(\d+)', line)
                    block_number = int(block_match.group(1)) if block_match else len(summaries)
                    
                    summary = BlockSummary(
                        block_number=block_number,
                        total_transactions=int(match.group(1)),
                        successful_transactions=int(match.group(2)),
                        cache_hits=int(match.group(3)),
                        cache_misses=int(match.group(4)),
                        cache_hit_rate=float(match.group(5).replace('%', '')),
                        gas_skipped=int(match.group(6)),
                        total_cache_lookup_us=int(match.group(7)),
                        avg_cache_lookup_us=int(match.group(8)),
                        total_evm_execution_us=int(match.group(9)),
                        avg_evm_execution_us=int(match.group(10)),
                        total_evm_setup_us=int(match.group(11)),
                        total_evm_cleanup_us=int(match.group(12)),
                        total_db_reads_us=int(match.group(13)),
                        total_db_writes_us=int(match.group(14)),
                        total_db_operations=int(match.group(15))
                    )
                    summaries.append(summary)
    
    return summaries

def analyze_transaction_performance(transactions: List[TransactionTiming]) -> Dict:
    """Analyze transaction performance patterns"""
    if not transactions:
        return {}
    
    # Separate cache hits and misses
    cache_hits = [tx for tx in transactions if tx.cache_hit]
    cache_misses = [tx for tx in transactions if not tx.cache_hit]
    
    analysis = {
        'total_transactions': len(transactions),
        'cache_hits': len(cache_hits),
        'cache_misses': len(cache_misses),
        'cache_hit_rate': len(cache_hits) / len(transactions) * 100 if transactions else 0,
        'successful_transactions': len([tx for tx in transactions if tx.success])
    }
    
    # Timing breakdowns
    if cache_hits:
        analysis['cache_hit_performance'] = {
            'avg_cache_lookup_us': statistics.mean([tx.cache_lookup_us for tx in cache_hits]),
            'avg_cache_validation_us': statistics.mean([tx.cache_validation_us for tx in cache_hits]),
            'avg_total_us': statistics.mean([tx.total_us for tx in cache_hits]),
            'median_total_us': statistics.median([tx.total_us for tx in cache_hits])
        }
    
    if cache_misses:
        analysis['cache_miss_performance'] = {
            'avg_cache_lookup_us': statistics.mean([tx.cache_lookup_us for tx in cache_misses]),
            'avg_evm_setup_us': statistics.mean([tx.evm_setup_us for tx in cache_misses]),
            'avg_evm_execution_us': statistics.mean([tx.evm_execution_us for tx in cache_misses]),
            'avg_evm_cleanup_us': statistics.mean([tx.evm_cleanup_us for tx in cache_misses]),
            'avg_db_reads_us': statistics.mean([tx.db_reads_us for tx in cache_misses]),
            'avg_db_writes_us': statistics.mean([tx.db_writes_us for tx in cache_misses]),
            'avg_db_operations': statistics.mean([tx.db_read_count + tx.db_write_count for tx in cache_misses]),
            'avg_total_us': statistics.mean([tx.total_us for tx in cache_misses]),
            'median_total_us': statistics.median([tx.total_us for tx in cache_misses])
        }
    
    # Overall database performance
    analysis['database_performance'] = {
        'total_db_reads_us': sum([tx.db_reads_us for tx in transactions]),
        'total_db_writes_us': sum([tx.db_writes_us for tx in transactions]),
        'total_db_operations': sum([tx.db_read_count + tx.db_write_count for tx in transactions]),
        'avg_db_read_time_us': statistics.mean([tx.db_reads_us for tx in transactions if tx.db_reads_us > 0]) if any(tx.db_reads_us > 0 for tx in transactions) else 0,
        'avg_db_write_time_us': statistics.mean([tx.db_writes_us for tx in transactions if tx.db_writes_us > 0]) if any(tx.db_writes_us > 0 for tx in transactions) else 0
    }
    
    return analysis

def create_detailed_performance_charts(transactions: List[TransactionTiming], output_prefix: str = ""):
    """Create comprehensive performance visualization"""
    if not transactions:
        return
        
    fig = plt.figure(figsize=(20, 15))
    
    # Separate cache hits and misses
    cache_hits = [tx for tx in transactions if tx.cache_hit]
    cache_misses = [tx for tx in transactions if not tx.cache_hit]
    
    # 1. Cache Hit vs Miss Performance
    ax1 = plt.subplot(3, 3, 1)
    if cache_hits and cache_misses:
        hit_times = [tx.total_us for tx in cache_hits]
        miss_times = [tx.total_us for tx in cache_misses]
        
        ax1.boxplot([hit_times, miss_times], labels=['Cache Hits', 'Cache Misses'])
        ax1.set_ylabel('Total Execution Time (μs)')
        ax1.set_title('Cache Hit vs Miss Performance')
        ax1.set_yscale('log')
    
    # 2. EVM Execution Phase Breakdown (Cache Misses Only)
    ax2 = plt.subplot(3, 3, 2)
    if cache_misses:
        phases = ['Setup', 'Execution', 'Cleanup', 'DB Reads', 'DB Writes']
        avg_times = [
            statistics.mean([tx.evm_setup_us for tx in cache_misses]),
            statistics.mean([tx.evm_execution_us for tx in cache_misses]),
            statistics.mean([tx.evm_cleanup_us for tx in cache_misses]),
            statistics.mean([tx.db_reads_us for tx in cache_misses]),
            statistics.mean([tx.db_writes_us for tx in cache_misses])
        ]
        
        ax2.bar(phases, avg_times, color=['skyblue', 'orange', 'green', 'red', 'purple'])
        ax2.set_ylabel('Average Time (μs)')
        ax2.set_title('EVM Execution Phase Breakdown (Cache Misses)')
        ax2.tick_params(axis='x', rotation=45)
    
    # 3. Database Operations Analysis
    ax3 = plt.subplot(3, 3, 3)
    db_read_times = [tx.db_reads_us for tx in transactions if tx.db_reads_us > 0]
    db_write_times = [tx.db_writes_us for tx in transactions if tx.db_writes_us > 0]
    
    if db_read_times or db_write_times:
        data = []
        labels = []
        if db_read_times:
            data.append(db_read_times)
            labels.append('DB Reads')
        if db_write_times:
            data.append(db_write_times)
            labels.append('DB Writes')
        
        ax3.boxplot(data, labels=labels)
        ax3.set_ylabel('Database Operation Time (μs)')
        ax3.set_title('Database Operation Performance')
    
    # 4. Gas Usage vs Execution Time
    ax4 = plt.subplot(3, 3, 4)
    gas_used = [tx.gas_used for tx in transactions]
    exec_times = [tx.total_us for tx in transactions]
    colors = ['red' if not tx.cache_hit else 'blue' for tx in transactions]
    
    scatter = ax4.scatter(gas_used, exec_times, c=colors, alpha=0.6, s=20)
    ax4.set_xlabel('Gas Used')
    ax4.set_ylabel('Execution Time (μs)')
    ax4.set_title('Gas Usage vs Execution Time')
    ax4.set_xscale('log')
    ax4.set_yscale('log')
    
    # Create legend for scatter plot
    from matplotlib.patches import Patch
    legend_elements = [Patch(facecolor='blue', label='Cache Hit'),
                      Patch(facecolor='red', label='Cache Miss')]
    ax4.legend(handles=legend_elements)
    
    # 5. Cache Lookup Overhead Distribution
    ax5 = plt.subplot(3, 3, 5)
    cache_lookup_times = [tx.cache_lookup_us for tx in transactions if tx.cache_lookup_us > 0]
    if cache_lookup_times:
        ax5.hist(cache_lookup_times, bins=50, alpha=0.7, color='green')
        ax5.set_xlabel('Cache Lookup Time (μs)')
        ax5.set_ylabel('Frequency')
        ax5.set_title('Cache Lookup Overhead Distribution')
    
    # 6. Transaction Index vs Performance
    ax6 = plt.subplot(3, 3, 6)
    indices = [tx.tx_index for tx in transactions]
    times = [tx.total_us for tx in transactions]
    
    ax6.scatter(indices, times, alpha=0.6, s=20)
    ax6.set_xlabel('Transaction Index in Block')
    ax6.set_ylabel('Execution Time (μs)')
    ax6.set_title('Transaction Performance vs Block Position')
    
    # 7. Memory Allocation Impact
    ax7 = plt.subplot(3, 3, 7)
    memory_times = [tx.memory_allocations_us for tx in transactions if tx.memory_allocations_us > 0]
    if memory_times:
        ax7.hist(memory_times, bins=30, alpha=0.7, color='purple')
        ax7.set_xlabel('Memory Allocation Time (μs)')
        ax7.set_ylabel('Frequency')
        ax7.set_title('Memory Allocation Overhead Distribution')
    
    # 8. Database Operation Counts
    ax8 = plt.subplot(3, 3, 8)
    read_counts = [tx.db_read_count for tx in transactions]
    write_counts = [tx.db_write_count for tx in transactions]
    
    if any(read_counts) or any(write_counts):
        ax8.scatter(read_counts, write_counts, alpha=0.6, s=20)
        ax8.set_xlabel('DB Read Operations')
        ax8.set_ylabel('DB Write Operations')
        ax8.set_title('Database Operation Patterns')
    
    # 9. Success Rate Analysis
    ax9 = plt.subplot(3, 3, 9)
    success_rate = len([tx for tx in transactions if tx.success]) / len(transactions) * 100
    cache_hit_rate = len(cache_hits) / len(transactions) * 100
    
    metrics = ['Success Rate', 'Cache Hit Rate']
    values = [success_rate, cache_hit_rate]
    colors = ['green', 'blue']
    
    bars = ax9.bar(metrics, values, color=colors, alpha=0.7)
    ax9.set_ylabel('Percentage (%)')
    ax9.set_title('Transaction Success and Cache Performance')
    ax9.set_ylim(0, 100)
    
    # Add value labels on bars
    for bar, value in zip(bars, values):
        height = bar.get_height()
        ax9.text(bar.get_x() + bar.get_width()/2., height + 1,
                f'{value:.1f}%', ha='center', va='bottom')
    
    plt.tight_layout()
    plt.savefig(f'{output_prefix}detailed_transaction_analysis.png', dpi=300, bbox_inches='tight')
    print(f"📊 Detailed analysis chart saved as '{output_prefix}detailed_transaction_analysis.png'")
    plt.show()

def generate_performance_report(transactions: List[TransactionTiming], summaries: List[BlockSummary] = None):
    """Generate comprehensive performance report"""
    analysis = analyze_transaction_performance(transactions)
    
    print("🔍 DETAILED TRANSACTION PERFORMANCE ANALYSIS")
    print("=" * 60)
    
    print(f"\n📊 Overall Statistics:")
    print(f"  Total Transactions:     {analysis['total_transactions']:,}")
    print(f"  Successful Transactions: {analysis['successful_transactions']:,}")
    print(f"  Cache Hits:             {analysis['cache_hits']:,}")
    print(f"  Cache Misses:           {analysis['cache_misses']:,}")
    print(f"  Cache Hit Rate:         {analysis['cache_hit_rate']:.1f}%")
    
    if 'cache_hit_performance' in analysis:
        print(f"\n⚡ Cache Hit Performance:")
        chp = analysis['cache_hit_performance']
        print(f"  Avg Cache Lookup:       {chp['avg_cache_lookup_us']:.1f}μs")
        print(f"  Avg Cache Validation:   {chp['avg_cache_validation_us']:.1f}μs")
        print(f"  Avg Total Time:         {chp['avg_total_us']:.1f}μs")
        print(f"  Median Total Time:      {chp['median_total_us']:.1f}μs")
    
    if 'cache_miss_performance' in analysis:
        print(f"\n🔥 Cache Miss Performance:")
        cmp = analysis['cache_miss_performance']
        print(f"  Avg Cache Lookup:       {cmp['avg_cache_lookup_us']:.1f}μs")
        print(f"  Avg EVM Setup:          {cmp['avg_evm_setup_us']:.1f}μs")
        print(f"  Avg EVM Execution:      {cmp['avg_evm_execution_us']:.1f}μs")
        print(f"  Avg EVM Cleanup:        {cmp['avg_evm_cleanup_us']:.1f}μs")
        print(f"  Avg DB Reads:           {cmp['avg_db_reads_us']:.1f}μs")
        print(f"  Avg DB Writes:          {cmp['avg_db_writes_us']:.1f}μs")
        print(f"  Avg DB Operations:      {cmp['avg_db_operations']:.1f}")
        print(f"  Avg Total Time:         {cmp['avg_total_us']:.1f}μs")
        print(f"  Median Total Time:      {cmp['median_total_us']:.1f}μs")
    
    print(f"\n💾 Database Performance:")
    dbp = analysis['database_performance']
    print(f"  Total DB Read Time:     {dbp['total_db_reads_us']:,}μs")
    print(f"  Total DB Write Time:    {dbp['total_db_writes_us']:,}μs")
    print(f"  Total DB Operations:    {dbp['total_db_operations']:,}")
    print(f"  Avg DB Read Time:       {dbp['avg_db_read_time_us']:.1f}μs")
    print(f"  Avg DB Write Time:      {dbp['avg_db_write_time_us']:.1f}μs")
    
    # Performance comparison
    if 'cache_hit_performance' in analysis and 'cache_miss_performance' in analysis:
        hit_median = analysis['cache_hit_performance']['median_total_us']
        miss_median = analysis['cache_miss_performance']['median_total_us']
        speedup = miss_median / hit_median if hit_median > 0 else float('inf')
        
        print(f"\n🎯 Performance Impact:")
        print(f"  Cache Hit Median:       {hit_median:.1f}μs")
        print(f"  Cache Miss Median:      {miss_median:.1f}μs")
        print(f"  Cache Speedup:          {speedup:.1f}x")
        
        if speedup > 1:
            print(f"  💡 Cache provides {speedup:.1f}x performance improvement")
        
    # Block-level analysis if available
    if summaries:
        print(f"\n📋 Block-Level Summary ({len(summaries)} blocks):")
        avg_cache_hit_rate = statistics.mean([s.cache_hit_rate for s in summaries])
        total_gas_skipped = sum([s.gas_skipped for s in summaries])
        
        print(f"  Average Cache Hit Rate: {avg_cache_hit_rate:.1f}%")
        print(f"  Total Gas Skipped:      {total_gas_skipped:,}")

if __name__ == "__main__":
    import sys
    
    if len(sys.argv) < 2:
        print("Usage: python3 detailed_transaction_analysis.py <log_file> [output_prefix]")
        sys.exit(1)
    
    log_file = sys.argv[1]
    output_prefix = sys.argv[2] if len(sys.argv) > 2 else ""
    
    print("🔍 Parsing detailed transaction timing logs...")
    
    # Parse transaction-level data
    transactions = parse_transaction_timing_logs(log_file)
    print(f"Found {len(transactions)} detailed transaction records")
    
    # Parse block-level summaries
    summaries = parse_block_summaries(log_file)
    print(f"Found {len(summaries)} block summaries")
    
    if not transactions and not summaries:
        print("❌ No transaction timing data found in log file")
        sys.exit(1)
    
    # Generate analysis
    if transactions:
        generate_performance_report(transactions, summaries)
        create_detailed_performance_charts(transactions, output_prefix)
    else:
        print("⚠️ No detailed transaction data found, only block summaries available")
        if summaries:
            print(f"Block summaries indicate {sum(s.total_transactions for s in summaries)} total transactions")