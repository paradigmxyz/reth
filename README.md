# <h1 align="center"> reth </h1>

*Blazing-fast implementation of the Ethereum protocol*

[![CI status](https://github.com/foundry-rs/reth/workflows/ci/badge.svg)][gh-ci]
[![cargo-deny status](https://github.com/foundry-rs/reth/workflows/deny/badge.svg)][gh-deny]
[![Codecov](https://img.shields.io/codecov/c/github/foundry-rs/reth?token=c24SDcMImE)][codecov]
[![Telegram Chat][tg-badge]][tg-url]

[tg-badge]: https://img.shields.io/endpoint?color=neon&logo=telegram&label=chat&style=flat-square&url=https%3A%2F%2Ftg.sumanjay.workers.dev%2Fparadigm%5Freth
[tg-url]: https://t.me/paradigm_reth

# 🚧 WARNING: UNDER CONSTRUCTION 🚧

This project is work in progress and subject to frequent changes as we are still working on wiring up each individual node component into a full syncing pipeline.

It has not been audited for security purposes and should not be used in production yet.

We will be updating the documentation with the completion status of each component, as well as include more contributing guidelines (design docs, architecture diagrams, repository layouts) and "good first issues". See the #Docs section below for more.

We appreciate your patience until we get there. Until then, we are happy to answer all questions in the Telegram link above.

# What does this solve? Why a new Rust implementation?

Reth is a new Apache/MIT-licensed full node implementation of Ethereum focused on contributor friendliness, modularity, and performance. Reth does not include code from any existing client but stands on the shoulders of giants including Geth, Erigon, OpenEthereum, Akula and more. We are working on a "lineage" document which will communicate the series of innovations.

Our goals are:
1. **Modularity**: Every component of Reth is built to be used as a library: well-tested, heavily documented and benchmarked. We envision that developers will import the node's crates, mix and match, and innovate on top of them. To achieve that, we are licensing Reth under the Apache/MIT permissive license.
2. **Performance**: Reth aims to be fast, so we used Rust and the [Erigon staged-sync](https://erigon.substack.com/p/erigon-stage-sync-and-control-flows) node architecture. We also use our Ethereum libraries (including [ethers-rs](https://github.com/gakonst/ethers-rs/) and [revm](https://github.com/bluealloy/revm/)) which we’ve battle-tested and optimized via [Foundry](https://github.com/foundry-rs/foundry/).
3. **Free for anyone to use any way they want**: Reth is free open source software, built for the community, by the community. By licensing the software under the Apache/MIT license, we want developers to use it without being bound by business licenses, or having to think about the virality of GPL-like licenses.

## Build

Rust minimum required version to build this project is 1.65.0 published 02.11.2022

## Docs

- [User docs][book]
- [Contributor docs](./docs)

[codecov]: https://app.codecov.io/gh/foundry-rs/reth
[gh-ci]: https://github.com/foundry-rs/reth/actions/workflows/ci.yml
[gh-deny]: https://github.com/foundry-rs/reth/actions/workflows/deny.yml
[book]: https://foundry-rs.github.io/reth/
