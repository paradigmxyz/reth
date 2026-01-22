---
title: TransactionHashNumbers Insert Performance Degrades Rapidly with Scale
labels:
    - A-db
    - C-perf
    - S-needs-design
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.005615Z
info:
    author: duyquang6
    created_at: 2025-11-13T02:35:35Z
    updated_at: 2026-01-20T02:17:04Z
---

## Summary

When attempting to sustain high transaction throughput (~70k TPS) on our devnet with **1-second block time** for approximately 5 minutes, block persistence performance degrades rapidly. The primary bottleneck is identified as inserting records into the `TransactionHashNumbers` table, which uses random hash keys (B256) causing expensive random I/O operations in MDBX's B-tree structure.

**Critical Impact:**
- `TransactionHashNumbers` insertion consumes a significant portion of the block persistence time, which make persistence task often take more than 1 second per block to just persist on high-end hardware
- The random key inserts cause **massive freelist growth** (10x increase), indicating severe page fragmentation
- Cascading delay problem: Under spammer load (~70k TPS), block flushing delays increase from 3 blocks to 6 blocks, and persistence time grows from ~1s to >1s per block, creating a feedback loop that worsens over time. Its is consequence of increasing time on save block & tx commit
- The random key inserts cause severe page fragmentation, evidenced by massive freelist growth:

Dirty idea to isolate issue: Remove `TransactionHashNumbers` out of block persistent to verify

With `TransactionHashNumbers` writes enabled (5-minute spammer test):
- Freelist size growth upto ~250,000 pages
- Indicates frequent page splits and rebalancing
- High fragmentation from random insert patterns -> make commit time more heavy under stress test

Blocks need to persist keep increasing, match with latency 6s of persist duration
<img width="1687" height="441" alt="Image" src="https://github.com/user-attachments/assets/e6b69243-eb02-442e-bf02-38586a8fb905" />

With `TransactionHashNumbers` writes disabled:
- Freelist size: ~30,000 pages
- 10x reduction in freelist size
- Reduce persist time 6s -> 2s in bench window 5min

Details:
Freelist before:
<img width="1549" height="299" alt="Image" src="https://github.com/user-attachments/assets/f399f860-a4bd-4cf7-9f3b-c93e54fd0354" />

Freelist after remove insert `TransactionHashNumbers`
<img width="1523" height="381" alt="Image" src="https://github.com/user-attachments/assets/118f5519-b38c-480a-a12a-03b88160fe9a" />

Persistent time before:

<img width="1707" height="413" alt="Image" src="https://github.com/user-attachments/assets/0629e215-0261-451e-86d6-75c0391aceff" />

With default config -> there is 3 blocks flush persistent per interval
At the end of the chart it need persist 6 blocks because persistent cannot keep up with speed of block creation -> so need 6s

Persistent time after removal:

<img width="1658" height="488" alt="Image" src="https://github.com/user-attachments/assets/7f9e2c11-9e65-4b7c-a22a-45239a144827" />

With default config -> there is 3 blocks flush persistent per interval -> the chart show under 3 second which is healthy

Insert block duration before (not include commit & update_history_indices time):

<img width="1660" height="496" alt="Image" src="https://github.com/user-attachments/assets/f16fe18e-6c04-44f0-b962-89f860ea6b42" />

Duration after removal:

<img width="1648" height="511" alt="Image" src="https://github.com/user-attachments/assets/0dd014b5-9ea5-4f5b-8973-6925f8e637a9" />

## Proposed Solutions

One idea that is move this random costly insert out of currently main persistent task, and handle it differently


### Additional context




Environment:
- AMD Ryzen 9 7950X3D (32 cores), NVMe 1TB SSD, 128GB RAM
- Block time 1s
- reth commit: `71c12479`
- Code related:
https://github.com/paradigmxyz/reth/blob/71c124798c8a15e6aeec1b4a4e6e9fc24073d6ac/crates/engine/tree/src/persistence.rs#L155-L160
https://github.com/paradigmxyz/reth/blob/71c124798c8a15e6aeec1b4a4e6e9fc24073d6ac/crates/storage/provider/src/providers/database/provider.rs#L2823-L2825

- Log Evidence when enable spammer:
```
2025-11-12 18:36:56.028 DEBUG engine::persistence: Saving range of blocks first=Some(NumHash { number: 308 }) last=Some(NumHash { number: 313 })
2025-11-12 18:36:56.027 DEBUG engine::persistence: Saved range of blocks first=Some(NumHash { number: 302 }) last=Some(NumHash { number: 307 })
2025-11-12 18:36:50.015 DEBUG engine::persistence: Saving range of blocks first=Some(NumHash { number: 302 }) last=Some(NumHash { number: 307 })
2025-11-12 18:36:50.014 DEBUG engine::persistence: Saved range of blocks first=Some(NumHash { number: 296 }) last=Some(NumHash { number: 301 })
2025-11-12 18:36:44.002 DEBUG engine::persistence: Saving range of blocks first=Some(NumHash { number: 296 }) last=Some(NumHash { number: 301 })
```
block persist is late, backlog 6 blocks to persist when spammer enable


- With simple isolated write test, pre-insert 30M records in `TransactionHashNumber`, it take ~500ms for mdbx to insert batch 70k records

- MDBX stats at 30M records tx hash numbers, BTree depth = 5 with over 558k leaf pages 

```
Running for mdbx.dat...
   open-MADV_DONTNEED 1954448..2097152
   readahead ON 0..1954443
Environment Info
  Pagesize: 4096
  Dynamic datafile: 12288..8796093022208 bytes (+4294967296/-0), 3..2147483648 pages (+1048576/-0)
  Current mapsize: 8796093022208 bytes, 2147483648 pages 
  Current datafile: 8589934592 bytes, 2097152 pages
  Last transaction ID: 20382
  Latter reader transaction ID: 20382 (0)
  Max readers: 112
  Number of reader slots uses: 1
Garbage Collection
  Pagesize: 4096
  Tree depth: 2
  Branch pages: 1
  Leaf pages: 5
  Overflow pages: 402
  Entries: 405
Page Usage
  Total: 2147483648 100%
  Backed: 2097152 0.10%
  Allocated: 1954443 0.09%
  Remained: 2145529205 99.9%
  Used: 1544876 0.07%
  GC: 409567 0.02%
  Reclaimable: 408762 0.02%
  Retained: 805 0.00%
  Available: 2145937967 99.9%
 Status of TransactionHashNumbers
  Pagesize: 4096
  Tree depth: 5
  Branch pages: 8631
  Leaf pages: 558653
  Overflow pages: 0
  Entries: 31151579
```
