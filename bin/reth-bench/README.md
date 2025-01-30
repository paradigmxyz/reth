# Benchmarking reth live sync with `reth-bench`

The binary contained in this directory, `reth-bench`, is a tool that can be used to benchmark the performance of the reth live sync. `reth-bench` is a general tool, and can be used for benchmarking node performance, as long as the node supports the engine API.

### A recap on node synchronization
Reth uses two primary methods for synchronizing the chain:
 * Historical sync, which is used to synchronize the chain from genesis to a known finalized block. This involves re-executing the entire chain history.
 * Live sync, which is used to synchronize the chain from a finalized block to the current head. This involves processing new blocks as they are produced.

Benchmarking historical sync for reth is fairly easy, because historical sync is a long-running, deterministic process.
Reth specifically contains the `--debug.tip` argument, which allows for running the historical sync pipeline to a specific block.
However, reth's historical sync applies optimizations that are not always possible when syncing new blocks.


Live sync, on the other hand, is a more complex process that is harder to benchmark. It is also more sensitive to network conditions of the CL.
In order to benchmark live sync, we need to simulate a CL in a controlled manner, so reth can use the same code paths it would when syncing new blocks.

### The `reth-bench` tool
The `reth-bench` tool is designed to benchmark performance of reth live sync.
It can also be used for debugging client spec implementations, as it replays historical blocks by mocking a CL client.
Performance is measured by latency and gas used in a block, as well as the computed gas used per second.
As long as the data is representative of real-world load, or closer to worst-case load test, the gas per second gives a rough sense of how much throughput the node would be able to handle.

## Prerequisites

If you will be collecting CPU profiles, make sure `reth` is compiled with the `profiling` profile.
Otherwise, running `make maxperf` at the root of the repo should be sufficient for collecting accurate performance metrics.

## Command Usage

`reth-bench` contains different commands to benchmark different patterns of engine API calls.
The `reth-bench new-payload-fcu` command is the most representative of ethereum mainnet live sync, alternating between sending `engine_newPayload` calls and `engine_forkchoiceUpdated` calls.

Below is an overview of how to run a benchmark:

### Setup

Make sure `reth` is running in the background with the proper configuration. This setup involves ensuring the node is at the correct state, setting up profiling tools, and possibly more depending on the purpose of the benchmark.

Any consensus layer client configured to connect to `reth` should be shut down, as all the engine API interactions during benchmarking will be driven by `reth-bench`.

Depending on the block range you want to use in the benchmark you may need to unwind your node.
The head of the node should be behind the lowest block in the range.

Starting with a synced ethereum mainnet node, to run a benchmark starting at block 21,000,000, the node would need to be unwound first:
```bash
reth stage unwind to-block 21000000
```

The following `reth-bench` command would then start the benchmark at block 21,000,000:
```bash
reth-bench new-payload-fcu --rpc-url <rpc-url> --from 21000000 --to <end_block> --jwtsecret <jwt_file_path>
```

Finally, make sure that reth is built using a build profile suitable for what you are trying to measure.
For example, if the purpose of the benchmark is to load test and measure `reth`'s maximum speed, it would be compiled with the `maxperf` profile:
```bash
make maxperf
```

If the purpose of the benchmark is for analyzing flamegraphs for a specific routine using `perf` or `samply`, it should be compiled with the `profiling` profile:
```bash
make profiling
```

If the purpose of the benchmark is to obtain `jemalloc` memory profiles that can then be analyzed by `jeprof`, it should be compiled with the `profiling` profile and the `jemalloc-prof` feature:
```bash
RUSTFLAGS="-C target-cpu=native" cargo build --profile profiling --features "jemalloc-prof,asm-keccak"
```

> [!NOTE]
> Jemalloc memory profiling currently only works on linux.

Finally, if the purpose of the benchmark is to profile the node when `snmalloc` is configured as the default allocator, it would be built with the following
command:
```bash
RUSTFLAGS="-C target-cpu=native" cargo build --profile profiling --no-default-features --features "snmalloc-native,asm-keccak"
```

