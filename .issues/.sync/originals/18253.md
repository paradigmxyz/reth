---
title: 'Tracking: EIP-7928 (BALs)'
labels:
    - C-enhancement
    - C-tracking-issue
    - E-Amsterdam
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
parent: 18783
synced_at: 2026-01-21T11:32:15.997743Z
info:
    author: Soubhik-10
    created_at: 2025-09-03T16:37:00Z
    updated_at: 2026-01-15T15:51:08Z
---

Fully implements [Block-Level Access](https://eips.ethereum.org/EIPS/eip-7928) Lists in reth.

### Problem Statement

Tx execution can't be parallelized because we don't know upfront which addresses/storage slots will be accessed. Current tx-level access lists (EIP-2930) are optional and not enforced.

### Proposed Solution

Solution: Block builders include a BAL with every block listing all accessed accounts + storage slots + post-execution values.

### Resources

* [EIP-7928 Spec](https://eips.ethereum.org/EIPS/eip-7928)
* [EEST PR](https://github.com/ethereum/execution-spec-tests/pull/2067)
* [Specs PR ](https://github.com/ethereum/execution-specs/pull/1381)
* [Remove BAL from EL block - EIPs#10937](https://github.com/ethereum/EIPs/pull/10937)
* [reth bal-devnet-1 branch](https://github.com/paradigmxyz/reth/tree/bal-devnet-1)

### Additional context
