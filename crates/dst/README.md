# reth-dst

Use `always!` and `sometimes!` to record Bedrock assertions from reth workloads:

```rust
use reth_dst::{always, sometimes};

always!(validator_root == follower_root, "follower agrees with validator");
sometimes!(recovered, "recovery exercised", serde_json::json!({"block": block}));
```

The macros return `()`. False observations are recorded without panicking;
reporting errors panic with the assertion name. Conditions and optional details
evaluate exactly once. Names must be nonempty constant strings.

Set `BEDROCK_ASSERTIONS_PATH` to a JSONL file path. The first macro call opens
it, and concurrent calls share one writer. The path is read once per process.
Without it, arguments still evaluate, but nothing is serialized or written.
Records use Bedrock's `Always` and `Sometimes` format. `reth-dst` adds the
`timestamp_unix_nano` field used by Bedrock's per-writer assertion collection.
Optional details appear in a `details` field.
Records are limited to 16 KiB.
