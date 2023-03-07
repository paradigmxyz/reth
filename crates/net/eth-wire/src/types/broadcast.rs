//! Types for broadcasting new data.
use crate::{EthMessage, EthVersion};
use bytes::Bytes;
use reth_codecs::derive_arbitrary;
use reth_primitives::{Block, TransactionSigned, H256, U128};
use reth_rlp::{
    Decodable, Encodable, RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper,
};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// This informs peers of new blocks that have appeared on the network.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NewBlockHashes(
    /// New block hashes and the block number for each blockhash.
    /// Clients should request blocks using a [`GetBlockBodies`](crate::GetBlockBodies) message.
    pub Vec<BlockHashNumber>,
);

// === impl NewBlockHashes ===

impl NewBlockHashes {
    /// Returns the latest block in the list of blocks.
    pub fn latest(&self) -> Option<&BlockHashNumber> {
        self.0.iter().fold(None, |latest, block| {
            if let Some(latest) = latest {
                return if latest.number > block.number { Some(latest) } else { Some(block) }
            }
            Some(block)
        })
    }
}

/// A block hash _and_ a block number.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BlockHashNumber {
    /// The block hash
    pub hash: H256,
    /// The block number
    pub number: u64,
}

impl From<Vec<BlockHashNumber>> for NewBlockHashes {
    fn from(v: Vec<BlockHashNumber>) -> Self {
        NewBlockHashes(v)
    }
}

impl From<NewBlockHashes> for Vec<BlockHashNumber> {
    fn from(v: NewBlockHashes) -> Self {
        v.0
    }
}

/// A new block with the current total difficulty, which includes the difficulty of the returned
/// block.
#[derive_arbitrary(rlp, 25)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NewBlock {
    /// A new block.
    pub block: Block,
    /// The current total difficulty.
    pub td: U128,
}

/// This informs peers of transactions that have appeared on the network and are not yet included
/// in a block.
#[derive_arbitrary(rlp, 10)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Transactions(
    /// New transactions for the peer to include in its mempool.
    pub Vec<TransactionSigned>,
);

impl From<Vec<TransactionSigned>> for Transactions {
    fn from(txs: Vec<TransactionSigned>) -> Self {
        Transactions(txs)
    }
}

impl From<Transactions> for Vec<TransactionSigned> {
    fn from(txs: Transactions) -> Self {
        txs.0
    }
}

/// Same as [`Transactions`] but this is intended as egress message send from local to _many_ peers.
///
/// The list of transactions is constructed on per-peers basis, but the underlying transaction
/// objects are shared.
#[derive_arbitrary(rlp, 20)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct SharedTransactions(
    /// New transactions for the peer to include in its mempool.
    pub Vec<Arc<TransactionSigned>>,
);

/// A wrapper type for all different new pooled transaction types
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewPooledTransactionHashes {
    /// A list of transaction hashes valid for [66-68)
    Eth66(NewPooledTransactionHashes66),
    /// A list of transaction hashes valid from [68..]
    ///
    /// Note: it is assumed that the payload is valid (all vectors have the same length)
    Eth68(NewPooledTransactionHashes68),
}

// === impl NewPooledTransactionHashes ===

impl NewPooledTransactionHashes {
    /// Returns `true` if the payload is valid for the given version
    pub fn is_valid_for_version(&self, version: EthVersion) -> bool {
        match self {
            NewPooledTransactionHashes::Eth66(_) => {
                matches!(version, EthVersion::Eth67 | EthVersion::Eth66)
            }
            NewPooledTransactionHashes::Eth68(_) => {
                matches!(version, EthVersion::Eth68)
            }
        }
    }

    /// Returns an iterator over all transaction hashes.
    pub fn iter_hashes(&self) -> impl Iterator<Item = &H256> + '_ {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0.iter(),
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes.iter(),
        }
    }

    /// Consumes the type and returns all hashes
    pub fn into_hashes(self) -> Vec<H256> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0,
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes,
        }
    }

    /// Returns an iterator over all transaction hashes.
    pub fn into_iter_hashes(self) -> impl Iterator<Item = H256> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0.into_iter(),
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes.into_iter(),
        }
    }

    /// Shortens the number of hashes in the message, keeping the first `len` hashes and dropping
    /// the rest. If `len` is greater than the number of hashes, this has no effect.
    pub fn truncate(&mut self, len: usize) {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0.truncate(len),
            NewPooledTransactionHashes::Eth68(msg) => {
                msg.types.truncate(len);
                msg.sizes.truncate(len);
                msg.hashes.truncate(len);
            }
        }
    }

    /// Returns true if the message is empty
    pub fn is_empty(&self) -> bool {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0.is_empty(),
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes.is_empty(),
        }
    }

    /// Returns the number of hashes in the message
    pub fn len(&self) -> usize {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0.len(),
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes.len(),
        }
    }
}

