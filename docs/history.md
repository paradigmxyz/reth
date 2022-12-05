# Reth development history

## Networking

History of sentry:
 - vorot93 first started implementing a rust devp2p stack in rust-ethereum/devp2p
 - vorot93 then started work on sentry, using rust-ethereum/devp2p, to satisfy the erigon architecture of modular components connected with gRPC.
 - The code from rust-ethereum/devp2p was merged into sentry, and rust-ethereum/devp2p was archived
 - vorot93 was working concurrently on akula, which used sentry to connect to the network.
 - The code from sentry was merged into akula, and sentry was archived.

Introducing ethp2p and devp2p-rs:
 - In the meantime, ethp2p was created as a clean room implementation of the eth wire protocol, for use in ranger.
 - Shortly after akula switched to GPL, the devp2p code from akula was extracted into devp2p-rs (also for use in ranger), from akula's `sentry/` directory.

Origins of Reth:
 - As reth development began, we tried to integrate components from devp2p-rs into reth. Realizing limitations (should we talk about limitations here? simple example is testability - see og testing PR), we started working on a design for a devp2p networking stack that would not have these limitations.

 - This resulted in #64, which focused on layering dependent subprotocols as generic async streams, then using those streams to construct higher level network abstractions.
 - We extracted the ECIES implementation from devp2p-rs, and left everything else behind.
 - Following the above design, we then implemented `P2PStream` and `EthStream`, corresponding to the `p2p` and `eth` subprotocol respectively.
 - The wire protocol used to decode messages in `EthStream` came from ethp2p, making it easy to get the full stack to work.

