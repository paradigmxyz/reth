# Reth Book
_Documentation for Reth users and developers._ 

[![Telegram Chat][tg-badge]][tg-url]

Reth (short for Rust Ethereum, [pronunciation](https://twitter.com/kelvinfichter/status/1597653609411268608)) is an **Ethereum full node implementation that is focused on being user-friendly, highly modular, as well as being fast and efficient.** 

<img src="https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-alpha.png" style="border-radius: 20px">

<!-- Add a quick description about Reth, what it is, the goals of the build, and any other quick overview information   -->

## What is this about?

[Reth](https://github.com/paradigmxyz/reth/) is an execution layer (EL) implementation that is compatible with all Ethereum consensus layer (CL) implementations that support the [Engine API](https://github.com/ethereum/execution-apis/tree/59e3a719021f48c1ef5653840e3ea5750e6af693/src/engine).

It is originally built and driven forward by [Paradigm](https://paradigm.xyz/), and is licensed under the Apache and MIT licenses.

As a full Ethereum node, Reth allows users to connect to the Ethereum network and interact with the Ethereum blockchain.

This includes sending and receiving transactions, querying logs and traces, as well as accessing and interacting with smart contracts.

Building a successful Ethereum node requires creating a high-quality implementation that is both secure and efficient, as well as being easy to use on consumer hardware. It also requires building a strong community of contributors who can help support and improve the software.

## What are the goals of Reth?

**1. Modularity**

Every component of Reth is built to be used as a library: well-tested, heavily documented and benchmarked. We envision that developers will import the node's crates, mix and match, and innovate on top of them.

Examples of such usage include, but are not limited to, spinning up standalone P2P networks, talking directly to a node's database, or "unbundling" the node into the components you need.

To achieve that, we are licensing Reth under the Apache/MIT permissive license.

**2. Performance**

Reth aims to be fast, so we used Rust and the [Erigon staged-sync](https://erigon.substack.com/p/erigon-stage-sync-and-control-flows) node architecture.

We also use our Ethereum libraries (including [Alloy](https://github.com/alloy-rs/alloy/) and [revm](https://github.com/bluealloy/revm/)) which we’ve battle-tested and optimized via [Foundry](https://github.com/foundry-rs/foundry/).

**3. Free for anyone to use any way they want**

Reth is free open source software, built for the community, by the community.

By licensing the software under the Apache/MIT license, we want developers to use it without being bound by business licenses, or having to think about the implications of GPL-like licenses.

**4. Client Diversity**

The Ethereum protocol becomes more antifragile when no node implementation dominates. This ensures that if there's a software bug, the network does not finalize a bad block. By building a new client, we hope to contribute to Ethereum's antifragility.

**5. Used by a wide demographic**

We want to solve for node operators that care about fast historical queries, but also for hobbyists who cannot operate on large hardware.

We also want to support teams and individuals who want both sync from genesis and via "fast sync".

We envision that Reth will be configurable enough  for the tradeoffs that each team faces.

## Who is this for?

Reth is a new Ethereum full node that allows users to sync and interact with the entire blockchain, including its historical state if in archive mode.
- Full node: It can be used as a full node, which stores and processes the entire blockchain, validates blocks and transactions, and participates in the consensus process. 
- Archive node: It can also be used as an archive node, which stores the entire history of the blockchain and is useful for applications that need access to historical data.

As a data engineer/analyst, or as a data indexer, you'll want to use Archive mode. For all other use cases where historical access is not needed, you can use Full mode.

## Is this secure?

Reth implements the specification of Ethereum as defined in the [ethereum/execution-specs](https://github.com/ethereum/execution-specs/) repository. To make sure the node is built securely, we run the following tests:

1. EVM state tests are run on every [Revm Pull Request](https://github.com/bluealloy/revm/blob/main/.github/workflows/ethereum-tests.yml)
1. Hive tests are [run every 24 hours](https://github.com/paradigmxyz/reth/blob/main/.github/workflows/hive.yml) in the main Reth repository.
1. We regularly re-sync multiple nodes from scratch.
1. We operate multiple nodes at the tip of Ethereum mainnet and various testnets.
1. We extensively unit test, fuzz test and document all our code, while also restricting PRs with aggressive lint rules.

We intend to also audit / fuzz the EVM & parts of the codebase. Please reach out if you're interested in collaborating on securing this codebase.

## Sections

Here are some useful sections to jump to:

- Install Reth by following the [guide](./installation/installation.md).
- Sync your node on any [official network](./run/run-a-node.md).
- View [statistics and metrics](./run/observability.md) about your node.
- Query the [JSON-RPC](./jsonrpc/intro.md) using Foundry's `cast` or `curl`.
- Set up your [development environment and contribute](./developers/contribute.md)!

> 📖 **About this book**
>
> The book is continuously rendered [here](https://paradigmxyz.github.io/reth/)!
> You can contribute to this book on [GitHub][gh-book].

[tg-badge]: https://img.shields.io/endpoint?color=neon&logo=telegram&label=chat&url=https%3A%2F%2Ftg.sumanjay.workers.dev%2Fparadigm%5Freth
[tg-url]: https://t.me/paradigm_reth
[gh-book]: https://github.com/paradigmxyz/reth/tree/main/book
