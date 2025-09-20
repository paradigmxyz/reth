#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.8"
# dependencies = [
#     "pandas",
#     "matplotlib",
#     "numpy",
# ]
# ///

# A simple script which plots graphs comparing two combined_latency.csv files
# output by reth-bench. The graphs which are plotted are:
#
# - A histogram of the percent change between latencies, bucketed by 1%
#   increments.
#
# - A simple line graph plotting the latencies of the two files against each
#   other.
#
# - A gas per second (gas/s) chart showing throughput over time.


import argparse
import pandas as pd
import matplotlib.pyplot as plt
import numpy as np
import sys
import os
from matplotlib.ticker import FuncFormatter

def get_output_filename(base_path, suffix=None):
    """Generate output filename with optional suffix."""
    if suffix is None:
        return base_path
    
    dir_name = os.path.dirname(base_path)
    base_name = os.path.basename(base_path)
    name, ext = os.path.splitext(base_name)
    
    new_name = f"{name}_{suffix}{ext}"
    return os.path.join(dir_name, new_name) if dir_name else new_name

def format_gas_units(value, pos):
    """Format gas values with appropriate units (gas, Kgas, Mgas, Ggas, Tgas)."""
    if value == 0:
        return '0'
    
    units = [
        (1e12, 'Tgas'),
        (1e9, 'Ggas'),
        (1e6, 'Mgas'),
        (1e3, 'Kgas'),
        (1, 'gas')
    ]
    
    abs_value = abs(value)
    for threshold, unit in units:
        if abs_value >= threshold:
            scaled_value = value / threshold
            if scaled_value >= 100:
                return f'{scaled_value:.0f} {unit}/s'
            elif scaled_value >= 10:
                return f'{scaled_value:.1f} {unit}/s'
            else:
                return f'{scaled_value:.2f} {unit}/s'
    
    return f'{value:.0f} gas/s'

def moving_average(data, window_size):
    """Calculate moving average with given window size."""
    if window_size <= 1:
        return data
    series = pd.Series(data)
    return series.rolling(window=window_size, center=True, min_periods=1).mean().values

