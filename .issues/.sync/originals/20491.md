---
title: 'Flashblocks: issue when handling chainstate updates on flashblocks consensus sync vs default sync'
labels:
    - C-bug
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.015377Z
info:
    author: sieniven
    created_at: 2025-12-18T14:26:12Z
    updated_at: 2026-01-10T08:50:18Z
---

### Describe the bug

## Issue

Referencing to the really nice PR added by @0x00101010 https://github.com/paradigmxyz/reth/pull/19786, it seems like on further testing, there are 3 potentially problematic issues that could surface from this design.

1. Currently FCU sent on the flashblocks consensus sets unsafe, safe and finalized as the flashblocks sequence hash. This is incorrect and violates optimism finality states, and is resolved in the WIP PR: https://github.com/paradigmxyz/reth/pull/20490
2. The current flashblocks consensus layer doesnt take into account the possibility that the default CL sync could be ahead of flashblocks. This can be resolved pretty easily by adding a chainstate check on the flashblocks consensus client to check with the chain provider the current latest tip, and skip if the FCU call is behind the curren tip. A WIP PR https://github.com/paradigmxyz/reth/pull/20490 should resolve this.
3. The other issue is flashblocks consensus layer is ahead of the CL sync, then this situation may happen which could be undesirable.
   - flashblocks advances chainstate to x -> sends FCU to x
   - CL sync is still at x-1 -> sends FCU to x-1
   - chainstate reorgs to x-1
   - Next flashblock sequence advances chainstate to x+1

I am thinking of what are the possible solutions to resolve this - perhaps allowing flashblocks RPC to advance the EL's chainstate might not be such a good idea afterall? However the performance improvements, if the chainstate is able to sync with flashblocks, is massive. Also, resolving issue 2 is not trivial as if the resolution is on the EL layer, it might touch core protocol which is dangerous if an actual re-org needs to happen.

## Solution 1

The easiest and least complicated solution to resolve this is for flashblocks consensus layer to not be able to advance chainstate, but pre-warm the payload and insert into the locally built blocks cache. Subsequently on CL sync, engine_newPayload will skip the payload validation logic all together which will help greatly in RPC syncs.

However, this solution could limit the flashblocks RPC potential since driving the chainstate and reducing chain latencies is dependent on the default CL sync.

## Solution 2

Perhaps an alternative solution will be to design a separate accumulated flashblock sequences chainstate that is used in the exposed eth APIs. We maintain this flashblocks state layer separate from the default sync (which updates the canonical chainstate). This flashblocks state layer is flushed once the default sync's block is received, and we similarly we implement solution 1 by default to speed up default RPC sync.

### Steps to reproduce

-

### Node logs

```text
In high stress test env, we can notice that flashblocks consensus and the default sync (FCU call from CL)
```

### Platform(s)

_No response_

### Container Type

Docker

### What version/commit are you on?

main branch, v1.9.3

### What database version are you on?

-

### Which chain / network are you on?

-

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

-

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
