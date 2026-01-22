---
title: 'feat: more efficient unwinding strategy'
labels:
    - A-op-reth
    - A-staged-sync
    - C-perf
    - M-prevent-stale
    - S-feedback-wanted
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.985907Z
info:
    author: "0x416e746f6e"
    created_at: 2025-04-14T09:50:12Z
    updated_at: 2025-09-10T16:51:39Z
---

### Describe the feature

when `op-reth` detects that it needs to unwind the chain - e.g. due to a (deep) re-org, or after a long (more than 12h) period of batcher being down, it does that sequentially (i.e. step-by-step from current head back to the point where local block hash matches the one from canonical chain):

<img width="689" alt="Image" src="https://github.com/user-attachments/assets/e765ba23-61c0-4bfd-a74f-b8b4cee96902" />

^-- this process can take quite long time.  e.g. in one case we observed the "speed" of unwinding was ~400 blocks per hour.

we were wondering if the unwind could be sped up with dichotomy?  for example:  

- set `min=0` (genesis); and `max=current` (the highest block no. `reth` has locally)
  - `min` could be set to the finalised head in case of `op-reth`
- set `probe=(min+max)/2`
- query block at `probe` height
  - if the hashes don't match:  set `max=probe`
  - if the hashed do match:  set `min=probe`

^-- repeat until `min==max`

### Additional context

_No response_