def main():
    parser = argparse.ArgumentParser(description='Generate histogram of total_latency percent differences between two CSV files')
    parser.add_argument('baseline_csv', help='First CSV file, used as the baseline/control')
    parser.add_argument('comparison_csv', help='Second CSV file, which is being compared to the baseline')
    parser.add_argument('-o', '--output', default='latency.png', help='Output image file (default: latency.png)')
    parser.add_argument('--graphs', default='all', help='Comma-separated list of graphs to plot: histogram, line, gas, all (default: all)')
    parser.add_argument('--average', type=int, metavar='N', help='Apply moving average over N blocks to smooth line and gas charts')
    parser.add_argument('--separate', action='store_true', help='Output each chart as a separate file')

    args = parser.parse_args()

    if args.graphs.lower() == 'all':
        selected_graphs = {'histogram', 'line', 'gas'}
    else:
        selected_graphs = set(graph.strip().lower() for graph in args.graphs.split(','))
        valid_graphs = {'histogram', 'line', 'gas'}
        invalid_graphs = selected_graphs - valid_graphs
        if invalid_graphs:
            print(f"Error: Invalid graph types: {', '.join(invalid_graphs)}. Valid options are: histogram, line, gas, all", file=sys.stderr)
            sys.exit(1)

    try:
        df1 = pd.read_csv(args.baseline_csv)
        df2 = pd.read_csv(args.comparison_csv)
    except FileNotFoundError as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)
    except Exception as e:
        print(f"Error reading CSV files: {e}", file=sys.stderr)
        sys.exit(1)

    if 'total_latency' not in df1.columns:
        print(f"Error: 'total_latency' column not found in {args.baseline_csv}", file=sys.stderr)
        sys.exit(1)

    if 'total_latency' not in df2.columns:
        print(f"Error: 'total_latency' column not found in {args.comparison_csv}", file=sys.stderr)
        sys.exit(1)

    if 'gas' in selected_graphs:
        if 'gas_used' not in df1.columns:
            print(f"Error: 'gas_used' column not found in {args.baseline_csv} (required for gas graph)", file=sys.stderr)
            sys.exit(1)
        if 'gas_used' not in df2.columns:
            print(f"Error: 'gas_used' column not found in {args.comparison_csv} (required for gas graph)", file=sys.stderr)
            sys.exit(1)

    if len(df1) != len(df2):
        print("Warning: CSV files have different number of rows. Using minimum length.", file=sys.stderr)
        min_len = min(len(df1), len(df2))
        df1 = df1.head(min_len)
        df2 = df2.head(min_len)

    latency1 = df1['total_latency'].values / 1000.0
    latency2 = df2['total_latency'].values / 1000.0

    with np.errstate(divide='ignore', invalid='ignore'):
        percent_diff = ((latency2 - latency1) / latency1) * 100

    mask = np.isfinite(percent_diff)
    latency1 = latency1[mask]
    latency2 = latency2[mask]
    percent_diff = percent_diff[mask]
    if 'gas' in selected_graphs:
        gas1 = df1['gas_used'].values[mask]
        gas2 = df2['gas_used'].values[mask]

    if len(percent_diff) == 0:
        print("Error: No valid percent differences could be calculated", file=sys.stderr)
        sys.exit(1)

    mean_diff = np.mean(percent_diff)
    median_diff = np.median(percent_diff)

    num_plots = len(selected_graphs)
    if num_plots == 0:
        print("Error: No valid graphs selected", file=sys.stderr)
        sys.exit(1)

    output_files = []
    
    if not args.separate:
        if num_plots == 1:
            fig, ax = plt.subplots(1, 1, figsize=(12, 6))
            axes = [ax]
        else:
            fig, axes = plt.subplots(num_plots, 1, figsize=(12, 6 * num_plots))
            axes = np.atleast_1d(axes)

    plot_idx = 0

    if 'histogram' in selected_graphs:
        if args.separate:
            fig, ax = plt.subplots(1, 1, figsize=(12, 6))
        else:
            ax = axes[plot_idx]
            
        min_diff = np.floor(percent_diff.min())
        max_diff = np.ceil(percent_diff.max())

        bins = np.arange(min_diff, max_diff + 1, 1)

        ax.hist(percent_diff, bins=bins, edgecolor='black', alpha=0.7)
        ax.set_xlabel('Percent Difference (%)')
        ax.set_ylabel('Number of Blocks')
        ax.set_title(f'Total Latency Percent Difference Histogram\n({args.baseline_csv} vs {args.comparison_csv})')
        ax.grid(True, alpha=0.3)

        ax.axvline(mean_diff, color='red', linestyle='--', label=f'Mean: {mean_diff:.2f}%')
        ax.axvline(median_diff, color='orange', linestyle='--', label=f'Median: {median_diff:.2f}%')
        ax.legend()
        
        if args.separate:
            plt.tight_layout()
            output_file = get_output_filename(args.output, 'histogram')
            plt.savefig(output_file, dpi=300, bbox_inches='tight')
            output_files.append(output_file)
            plt.close(fig)
        else:
            plot_idx += 1

    if 'line' in selected_graphs:
        if args.separate:
            fig, ax = plt.subplots(1, 1, figsize=(12, 6))
        else:
            ax = axes[plot_idx]
            
        comparison_color = 'green' if median_diff < 0 else 'red'

        plot_latency1 = latency1
        plot_latency2 = latency2
        
        if args.average:
            plot_latency1 = moving_average(plot_latency1, args.average)
            plot_latency2 = moving_average(plot_latency2, args.average)
        if 'block_number' in df1.columns and 'block_number' in df2.columns:
            block_numbers = df1['block_number'].values[:len(plot_latency1)]
            ax.plot(block_numbers, plot_latency1, 'orange', alpha=0.7, label=f'Baseline ({args.baseline_csv})')
            ax.plot(block_numbers, plot_latency2, comparison_color, alpha=0.7, label=f'Comparison ({args.comparison_csv})')
            ax.set_xlabel('Block Number')
            ax.set_ylabel('Total Latency (ms)')
            title = 'Total Latency vs Block Number'
            if args.average:
                title += f' ({args.average}-block moving average)'
            ax.set_title(title)
            ax.grid(True, alpha=0.3)
            ax.legend()
        else:
            indices = np.arange(len(plot_latency1))
            ax.plot(indices, plot_latency1, 'orange', alpha=0.7, label=f'Baseline ({args.baseline_csv})')
            ax.plot(indices, plot_latency2, comparison_color, alpha=0.7, label=f'Comparison ({args.comparison_csv})')
            ax.set_xlabel('Block Index')
            ax.set_ylabel('Total Latency (ms)')
            title = 'Total Latency vs Block Index'
            if args.average:
                title += f' ({args.average}-block moving average)'
            ax.set_title(title)
            ax.grid(True, alpha=0.3)
            ax.legend()
        
        if args.separate:
            plt.tight_layout()
            output_file = get_output_filename(args.output, 'line')
            plt.savefig(output_file, dpi=300, bbox_inches='tight')
            output_files.append(output_file)
            plt.close(fig)
        else:
            plot_idx += 1

    if 'gas' in selected_graphs:
        if args.separate:
            fig, ax = plt.subplots(1, 1, figsize=(12, 6))
        else:
            ax = axes[plot_idx]
            
        latency1_sec = df1['total_latency'].values[mask] / 1_000_000.0
        latency2_sec = df2['total_latency'].values[mask] / 1_000_000.0
        
        gas_per_sec1 = gas1 / latency1_sec
        gas_per_sec2 = gas2 / latency2_sec
        
        original_gas_per_sec1 = gas_per_sec1.copy()
        original_gas_per_sec2 = gas_per_sec2.copy()
        
        if args.average:
            gas_per_sec1 = moving_average(gas_per_sec1, args.average)
            gas_per_sec2 = moving_average(gas_per_sec2, args.average)
        
        median_gas_per_sec1 = np.median(original_gas_per_sec1)
        median_gas_per_sec2 = np.median(original_gas_per_sec2)
        comparison_color = 'green' if median_gas_per_sec2 > median_gas_per_sec1 else 'red'
        
        if 'block_number' in df1.columns and 'block_number' in df2.columns:
            block_numbers = df1['block_number'].values[:len(gas_per_sec1)]
            ax.plot(block_numbers, gas_per_sec1, 'orange', alpha=0.7, label=f'Baseline ({args.baseline_csv})')
            ax.plot(block_numbers, gas_per_sec2, comparison_color, alpha=0.7, label=f'Comparison ({args.comparison_csv})')
            ax.set_xlabel('Block Number')
            ax.set_ylabel('Gas Throughput')
            title = 'Gas Throughput vs Block Number'
            if args.average:
                title += f' ({args.average}-block moving average)'
            ax.set_title(title)
            ax.grid(True, alpha=0.3)
            ax.legend()
            formatter = FuncFormatter(format_gas_units)
            ax.yaxis.set_major_formatter(formatter)
        else:
            indices = np.arange(len(gas_per_sec1))
            ax.plot(indices, gas_per_sec1, 'orange', alpha=0.7, label=f'Baseline ({args.baseline_csv})')
            ax.plot(indices, gas_per_sec2, comparison_color, alpha=0.7, label=f'Comparison ({args.comparison_csv})')
            ax.set_xlabel('Block Index')
            ax.set_ylabel('Gas Throughput')
            title = 'Gas Throughput vs Block Index'
            if args.average:
                title += f' ({args.average}-block moving average)'
            ax.set_title(title)
            ax.grid(True, alpha=0.3)
            ax.legend()
            formatter = FuncFormatter(format_gas_units)
            ax.yaxis.set_major_formatter(formatter)
        
        if args.separate:
            plt.tight_layout()
            output_file = get_output_filename(args.output, 'gas')
            plt.savefig(output_file, dpi=300, bbox_inches='tight')
            output_files.append(output_file)
            plt.close(fig)
        else:
            plot_idx += 1

    if not args.separate:
        plt.tight_layout()
        plt.savefig(args.output, dpi=300, bbox_inches='tight')
        output_files.append(args.output)

    graph_types = []
    if 'histogram' in selected_graphs:
        graph_types.append('histogram')
    if 'line' in selected_graphs:
        graph_types.append('latency graph')
    if 'gas' in selected_graphs:
        graph_types.append('gas/s graph')
    graph_desc = ' and '.join(graph_types)
    
    if args.separate:
        print(f"Saved {len(output_files)} separate files:")
        for output_file in output_files:
            print(f"  - {output_file}")
    else:
        print(f"{graph_desc.capitalize()} saved to {args.output}")

    print(f"\nStatistics:")
    print(f"Mean percent difference: {mean_diff:.2f}%")
    print(f"Median percent difference: {median_diff:.2f}%")
    print(f"Standard deviation: {np.std(percent_diff):.2f}%")
    print(f"Min: {percent_diff.min():.2f}%")
    print(f"Max: {percent_diff.max():.2f}%")
    print(f"Total blocks analyzed: {len(percent_diff)}")
    
    if 'gas' in selected_graphs:
        print(f"\nGas/s Statistics:")
        print(f"Baseline median gas/s: {median_gas_per_sec1:,.0f}")
        print(f"Comparison median gas/s: {median_gas_per_sec2:,.0f}")
        gas_diff_percent = ((median_gas_per_sec2 - median_gas_per_sec1) / median_gas_per_sec1) * 100
        print(f"Gas/s percent change: {gas_diff_percent:+.2f}%")

if __name__ == '__main__':
    main()
