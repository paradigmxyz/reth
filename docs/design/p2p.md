# P2P

> Explanation of the [Reth P2P Stack](../../crates/net/p2p) design process

* Our initial design exploration started in [#64](https://github.com/paradigmxyz/reth/issues/64), which focused on layering dependent subprotocols as generic async streams, then using those streams to construct higher level network abstractions.
* Following the above design, we then implemented `P2PStream` and `EthStream`, corresponding to the `p2p` and `eth` subprotocol respectively.
* The wire protocol used to decode messages in `EthStream` came from ethp2p, making it easy to get the full stack to work.
