# <h1 align="center"> reth 🏗️🚧 </h1>

**Blazing-fast implementation of the Ethereum protocol**

*The project is still work in progress, see the [disclaimer below](#-warning-under-construction-).*

[![CI status](https://github.com/foundry-rs/reth/workflows/ci/badge.svg)][gh-ci]
[![cargo-deny status](https://github.com/foundry-rs/reth/workflows/deny/badge.svg)][gh-deny]
[![Codecov](https://img.shields.io/codecov/c/github/foundry-rs/reth?token=c24SDcMImE)][codecov]
[![Telegram Chat][tg-badge]][tg-url]

[tg-badge]: https://img.shields.io/endpoint?color=neon&logo=telegram&label=chat&style=flat-square&url=https%3A%2F%2Ftg.sumanjay.workers.dev%2Fparadigm%5Freth
[tg-url]: https://t.me/paradigm_reth

# What is this?

Reth is a new Apache/MIT-licensed full node implementation of Ethereum focused on contributor friendliness, modularity, and performance - built by [Paradigm](https://paradigm.xyz/) and the community.

An Ethereum node is a piece of software that connects to the Ethereum network and allows users to send and receive transactions, as well as access and interact with the Ethereum blockchain. The Ethereum network is decentralized, which means that it relies on a network of nodes to validate and process transactions, rather than a central authority.

# What are the goals of this project?

Our goals are:
1. **Modularity**: Every component of Reth is built to be used as a library: well-tested, heavily documented and benchmarked. We envision that developers will import the node's crates, mix and match, and innovate on top of them. Examples of such usage include but are not limited to spinning up standalone P2P networks, talking directly to a node's database, or "unbundling" the node into the components you need. To achieve that, we are licensing Reth under the Apache/MIT permissive license.
2. **Performance**: Reth aims to be fast, so we used Rust and the [Erigon staged-sync](https://erigon.substack.com/p/erigon-stage-sync-and-control-flows) node architecture. We also use our Ethereum libraries (including [ethers-rs](https://github.com/gakonst/ethers-rs/) and [revm](https://github.com/bluealloy/revm/)) which we’ve battle-tested and optimized via [Foundry](https://github.com/foundry-rs/foundry/).
3. **Free for anyone to use any way they want**: Reth is free open source software, built for the community, by the community. By licensing the software under the Apache/MIT license, we want developers to use it without being bound by business licenses, or having to think about the implications of GPL-like licenses.
4. **Client Diversity**: The Ethereum protocol becomes more antifragile when no node implementation dominates. This ensures that if there's a software bug, the network does not finalize a bad block. By building a new client, we hope to contribute to Ethereum's antifragility.
5. **Support as many EVM chains as possible**: We aspire that Reth can full-sync not only Ethereum, but also other chains like Optimism, Polygon, Binance Smart Chain, and more. If you're working on any of these projects, please reach out.
6. **Archive & pruned nodes, full sync and fast syncs**: We want to solve for node operators that care about fast historical queries, but also for hobbyists who cannot operate on large hardware. We also want to support teams and individuals who want both sync from genesis and via "fast sync". We envision that Reth will be configurable enough and provide configurable "profiles" for the tradeoffs that each team faces.

# Status

The project is not ready for use. We hope to have full sync implemented sometime in January/February 2022, followed by optimizations. In the meantime, we're working on making sure every crate of the repository is well documented, abstracted and tested.

---

# For Developers

## Build & Test

Rust minimum required version to build this project is 1.65.0 published 02.11.2022

```sh
git clone https://github.com/foundry-rs/reth
cd reth
cargo test --all
```

## Contributing and Getting Help

If you want to contribute, or follow along with contributor discussion, you can use our [main telegram](https://t.me/paradigm_reth) to chat with us about the development of Reth!

If you have any questions, first see if the answer to your question can be found in the [book][book], or in the relevant [crate](./docs/repo/layout.md).

If the answer is not there:

-   Join the [Telegram][tg-url] to get help, or
-   Open a [discussion](https://github.com/foundry-rs/reth/discussions/new) with your question, or
-   Open an issue with [the bug](https://github.com/foundry-rs/reth/issues/new)

Guidelines on how to contribute can be found in our [`CONTRIBUTING.md`](./CONTRIBUTING.md). Get started with contributing in our [contributor docs](./docs)

# Security

See [`SECURITY.md`](./SECURITY.md).

# Acknowledgements

Reth is a new implementation of the Ethereum protocol, and does not include code from any existing client. In the process of developing the node we investigated the design decisions other nodes have made to understand what is done well, what is not, and where we can improve the status quo. 

None of this would have been possible without them, so big shoutout to the teams below for the inspiration:
* [Geth](https://github.com/ethereum/go-ethereum/):
* [Erigon](https://github.com/ledgerwatch/erigon) (fka Turbo-Geth): Erigon pioneered the ["Staged Sync" architecture](https://erigon.substack.com/p/erigon-stage-sync-and-control-flows) that Reth is using, as well as [introduced MDBX](https://github.com/ledgerwatch/erigon/wiki/Choice-of-storage-engine) as the database of choice.
* [Akula](https://github.com/akula-bft/akula/) (deprecated): Reth uses forks of the Apache versions of the [MDBX Bindings](https://github.com/foundry-rs/reth/pull/132), [FastRLP](https://github.com/foundry-rs/reth/pull/63) and [ECIES](https://github.com/foundry-rs/reth/pull/80) . While we could have reimplemented these packages, they implement standardized Ethereum protocols, and bindings to a database. Given that they were already released as Apache, we decided it was easier to use that code to iterate faster. We thank the Akula team for their contributions to the Rust Ethereum ecosystem and for publishing these packages.

[codecov]: https://app.codecov.io/gh/foundry-rs/reth
[gh-ci]: https://github.com/foundry-rs/reth/actions/workflows/ci.yml
[gh-deny]: https://github.com/foundry-rs/reth/actions/workflows/deny.yml
[book]: https://foundry-rs.github.io/reth/

# 🚧 WARNING: UNDER CONSTRUCTION 🚧

This project is work in progress and subject to frequent changes as we are still working on wiring up each individual node component into a full syncing pipeline.

It has not been audited for security purposes and should not be used in production yet.

We will be updating the documentation with the completion status of each component, as well as include more contributing guidelines (design docs, architecture diagrams, repository layouts) and "good first issues". See the "Contributing and Getting Help" section below for more.

We appreciate your patience until we get there. Until then, we are happy to answer all questions in the Telegram link above.
