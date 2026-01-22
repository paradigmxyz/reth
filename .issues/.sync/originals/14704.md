---
title: 'Tracking: SDK examples'
labels:
    - A-sdk
    - C-tracking-issue
assignees:
    - jenpaff
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.982241Z
info:
    author: jenpaff
    created_at: 2025-02-25T16:03:57Z
    updated_at: 2025-07-15T17:27:55Z
---

## Feature Description

We want to be able to express EVM chains via the Reth SDK, enabling these chains to leverage our standardized infrastructure rather than maintaining their own forks.

### Problem Statement
There are several teams that fork Reth and find it difficult to maintain the fork. We want these teams to be able to use the Reth SDK for their specific chain requirements.

### In Scope
- Work with World-chain (they already use Reth SDK), incorporate their suggestions & improvements:
- Document the process for other chains to leverage Reth SDK, creating a clear path for adoption
- Examples we want to showcase: custom Header and Transaction Type, custom RPC Type, custom engine API
