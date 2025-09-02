#!/usr/bin/env python3
"""
NewPayload Timing Log Analyzer

This script parses timing logs from Reth's newPayload process and generates
comparative analysis between different branches/implementations.

Usage:
    python analyze_newpayload_timing.py <log_file> [--output report.txt]
    python analyze_newpayload_timing.py --compare branch1.log branch2.log
"""

import re
import json
import argparse
import sys
from collections import defaultdict, OrderedDict
from dataclasses import dataclass
from typing import Dict, List, Optional, Tuple
import statistics

@dataclass
class TimingEvent:
    block_number: int
    block_hash: str
    phase: str
    event: str
    elapsed_us: int
    total_txs: Optional[int] = None
    cache_hits: Optional[int] = None
    cache_misses: Optional[int] = None
    db_calls: Optional[int] = None
    gas_used: Optional[int] = None

class NewPayloadTimingAnalyzer:
    def __init__(self):
        # Phase descriptions for reporting
        self.phase_descriptions = {
            "A1": "RPC Entry Point",
            "A4": "Block Execution Setup", 
            "C1": "Transaction Execution Start",
            "C4": "Transaction Execution Complete",
            "D4": "Cache Validation Complete",
            "E3": "State Root Complete", 
            "F4": "Block Execution Complete"
        }
        
        # Parse newpayload timing logs
        self.timing_pattern = re.compile(
            r'NEWPAYLOAD_TIMING.*?block_number=(\d+).*?block_hash=(0x[a-f0-9]+).*?'
            r'phase="([^"]+)".*?event="([^"]+)".*?elapsed_since_start_us=(\d+)'
        )
        
    def parse_log_file(self, log_file: str) -> Dict[int, List[TimingEvent]]:
        """Parse timing events from log file, grouped by block number"""
        events_by_block = defaultdict(list)
        
        try:
            with open(log_file, 'r') as f:
                for line in f:
                    if 'NEWPAYLOAD_TIMING' in line:
                        event = self._parse_timing_line(line)
                        if event:
                            events_by_block[event.block_number].append(event)
        except FileNotFoundError:
            print(f"Error: Log file '{log_file}' not found")
            return {}
        
        return dict(events_by_block)
    
    def _parse_timing_line(self, line: str) -> Optional[TimingEvent]:
        """Parse a single timing log line"""
        match = self.timing_pattern.search(line)
        if not match:
            return None
            
        block_number = int(match.group(1))
        block_hash = match.group(2)
        phase = match.group(3)
        event = match.group(4)
        elapsed_us = int(match.group(5))
        
        # Extract additional fields if present
        total_txs = self._extract_field(line, 'total_txs')
        cache_hits = self._extract_field(line, 'cache_hits')
        cache_misses = self._extract_field(line, 'cache_misses')
        db_calls = self._extract_field(line, 'db_calls')
        gas_used = self._extract_field(line, 'gas_used')
        
        return TimingEvent(
            block_number=block_number,
            block_hash=block_hash, 
            phase=phase,
            event=event,
            elapsed_us=elapsed_us,
            total_txs=total_txs,
            cache_hits=cache_hits,
            cache_misses=cache_misses,
            db_calls=db_calls,
            gas_used=gas_used
        )
    
    def _extract_field(self, line: str, field: str) -> Optional[int]:
        """Extract numeric field from log line"""
        pattern = rf'{field}=(\d+)'
        match = re.search(pattern, line)
        return int(match.group(1)) if match else None
    
    def analyze_block_timing(self, events: List[TimingEvent]) -> Dict:
        """Analyze timing for a single block"""
        if not events:
            return {}
            
        # Sort events by elapsed time
        events.sort(key=lambda e: e.elapsed_us)
        
        analysis = {
            'block_number': events[0].block_number,
            'block_hash': events[0].block_hash,
            'total_time_us': events[-1].elapsed_us if events else 0,
            'phases': {},
            'cache_stats': {},
            'total_txs': events[0].total_txs if events else 0
        }
        
        # Calculate phase durations
        prev_time = 0
        for event in events:
            phase_time = event.elapsed_us - prev_time
            analysis['phases'][f"{event.phase}_{event.event}"] = {
                'duration_us': phase_time,
                'cumulative_us': event.elapsed_us,
                'description': self.phase_descriptions.get(event.phase, event.phase)
            }
            prev_time = event.elapsed_us
            
            # Collect cache stats
            if event.cache_hits is not None:
                analysis['cache_stats']['hits'] = event.cache_hits
            if event.cache_misses is not None:
                analysis['cache_stats']['misses'] = event.cache_misses
            if event.db_calls is not None:
                analysis['cache_stats']['db_calls'] = event.db_calls
        
        return analysis
    
    def generate_summary_report(self, blocks_data: Dict[int, List[TimingEvent]]) -> str:
        """Generate comprehensive timing summary report"""
        if not blocks_data:
            return "No timing data found in log file"
            
        report = ["# NewPayload Timing Analysis Report\n"]
        
        # Overall statistics
        all_blocks = []
        total_times = []
        
        for block_num, events in blocks_data.items():
            analysis = self.analyze_block_timing(events)
            if analysis:
                all_blocks.append(analysis)
                total_times.append(analysis['total_time_us'])
        
        if not total_times:
            return "No valid timing data found"
            
        # Summary statistics
        report.append(f"## Summary Statistics")
        report.append(f"- Blocks analyzed: {len(all_blocks)}")
        report.append(f"- Average total time: {statistics.mean(total_times):.0f} μs")
        report.append(f"- Median total time: {statistics.median(total_times):.0f} μs") 
        report.append(f"- Min total time: {min(total_times):.0f} μs")
        report.append(f"- Max total time: {max(total_times):.0f} μs")
        report.append("")
        
        # Phase breakdown
        phase_times = defaultdict(list)
        for block in all_blocks:
            for phase, data in block['phases'].items():
                phase_times[phase].append(data['duration_us'])
        
        report.append("## Average Phase Durations")
        for phase, times in sorted(phase_times.items()):
            avg_time = statistics.mean(times)
            report.append(f"- {phase}: {avg_time:.0f} μs")
        report.append("")
        
        # Cache effectiveness
        cache_hits_total = sum(b['cache_stats'].get('hits', 0) for b in all_blocks)
        cache_misses_total = sum(b['cache_stats'].get('misses', 0) for b in all_blocks)
        if cache_hits_total + cache_misses_total > 0:
            hit_rate = cache_hits_total / (cache_hits_total + cache_misses_total) * 100
            report.append("## Cache Effectiveness")
            report.append(f"- Total cache hits: {cache_hits_total}")
            report.append(f"- Total cache misses: {cache_misses_total}")
            report.append(f"- Cache hit rate: {hit_rate:.1f}%")
            report.append("")
        
        # Top slowest blocks
        slowest_blocks = sorted(all_blocks, key=lambda b: b['total_time_us'], reverse=True)[:5]
        report.append("## Slowest Blocks")
        for i, block in enumerate(slowest_blocks, 1):
            report.append(f"{i}. Block {block['block_number']}: {block['total_time_us']:.0f} μs "
                         f"({block.get('total_txs', 'N/A')} txs)")
        
        return "\n".join(report)
    
    def compare_branches(self, log1: str, log2: str, label1: str = "Branch 1", label2: str = "Branch 2") -> str:
        """Compare timing data between two branches"""
        data1 = self.parse_log_file(log1)
        data2 = self.parse_log_file(log2)
        
        if not data1 or not data2:
            return "Error: Could not parse one or both log files"
        
        # Find common blocks between the two datasets
        common_blocks = set(data1.keys()) & set(data2.keys())
        if not common_blocks:
            return "No common blocks found between the two log files"
        
        report = [f"# Comparative Analysis: {label1} vs {label2}\n"]
        
        # Compare total times for common blocks
        improvements = []
        regressions = []
        
        for block_num in sorted(common_blocks):
            analysis1 = self.analyze_block_timing(data1[block_num])
            analysis2 = self.analyze_block_timing(data2[block_num])
            
            if analysis1 and analysis2:
                time1 = analysis1['total_time_us']
                time2 = analysis2['total_time_us']
                diff = time2 - time1
                percent_change = (diff / time1) * 100 if time1 > 0 else 0
                
                block_comparison = {
                    'block': block_num,
                    'time1': time1,
                    'time2': time2,
                    'diff': diff,
                    'percent': percent_change
                }
                
                if diff < 0:
                    improvements.append(block_comparison)
                else:
                    regressions.append(block_comparison)
        
        # Overall comparison
        if improvements or regressions:
            all_diffs = [b['diff'] for b in improvements + regressions]
            avg_diff = statistics.mean(all_diffs)
            report.append(f"## Overall Performance")
            report.append(f"- Average time difference: {avg_diff:.0f} μs")
            report.append(f"- Blocks improved: {len(improvements)}")
            report.append(f"- Blocks regressed: {len(regressions)}")
            report.append("")
            
        # Biggest improvements
        if improvements:
            improvements.sort(key=lambda b: b['diff'])
            report.append("## Biggest Improvements")
            for i, block in enumerate(improvements[:5], 1):
                report.append(f"{i}. Block {block['block']}: {block['diff']:.0f} μs "
                             f"({block['percent']:.1f}% faster)")
            report.append("")
            
        # Biggest regressions
        if regressions:
            regressions.sort(key=lambda b: b['diff'], reverse=True)
            report.append("## Biggest Regressions")
            for i, block in enumerate(regressions[:5], 1):
                report.append(f"{i}. Block {block['block']}: +{block['diff']:.0f} μs "
                             f"({block['percent']:.1f}% slower)")
        
        return "\n".join(report)

def main():
    parser = argparse.ArgumentParser(description='Analyze newPayload timing logs')
    parser.add_argument('log_file', help='Path to the log file to analyze')
    parser.add_argument('--output', '-o', help='Output file for the report')
    parser.add_argument('--compare', help='Compare with another log file')
    parser.add_argument('--label1', default='Current Branch', help='Label for first log file')
    parser.add_argument('--label2', default='Comparison Branch', help='Label for second log file')
    
    args = parser.parse_args()
    
    analyzer = NewPayloadTimingAnalyzer()
    
    if args.compare:
        # Comparative analysis
        report = analyzer.compare_branches(args.log_file, args.compare, args.label1, args.label2)
    else:
        # Single file analysis
        blocks_data = analyzer.parse_log_file(args.log_file)
        report = analyzer.generate_summary_report(blocks_data)
    
    if args.output:
        with open(args.output, 'w') as f:
            f.write(report)
        print(f"Report written to {args.output}")
    else:
        print(report)

if __name__ == '__main__':
    main()