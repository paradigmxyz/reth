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

    args = parser.parse_args()

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

    # Create histogram with 1% buckets
    min_diff = np.floor(percent_diff.min())
    max_diff = np.ceil(percent_diff.max())

    bins = np.arange(min_diff, max_diff + 1, 1)

    # Create figure with two subplots
    fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(12, 12))

    # Top subplot: Histogram
    ax1.hist(percent_diff, bins=bins, edgecolor='black', alpha=0.7)
    ax1.set_xlabel('Percent Difference (%)')
    ax1.set_ylabel('Number of Blocks')
    ax1.set_title(f'Total Latency Percent Difference Histogram\n({args.baseline_csv} vs {args.comparison_csv})')
    ax1.grid(True, alpha=0.3)

    # Add statistics to the histogram
    mean_diff = np.mean(percent_diff)
    median_diff = np.median(percent_diff)
    ax1.axvline(mean_diff, color='red', linestyle='--', label=f'Mean: {mean_diff:.2f}%')
    ax1.axvline(median_diff, color='orange', linestyle='--', label=f'Median: {median_diff:.2f}%')
    ax1.legend()

    # Bottom subplot: Latency vs Block Number
    if 'block_number' in df1.columns and 'block_number' in df2.columns:
        block_numbers = df1['block_number'].values[:len(percent_diff)]
        ax2.plot(block_numbers, latency1[:len(percent_diff)], 'b-', alpha=0.7, label=f'Baseline ({args.baseline_csv})')
        ax2.plot(block_numbers, latency2[:len(percent_diff)], 'r-', alpha=0.7, label=f'Comparison ({args.comparison_csv})')
        ax2.set_xlabel('Block Number')
        ax2.set_ylabel('Total Latency (ms)')
        ax2.set_title('Total Latency vs Block Number')
        ax2.grid(True, alpha=0.3)
        ax2.legend()
    else:
        # If no block_number column, use index
        indices = np.arange(len(percent_diff))
        ax2.plot(indices, latency1[:len(percent_diff)], 'b-', alpha=0.7, label=f'Baseline ({args.baseline_csv})')
        ax2.plot(indices, latency2[:len(percent_diff)], 'r-', alpha=0.7, label=f'Comparison ({args.comparison_csv})')
        ax2.set_xlabel('Block Index')
        ax2.set_ylabel('Total Latency (ms)')
        ax2.set_title('Total Latency vs Block Index')
        ax2.grid(True, alpha=0.3)
        ax2.legend()

    plt.tight_layout()
    plt.savefig(args.output, dpi=300, bbox_inches='tight')
    print(f"Histogram and latency graph saved to {args.output}")

    print(f"\nStatistics:")
    print(f"Mean percent difference: {mean_diff:.2f}%")
    print(f"Median percent difference: {median_diff:.2f}%")
    print(f"Standard deviation: {np.std(percent_diff):.2f}%")
    print(f"Min: {percent_diff.min():.2f}%")
    print(f"Max: {percent_diff.max():.2f}%")
    print(f"Total blocks analyzed: {len(percent_diff)}")

if __name__ == '__main__':
    main()
