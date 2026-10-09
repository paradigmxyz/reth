# reth-dst

Use `always!` and `sometimes!` to record Bedrock properties from reth workloads:

```rust
use reth_dst::{always, sometimes};

always!(validator_root == follower_root, "follower agrees with validator");
sometimes!(recovered, "recovery exercised", serde_json::json!({"block": block}));
```

The macros return `()`. False observations are recorded without panicking.
Reporting errors panic with the property name. Conditions and optional details
evaluate exactly once. Names must be nonempty constant strings; put changing
values in details.

Set `BEDROCK_ASSERTIONS_PATH` to an existing directory's JSONL file path. The
first call opens the file, emits the full compiled catalog (including sites
that never execute), then appends observations. Call `reth_dst::init()?` at
startup if no macro may execute. The output path is read once per process.
Concurrent calls share a writer. Without the variable, the sink is disabled:
arguments still evaluate, but no file is created or value serialized.

`reth_dst::catalog::properties()` lists compiled sites, and
`reth_dst::catalog::write(output)` exports their JSONL declarations without
running conditions or opening the sink. Records use Bedrock's `Always` and
`Sometimes` assertion format, with `hit`, `property_id`, and `details` fields
for Rift-compatible catalog and observation semantics. A record is limited
to 16 KiB.
