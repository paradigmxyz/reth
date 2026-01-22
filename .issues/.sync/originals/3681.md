---
title: 'feature request: index function calls'
labels:
    - A-db
    - A-rpc
    - A-staged-sync
    - C-enhancement
    - D-complex
    - M-prevent-stale
    - S-feedback-wanted
    - S-needs-investigation
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.969717Z
info:
    author: charles-cooper
    created_at: 2023-07-09T15:26:15Z
    updated_at: 2025-03-11T11:12:40Z
---

### Describe the feature

index function calls and provide an interface similar to events, so that eventless contracts are more feasible in the sense that they a) don't require external tooling and b) can provide similar or better performance guarantees as events.

i haven't thought about it too much, but the interface should basically be like `eth_getLogs` but under a new RPC method (maybe `eth_getCalls`). the params should be largely the same, although the topics api needs to change. i'm thinking the topic can be one of three options:
- method id,
- word id (which word in calldata, after the method id),
- offset and length in calldata.

the last option is for future compatibility in case the abi changes (a la https://github.com/vyperlang/vyper/issues/2542), or in case users have their own way of calling a contract. the first two options basically translate to calldata[0:4] and calldata[32 * word_id + 4: 32 * word_id + 36].

i think the optimization of the queries should be left up to the node, but i can think of a few strategies:
- naive search, this is probably a good default
- pre-indexing certain kinds of queries. probably method ids (and maybe calldata words which are found to have low cardinality) are a good candidate here
- "smart" indexing - if a particular kind of query is noticed to be a hotspot, it can be indexed on the fly
- add an RPC method or other option to provide manual control of indexing to the user, e.g. `admin_indexCalls`

### Additional context

_No response_
