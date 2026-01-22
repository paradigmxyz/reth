---
title: Cannot log rpc calls even with appropriate verbosity
labels:
    - A-observability
    - C-bug
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.987067Z
info:
    author: IvanWeber42
    created_at: 2025-05-19T10:05:02Z
    updated_at: 2025-06-26T22:41:22Z
---

### Describe the bug

According to docs setting `verbosity` to `-vvvvv` should log the `trace!` entries which include "Serving..." traces that show which endpoints are being called, however this is not the case.

### Steps to reproduce

Set the verbosity value to `-vvvvv`.

The logs will now show `debug` traces, but `trace` traces are still not logged although the appropriate level has been set.

### Node logs

```text
8f206e07a41a01d6b4667, finalized_block_hash: 0x2db8fbb1552c7e503f058e8d86460841b3522eea68e26858686747f46d04899c }, Valid))
2025-05-19T09:59:30.584154Z  INFO reth_node_events::node: Canonical chain committed number=7257009 hash=0x5e662fa0a24d51e0ba7a3a42bf38492cf3c7e2e76a63567c390bf3a4e2708fe9 elapsed=59.902µs
2025-05-19T09:59:30.600253Z DEBUG providers::db: Inserted block body block_number=7257007 actions=[(InsertBlockBodyIndices, 35.485µs), (InsertTransactionBlocks, 16.444µs)]
2025-05-19T09:59:30.600281Z DEBUG providers::db: Inserted block block_number=7257007 actions=[(GetParentTD, 28.939µs), (InsertCanonicalHeaders, 49.08µs), (InsertHeaders, 18.826µs), (InsertHeaderTerminalDifficulties, 10.967µs), (InsertHeaderNumbers, 87.43µs), (GetNextTxNum, 7.312µs)]
2025-05-19T09:59:30.635803Z DEBUG provider::storage_writer: Appended block data range=7257007..=7257007
2025-05-19T09:59:30.644050Z DEBUG provider::static_file: Commit segment=Headers path="/datadir/static_files/static_file_headers_7000000_7499999" duration=8.221168ms
2025-05-19T09:59:30.651432Z DEBUG provider::static_file: Commit segment=Transactions path="/datadir/static_files/static_file_transactions_7000000_7499999" duration=7.244969ms
2025-05-19T09:59:30.659031Z DEBUG provider::static_file: Commit segment=Receipts path="/datadir/static_files/static_file_receipts_7000000_7499999" duration=7.518568ms
2025-05-19T09:59:30.739921Z DEBUG storage::db::mdbx: Commit total_duration=80.797372ms commit_latency=Some(CommitLatency(MDBX_commit_latency { preparation: 0, gc_wallclock: 5, audit: 0, write: 0, sync: 5284, ending: 0, whole: 5290, gc_cputime: 0, gc_prof: MDBX_commit_latency__bindgen_ty_1 { wloops: 0, coalescences: 0, wipes: 0, flushes: 0, kicks: 0, work_counter: 0, work_rtime_monotonic: 0, work_xtime_cpu: 0, work_rsteps: 0, work_xpages: 0, work_majflt: 0, self_counter: 0, self_rtime_monotonic: 0, self_xtime_cpu: 0, self_rsteps: 0, self_xpages: 0, self_majflt: 0 } })) is_read_only=false
2025-05-19T09:59:31.083876Z DEBUG engine::tree: received no engine message for some time, while waiting for persistence task to complete
2025-05-19T09:59:31.083903Z DEBUG engine::tree: Finished persisting, calling finish last_persisted_block_hash=0xb5ca6c1b644b668c65f5f04c9870e148890c2a0d393e7d070f3f04255fabea22 last_persisted_block_number=7257007
2025-05-19T09:59:31.083966Z DEBUG engine::tree: Removing blocks from the tree upper_bound=NumHash { number: 7257007, hash: 0xb5ca6c1b644b668c65f5f04c9870e148890c2a0d393e7d070f3f04255fabea22 } finalized_num_hash=Some(NumHash { number: 7256179, hash: 0x2db8fbb1552c7e503f058e8d86460841b3522eea68e26858686747f46d04899c })
2025-05-19T09:59:31.083984Z DEBUG engine::tree: Removing canonical blocks from the tree upper_bound=7257007 last_persisted_hash=0xb5ca6c1b644b668c65f5f04c9870e148890c2a0d393e7d070f3f04255fabea22
2025-05-19T09:59:31.083990Z DEBUG engine::tree: Attempting to remove block walking back from the head num_hash=NumHash { number: 7257007, hash: 0xb5ca6c1b644b668c65f5f04c9870e148890c2a0d393e7d070f3f04255fabea22 }
2025-05-19T09:59:31.083998Z DEBUG engine::tree: Removed block walking back from the head num_hash=NumHash { number: 7257007, hash: 0xb5ca6c1b644b668c65f5f04c9870e148890c2a0d393e7d070f3f04255fabea22 }
2025-05-19T09:59:31.084003Z DEBUG engine::tree: Removed canonical blocks from the tree upper_bound=7257007 last_persisted_hash=0xb5ca6c1b644b668c65f5f04c9870e148890c2a0d393e7d070f3f04255fabea22
2025-05-19T09:59:32.449857Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0x5e662fa0a24d51e0ba7a3a42bf38492cf3c7e2e76a63567c390bf3a4e2708fe9, number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb))
2025-05-19T09:59:32.450373Z DEBUG engine::tree: Inserting new block into tree block=NumHash { number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb } parent=0x5e662fa0a24d51e0ba7a3a42bf38492cf3c7e2e76a63567c390bf3a4e2708fe9 state_root=0x88e23c63a35c8db36a6ee14d4e4c465dbffe1b1dffd26129bccad44dfeff072c
2025-05-19T09:59:32.450952Z DEBUG engine::tree: found canonical state for block in memory, creating provider builder hash=0x5e662fa0a24d51e0ba7a3a42bf38492cf3c7e2e76a63567c390bf3a4e2708fe9 historical=0xb5ca6c1b644b668c65f5f04c9870e148890c2a0d393e7d070f3f04255fabea22
2025-05-19T09:59:32.451028Z DEBUG engine::tree: Executing block block=NumHash { number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb }
2025-05-19T09:59:32.489369Z DEBUG engine::tree: Executed block elapsed=38.32586ms number=7257010
2025-05-19T09:59:32.490296Z DEBUG engine::tree: Calculating block state root block=NumHash { number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb }
2025-05-19T09:59:32.490359Z DEBUG engine::tree: Parent found in memory parent_hash=0x5e662fa0a24d51e0ba7a3a42bf38492cf3c7e2e76a63567c390bf3a4e2708fe9 historical=0xb5ca6c1b644b668c65f5f04c9870e148890c2a0d393e7d070f3f04255fabea22 blocks=2
2025-05-19T09:59:32.490387Z DEBUG engine::tree: Empty revert state block_number=7257007 best_block_number=7257007
2025-05-19T09:59:32.490922Z DEBUG trie::parallel_state_root: pre-calculating storage roots len=28
2025-05-19T09:59:32.555150Z  INFO engine::tree: Regular root task finished block=NumHash { number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb } regular_state_root=0x88e23c63a35c8db36a6ee14d4e4c465dbffe1b1dffd26129bccad44dfeff072c
2025-05-19T09:59:32.555177Z DEBUG engine::tree: Calculated state root root_elapsed=64.860128ms block=NumHash { number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb }
2025-05-19T09:59:32.555208Z DEBUG engine::tree: updating pending block pending=NumHash { number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb }
2025-05-19T09:59:32.555238Z DEBUG engine::tree: Finished inserting block block=NumHash { number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb }
2025-05-19T09:59:32.555366Z DEBUG reth::cli: Event: Handler(CanonicalBlockAdded(NumHash { number: 7257010, hash: 0xb3d7371aad03995065cc4a643f0af42a54c33428ffd2f4ad27dfe46ee6f825bb }, 104.336319ms))
2025-05-19T09:59:32.555714Z DEBUG engine::caching: Updated state caches
```

### Platform(s)

Unix

### Container Type

Kubernetes

### What version/commit are you on?

I am using version 1.3.12 of op-reth

### What database version are you on?

2

### Which chain / network are you on?

soneium-mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
