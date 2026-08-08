# Reth RPC Compatibility Tests

This crate runs the `ethereum/execution-apis` RPC compatibility fixtures against an embedded Reth
node without Hive, Docker, or Go.

The default configuration resolves `ethereum/execution-apis` branch `main` on every online fetch
and caches the fixture by its exact commit SHA. If that SHA is already cached it is reused without
downloading the archive again. Both `repository` and `branch` are configurable, so the same runner
can test an execution-apis fork. The runner imports `chain.rlp`, applies `headfcu.json`, executes
every selected `.io` conversation over raw HTTP, and writes JSON and JUnit reports. Its embedded
node computes state roots for `eth_simulateV1` responses.

```console
cargo run -p reth-rpc-compat-tests --bin reth-rpc-compat -- fetch
cargo run -p reth-rpc-compat-tests --bin reth-rpc-compat -- check --offline
cargo run -p reth-rpc-compat-tests --bin reth-rpc-compat -- run --offline
```

For example, point the fixture resolver at a fork in `rpc-compat.toml`:

```toml
[fixture]
repository = "example/execution-apis"
branch = "feature/rpc-tests"
```

Useful controls include:

```console
# Local fixture override and method filter
reth-rpc-compat run --fixture ../execution-apis --include 'eth_getBlockBy*'

# Reproducible commit override (bypasses branch resolution)
reth-rpc-compat fetch --revision 50d1e5e0b6f5a5046e45421e5c84497ab6e55e6c

# Outcome controls
reth-rpc-compat run --skip 'txpool_*' --ignore 'eth_call/flaky-case' \
  --xfail 'eth_getStorageAt/get-invalid-key'

# Named configuration and reports
reth-rpc-compat run --profile archive --choice receipts=either \
  --report-json report.json --report-junit junit.xml
```

Normal fixtures use strict JSON comparison with Hive-compatible number handling and error-message
redaction. Set `ignore_error_data = true` under `[run]` to also ignore only the `data` field of
JSON-RPC error objects; result objects and error codes remain strict. `speconly` fixtures validate
successful results against OpenRPC schemas. The schema catalog is built in Rust directly from the
resolved execution-apis sources when `openrpc.json` is not present.

Tests that fail only under strict error-data comparison belong in
`expected_failures_when_error_data_checked`. They are treated as ordinary tests when
`ignore_error_data = true`, avoiding unexpected-pass results.

Configuration lives in `rpc-compat.toml`. A non-empty profile or CLI include list replaces the
previous include selection; exclusion and outcome controls are additive. Profiles can also override
the expected response for exact test IDs with alternative `.io`/JSON files. Choices group mutually
exclusive profiles and may define a default option. A passing expected failure is treated as an
unexpected pass and fails CI; ignored tests are always reported but never affect the exit status.

```toml
[choices.receipt-shape]
default = "fixture"

[choices.receipt-shape.options.fixture]

[choices.receipt-shape.options.alternative.responses]
"eth_getTransactionReceipt/get-legacy-receipt" = ["responses/legacy-receipt.json"]
```

Unless `--fail-fast` is explicitly selected, every test and every exchange is executed. All response
mismatches are collected and written to the terminal, JSON report, and JUnit report before the
process returns a failing status for unexpected failures. The dedicated `rpc-compat` workflow owns
end-to-end execution; the crate is not registered as a normal `e2e_testsuite` target. Set
`github_token_env` in `[fixture]` to the name of an API-token environment variable when authenticated
GitHub revision lookup is desired.
