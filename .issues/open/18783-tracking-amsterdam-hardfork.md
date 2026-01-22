---
title: 'Tracking: Amsterdam Hardfork'
labels:
    - C-enhancement
    - C-tracking-issue
    - E-Amsterdam
assignees:
    - mediocregopher
    - rakita
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.999793Z
info:
    author: jenpaff
    created_at: 2025-09-30T09:34:47Z
    updated_at: 2025-12-14T16:39:59Z
---

## Overview 

Tracking issue for **Glamsterdam** hardfork implementation in Reth.
Meta EIP: [EIP-7773](https://eips.ethereum.org/EIPS/eip-7773)

## EIPs Scheduled for Inclusion

### [EIP-7732: Enshrined Proposer-Builder Separation](https://eips.ethereum.org/EIPS/eip-7732)

**TLDR**: Decouples block validation by introducing temporal separation where validators commit to execution payloads in advance, and a Payload Timeliness Committee (PTC) attests to availability, deferring full execution validation to the next block.

 ### [EIP-7928: Block-Level Access Lists (BALs)](https://eips.ethereum.org/EIPS/eip-7928)

**TLDR**: Introduces block-level access lists recording all state accesses during execution, enabling parallel transaction execution, parallel disk reads, and executionless state updates.

## Activation Timeline

| Network | Activation Epoch | Activation Timestamp |
|---------|------------------|----------------------|
| Sepolia | TBD              | TBD                  |
| Holešky | TBD              | TBD                  |
| Mainnet | TBD              | TBD                  |

## References
- [Ethereum Magicians Discussion](https://ethereum-magicians.org/t/eip-7773-glamsterdam-network-upgrade-meta-thread/21195)
