---
title: Support for eth_getLogsWithCursor
labels:
    - C-enhancement
    - D-good-first-issue
assignees:
    - iTranscend
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.001796Z
info:
    author: kahuang
    created_at: 2025-11-03T19:07:36Z
    updated_at: 2025-11-22T10:08:35Z
---

### Describe the feature

On high-performance chains that generate lots of logs, its cumbersome to use eth_getLogs to fetch all the logs from a block range. RPCs typically have a limit (20k by default), and otherwise uncapping the limit causes bloat in response sizes and requires all middleware to handle the increased size.

Proposal: new method, eth_getLogsWithCursor. Adds an optional cursor field which allows pagination of a response, and also returns a cursor, if there are more results to fetch.

Example flow:
eth_getLogsWithCursor(0,100,cursor=nil)
resp... logs + cursor=[b64 encoded opaque value]

To continue pagination
eth_getLogsWIthCursor(0,100,cursor=[b64 encoded opaque value])

If no cursor returned, all results were included in the response.

### Additional context

_No response_
