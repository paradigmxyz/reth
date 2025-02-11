# Reth SDK docs and guides for developers

## Core Primitives

### Block

A block is a type that consists of a header and body.
Defined by the `Block` trait which provides core block operations and ecapsulates all block internals.
Can exist in different states: basic, sealed (with hash), and recovered (with transaction signers)
Supports operations like sealing (also known as hashing) and transaction signer recovery

### Block Header

Contains block metadata and state information
Implements `BlockHeader` trait
Used to derive the unique block hash
Required properties: must be clonable, hashable, serializable/deserializable via RLP encoding

## Block Body

Contains the actual block content (transactions, uncle headers, withdrawals)
In Ethereum: contains transactions, uncle headers, and withdrawals
In Optimism: contains only transactions (empty uncle headers and withdrawals)

## Transactions

Contained within the block body.
Require recovery to determine the sender's address

## Receipts

Receipts are creating during transaction execution and an outcome of blockexecution.

## Operations

### Sealing

Sealing computes the hash of a sealable entity and stores the hash in a dedicated `Sealed` type.
Sealing a `BlockHeader` gives you a `SealedHeader<BlockHeader>`.
Sealing a `Block` gives you a `SealedBlock<Block>`.

### Recovery

Recovery is a fallible operation on transactions to determine the sender's address of a transaction.
Recovering a transactions gives you a `Recovered<T>`.
Recovering all transactions in a block gives you a `RecoveredBlock<Block>` which contains all recovered transactions.

## Components

In Reth "components" are the building blocks the make up a node, such as:

- Network (p2p,discovery)
- RPC
- Transaction-pool
- EVM + Executor
- Consensus
- Payload building

## Node

A node combines all its components and advances the chain based on an external input.
In Ethereum and Optimism the `engine API` advances the chain.

## Reth and Alloy

Reth tries to reuse as many types as possible from the [alloy](#alloy) ecosystem, such as transaction types, EIP types,
RPC types and various traits. Alloy is meant as the foundation that provides all primitive building blocks.
Where needed Reth extends alloy's primitive traits (e.g. Transaction) with reth specific requirements, for example
encoding.


[alloy]: https://github.com/alloy-rs/alloy/