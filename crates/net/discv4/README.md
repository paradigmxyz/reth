# <h1 align="center"> discv4 </h1>

This is a rust implementation of
the [Discovery v4](https://github.com/ethereum/devp2p/blob/40ab248bf7e017e83cc9812a4e048446709623e8/discv4.md)
peer discovery protocol.

For comparison to Discovery v5,
see [discv5#comparison-with-node-discovery-v4](https://github.com/ethereum/devp2p/blob/40ab248bf7e017e83cc9812a4e048446709623e8/discv5/discv5.md#comparison-with-node-discovery-v4)

This is inspired by the [discv5](https://github.com/sigp/discv5) crate and reuses its kademlia implementation.

## Finding peers

The discovery service continuously attempts to connect to other nodes on the network until it has found enough peers.
If UPnP (Universal Plug and Play) is supported by the router the service is running on, it will also accept connections
from external nodes. In the discovery protocol, nodes exchange information about where the node can be reached to
eventually establish ``RLPx`` sessions.

## Trouble Shooting

The discv4 protocol depends on the local system clock. If the clock is not accurate it can cause connectivity issues
because the expiration timestamps might be wrong.