impl From<NewPooledTransactionHashes> for EthMessage {
    fn from(value: NewPooledTransactionHashes) -> Self {
        match value {
            NewPooledTransactionHashes::Eth66(msg) => EthMessage::NewPooledTransactionHashes66(msg),
            NewPooledTransactionHashes::Eth68(msg) => EthMessage::NewPooledTransactionHashes68(msg),
        }
    }
}

impl From<NewPooledTransactionHashes66> for NewPooledTransactionHashes {
    fn from(hashes: NewPooledTransactionHashes66) -> Self {
        Self::Eth66(hashes)
    }
}

impl From<NewPooledTransactionHashes68> for NewPooledTransactionHashes {
    fn from(hashes: NewPooledTransactionHashes68) -> Self {
        Self::Eth68(hashes)
    }
}

/// This informs peers of transaction hashes for transactions that have appeared on the network,
/// but have not been included in a block.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NewPooledTransactionHashes66(
    /// Transaction hashes for new transactions that have appeared on the network.
    /// Clients should request the transactions with the given hashes using a
    /// [`GetPooledTransactions`](crate::GetPooledTransactions) message.
    pub Vec<H256>,
);

impl From<Vec<H256>> for NewPooledTransactionHashes66 {
    fn from(v: Vec<H256>) -> Self {
        NewPooledTransactionHashes66(v)
    }
}

/// Same as [`NewPooledTransactionHashes66`] but extends that that beside the transaction hashes,
/// the node sends the transaction types and their sizes (as defined in EIP-2718) as well.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NewPooledTransactionHashes68 {
    /// Transaction types for new transactions that have appeared on the network.
    ///
    /// ## Note on RLP encoding and decoding
    ///
    /// In the [eth/68 spec](https://eips.ethereum.org/EIPS/eip-5793#specification) this is defined
    /// the following way:
    ///  * `[type_0: B_1, type_1: B_1, ...]`
    ///
    /// This would make it seem like the [`Encodable`](reth_rlp::Encodable) and
    /// [`Decodable`](reth_rlp::Decodable) implementations should directly use a `Vec<u8>` for
    /// encoding and decoding, because it looks like this field should be encoded as a _list_ of
    /// bytes.
    ///
    /// However, [this is implemented in geth as a `[]byte`
    /// type](https://github.com/ethereum/go-ethereum/blob/82d934b1dd80cdd8190803ea9f73ed2c345e2576/eth/protocols/eth/protocol.go#L308-L313),
    /// which [ends up being encoded as a RLP
    /// string](https://github.com/ethereum/go-ethereum/blob/82d934b1dd80cdd8190803ea9f73ed2c345e2576/rlp/encode_test.go#L171-L176),
    /// **not** a RLP list.
    ///
    /// Because of this, we do not directly use the `Vec<u8>` when encoding and decoding, and
    /// instead use the [`Encodable`](reth_rlp::Encodable) and [`Decodable`](reth_rlp::Decodable)
    /// implementations for `&[u8]` instead, which encodes into a RLP string, and expects an RLP
    /// string when decoding.
    pub types: Vec<u8>,
    /// Transaction sizes for new transactions that have appeared on the network.
    pub sizes: Vec<usize>,
    /// Transaction hashes for new transactions that have appeared on the network.
    pub hashes: Vec<H256>,
}

impl Encodable for NewPooledTransactionHashes68 {
    fn length(&self) -> usize {
        #[derive(RlpEncodable)]
        struct EncodableNewPooledTransactionHashes68<'a> {
            types: &'a [u8],
            sizes: &'a Vec<usize>,
            hashes: &'a Vec<H256>,
        }

        let encodable = EncodableNewPooledTransactionHashes68 {
            types: &self.types[..],
            sizes: &self.sizes,
            hashes: &self.hashes,
        };

        encodable.length()
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        #[derive(RlpEncodable)]
        struct EncodableNewPooledTransactionHashes68<'a> {
            types: &'a [u8],
            sizes: &'a Vec<usize>,
            hashes: &'a Vec<H256>,
        }

        let encodable = EncodableNewPooledTransactionHashes68 {
            types: &self.types[..],
            sizes: &self.sizes,
            hashes: &self.hashes,
        };

        encodable.encode(out);
    }
}

