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


import argparse
import pandas as pd
import matplotlib.pyplot as plt
import numpy as np
import sys

def main():
    parser = argparse.ArgumentParser(description='Generate histogram of total_latency percent differences between two CSV files')
    parser.add_argument('baseline_csv', help='First CSV file, used as the baseline/control')
    parser.add_argument('comparison_csv', help='Second CSV file, which is being compared to the baseline')
    parser.add_argument('-o', '--output', default='latency.png', help='Output image file (default: latency.png)')
    parser.add_argument('--graphs', default='all', help='Comma-separated list of graphs to plot: histogram, line, all (default: all)')

    args = parser.parse_args()

    # Parse graph selection
    if args.graphs.lower() == 'all':
        selected_graphs = {'histogram', 'line'}
    else:
        selected_graphs = set(graph.strip().lower() for graph in args.graphs.split(','))
        valid_graphs = {'histogram', 'line'}
        invalid_graphs = selected_graphs - valid_graphs
        if invalid_graphs:
            print(f"Error: Invalid graph types: {', '.join(invalid_graphs)}. Valid options are: histogram, line, all", file=sys.stderr)
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

    if len(df1) != len(df2):
        print("Warning: CSV files have different number of rows. Using minimum length.", file=sys.stderr)
        min_len = min(len(df1), len(df2))
        df1 = df1.head(min_len)
        df2 = df2.head(min_len)

    latency1 = df1['total_latency'].values
    latency2 = df2['total_latency'].values

    # Handle division by zero
    with np.errstate(divide='ignore', invalid='ignore'):
        percent_diff = ((latency2 - latency1) / latency1) * 100

    # Remove infinite and NaN values
    percent_diff = percent_diff[np.isfinite(percent_diff)]

    if len(percent_diff) == 0:
        print("Error: No valid percent differences could be calculated", file=sys.stderr)
        sys.exit(1)

    # Calculate statistics once for use in graphs and output
    mean_diff = np.mean(percent_diff)
    median_diff = np.median(percent_diff)

    # Determine number of subplots and create figure
    num_plots = len(selected_graphs)
    if num_plots == 0:
        print("Error: No valid graphs selected", file=sys.stderr)
        sys.exit(1)

    if num_plots == 1:
        fig, ax = plt.subplots(1, 1, figsize=(12, 6))
        axes = [ax]
    else:
        fig, axes = plt.subplots(num_plots, 1, figsize=(12, 6 * num_plots))

    plot_idx = 0

    # Plot histogram if selected
    if 'histogram' in selected_graphs:
        min_diff = np.floor(percent_diff.min())
        max_diff = np.ceil(percent_diff.max())

        # Create histogram with 1% buckets
        bins = np.arange(min_diff, max_diff + 1, 1)

        ax = axes[plot_idx]
        ax.hist(percent_diff, bins=bins, edgecolor='black', alpha=0.7)
        ax.set_xlabel('Percent Difference (%)')
        ax.set_ylabel('Number of Blocks')
        ax.set_title(f'Total Latency Percent Difference Histogram\n({args.baseline_csv} vs {args.comparison_csv})')
        ax.grid(True, alpha=0.3)

        # Add statistics to the histogram
        ax.axvline(mean_diff, color='red', linestyle='--', label=f'Mean: {mean_diff:.2f}%')
        ax.axvline(median_diff, color='orange', linestyle='--', label=f'Median: {median_diff:.2f}%')
        ax.legend()
        plot_idx += 1

    # Plot line graph if selected
    if 'line' in selected_graphs:
        # Determine comparison color based on median change. The median being
        # negative means processing time got faster, so that becomes green.
        comparison_color = 'green' if median_diff < 0 else 'red'

        ax = axes[plot_idx]
        if 'block_number' in df1.columns and 'block_number' in df2.columns:
            block_numbers = df1['block_number'].values[:len(percent_diff)]
            ax.plot(block_numbers, latency1[:len(percent_diff)], 'orange', alpha=0.7, label=f'Baseline ({args.baseline_csv})')
            ax.plot(block_numbers, latency2[:len(percent_diff)], comparison_color, alpha=0.7, label=f'Comparison ({args.comparison_csv})')
            ax.set_xlabel('Block Number')
            ax.set_ylabel('Total Latency (ms)')
            ax.set_title('Total Latency vs Block Number')
            ax.grid(True, alpha=0.3)
            ax.legend()
        else:
            # If no block_number column, use index
            indices = np.arange(len(percent_diff))
            ax.plot(indices, latency1[:len(percent_diff)], 'orange', alpha=0.7, label=f'Baseline ({args.baseline_csv})')
            ax.plot(indices, latency2[:len(percent_diff)], comparison_color, alpha=0.7, label=f'Comparison ({args.comparison_csv})')
            ax.set_xlabel('Block Index')
            ax.set_ylabel('Total Latency (ms)')
            ax.set_title('Total Latency vs Block Index')
            ax.grid(True, alpha=0.3)
            ax.legend()
        plot_idx += 1

    plt.tight_layout()
    plt.savefig(args.output, dpi=300, bbox_inches='tight')

    # Create graph type description for output message
    graph_types = []
    if 'histogram' in selected_graphs:
        graph_types.append('histogram')
    if 'line' in selected_graphs:
        graph_types.append('latency graph')
    graph_desc = ' and '.join(graph_types)
    print(f"{graph_desc.capitalize()} saved to {args.output}")

    # Always print statistics
    print(f"\nStatistics:")
    print(f"Mean percent difference: {mean_diff:.2f}%")
    print(f"Median percent difference: {median_diff:.2f}%")
    print(f"Standard deviation: {np.std(percent_diff):.2f}%")
    print(f"Min: {percent_diff.min():.2f}%")
    print(f"Max: {percent_diff.max():.2f}%")
    print(f"Total blocks analyzed: {len(percent_diff)}")

if __name__ == '__main__':
    main()
