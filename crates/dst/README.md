# reth-dst

Property assertions for Bedrock workloads. Run `cargo doc -p reth-dst --open`
for macro contracts, reporting configuration, and catalog export.

```rust
use reth_dst::{always, sometimes};

always!(validator_root == follower_root, "follower agrees with validator");
sometimes!(recovered, "recovery exercised", serde_json::json!({"block": block}));
```
