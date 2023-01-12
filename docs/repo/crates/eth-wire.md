# eth-wire

The `eth-wire` crate provides low level abstractions of the Ethereum Wire protocol described [here](https://github.com/ethereum/devp2p/blob/master/caps/eth.md).

The crate can be thought of as having 2 components:

1. Data structures which serialize and deserialize the eth protcol messages into Rust compatible types.
2. Abstractions over Tokio Streams which operate on these types.

## Types
The most basic type is an `ProtocolMessage`. It describes all messages that reth can send/receive.

[File: crates/net/eth-wire/src/types/message.rs](...)
```rust, ignore
/// An `eth` protocol message, containing a message ID and payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolMessage {
    pub message_type: EthMessageID,
    pub message: EthMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EthMessage {
    /// Status is required for the protocol handshake
    Status(Status),
    /// The following messages are broadcast to the network
    NewBlockHashes(NewBlockHashes),
    NewBlock(Box<NewBlock>),
    Transactions(Transactions),
    NewPooledTransactionHashes(NewPooledTransactionHashes),

    // The following messages are request-response message pairs
    GetBlockHeaders(RequestPair<GetBlockHeaders>),
    BlockHeaders(RequestPair<BlockHeaders>),
    GetBlockBodies(RequestPair<GetBlockBodies>),
    BlockBodies(RequestPair<BlockBodies>),
    GetPooledTransactions(RequestPair<GetPooledTransactions>),
    PooledTransactions(RequestPair<PooledTransactions>),
    GetNodeData(RequestPair<GetNodeData>),
    NodeData(RequestPair<NodeData>),
    GetReceipts(RequestPair<GetReceipts>),
    Receipts(RequestPair<Receipts>),
}

/// Represents message IDs for eth protocol messages.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EthMessageID {
    Status = 0x00,
    NewBlockHashes = 0x01,
    Transactions = 0x02,
    GetBlockHeaders = 0x03,
    BlockHeaders = 0x04,
    GetBlockBodies = 0x05,
    BlockBodies = 0x06,
    NewBlock = 0x07,
    NewPooledTransactionHashes = 0x08,
    GetPooledTransactions = 0x09,
    PooledTransactions = 0x0a,
    GetNodeData = 0x0d,
    NodeData = 0x0e,
    GetReceipts = 0x0f,
    Receipts = 0x10,
}

```
Messages can either be broadcast to the network, or can be a request/response message to a single peer. This 2nd type of message is 
described using a `RequestPair` struct, which is simply a concatenation of the underlying message with a request id.

[File: crates/net/eth-wire/src/types/message.rs](...)
```rust, ignore
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPair<T> {
    /// id for the contained request or response message
    pub request_id: u64,

    /// the request or response message payload
    pub message: T,
}

```
Every `Ethmessage` has a correspoding rust struct which implements the `Encodable` and `Decodable` traits.
These traits are defined as follows:

[Crate: crates/common/rlp](...)
```rust, ignore
pub trait Decodable: Sized {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError>;
}
#[auto_impl(&)]
#[cfg_attr(feature = "alloc", auto_impl(Box, Arc))]
pub trait Encodable {
    fn encode(&self, out: &mut dyn BufMut);
    fn length(&self) -> usize {
        let mut out = BytesMut::new();
        self.encode(&mut out);
        out.len()
    }
}
```
These traits describe how the `Ethmessage` should be serialized/deserialized into raw bytes using the RLP format.
In reth all rlp encoding is handled by the rlp and rlp-derive crates.
You can learn more about RLP by looking at the [ETH wiki](https://ethereum.org/en/developers/docs/data-structures-and-encoding/rlp/).


### Example: The Transactions message
Let's understand how an `EthMessage` is implemented by taking a look at the `Transactions` Message. The eth specification describes a Transaction message as a list of RLP encoded transactions:

[File: ethereum/devp2p/caps/eth.md](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#transactions-0x02)
```
Transactions (0x02)
[tx₁, tx₂, ...]

Specify transactions that the peer should make sure is included on its transaction queue. The items in the list are transactions in the format described in the main Ethereum specification. Transactions messages must contain at least one (new) transaction, empty Transactions messages are discouraged and may lead to disconnection.
...

```

In reth, this is represented as:

[File: crates/net/eth-wire/src/types/broadcast.rs](...)
```rust,ignore
pub struct Transactions(
    /// New transactions for the peer to include in its mempool.
    pub Vec<TransactionSigned>,
);
```

And the corresponding trait implementations:

[File: crates/primitives/src/transaction/mod.rs](...)
```rust, ignore
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Default)]
pub struct TransactionSigned {
    /// Transaction hash
    pub hash: TxHash,
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: Transaction,
}

impl Encodable for TransactionSigned {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.encode_inner(out, true);
    }

    fn length(&self) -> usize {
        let len = self.payload_len();
        len + length_of_length(len)
    }
}

impl Decodable for TransactionSigned {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        // Implementation omitted for brevity
        //...
    }

```
Now that we know how the types work, let's take a look at how these are utilized in the network.

## P2PStream
The most basic stream to talk to other peers is the P2P stream. It takes an underlying stream of bytes and does the following:

1. Tracks and Manages Ping and pong messages and sends them when needed.
2. Decompresses/Compresses the underlying bytes using snappy. ([EIP 706](https://eips.ethereum.org/EIPS/eip-706))
3. Sends the decompressed bytes to its parent stream. 

// Add code here


## EthStream
The P2Pstream is then consumed by a higher level EthStream which performs the RLP decoding/encoding.
// TODO

The Ethstream is then consumed by the SessionManager using ActiveSession and PendingSessions
// TODO

