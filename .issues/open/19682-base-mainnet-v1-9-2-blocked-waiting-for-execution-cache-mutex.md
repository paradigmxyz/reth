---
title: Base mainnet - v1.9.2 - Blocked waiting for execution cache mutex
labels:
    - C-bug
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.005063Z
info:
    author: Igor-rockawayx
    created_at: 2025-11-12T15:14:12Z
    updated_at: 2026-01-21T02:17:11Z
---

### Describe the bug

After upgrading our op-reth nodes to v1.9.2 we see a lot of `WARN Blocked waiting for execution cache mutex` warnings in the logs especially for Base mainnet - basically every block. 
op-node version: v1.16.1
logs snippet (full log attached bellow):
```
2025-11-12T14:06:03.211519Z  INFO Regular root task finished regular_state_root=0x5d9d202a6b1e8a7a550062874bb4d0c5ffcade33aaf4cc51c2d9eb7195ea6913 elapsed=1.952131627s
2025-11-12T14:06:03.211911Z  INFO Block added to canonical chain number=38083464 hash=0xb30c25ed4564edf15935229bc94d7d89260c52205161fa07c09dfcb9a23d2f3b peers=96 txs=411 gas_used=57.76Mgas gas_throughput=15.75Mgas/second gas_limit=250.00Mgas full=23.1% base_fee=0.01Gwei blobs=0 excess_blobs=0 elapsed=3.668610839s
2025-11-12T14:06:03.213376Z  INFO Canonical chain committed number=38083464 hash=0xb30c25ed4564edf15935229bc94d7d89260c52205161fa07c09dfcb9a23d2f3b elapsed=721.658µs
2025-11-12T14:06:03.222002Z  INFO Received block from consensus engine number=38083465 hash=0xdf9d50f0fc263bebf5d5a8cecd816909a1dafa0b6788f4d0fe8f7b5045d38328
2025-11-12T14:06:03.237261Z  WARN Blocked waiting for execution cache mutex blocked_for=15.046288ms
2025-11-12T14:06:06.819664Z  INFO Regular root task finished regular_state_root=0x84734507841d95500e67f70b9bb4a5e63f7ec451d21b2cf46252bab4174cc83b elapsed=1.885209281s
2025-11-12T14:06:06.820110Z  INFO Block added to canonical chain number=38083465 hash=0xdf9d50f0fc263bebf5d5a8cecd816909a1dafa0b6788f4d0fe8f7b5045d38328 peers=96 txs=414 gas_used=68.02Mgas gas_throughput=18.90Mgas/second gas_limit=250.00Mgas full=27.2% base_fee=0.01Gwei blobs=0 excess_blobs=0 elapsed=3.597918276s
2025-11-12T14:06:06.821798Z  INFO Canonical chain committed number=38083465 hash=0xdf9d50f0fc263bebf5d5a8cecd816909a1dafa0b6788f4d0fe8f7b5045d38328 elapsed=753.897µs
2025-11-12T14:06:06.826754Z  INFO Received block from consensus engine number=38083466 hash=0xe75b6253ef7ef1bef4d5cbbf93cc187371ab03a47cdc35d716a0bfe72d57b78c
2025-11-12T14:06:06.844438Z  WARN Blocked waiting for execution cache mutex blocked_for=17.430246ms
```

### Steps to reproduce

restarted with the new binary

### Node logs

```text
https://drive.google.com/file/d/1L5Pa-JyWZw93d9IZgL8nu4Lnkn1Y1nCo/view?usp=sharing
```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

v1.9.2

### What database version are you on?

Current database version: 2
Local database version: 2

### Which chain / network are you on?

base (mainnet)

### What type of node are you running?

Pruned with custom reth.toml config

### What prune config do you use, if any?

      --prune.senderrecovery.distance 2592000 \
      --prune.accounthistory.distance 2592000 \
      --prune.storagehistory.distance 2592000 \

### If you've built Reth from source, provide the full command you used

cargo build --profile maxperf --features jemalloc,asm-keccak --bin op-reth --manifest-path crates/optimism/bin/Cargo.toml --locked

### Code of Conduct

- [x] I agree to follow the Code of Conduct
