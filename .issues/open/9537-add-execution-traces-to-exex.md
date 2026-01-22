---
title: Add execution traces to ExEx
labels:
    - A-execution
    - A-exex
    - A-sdk
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.975147Z
info:
    author: pistomat
    created_at: 2024-07-16T08:15:51Z
    updated_at: 2024-08-08T21:08:12Z
---

### Describe the feature

I want to consume execution traces in my ExEx. 

Currently the Chain struct does not contain them. The obvious solution would be to re-execute the block with a custom executor, but that would mean unnecessary additional computation. 

I hear there is a plan to add a custom executor with an inspector. As per reth telegram group discussion, I am adding this issue.

CC @mattsse @shekhirin 

### Additional context

_No response_
