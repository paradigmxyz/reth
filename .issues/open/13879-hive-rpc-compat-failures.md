---
title: hive rpc-compat failures
labels:
    - A-ci
    - C-hivetest
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.98079Z
info:
    author: fgimenez
    created_at: 2025-01-20T14:20:16Z
    updated_at: 2025-08-27T13:16:21Z
---

- [ ] debug_getRawBlock/get-invalid-number (reth)
- [ ] debug_getRawHeader/get-invalid-number (reth)
- [ ] debug_getRawReceipts/get-invalid-number (reth)
- [ ] debug_getRawReceipts/get-block-n (reth)
- [ ] debug_getRawTransaction/get-invalid-hash (reth)
- [ ] eth_call/call-callenv (reth)
- [ ] eth_getStorageAt/get-storage-invalid-key-too-large (reth)
- [ ] eth_getStorageAt/get-storage-invalid-key (reth)
- [x] eth_getTransactionReceipt/get-access-list (reth)
- [x] eth_getTransactionReceipt/get-blob-tx (reth)
- [x] eth_getTransactionReceipt/get-dynamic-fee (reth)
- [ ] eth_getTransactionReceipt/get-legacy-contract (reth)
- [ ] eth_getTransactionReceipt/get-legacy-input (reth)
- [ ] eth_getTransactionReceipt/get-legacy-receipt (reth)
 
