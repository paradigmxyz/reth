# Execution Extensions (ExEx)

## What are Execution Extensions?

Execution Extensions allow developers to build their own infrastructure that relies on Reth
as a base for driving the chain (be it [Ethereum](../../run/mainnet.md) or [OP Stack](../../run/optimism.md)) forward.

An Execution Extension is a task that derives its state from changes in Reth's state.
Some examples of such state derivations are rollups, bridges, and indexers.

Read more about things you can build with Execution Extensions in the [Paradigm blog](https://www.paradigm.xyz/2024/05/reth-exex).

## How do I build an Execution Extension?

Let's dive into how to build our own ExEx (short for Execution Extension) from scratch, add tests for it,
and run it on the Holesky testnet.

1. [How do ExExes work?](./how-it-works.md)
1. [Hello World](./hello-world.md)
