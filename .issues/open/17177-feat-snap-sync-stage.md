---
title: 'feat: snap sync stage'
labels:
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 15432
synced_at: 2026-01-21T11:32:15.992064Z
info:
    author: rkrasiuk
    created_at: 2025-07-02T13:47:57Z
    updated_at: 2025-10-05T17:42:04Z
---

### Describe the feature

Create new `SnapSyncStage` that will be responsible for querying peers for ranges of trie data and inserting it into the database. Leave the healing algorithm and database inserts of trie nodes as placeholders for now. If enabled, this stage should _replace_ `SenderRecoveryStage`, `ExecutionStage` and `PruneSenderRecoveryStage`.

The stage should subscribe to the stream of head headers from the consensus engine to be able to update the target state root in snap peer requests.

The stage should adhere to the following algorithm:
1. Retrieve the latest header from the engine
2. Check if the hashed state in `tables::HashedAccounts` is empty:
  a. empty - start downloading ranges from `0x0000...` account hash
  b. not empty - start downloading ranges from the last entry in the database
3. Paginate over trie ranges using `GetAccountRange` request (leave storage ranges for the follow up)
  a. If no data is returned for current target state root, return to 1. 
4. Repeat 1. to 3. until the state is retrieved (final range until account `0xffff...` is fetched)

### Additional context

_No response_
