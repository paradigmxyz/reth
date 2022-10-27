# <h1 align="center"> reth </h1>

*Blazing-fast implementation of the Ethereum protocol*

[![CI status](https://github.com/foundry-rs/reth/workflows/ci/badge.svg)][gh-ci]
[![cargo-deny status](https://github.com/foundry-rs/reth/workflows/deny/badge.svg)][gh-deny]
[![Codecov](https://img.shields.io/codecov/c/github/foundry-rs/reth?token=c24SDcMImE)][codecov]

## Build

To build this project we are currently using Rust nightly for GAT support, that is planed to release in rust 1.65 (4th Nov 2022). GAT's are used for the `Database` trait in `reth-interface`. 

## Docs

Contributor docs can be found [here](./docs).

[codecov]: https://app.codecov.io/gh/foundry-rs/reth
[gh-ci]: https://github.com/foundry-rs/reth/actions/workflows/ci.yml
[gh-deny]: https://github.com/foundry-rs/reth/actions/workflows/deny.yml
