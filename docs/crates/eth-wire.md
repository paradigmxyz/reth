# eth-wire

The `eth-wire` crate provides abstractions over the [RLPx](https://github.com/ethereum/devp2p/blob/master/rlpx.md) and 
[Eth wire](https://github.com/ethereum/devp2p/blob/master/caps/eth.md) protocols. 

This crate can be thought of as having 2 components:

1. Data structures which serialize and deserialize the eth protcol messages into Rust compatible types.
2. Abstractions over Tokio Streams which operate on these types.

(Note that ECIES is implemented in a  seperate `reth-ecies` crate.)
## Types
The most basic Eth-wire type is an `ProtocolMessage`. It describes all messages that reth can send/receive.

[File: crates/net/eth-wire/src/types/message.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/types/message.rs)
```rust, ignore
/// An `eth` protocol message, containing a message ID and payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolMessage {
    pub message_type: EthMessageID,
    pub message: EthMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EthMessage {
    Status(Status),
    NewBlockHashes(NewBlockHashes),
    Transactions(Transactions),
    NewPooledTransactionHashes(NewPooledTransactionHashes),
    GetBlockHeaders(RequestPair<GetBlockHeaders>),
    // ...
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
    // ...
    NodeData = 0x0e,
    GetReceipts = 0x0f,
    Receipts = 0x10,
}

```
Messages can either be broadcast to the network, or can be a request/response message to a single peer. This 2nd type of message is 
described using a `RequestPair` struct, which is simply a concatenation of the underlying message with a request id.

[File: crates/net/eth-wire/src/types/message.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/types/message.rs)
```rust, ignore
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPair<T> {
    pub request_id: u64,
    pub message: T,
}

```
Every `Ethmessage` has a correspoding rust struct which implements the `Encodable` and `Decodable` traits.
These traits are defined as follows:

[Crate: crates/rlp](https://github.com/paradigmxyz/reth/tree/1563506aea09049a85e5cc72c2894f3f7a371581/crates/rlp)
```rust, ignore
pub trait Decodable: Sized {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError>;
}
pub trait Encodable {
    fn encode(&self, out: &mut dyn BufMut);
    fn length(&self) -> usize;
}
```
These traits describe how the `Ethmessage` should be serialized/deserialized into raw bytes using the RLP format.
In reth all [RLP](https://ethereum.org/en/developers/docs/data-structures-and-encoding/rlp/) encode/decode operations are handled by the `common/rlp` and `common/rlp-derive` crates.

Note that the `ProtocolMessage` itself implements these traits, so any stream of bytes can be converted into it by calling `ProtocolMessage::decode()` and vice versa with `ProtocolMessage::encode()`. The message type is determined by the first byte of the byte stream.

### Example: The Transactions message
Let's understand how an `EthMessage` is implemented by taking a look at the `Transactions` Message. The eth specification describes a Transaction message as a list of RLP encoded transactions:

[File: ethereum/devp2p/caps/eth.md](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#transactions-0x02)
```
Transactions (0x02)
[tx₁, tx₂, ...]

Specify transactions that the peer should make sure is included on its transaction queue. 
The items in the list are transactions in the format described in the main Ethereum specification. 
...

```

In reth, this is represented as:

[File: crates/net/eth-wire/src/types/broadcast.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/types/broadcast.rs)
```rust,ignore
pub struct Transactions(
    /// New transactions for the peer to include in its mempool.
    pub Vec<TransactionSigned>,
);
```

And the corresponding trait implementations are present in the primitives crate.

[File: crates/primitives/src/transaction/mod.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/primitives/src/transaction/mod.rs)
```rust, ignore
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Default)]
pub struct TransactionSigned {
    pub hash: TxHash,
    pub signature: Signature,
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
}
```
Now that we know how the types work, let's take a look at how these are utilized in the network.

## P2PStream
The lowest level stream to communicate with other peers is the P2P stream. It takes an underlying Tokio stream and does the following:

- Tracks and Manages Ping and pong messages and sends them when needed.
- Keeps track of the SharedCapabilities between the reth node and its peers.
- Receives bytes from peers, decompresses and forwards them to its parent stream. 
- Receives bytes from its parent stream, compresses them and sends it to peers.

Decompression/Compression of bytes is done with snappy algorithm ([EIP 706](https://eips.ethereum.org/EIPS/eip-706)) 
using the external `snap` crate. 

[File: crates/net/eth-wire/src/p2pstream.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/p2pstream.rs)
```rust,ignore
#[pin_project]
pub struct P2PStream<S> {
    #[pin]
    inner: S,
    encoder: snap::raw::Encoder,
    decoder: snap::raw::Decoder,
    pinger: Pinger,
    shared_capability: SharedCapability,
    outgoing_messages: VecDeque<Bytes>,
    disconnecting: bool,
}
```
### Pinger
To manage pinging, an instance of the `Pinger` struct is used. This is a state machine which keeps track of how many pings
we have sent/received and the timeouts associated with them.

[File: crates/net/eth-wire/src/pinger.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/pinger.rs)
```rust,ignore
#[derive(Debug)]
pub(crate) struct Pinger {
    /// The timer used for the next ping.
    ping_interval: Interval,
    /// The timer used for the next ping.
    timeout_timer: Pin<Box<Sleep>>,
    timeout: Duration,
    state: PingState,
}

/// This represents the possible states of the pinger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PingState {
    /// There are no pings in flight, or all pings have been responded to.
    Ready,
    /// We have sent a ping and are waiting for a pong, but the peer has missed n pongs.
    WaitingForPong,
    /// The peer has failed to respond to a ping.
    TimedOut,
}
```

State transitions are then implemented like a future, with the `poll_ping` function advancing the state of the pinger.

[File: crates/net/eth-wire/src/pinger.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/pinger.rs)
```rust, ignore
pub(crate) fn poll_ping(
    &mut self,
    cx: &mut Context<'_>,
) -> Poll<Result<PingerEvent, PingerError>> {
    match self.state() {
        PingState::Ready => {
            if self.ping_interval.poll_tick(cx).is_ready() {
                self.timeout_timer.as_mut().reset(Instant::now() + self.timeout);
                self.state = PingState::WaitingForPong;
                return Poll::Ready(Ok(PingerEvent::Ping))
            }
        }
        PingState::WaitingForPong => {
            if self.timeout_timer.is_elapsed() {
                self.state = PingState::TimedOut;
                return Poll::Ready(Ok(PingerEvent::Timeout))
            }
        }
        PingState::TimedOut => {
            return Poll::Pending
        }
    };
    Poll::Pending
```

### Sending and receiving data
To send and receive data, the P2PStream itself is a future which implements the `Stream` and `Sink` traits from the `futures` crate.

For the `Stream` trait, the `inner` stream is polled, decompressed and returned. Most of the code is just 
error handling and is omitted here for clarity.

[File: crates/net/eth-wire/src/p2pstream.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/p2pstream.rs)
```rust,ignore

impl<S> Stream for P2PStream<S> {
    type Item = Result<BytesMut, P2PStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        while let Poll::Ready(res) = this.inner.poll_next_unpin(cx) {
            let bytes = match res {
                Some(Ok(bytes)) => bytes,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => return Poll::Ready(None),
            };
            let decompressed_len = snap::raw::decompress_len(&bytes[1..])?;
            let mut decompress_buf = BytesMut::zeroed(decompressed_len + 1);
            this.decoder.decompress(&bytes[1..], &mut decompress_buf[1..])?;
            // ... Omitted Error handling
            decompress_buf[0] = bytes[0] - this.shared_capability.offset();
            return Poll::Ready(Some(Ok(decompress_buf)))
        }
    }
}
```

Similarly, for the `Sink` trait, we do the reverse, compressing and sending data out to the `inner` stream.
The important functions in this trait are shown below.

[File: crates/net/eth-wire/src/p2pstream.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/p2pstream.rs)
```rust, ignore
impl<S> Sink<Bytes> for P2PStream<S> {
    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let this = self.project();
        let mut compressed = BytesMut::zeroed(1 + snap::raw::max_compress_len(item.len() - 1));
        let compressed_size = this.encoder.compress(&item[1..], &mut compressed[1..])?;
        compressed.truncate(compressed_size + 1);
        compressed[0] = item[0] + this.shared_capability.offset();
        this.outgoing_messages.push_back(compressed.freeze());
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut this = self.project();
        loop {
            match ready!(this.inner.as_mut().poll_flush(cx)) {
                Err(err) => return Poll::Ready(Err(err.into())),
                Ok(()) => {
                    if let Some(message) = this.outgoing_messages.pop_front() {
                        if let Err(err) = this.inner.as_mut().start_send(message) {
                            return Poll::Ready(Err(err.into()))
                        }
                    } else {
                        return Poll::Ready(Ok(()))
                    }
                }
            }
        }
    }
}
```


## EthStream
The EthStream is very simple, it does not keep track of any state, it simply wraps the P2Pstream.

[File: crates/net/eth-wire/src/ethstream.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/ethstream.rs)
```rust,ignore
#[pin_project]
pub struct EthStream<S> {
    #[pin]
    inner: S,
}
```
EthStream's only job is to perform the RLP decoding/encoding, using the `ProtocolMessage::decode()` and `ProtocolMessage::encode()`
functions we looked at earlier. 

[File: crates/net/eth-wire/src/ethstream.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/ethstream.rs)
```rust,ignore
impl<S, E> Stream for EthStream<S> {
    // ...
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let bytes = ready!(this.inner.poll_next(cx)).unwrap();
        // ...
        let msg = match ProtocolMessage::decode(&mut bytes.as_ref()) {
            Ok(m) => m,
            Err(err) => {
                return Poll::Ready(Some(Err(err.into())))
            }
        };
        Poll::Ready(Some(Ok(msg.message)))
    }
}

impl<S, E> Sink<EthMessage> for EthStream<S> {
    // ...
    fn start_send(self: Pin<&mut Self>, item: EthMessage) -> Result<(), Self::Error> {
        // ...
        let mut bytes = BytesMut::new();
        ProtocolMessage::from(item).encode(&mut bytes);

        let bytes = bytes.freeze();
        self.project().inner.start_send(bytes)?;
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx).map_err(Into::into)
    }
}
```
## Unauthed streams
For a session to be established, peers in the Ethereum network must first exchange a `Hello` message in the RLPx layer and then a
`Status` message in the eth-wire layer. 

To perform these, reth has special `Unauthed` versions of streams described above.

The `UnauthedP2Pstream` does the `Hello` handshake and returns a `P2PStream`.

[File: crates/net/eth-wire/src/p2pstream.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/p2pstream.rs)
```rust, ignore
#[pin_project]
pub struct UnauthedP2PStream<S> {
    #[pin]
    inner: S,
}

impl<S> UnauthedP2PStream<S> {
    // ...
    pub async fn handshake(mut self, hello: HelloMessage) -> Result<(P2PStream<S>, HelloMessage), Error> {
        let mut raw_hello_bytes = BytesMut::new();
        P2PMessage::Hello(hello.clone()).encode(&mut raw_hello_bytes);

        self.inner.send(raw_hello_bytes.into()).await?;
        let first_message_bytes = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.inner.next()).await;

        let their_hello = match P2PMessage::decode(&mut &first_message_bytes[..]) {
            Ok(P2PMessage::Hello(hello)) => Ok(hello),
            // ...
            }
        }?;
        let stream = P2PStream::new(self.inner, capability);

        Ok((stream, their_hello))
    }
}

```
Similarly, UnauthedEthStream does the `Status` handshake and returns an `EthStream`. The code is [here](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/ethstream.rs)


