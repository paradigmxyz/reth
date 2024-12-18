# Execution Extensions (ExEx)

## What are Execution Extensions?

Execution Extensions (or ExExes, for short) allow developers to build their own infrastructure that relies on Reth
as a base for driving the chain (be it [Ethereum](../../run/mainnet.md) or [OP Stack](../../run/optimism.md)) forward.

An Execution Extension is a task that derives its state from changes in Reth's state.
Some examples of such state derivations are rollups, bridges, and indexers.

They are called Execution Extensions because the main trigger for them is the execution of new blocks (or reorgs of old blocks)
initiated by Reth.

Read more about things you can build with Execution Extensions in the [Paradigm blog](https://www.paradigm.xyz/2024/05/reth-exex).

## What Execution Extensions are not

Execution Extensions are not separate processes that connect to the main Reth node process.
Instead, ExExes are compiled into the same binary as Reth, and run alongside it, using shared memory for communication.

If you want to build an Execution Extension that sends data into a separate process, check out the [Remote](./remote.md) chapter.

## How do I build an Execution Extension?

Let's dive into how to build our own ExEx from scratch, add tests for it,
and run it on the Holesky testnet.

1. [How do ExExes work?](./how-it-works.md)
1. [Hello World](./hello-world.md)
1. [Tracking State](./tracking-state.md)
1. [Remote](./remote.md)
