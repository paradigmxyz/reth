# <h1 align="center"> reth 🏗️🚧 </h1>

**Modular, contributor-friendly and blazing-fast implementation of the Ethereum protocol**

![](./assets/reth.jpg)

*The project is still work in progress, see the [disclaimer below](#-warning-under-construction-).*

[![CI status](https://github.com/paradigmxyz/reth/workflows/ci/badge.svg)][gh-ci]
[![cargo-deny status](https://github.com/paradigmxyz/reth/workflows/deny/badge.svg)][gh-deny]
[![Codecov](https://img.shields.io/codecov/c/github/paradigmxyz/reth?token=c24SDcMImE)][codecov]
[![Telegram Chat][tg-badge]][tg-url]

[tg-badge]: https://img.shields.io/endpoint?color=neon&logo=telegram&label=chat&style=flat-square&url=https%3A%2F%2Ftg.sumanjay.workers.dev%2Fparadigm%5Freth
[tg-url]: https://t.me/paradigm_reth

# What is Reth? What are its goals?

Reth (short for Rust Ethereum, [pronunciation](https://twitter.com/kelvinfichter/status/1597653609411268608)) is a new Ethereum full node implementation that is focused on being user-friendly, highly modular, as well as being fast and efficient. Reth is an Execution Layer (EL) and is compatible with all Ethereum Consensus Layer (CL) implementations that support the [Engine API](https://github.com/ethereum/execution-apis/tree/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine). It is originally built and driven forward by [Paradigm](https://paradigm.xyz/), and is licensed under the Apache and MIT licenses.

As a full Ethereum node, Reth allows users to connect to the Ethereum network and interact with the Ethereum blockchain. This includes sending and receiving transactions/logs/traces, as well as accessing and interacting with smart contracts. Building a successful Ethereum node requires creating a high-quality implementation that is both secure and efficient, as well as being easy to use on consumer hardware. It also requires building a strong community of contributors who can help support and improve the software.

More concretely, our goals are:
1. **Modularity**: Every component of Reth is built to be used as a library: well-tested, heavily documented and benchmarked. We envision that developers will import the node's crates, mix and match, and innovate on top of them. Examples of such usage include but are not limited to spinning up standalone P2P networks, talking directly to a node's database, or "unbundling" the node into the components you need. To achieve that, we are licensing Reth under the Apache/MIT permissive license. You can learn more about the project's components [here](./docs/repo/layout.md).
2. **Performance**: Reth aims to be fast, so we used Rust and the [Erigon staged-sync](https://erigon.substack.com/p/erigon-stage-sync-and-control-flows) node architecture. We also use our Ethereum libraries (including [ethers-rs](https://github.com/gakonst/ethers-rs/) and [revm](https://github.com/bluealloy/revm/)) which we’ve battle-tested and optimized via [Foundry](https://github.com/foundry-rs/foundry/).
3. **Free for anyone to use any way they want**: Reth is free open source software, built for the community, by the community. By licensing the software under the Apache/MIT license, we want developers to use it without being bound by business licenses, or having to think about the implications of GPL-like licenses.
4. **Client Diversity**: The Ethereum protocol becomes more antifragile when no node implementation dominates. This ensures that if there's a software bug, the network does not finalize a bad block. By building a new client, we hope to contribute to Ethereum's antifragility.
5. **Support as many EVM chains as possible**: We aspire that Reth can full-sync not only Ethereum, but also other chains like Optimism, Polygon, BNB Smart Chain, and more. If you're working on any of these projects, please reach out.
6. **Configurability**: We want to solve for node operators that care about fast historical queries, but also for hobbyists who cannot operate on large hardware. We also want to support teams and individuals who want both sync from genesis and via "fast sync". We envision that Reth will be configurable enough and provide configurable "profiles" for the tradeoffs that each team faces.


# Status

The project is not ready for use. We hope to have full sync implemented sometime in January/February 2023, followed by optimizations. In the meantime, we're working on making sure every crate of the repository is well documented, abstracted and tested.

---

# For Developers

## Running Reth

See the [Reth Book](https://paradigmxyz.github.io/reth) for instructions on how to run Reth.

## Build & Test

Rust minimum required version to build this project is 1.65.0 published 02.11.2022

Prerequisites: libclang, `libclang-dev` on Debian

To test Reth, you will need to have [Geth  installed.](https://geth.ethereum.org/docs/getting-started/installing-geth)

```sh
git clone https://github.com/paradigmxyz/reth
cd reth
cargo test --all
```


## Contributing and Getting Help

If you want to contribute, or follow along with contributor discussion, you can use our [main telegram](https://t.me/paradigm_reth) to chat with us about the development of Reth!

See our [Project Layout](./docs/repo/layout.md) to understand more about the repository's structure, and descriptions about each package.

If you have any questions, first see if the answer to your question can be found in the [book][book].

If the answer is not there:

-   Join the [Telegram][tg-url] to get help, or
-   Open a [discussion](https://github.com/paradigmxyz/reth/discussions/new) with your question, or
-   Open an issue with [the bug](https://github.com/paradigmxyz/reth/issues/new)

Guidelines on how to contribute can be found in our [`CONTRIBUTING.md`](./CONTRIBUTING.md). Get started with contributing in our [contributor docs](./docs)

# Security

See [`SECURITY.md`](./SECURITY.md).

# Acknowledgements

Reth is a new implementation of the Ethereum protocol. In the process of developing the node we investigated the design decisions other nodes have made to understand what is done well, what is not, and where we can improve the status quo.

None of this would have been possible without them, so big shoutout to the teams below:
* [Geth](https://github.com/ethereum/go-ethereum/): We would like to express our heartfelt gratitude to the go-ethereum team for their outstanding contributions to Ethereum over the years. Their tireless efforts and dedication have helped to shape the Ethereum ecosystem and make it the vibrant and innovative community it is today. Thank you for your hard work and commitment to the project.
* [Erigon](https://github.com/ledgerwatch/erigon) (fka Turbo-Geth): Erigon pioneered the ["Staged Sync" architecture](https://erigon.substack.com/p/erigon-stage-sync-and-control-flows) that Reth is using, as well as [introduced MDBX](https://github.com/ledgerwatch/erigon/wiki/Choice-of-storage-engine) as the database of choice. We thank Erigon for pushing the state of the art research on the performance limits of Ethereum nodes.
* [Akula](https://github.com/akula-bft/akula/): Reth uses forks of the Apache versions of Akula's [MDBX Bindings](https://github.com/paradigmxyz/reth/pull/132), [FastRLP](https://github.com/paradigmxyz/reth/pull/63) and [ECIES](https://github.com/paradigmxyz/reth/pull/80) . Given that these packages were already released under the Apache License, and they implement standardized solutions, we decided not to reimplement them to iterate faster. We thank the Akula team for their contributions to the Rust Ethereum ecosystem and for publishing these packages.

[codecov]: https://app.codecov.io/gh/paradigmxyz/reth
[gh-ci]: https://github.com/paradigmxyz/reth/actions/workflows/ci.yml
[gh-deny]: https://github.com/paradigmxyz/reth/actions/workflows/deny.yml
[book]: https://paradigmxyz.github.io/reth/

# 🚧 WARNING: UNDER CONSTRUCTION 🚧

This project is work in progress and subject to frequent changes as we are still working on wiring up each individual node component into a full syncing pipeline.

It has not been audited for security purposes and should not be used in production yet.

We will be updating the documentation with the completion status of each component, as well as include more contributing guidelines (design docs, architecture diagrams, repository layouts) and "good first issues". See the "Contributing and Getting Help" section above for more.

We appreciate your patience until we get there. Until then, we are happy to answer all questions in the Telegram link above.
