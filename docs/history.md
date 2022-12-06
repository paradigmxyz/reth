# Reth development history

This document intends to communicate the history for each component of the Reth stack.

## Networking

* [`Sentry`](https://github.com/vorot93/sentry), a pluggable p2p node following the [Erigon gRPC architecture](https://erigon.substack.com/p/current-status-of-silkworm-and-silkrpc):
    - [`vorot93`](https://github.com/vorot93/) first started by implementing a rust devp2p stack in [`rust-ethereum/devp2p`](https://github.com/vorot93/devp2p) (moved to vorot93/devp2p)
    - vorot93 then started work on sentry, using rust-ethereum/devp2p, to satisfy the erigon architecture of modular components connected with gRPC.
    - The code from rust-ethereum/devp2p was merged into sentry, and rust-ethereum/devp2p was archived
    - vorot93 was working concurrently on akula, which used sentry to connect to the network.
    - The code from sentry was merged into akula, and sentry was deleted (hence the 404 on the sentry link)

* [`Ranger`](https://github.com/Rjected/ranger), an ethereum p2p client capable of interacting with peers without a full node:
    * [Rjected](https://github.com/Rjected/) built Ranger for p2p network research
    * It required extracting devp2p-rs from Akula's sentry directory to a separate repository (keeping the GPL License), so that it could be used in Ranger (also GPL Licensed)
    * It also required creating [`ethp2p`](https://github.com/Rjected/ethp2p), a clean room implementation of the [eth wire](https://github.com/ethereum/devp2p/blob/master/caps/eth.md) protocol, for use in ranger.

 * [`Reth's P2P Stack`](../crates/net/)
    * When reth development began, we explored integrating components from devp2p-rs into reth, but ultimately found it hard to test and modularize
    * As a result, we iterated on a design for a devp2p networking stack that would not have these limitations.
    * This resulted in [#64](https://github.com/foundry-rs/reth/issues/64), which focused on layering dependent subprotocols as generic async streams, then using those streams to construct higher level network abstractions.
    * We only [extracted]() the [ECIES](https://en.wikipedia.org/wiki/Integrated_Encryption_Scheme) implementation from devp2p-rs [from back when it was licensed as Apache](https://github.com/akula-bft/akula/blob/74b172ee1d2d2a4f04ce057b5a76679c1b83df9c/src/sentry/devp2p/ecies/proto.rs), and left everything else behind.
    * Following the above design, we then implemented `P2PStream` and `EthStream`, corresponding to the `p2p` and `eth` subprotocol respectively.
    * The wire protocol used to decode messages in `EthStream` came from ethp2p, making it easy to get the full stack to work.
