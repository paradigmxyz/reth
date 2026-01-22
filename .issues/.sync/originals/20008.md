---
title: Discv5 ENR auto update should be disabled when NAT is disabled or explicit addresses are set
labels:
    - A-networking
    - C-bug
assignees:
    - 0xVasconcelos
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.009101Z
info:
    author: 0xVasconcelos
    created_at: 2025-11-27T08:09:08Z
    updated_at: 2025-11-27T09:16:18Z
---

### Describe the bug

I am setting up a node with fixed public IP + port. When manually set address:port, the UDP port get later replaced by the ENR auto update.

```
➜  ~ discv5-cli request-enr --multiaddr /ip4/34.159.243.134/udp/30303/p2p/16Uiu2HAm9eEpzaZKR2Q9sekPE9WDcqtVLGmGFV1tSJvXYxfCCjqk
2025-11-25T13:02:41.515Z INFO  [discv5_cli::request_enr] Requesting ENR for: /ip4/34.159.243.134/udp/30303/p2p/16Uiu2HAm9eEpzaZKR2Q9sekPE9WDcqtVLGmGFV1tSJvXYxfCCjqk
2025-11-25T13:02:41.515Z INFO  [discv5::service] Discv5 Service started mode=Ip4
2025-11-25T13:02:41.932Z INFO  [discv5_cli::request_enr] ENR Found:
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] Sequence No:2
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] NodeId:0x400e..8b9c
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] Libp2p PeerId:16Uiu2HAm9eEpzaZKR2Q9sekPE9WDcqtVLGmGFV1tSJvXYxfCCjqk
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] IP:34.159.58.213
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] TCP Port:30303
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] UDP Port:45056
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] Known multiaddrs:
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] /ip4/34.159.58.213/udp/45056/p2p/16Uiu2HAm9eEpzaZKR2Q9sekPE9WDcqtVLGmGFV1tSJvXYxfCCjqk
2025-11-25T13:02:41.933Z INFO  [discv5_cli::request_enr] /ip4/34.159.58.213/tcp/30303/p2p/16Uiu2HAm9eEpzaZKR2Q9sekPE9WDcqtVLGmGFV1tSJvXYxfCCjqk
```


I have manually added this patch below in my node fork and seems to work properly for my use case. But maybe this could break other things. I would like to open a PR with a patch if this is the correct approach to solve that.

```rust
if discv5_addr.is_some() || discv5_addr_ipv6.is_some() || self.disable_nat {
    builder.disable_enr_update();
}
```

### Steps to reproduce

Start op-reth with given params: 

```
--discovery.v5.port=30303
--discovery.v5.addr=34.159.243.134
--disable-nat
--disable-discv4-discovery
```

### Platform(s)

Linux (x86)

### Container Type

Kubernetes

### What version/commit are you on?

Commit 2e5a155b6d05345924f06e16a98fc2953e341126

### What database version are you on?

Current database version: 2
Local database version: 2

### Which chain / network are you on?

dev

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