impl Decodable for NewPooledTransactionHashes68 {
    fn decode(buf: &mut &[u8]) -> Result<Self, reth_rlp::DecodeError> {
        #[derive(RlpDecodable)]
        struct EncodableNewPooledTransactionHashes68 {
            types: Bytes,
            sizes: Vec<usize>,
            hashes: Vec<H256>,
        }

        let encodable = EncodableNewPooledTransactionHashes68::decode(buf)?;
        Ok(Self { types: encodable.types.into(), sizes: encodable.sizes, hashes: encodable.hashes })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bytes::BytesMut;
    use hex_literal::hex;
    use reth_rlp::{Decodable, Encodable};

    use super::*;

    /// Takes as input a struct / encoded hex message pair, ensuring that we encode to the exact hex
    /// message, and decode to the exact struct.
    fn test_encoding_vector<T: Encodable + Decodable + PartialEq + std::fmt::Debug>(
        input: (T, &[u8]),
    ) {
        let (expected_decoded, expected_encoded) = input;
        let mut encoded = BytesMut::new();
        expected_decoded.encode(&mut encoded);

        assert_eq!(hex::encode(&encoded), hex::encode(expected_encoded));

        let decoded = T::decode(&mut encoded.as_ref()).unwrap();
        assert_eq!(expected_decoded, decoded);
    }

    #[test]
    fn can_return_latest_block() {
        let mut blocks = NewBlockHashes(vec![BlockHashNumber { hash: H256::random(), number: 0 }]);
        let latest = blocks.latest().unwrap();
        assert_eq!(latest.number, 0);

        blocks.0.push(BlockHashNumber { hash: H256::random(), number: 100 });
        blocks.0.push(BlockHashNumber { hash: H256::random(), number: 2 });
        let latest = blocks.latest().unwrap();
        assert_eq!(latest.number, 100);
    }

    #[test]
    fn eth_68_tx_hash_roundtrip() {
        let vectors = vec![
            (
            NewPooledTransactionHashes68 { types: vec![], sizes: vec![], hashes: vec![] },
            &hex!("c380c0c0")[..],
            ),
            (
            NewPooledTransactionHashes68 {
                types: vec![0x00],
                sizes: vec![0x00],
                hashes: vec![H256::from_str(
                    "0x0000000000000000000000000000000000000000000000000000000000000000",
                )
                .unwrap()],
            },
            &hex!("e500c180e1a00000000000000000000000000000000000000000000000000000000000000000")[..],
            ),
            (
            NewPooledTransactionHashes68 {
                types: vec![0x00, 0x00],
                sizes: vec![0x00, 0x00],
                hashes: vec![
                    H256::from_str(
                        "0x0000000000000000000000000000000000000000000000000000000000000000",
                    )
                    .unwrap(),
                    H256::from_str(
                        "0x0000000000000000000000000000000000000000000000000000000000000000",
                    )
                    .unwrap(),
                ],
            },
            &hex!("f84a820000c28080f842a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000")[..],
            ),
            (
            NewPooledTransactionHashes68 {
                types: vec![0x02],
                sizes: vec![0xb6],
                hashes: vec![H256::from_str(
                    "0xfecbed04c7b88d8e7221a0a3f5dc33f220212347fc167459ea5cc9c3eb4c1124",
                )
                .unwrap()],
            },
            &hex!("e602c281b6e1a0fecbed04c7b88d8e7221a0a3f5dc33f220212347fc167459ea5cc9c3eb4c1124")[..],
            ),
            (
            NewPooledTransactionHashes68 {
                types: vec![0xff, 0xff],
                sizes: vec![0xffffffff, 0xffffffff],
                hashes: vec![
                    H256::from_str(
                        "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                    )
                    .unwrap(),
                    H256::from_str(
                        "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                    )
                    .unwrap(),
                ],
            },
            &hex!("f85282ffffca84ffffffff84fffffffff842a0ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa0ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")[..],
            ),
            (
            NewPooledTransactionHashes68 {
                types: vec![0xff, 0xff],
                sizes: vec![0xffffffff, 0xffffffff],
                hashes: vec![
                    H256::from_str(
                        "0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafe",
                    )
                    .unwrap(),
                    H256::from_str(
                        "0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafe",
                    )
                    .unwrap(),
                ],
            },
            &hex!("f85282ffffca84ffffffff84fffffffff842a0beefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafea0beefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafe")[..],
            ),
            (
            NewPooledTransactionHashes68 {
                types: vec![0x10, 0x10],
                sizes: vec![0xdeadc0de, 0xdeadc0de],
                hashes: vec![
                    H256::from_str(
                        "0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2",
                    )
                    .unwrap(),
                    H256::from_str(
                        "0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2",
                    )
                    .unwrap(),
                ],
            },
            &hex!("f852821010ca84deadc0de84deadc0def842a03b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2a03b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2")[..],
            ),
            (
            NewPooledTransactionHashes68 {
                types: vec![0x6f, 0x6f],
                sizes: vec![0x7fffffff, 0x7fffffff],
                hashes: vec![
                    H256::from_str(
                        "0x0000000000000000000000000000000000000000000000000000000000000002",
                    )
                    .unwrap(),
                    H256::from_str(
                        "0x0000000000000000000000000000000000000000000000000000000000000002",
                    )
                    .unwrap(),
                ],
            },
            &hex!("f852826f6fca847fffffff847ffffffff842a00000000000000000000000000000000000000000000000000000000000000002a00000000000000000000000000000000000000000000000000000000000000002")[..],
            ),
        ];

        for vector in vectors {
            test_encoding_vector(vector);
        }
    }
}