### Run the Benchmark:
First, start the reth node. Here is an example that runs `reth` compiled with the `profiling` profile, runs `samply`, and configures `reth` to run with metrics enabled:
```bash
samply record -p 3001 target/profiling/reth node --metrics localhost:9001 --authrpc.jwtsecret <jwt_file_path>
```

```bash
reth-bench new-payload-fcu --rpc-url <rpc-url> --from <start_block> --to <end_block> --jwtsecret <jwt_file_path>
```

Replace `<start_block>`, `<end_block>`, and `<jwt_file_path>` with the appropriate values for your testing environment. `<rpc-url>` should be the URL of an RPC endpoint that can provide the blocks that will be used during the execution.
This should NOT be the node that is being used for the benchmark. The node behind `--rpc-url` will be used as a data source for fetching real blocks, so they can be replayed in
the benchmark. The node being benchmarked will not have these blocks.
Note that this assumes that the benchmark node's engine API is running on `http://127.0.0.1:8551`, which is set as a default value in `reth-bench`. To configure this value, use the `--engine-rpc-url` flag.

### Observe Outputs

After running the command, `reth-bench` will output benchmark results, showing processing speeds and gas usage, which are useful metrics for analyzing the node's performance.
Note that the `GGas/s` metric does _not_ include just execution, it currently measures the total `newPayload` latency, over the total gas used in the block.

Example output:
```
2024-05-30T00:45:20.806691Z  INFO Running benchmark using data from RPC URL: http://<rpc-url>:8545
// ... logs per block
2024-05-30T00:45:34.203172Z  INFO Total Ggas/s: 0.15 total_duration=5.085704882s total_gas_used=741620668.0
```

### Stop and Review

Once the benchmark completes, terminate the `reth` process and review the logs and performance metrics collected, if any.

If using `samply`, the `reth` node should be stopped (using Ctrl+C or `kill`). The `samply` tool will output the following on the command line:
```bash
All tasks terminated.
Local server listening at http://127.0.0.1:3001
Press Ctrl+C to stop.
```
As long as the `reth` node was compiled using the `profiling` profile, or with any other profile that includes debug symbols, the URL can be used to analyze flamegraphs, stack
charts, and other formats useful for analyzing the sampled stack data. There should also be an "Upload local profile" button on the `samply` web interface that can be used to
upload the profile and receive a permalink that can be shared.
This can be extremely useful for providing data and context to team members to justify an optimization, or to share with others to help diagnose a performance issue.

If `prometheus` is set up and scraping a `reth` node that has `--metrics` enabled, other metrics can be analyzed, like execution latency, state root latency, memory and CPU
usage over time, and many other metrics that are useful for diagnosing performance issues.

### Repeat

To reproduce the benchmark, first re-set the node to the block that the benchmark started at, using `reth stage unwind` as mentioned above, and repeat all of the above steps.

## Additional Considerations

- **RPC Configuration**: The RPC endpoints should be accessible and configured correctly, specifically the RPC endpoint must support `eth_getBlockByNumber` and support fetching full transactions. The benchmark will make one RPC query per block as fast as possible, so ensure the RPC endpoint does not rate limit or block requests after a certain volume.
- **Reproducibility**: Ensure that the node is at the same state before attempting to retry a benchmark. The `new-payload-fcu` command specifically will commit to the database, so the node must be rolled back using `reth stage unwind` to reproducibly retry benchmarks.
- **Profiling tools**: If you are collecting CPU profiles, tools like [`samply`](https://github.com/mstange/samply) and [`perf`](https://perf.wiki.kernel.org/index.php/Main_Page) can be useful for analyzing node performance.
- **Benchmark Data**: `reth-bench` additionally contains a `--benchmark.output` flag, which will output gas used benchmarks across the benchmark range in CSV format. This may be useful for further data analysis.
- **Platform Information**: To ensure accurate and reproducible benchmarking, document the platform details, including hardware specifications, OS version, and any other relevant information before publishing any benchmarks.
