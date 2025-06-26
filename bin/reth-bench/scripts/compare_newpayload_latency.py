#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.8"
# dependencies = [
#     "pandas",
#     "matplotlib", 
#     "numpy",
#     "seaborn",
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
import seaborn as sns
from matplotlib.ticker import FuncFormatter

# Set seaborn style for better aesthetics
sns.set_style("whitegrid")
sns.set_palette("husl")

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

    latency1 = df1['total_latency'].values / 1000  # Convert microseconds to milliseconds
    latency2 = df2['total_latency'].values / 1000  # Convert microseconds to milliseconds

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

    # Remove .csv extension from filenames for cleaner display
    baseline_name = args.baseline_csv.replace('.csv', '') if args.baseline_csv.endswith('.csv') else args.baseline_csv
    comparison_name = args.comparison_csv.replace('.csv', '') if args.comparison_csv.endswith('.csv') else args.comparison_csv
    
    # Top subplot: Histogram
    sns.histplot(data=percent_diff, bins=bins, edgecolor='black', alpha=0.7, ax=ax1)
    ax1.set_xlabel('Percent Difference (%)', fontsize=12)
    ax1.set_ylabel('Number of Blocks', fontsize=12)
    ax1.set_title(f'Total Latency Percent Difference Histogram\n({baseline_name} vs {comparison_name})', fontsize=14)

    # Add statistics to the histogram
    mean_diff = np.mean(percent_diff)
    median_diff = np.median(percent_diff)
    ax1.axvline(mean_diff, color='red', linestyle='--', linewidth=2, label=f'Mean: {mean_diff:.2f}%')
    ax1.axvline(median_diff, color='orange', linestyle='--', linewidth=2, label=f'Median: {median_diff:.2f}%')
    ax1.legend(fontsize=11)

    # Bottom subplot: Latency vs Block Number with rolling average
    target_points = 50
    data_length = len(percent_diff)
    window_size = max(1, data_length // target_points)
    
    def rolling_average(data, window):
        """Calculate rolling average with given window size"""
        if window == 1:
            return data
        return np.convolve(data, np.ones(window)/window, mode='valid')
    
    if 'block_number' in df1.columns and 'block_number' in df2.columns:
        block_numbers = df1['block_number'].values[:len(percent_diff)]
        
        # Apply rolling average
        latency1_smooth = rolling_average(latency1[:len(percent_diff)], window_size)
        latency2_smooth = rolling_average(latency2[:len(percent_diff)], window_size)
        block_numbers_smooth = rolling_average(block_numbers, window_size)
        
        ax2.plot(block_numbers_smooth, latency1_smooth, 'b-', alpha=0.7, label=baseline_name)
        ax2.plot(block_numbers_smooth, latency2_smooth, 'r-', alpha=0.7, label=comparison_name)
        ax2.set_xlabel('Block Number', fontsize=12)
        ax2.set_ylabel('Total Latency (ms)', fontsize=12)
        ax2.set_title(f'Total Latency vs Block Number (Rolling Average, Window={window_size})', fontsize=14)
        ax2.legend(fontsize=11)
        # Format X axis to avoid scientific notation
        ax2.xaxis.set_major_formatter(FuncFormatter(lambda x, p: f'{int(x):,}'))
    else:
        # If no block_number column, use index
        indices = np.arange(len(percent_diff))
        
        # Apply rolling average
        latency1_smooth = rolling_average(latency1[:len(percent_diff)], window_size)
        latency2_smooth = rolling_average(latency2[:len(percent_diff)], window_size)
        indices_smooth = rolling_average(indices, window_size)
        
        ax2.plot(indices_smooth, latency1_smooth, 'b-', alpha=0.7, label=baseline_name)
        ax2.plot(indices_smooth, latency2_smooth, 'r-', alpha=0.7, label=comparison_name)
        ax2.set_xlabel('Block Index', fontsize=12)
        ax2.set_ylabel('Total Latency (ms)', fontsize=12)
        ax2.set_title(f'Total Latency vs Block Index (Rolling Average, Window={window_size})', fontsize=14)
        ax2.legend(fontsize=11)

    plt.tight_layout()
    plt.savefig(args.output, dpi=300, bbox_inches='tight')
    print(f"Combined histogram and latency graph saved to {args.output}")
    
    # Save individual charts
    output_base = args.output.replace('.png', '')
    
    # Save histogram only
    fig1, ax1_only = plt.subplots(1, 1, figsize=(12, 6))
    sns.histplot(data=percent_diff, bins=bins, edgecolor='black', alpha=0.7, ax=ax1_only)
    ax1_only.set_xlabel('Percent Difference (%)', fontsize=12)
    ax1_only.set_ylabel('Number of Blocks', fontsize=12)
    ax1_only.set_title(f'Total Latency Percent Difference Histogram\n({baseline_name} vs {comparison_name})', fontsize=14)
    ax1_only.axvline(mean_diff, color='red', linestyle='--', linewidth=2, label=f'Mean: {mean_diff:.2f}%')
    ax1_only.axvline(median_diff, color='orange', linestyle='--', linewidth=2, label=f'Median: {median_diff:.2f}%')
    ax1_only.legend(fontsize=11)
    plt.tight_layout()
    histogram_file = f"{output_base}_histogram.png"
    plt.savefig(histogram_file, dpi=300, bbox_inches='tight')
    plt.close(fig1)
    print(f"Histogram saved to {histogram_file}")
    
    # Save latency chart only
    fig2, ax2_only = plt.subplots(1, 1, figsize=(12, 6))
    if 'block_number' in df1.columns and 'block_number' in df2.columns:
        ax2_only.plot(block_numbers_smooth, latency1_smooth, 'b-', alpha=0.7, label=baseline_name)
        ax2_only.plot(block_numbers_smooth, latency2_smooth, 'r-', alpha=0.7, label=comparison_name)
        ax2_only.set_xlabel('Block Number', fontsize=12)
        ax2_only.set_ylabel('Total Latency (ms)', fontsize=12)
        ax2_only.set_title(f'Total Latency vs Block Number (Rolling Average, Window={window_size})', fontsize=14)
        ax2_only.legend(fontsize=11)
        ax2_only.xaxis.set_major_formatter(FuncFormatter(lambda x, p: f'{int(x):,}'))
    else:
        ax2_only.plot(indices_smooth, latency1_smooth, 'b-', alpha=0.7, label=baseline_name)
        ax2_only.plot(indices_smooth, latency2_smooth, 'r-', alpha=0.7, label=comparison_name)
        ax2_only.set_xlabel('Block Index', fontsize=12)
        ax2_only.set_ylabel('Total Latency (ms)', fontsize=12)
        ax2_only.set_title(f'Total Latency vs Block Index (Rolling Average, Window={window_size})', fontsize=14)
        ax2_only.legend(fontsize=11)
    plt.tight_layout()
    chart_file = f"{output_base}_chart.png"
    plt.savefig(chart_file, dpi=300, bbox_inches='tight')
    plt.close(fig2)
    print(f"Latency chart saved to {chart_file}")
    
    # Close the combined figure
    plt.close(fig)

    print(f"\nStatistics:")
    print(f"Mean percent difference: {mean_diff:.2f}%")
    print(f"Median percent difference: {median_diff:.2f}%")
    print(f"Standard deviation: {np.std(percent_diff):.2f}%")
    print(f"Min: {percent_diff.min():.2f}%")
    print(f"Max: {percent_diff.max():.2f}%")
    print(f"Total blocks analyzed: {len(percent_diff)}")

if __name__ == '__main__':
    main()
