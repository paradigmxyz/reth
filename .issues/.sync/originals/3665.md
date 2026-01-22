---
title: 'Tracking: discovery quality metrics'
labels:
    - A-devp2p
    - A-discv4
    - A-discv5
    - A-observability
    - C-enhancement
    - C-tracking-issue
    - D-good-first-issue
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.969342Z
info:
    author: Rjected
    created_at: 2023-07-07T18:48:08Z
    updated_at: 2025-08-26T09:52:43Z
---

### Describe the feature

I was very impressed by https://github.com/ethereum/go-ethereum/pull/27621 and thought it would be a good idea for us to replicate some of these metrics. This tracks what we would need to add in reth, and what we would need to add in the existing grafana dashboard. Each of these tasks are likely small, so anyone who would like to take one should let me know, and I'll create an issue to track the individual task!

# [Reth - Peer Discovery dashboard](https://reth.paradigm.xyz/d/fd2d69b5-ca32-45d0-946e-c00ddcd7052c/reth---peer-discovery?orgId=1&refresh=30s)

## Discovery

We would need an additional metric for nodes in kbuckets, and graph in grafana:
```[tasklist]
### Discovery Metrics
- [ ] `discover/bucket/{index}/count` - the number of nodes in bucket index
- [x] Dashboard for the kbuckets metric: https://reth.paradigm.xyz/d/fd2d69b5-ca32-45d0-946e-c00ddcd7052c/reth---peer-discovery?orgId=1&refresh=30s&from=1724829418332&to=1724833018332&viewPanel=198; https://reth.paradigm.xyz/d/fd2d69b5-ca32-45d0-946e-c00ddcd7052c/reth---peer-discovery?orgId=1&refresh=30s&from=1724829458983&to=1724833058983&viewPanel=201
```

# [Reth - Transaction Pool dashboard](https://reth.paradigm.xyz/d/d47d679c-c3b8-40b6-852d-cbfaa2dcdb37/reth---transaction-pool?orgId=1&refresh=30s)

## `eth` handshake

We would need to create metrics for `eth` handshake errors that map to these:
```[tasklist]
### Eth protocol metrics
- [ ] `eth/protocols/eth/{ingress|egress}/handshake/error/peer` - unexpected peer behavior
- [ ] `eth/protocols/eth/{ingress|egress}/handshake/error/timeout` - handshake timeout
- [ ] `eth/protocols/eth/{ingress|egress}/handshake/error/network` - wrong network id
- [ ] `eth/protocols/eth/{ingress|egress}/handshake/error/version` - wrong protocol version
- [ ] `eth/protocols/eth/{ingress|egress}/handshake/error/genesis` - wrong genesis
- [ ] `eth/protocols/eth/{ingress|egress}/handshake/error/forkid` - wrong forkid
```

## `p2p` dials

We already have some error metrics:
https://github.com/paradigmxyz/reth/blob/526f624e1cbfd659184873bffe12ec602678b57f/crates/net/network/src/metrics.rs#L64-L110

So we should make sure that the error metrics are in the grafana dashboard and named appropriately, and add the metrics which we don't already have:
```[tasklist]
### P2P dial metrics
- [ ] `p2p/dials/error/connection` - unable to initiate TCP connection to target
- [ ] `p2p/dials/error/saturated` - local client is already connected to its maximum number of peers
- [ ] `p2p/dials/error/known` - dialing an already connected peer
- [ ] `p2p/dials/error/self` - dialing the local node's id
- [ ] `p2p/dials/error/useless` - dialing a peer that doesn't share an capabilities with the local node
- [ ] `p2p/dials/error/id/unexpected` - dialed peer repsoned with different id than expected
- [ ] `p2p/dials/error/rlpx/enc` - error negotiating the rlpx encryption during dial
- [ ] `p2p/dials/error/rlpx/proto` - error during rlpx protocol handshake`
- [ ] https://github.com/paradigmxyz/reth/issues/3667
```
Out of these, I don't think we have a metric similar to `p2p/dials/success`.

## Grafana
In addition to these, it would be nice to create a graph similar to the `Dial Quality` graph from the original PR:
![image](https://github.com/ethereum/go-ethereum/assets/14004106/773ae2b6-5d36-48d7-bd37-bc7829317443)

### Additional context

_No response_
