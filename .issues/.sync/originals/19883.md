---
title: Integrate snap-sync protocol in p2p stack
labels:
    - A-devp2p
    - C-enhancement
    - D-complex
assignees:
    - 0xKarl98
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 15432
synced_at: 2026-01-21T11:32:16.007684Z
info:
    author: mattsse
    created_at: 2025-11-20T13:39:14Z
    updated_at: 2025-11-20T13:55:40Z
---

we already have support for the protocol itself

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/eth-wire/src/eth_snap_stream.rs#L60-L60

we need to integrate this into the p2p stack:

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network/src/session/active.rs#L93-L93

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network/src/session/conn.rs#L25-L38

for this we need new variant for the ethsnap stream

since this stream then bubbles up new snap messages, we need to rout them accordingly

for example we need new request/response variants:

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network-api/src/events.rs#L184-L186

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network/src/message.rs#L84-L84

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network/src/message.rs#L58-L59

all of this is similar to how we route eth requests, 
incoming requests eventually end up at this type:

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network/src/eth_requests.rs#L55-L55

via:
https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network/src/eth_requests.rs#L306-L306

for snap we can repurpose this type so that it can also handle snaprequests, for now just noops (empty responses).

snap support must be opt-in for now, and should be configured via the config:
protocols are stored in this set:
https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/eth-wire/src/hello.rs#L33-L34

so we need another helper setter here

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network/src/config.rs#L331-L334

that is like `fn set_snap_protocols` or `with_snap(bool){self.set_snap_protocols}`


ideally we then also integrate the snap support here as well:

https://github.com/paradigmxyz/reth/blob/d550e4eb07e677733b31d5dcddb8a31de501026a/crates/net/network/src/fetch/mod.rs#L43-L47

this part can be done separately because the changes above are already quite large https://github.com/paradigmxyz/reth/issues/19882

@0xKarl98 

