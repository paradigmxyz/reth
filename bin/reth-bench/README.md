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

Below is an overview of how to execute a benchmark:

 1. **Setup**: Make sure `reth` is running in the background with the proper configuration. This setup involves ensuring the node is at the correct state, setting up profiling tools, and possibly more depending on the purpose of the benchmark's.

 2. **Run the Benchmark**:
    ```bash
    reth-bench new-payload-fcu --rpc-url http://<rpc-url>:8545 --from <start_block> --to <end_block> --jwtsecret <jwt_file_path>
    ```

    Replace `<rpc-url>`, `<start_block>`, `<end_block>`, and `<jwt_file_path>` with the appropriate values for your testing environment.
    Note that this assumes that the benchmark node's engine API is running on `http://127.0.0.1:8545`, which is set as a default value in `reth-bench`. To configure this value, use the `--engine-rpc-url` flag.

 3. **Observe Outputs**: Upon running the command, `reth-bench` will output benchmark results, showing processing speeds and gas usage, which are crucial for analyzing the node's performance.

    Example output:
    ```
    2024-05-30T00:45:20.806691Z  INFO Running benchmark using data from RPC URL: http://<rpc-url>:8545
    // ... logs per block
    2024-05-30T00:45:34.203172Z  INFO Total Ggas/s: 0.15 total_duration=5.085704882s total_gas_used=741620668.0
    ```

 4. **Stop and Review**: Once the benchmark completes, terminate the `reth` process and review the logs and performance metrics collected, if any.
 5. **Repeat**.

## Additional Considerations

- **RPC Configuration**: The RPC endpoints should be accessible and configured correctly, specifically the RPC endpoint must support `eth_getBlockByNumber` and support fetching full transactions. The benchmark will make one RPC query per block as fast as possible, so ensure the RPC endpoint does not rate limit or block requests after a certain volume.
- **Reproducibility**: Ensure that the node is at the same state before attempting to retry a benchmark. The `new-payload-fcu` command specifically will commit to the database, so the node must be rolled back using `reth stage unwind` to reproducibly retry benchmarks.
- **Profiling tools**: If you are collecting CPU profiles, tools like [`samply`](https://github.com/mstange/samply) and [`perf`](https://perf.wiki.kernel.org/index.php/Main_Page) can be useful for analyzing node performance.
- **Benchmark Data**: `reth-bench` additionally contains a `--benchmark.output` flag, which will output gas used benchmarks across the benchmark range in CSV format. This may be useful for further data analysis.
- **Platform Information**: To ensure accurate and reproducible benchmarking, document the platform details, including hardware specifications, OS version, and any other relevant information before publishing any benchmarks.

