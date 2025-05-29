# zkRess Protocol (ZKRESS)

The `zk-ress` protocols run on top of [RLPx], allowing a stateless nodes to fetch necessary data (proof, header, block body) from a stateful node. The protocols are an optional extension for peers that support (or are interested in) stateless Ethereum full nodes.

## Overview

The `zk-ress` protocol is designed to provide support for stateless nodes. It supports retrieving an execution proof for a target new payload from a stateful peer, as well full block data (headers and bodies). The `zk-ress` protocol is intended to be run alongside other protocols (e.g., `eth`), rather than as a standalone protocol.

## Basic operation

Once a connection is established, a [NodeType] message must be sent. After the peer's node type is validated according to the connection rules, any other protocol messages may be sent. The `zk-ress` session will be terminated if the peer combination is invalid (e.g., stateful-to-stateful connections are not needed).

Within a session, four types of messages can be exchanged: headers, bodies, and execution proof.

During the startup phase, a stateless node downloads the necessary ancestor blocks (headers and bodies) via the header and body messages. When the stateless node receives a new payload through the engine API, it requests an execution proof — an arbitrary proof allowing to verify block validity — using the proof message. All requests are sent to the connected stateful peer and occur synchronously.

## Protocol Messages

In most messages, the first element of the message data list is the request-id. For requests, this is a 64-bit integer value chosen by the requesting peer. The responding peer must mirror the value in the request-id element of the response message.

### NodeType (0x00)

`[nodetype]`

Informs a peer of its node type. This message should be sent immediately after the connection is established and before any other RESS protocol messages.

There are two types of nodes in the network:

| ID  | Node Type |
| --- | --------- |
| 0   | Stateless |
| 1   | Stateful  |


The following table shows which connections between node types are valid:

|           | stateless | stateful |
| --------- | --------- | -------- |
| stateless | true      | true     |
| stateful  | true      | false    |


### GetHeaders (0x01)

`[request-id: P, [blockhash: B_32, limit: P]]`

Require the peer to return a Headers message. The response must contain up to limit block headers, beginning at blockhash in the canonical chain and traversing towards the genesis block (descending order).

### Headers (0x02)

`[request-id: P, [header₁, header₂, ...]]`

This is the response to GetHeaders, containing the requested headers. The header list may be empty if none of the requested block headers were found. The number of headers that can be requested in a single message may be subject to implementation-defined limits.

### GetBlockBodies (0x03)

`[request-id: P, [blockhash₁: B_32, blockhash₂: B_32, ...]]`

This message requests block body data by hash. The number of blocks that can be requested in a single message may be subject to implementation-defined limits.

### BlockBodies (0x04)

`[request-id: P, [block-body₁, block-body₂, ...]]`

This is the response to GetBlockBodies. The items in the list contain the body data of the requested blocks. The list may be empty if none of the requested blocks were available.

### GetProof (0x05)

`[request-id: P, [blockhash: B_32]]`

Require peer to return an execution proof message containing the execution proof of the given block hash. 

### Proof (0x06)

`[request-id: P, [node₁: bytes, node₂: bytes, ...]]`

This is the response to GetProof, providing the requested execution proof. Response corresponds to a block hash of the GetProof request.

[NodeType]: #NodeType-0x00
[RLPx]: https://github.com/ethereum/devp2p/blob/master/rlpx.md
