# reth 

[![CI status](https://github.com/paradigmxyz/reth/workflows/ci/badge.svg)][gh-ci]
[![cargo-deny status](https://github.com/paradigmxyz/reth/workflows/deny/badge.svg)][gh-deny]
[![Codecov](https://img.shields.io/codecov/c/github/paradigmxyz/reth?token=c24SDcMImE)][codecov]
[![Telegram Chat][tg-badge]][tg-url]

**Modular, contributor-friendly and blazing-fast implementation of the Ethereum protocol**

![](./assets/reth-alpha.png)

**[Install](https://paradigmxyz.github.io/reth/installation/installation.html)**
| [User Book](https://paradigmxyz.github.io/reth)
| [Developer Docs](./docs)
| [Crate Docs](https://paradigmxyz.github.io/reth/docs)

*The project is still work in progress, see the [disclaimer below](#status).*

[codecov]: https://app.codecov.io/gh/paradigmxyz/reth
[gh-ci]: https://github.com/paradigmxyz/reth/actions/workflows/ci.yml
[gh-deny]: https://github.com/paradigmxyz/reth/actions/workflows/deny.yml
[tg-badge]: https://img.shields.io/endpoint?color=neon&logo=telegram&label=chat&url=https%3A%2F%2Ftg.sumanjay.workers.dev%2Fparadigm%5Freth

## What is Reth?

Reth (short for Rust Ethereum, [pronunciation](https://twitter.com/kelvinfichter/status/1597653609411268608)) is a new Ethereum full node implementation that is focused on being user-friendly, highly modular, as well as being fast and efficient. Reth is an Execution Layer (EL) and is compatible with all Ethereum Consensus Layer (CL) implementations that support the [Engine API](https://github.com/ethereum/execution-apis/tree/a0d03086564ab1838b462befbc083f873dcf0c0f/src/engine). It is originally built and driven forward by [Paradigm](https://paradigm.xyz/), and is licensed under the Apache and MIT licenses.

## Goals

As a full Ethereum node, Reth allows users to connect to the Ethereum network and interact with the Ethereum blockchain. This includes sending and receiving transactions/logs/traces, as well as accessing and interacting with smart contracts. Building a successful Ethereum node requires creating a high-quality implementation that is both secure and efficient, as well as being easy to use on consumer hardware. It also requires building a strong community of contributors who can help support and improve the software.

More concretely, our goals are:

1. **Modularity**: Every component of Reth is built to be used as a library: well-tested, heavily documented and benchmarked. We envision that developers will import the node's crates, mix and match, and innovate on top of them. Examples of such usage include but are not limited to spinning up standalone P2P networks, talking directly to a node's database, or "unbundling" the node into the components you need. To achieve that, we are licensing Reth under the Apache/MIT permissive license. You can learn more about the project's components [here](./docs/repo/layout.md).
2. **Performance**: Reth aims to be fast, so we used Rust and the [Erigon staged-sync](https://erigon.substack.com/p/erigon-stage-sync-and-control-flows) node architecture. We also use our Ethereum libraries (including [ethers-rs](https://github.com/gakonst/ethers-rs/) and [revm](https://github.com/bluealloy/revm/)) which we’ve battle-tested and optimized via [Foundry](https://github.com/foundry-rs/foundry/).
3. **Free for anyone to use any way they want**: Reth is free open source software, built for the community, by the community. By licensing the software under the Apache/MIT license, we want developers to use it without being bound by business licenses, or having to think about the implications of GPL-like licenses.
4. **Client Diversity**: The Ethereum protocol becomes more antifragile when no node implementation dominates. This ensures that if there's a software bug, the network does not finalize a bad block. By building a new client, we hope to contribute to Ethereum's antifragility.
5. **Support as many EVM chains as possible**: We aspire that Reth can full-sync not only Ethereum, but also other chains like Optimism, Polygon, BNB Smart Chain, and more. If you're working on any of these projects, please reach out.
6. **Configurability**: We want to solve for node operators that care about fast historical queries, but also for hobbyists who cannot operate on large hardware. We also want to support teams and individuals who want both sync from genesis and via "fast sync". We envision that Reth will be configurable enough and provide configurable "profiles" for the tradeoffs that each team faces.

## Status

The project is **not ready for production use**.

Reth is fully capable of syncing, however, there are still some missing features, and we are still working on performance and stability. Because of this, we are still introducing breaking changes.

It has **not been audited for security purposes** and should not be used in production yet.

We will be updating the documentation with the completion status of each component, as well as include more contributing guidelines (design docs, architecture diagrams, repository layouts) and "good first issues".

We appreciate your patience until we get there. Until then, we are happy to answer all questions in the Telegram link above.

## For Users

See the [Reth Book](https://paradigmxyz.github.io/reth) for instructions on how to install and run Reth.

## For Developers

### Using reth as a library

You can use individual crates of reth in your project.

The crate docs can be found [here](https://paradigmxyz.github.io/reth/docs).

For a general overview of the crates, see [Project Layout](./docs/repo/layout.md).

### Contributing

If you want to contribute, or follow along with contributor discussion, you can use our [main telegram](https://t.me/paradigm_reth) to chat with us about the development of Reth!

- Our contributor guidelines can be found in [`CONTRIBUTING.md`](./CONTRIBUTING.md).
- See our [contributor docs](./docs) for more information on the project. A good starting point is [Project Layout](./docs/repo/layout.md).

### Building and testing

<!--
When updating this, also update:
- clippy.toml
- Cargo.toml
- .github/workflows/lint.yml
-->

The Minimum Supported Rust Version (MSRV) of this project is [1.70.0](https://blog.rust-lang.org/2023/06/01/Rust-1.70.0.html).

See the book for detailed instructions on how to [build from source](https://paradigmxyz.github.io/reth/installation/source.html).

To fully test Reth, you will need to have [Geth installed](https://geth.ethereum.org/docs/getting-started/installing-geth), but it is possible to run a subset of tests without Geth.

First, clone the repository:

```sh
git clone https://github.com/paradigmxyz/reth
cd reth
```

Next, run the tests:

```sh
# Without Geth
cargo test --workspace

# With Geth
cargo test --workspace --features geth-tests

# With Ethereum Foundation tests
#
# Note: Requires cloning https://github.com/ethereum/tests
# 
#   cd testing/ef-tests && git clone https://github.com/ethereum/tests ethereum-tests
cargo test -p ef-tests --features ef-tests
```

We recommend using [`cargo nextest`](https://nexte.st/) to speed up testing. With nextest installed, simply substitute `cargo test` with `cargo nextest run`.

> **Note**
> 
> Some tests use random number generators to generate test data. If you want to use a deterministic seed, you can set the `SEED` environment variable.

## Getting Help

If you have any questions, first see if the answer to your question can be found in the [book][book].

If the answer is not there:

- Join the [Telegram][tg-url] to get help, or
- Open a [discussion](https://github.com/paradigmxyz/reth/discussions/new) with your question, or
- Open an issue with [the bug](https://github.com/paradigmxyz/reth/issues/new?assignees=&labels=C-bug%2CS-needs-triage&projects=&template=bug.yml)

## Security

See [`SECURITY.md`](./SECURITY.md).

## Acknowledgements

Reth is a new implementation of the Ethereum protocol. In the process of developing the node we investigated the design decisions other nodes have made to understand what is done well, what is not, and where we can improve the status quo.

None of this would have been possible without them, so big shoutout to the teams below:
* [Geth](https://github.com/ethereum/go-ethereum/): We would like to express our heartfelt gratitude to the go-ethereum team for their outstanding contributions to Ethereum over the years. Their tireless efforts and dedication have helped to shape the Ethereum ecosystem and make it the vibrant and innovative community it is today. Thank you for your hard work and commitment to the project.
* [Erigon](https://github.com/ledgerwatch/erigon) (fka Turbo-Geth): Erigon pioneered the ["Staged Sync" architecture](https://erigon.substack.com/p/erigon-stage-sync-and-control-flows) that Reth is using, as well as [introduced MDBX](https://github.com/ledgerwatch/erigon/wiki/Choice-of-storage-engine) as the database of choice. We thank Erigon for pushing the state of the art research on the performance limits of Ethereum nodes.
* [Akula](https://github.com/akula-bft/akula/): Reth uses forks of the Apache versions of Akula's [MDBX Bindings](https://github.com/paradigmxyz/reth/pull/132), [FastRLP](https://github.com/paradigmxyz/reth/pull/63) and [ECIES](https://github.com/paradigmxyz/reth/pull/80) . Given that these packages were already released under the Apache License, and they implement standardized solutions, we decided not to reimplement them to iterate faster. We thank the Akula team for their contributions to the Rust Ethereum ecosystem and for publishing these packages.

[book]: https://paradigmxyz.github.io/reth/
[tg-url]: https://t.me/paradigm_reth
