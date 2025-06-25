# Test Plan for Baseline Comparison Feature

## Summary

Successfully implemented the `--baseline` argument for `reth-bench` that:

1. **Added `--baseline` argument** to `BenchmarkArgs` in `crates/node/core/src/args/benchmark_args.rs`
   - Accepts a CSV file path from a previous benchmark run
   - Properly documented with help text

2. **Created baseline comparison infrastructure** in `bin/reth-bench/src/bench/output.rs`:
   - `BaselineData` struct for deserializing CSV data
   - `BaselineComparisonResult` struct for comparison results  
   - `load_baseline_data()` function to parse CSV files
   - `create_baseline_comparison()` function to compute differences
   - New CSV output suffix `BASELINE_COMPARISON_OUTPUT_SUFFIX`

3. **Updated both benchmark commands**:
   - `new_payload_fcu.rs` - Supports baseline comparison with combined latency data
   - `new_payload_only.rs` - Supports baseline comparison with newPayload only data

## Key Features

- **CSV Input**: Reads baseline data from existing CSV output files
- **Per-block Comparison**: Compares newPayload latencies for each matching block
- **Statistical Output**: Provides absolute and percentage differences
- **CSV Output**: Generates `baseline_comparison.csv` with structured data
- **Logging**: Real-time comparison info during benchmark execution
- **Error Handling**: Graceful fallback when baseline data is missing or invalid

## Example Usage

```bash
# Run initial benchmark 
reth-bench new-payload-fcu --rpc-url http://localhost:8545 --from 1000 --to 1100 -o ./baseline_run/

# Run comparison benchmark
reth-bench new-payload-fcu --rpc-url http://localhost:8545 --from 1000 --to 1100 -o ./comparison_run/ --baseline ./baseline_run/combined_latency.csv
```

## Output Files

When `--baseline` is provided, an additional CSV file is generated:
- `baseline_comparison.csv` - Contains block-by-block comparisons with columns:
  - `block_number` - The block being compared
  - `baseline_latency` - Baseline newPayload latency in microseconds  
  - `current_latency` - Current run newPayload latency in microseconds
  - `latency_diff` - Absolute difference (current - baseline) in microseconds
  - `percent_diff` - Percentage change ((current - baseline) / baseline * 100)

## Implementation Details

- **Backward Compatible**: Existing functionality unchanged when `--baseline` not provided
- **Flexible Input**: Works with both `combined_latency.csv` and `new_payload_latency.csv` formats
- **Performance**: Minimal overhead - only processes baseline data when argument provided
- **Error Resilient**: Continues operation even if some baseline blocks are missing

## Next Steps for Testing

1. Build the project with proper libclang setup
2. Create sample baseline CSV data
3. Run comparison benchmarks to verify output format
4. Validate percentage calculations are correct
5. Test edge cases (missing blocks, invalid CSV format)

The implementation is complete and ready for testing once the build environment is properly configured.