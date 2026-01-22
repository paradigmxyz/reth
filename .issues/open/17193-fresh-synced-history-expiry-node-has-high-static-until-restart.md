---
title: Fresh-synced history expiry node has high static until restart
labels:
    - A-db
    - A-sdk
    - C-bug
assignees:
    - RomanHodulak
milestone: v1.11
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17230
synced_at: 2026-01-21T11:32:15.992247Z
info:
    author: yorickdowne
    created_at: 2025-07-03T10:16:35Z
    updated_at: 2026-01-13T23:45:46Z
---

### Describe the bug

A Reth node synced with history expiry will have a static directory of around 726GB in size.

After a restart of Erigon, it deletes the historic blocks and static is around 443GB.

Arguably, Reth should delete static without restart, after it is “done” with that static file - whenever that is in the sync process.

### Steps to reproduce

Sync a Reth node with history expiry, do not restart.

Observe size of static directory

Restart Reth

Observe size of static directory

### Node logs

```text

```

### Platform(s)

Linux (x86)

### Container Type

Docker

### What version/commit are you on?

v1.5.0

### What database version are you on?

Didn’t check, it’s a fresh sync on v1.5.0

### Which chain / network are you on?

Mainnet Ethereum 

### What type of node are you running?

Pruned with custom reth.toml config

### What prune config do you use, if any?

History expiry

`--block-interval 5 --prune.senderrecovery.full --prune.accounthistory.distance 10064 --prune.storagehistory.distance 100064 --prune.bodies.pre-merge --prune.receipts.before 15537394`

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
