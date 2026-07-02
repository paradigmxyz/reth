//! Types for broadcasting new data.

use crate::{EthMessage, EthVersion, NetworkPrimitives};
use alloc::{sync::Arc, vec::Vec};
use alloy_primitives::{
    map::{B256Map, B256Set},
    Bytes, TxHash, B128, B256, U128,
};
use alloy_rlp::{
    Decodable, Encodable, Header, RlpDecodable, RlpDecodableWrapper, RlpEncodable,
    RlpEncodableWrapper, EMPTY_STRING_CODE,
};
use core::{fmt::Debug, mem};
use derive_more::{Constructor, Deref, DerefMut, From, IntoIterator};
use reth_codecs_derive::{add_arbitrary_tests, generate_tests};
use reth_ethereum_primitives::TransactionSigned;
use reth_primitives_traits::{Block, InMemorySize, SignedTransaction};

/// This informs peers of new blocks that have appeared on the network.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Default,
    Deref,
    DerefMut,
    IntoIterator,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct NewBlockHashes(
    /// New block hashes and the block number for each blockhash.
    /// Clients should request blocks using a [`GetBlockBodies`](crate::GetBlockBodies) message.
    pub Vec<BlockHashNumber>,
);

// === impl NewBlockHashes ===

impl NewBlockHashes {
    /// Returns the latest block in the list of blocks.
    pub fn latest(&self) -> Option<&BlockHashNumber> {
        self.iter().max_by_key(|b| b.number)
    }
}

/// A block hash _and_ a block number.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct BlockHashNumber {
    /// The block hash
    pub hash: B256,
    /// The block number
    pub number: u64,
}

impl From<Vec<BlockHashNumber>> for NewBlockHashes {
    fn from(v: Vec<BlockHashNumber>) -> Self {
        Self(v)
    }
}

impl From<NewBlockHashes> for Vec<BlockHashNumber> {
    fn from(v: NewBlockHashes) -> Self {
        v.0
    }
}

/// A trait for block payloads transmitted through p2p.
pub trait NewBlockPayload:
    Encodable + Decodable + Clone + Eq + Debug + Send + Sync + Unpin + 'static
{
    /// The block type.
    type Block: Block;

    /// Returns a reference to the block.
    fn block(&self) -> &Self::Block;
}

/// A new block with the current total difficulty, which includes the difficulty of the returned
/// block.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct NewBlock<B = reth_ethereum_primitives::Block> {
    /// A new block.
    pub block: B,
    /// The current total difficulty.
    pub td: U128,
}

impl<B: Block + 'static> NewBlockPayload for NewBlock<B> {
    type Block = B;

    fn block(&self) -> &Self::Block {
        &self.block
    }
}

generate_tests!(#[rlp, 25] NewBlock<reth_ethereum_primitives::Block>, EthNewBlockTests);

/// This informs peers of transactions that have appeared on the network and are not yet included
/// in a block.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Default,
    Deref,
    IntoIterator,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp, 10)]
pub struct Transactions<T = TransactionSigned>(
    /// New transactions for the peer to include in its mempool.
    pub Vec<T>,
);

impl<T: SignedTransaction> Transactions<T> {
    /// Returns `true` if the list of transactions contains any blob transactions.
    pub fn has_eip4844(&self) -> bool {
        self.iter().any(|tx| tx.is_eip4844())
    }
}

impl<T> From<Vec<T>> for Transactions<T> {
    fn from(txs: Vec<T>) -> Self {
        Self(txs)
    }
}

impl<T> From<Transactions<T>> for Vec<T> {
    fn from(txs: Transactions<T>) -> Self {
        txs.0
    }
}

impl<T: Decodable + InMemorySize> Transactions<T> {
    /// Decodes the RLP list of transactions, stopping once the cumulative
    /// [`InMemorySize`] of decoded transactions exceeds `memory_budget` bytes.
    /// Any remaining transactions in the payload are skipped.
    pub fn decode_with_memory_budget(
        buf: &mut &[u8],
        memory_budget: usize,
    ) -> alloy_rlp::Result<Self> {
        decode_list_with_memory_budget(buf, memory_budget).map(Self)
    }
}

/// Decodes an RLP list, stopping once the cumulative [`InMemorySize`] of decoded items exceeds
/// `memory_budget` bytes. Any remaining items in the payload are skipped.
pub fn decode_list_with_memory_budget<T: Decodable + InMemorySize>(
    buf: &mut &[u8],
    memory_budget: usize,
) -> alloy_rlp::Result<Vec<T>> {
    let header = Header::decode(buf)?;
    if !header.list {
        return Err(alloy_rlp::Error::UnexpectedString);
    }
    if buf.len() < header.payload_length {
        return Err(alloy_rlp::Error::InputTooShort);
    }

    let (payload, rest) = buf.split_at(header.payload_length);
    let mut payload = payload;

    let mut txs = Vec::with_capacity(estimated_transaction_list_capacity(header.payload_length));
    let mut total_size = 0usize;

    while !payload.is_empty() {
        let item = T::decode(&mut payload)?;
        total_size = total_size.saturating_add(item.size());

        if total_size > memory_budget {
            break;
        }

        txs.push(item);
    }

    *buf = rest;
    Ok(txs)
}

// Keep this as a conservative hint: small lists stay allocation-free until the first push, while
// large untrusted payloads cannot force an outsized preallocation.
const MIN_TRANSACTION_RLP_SIZE_ESTIMATE: usize = 128;
const MIN_PREALLOCATED_TRANSACTIONS: usize = 4;
const MAX_PREALLOCATED_TRANSACTIONS: usize = 1024;

const fn estimated_transaction_list_capacity(payload_length: usize) -> usize {
    let estimate = payload_length / MIN_TRANSACTION_RLP_SIZE_ESTIMATE;
    if estimate < MIN_PREALLOCATED_TRANSACTIONS {
        0
    } else if estimate > MAX_PREALLOCATED_TRANSACTIONS {
        MAX_PREALLOCATED_TRANSACTIONS
    } else {
        estimate
    }
}

/// Same as [`Transactions`] but this is intended as egress message send from local to _many_ peers.
///
/// The list of transactions is constructed on per-peers basis, but the underlying transaction
/// objects are shared.
#[derive(
    Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Deref, IntoIterator,
)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp, 20)]
pub struct SharedTransactions<T = TransactionSigned>(
    /// New transactions for the peer to include in its mempool.
    pub Vec<Arc<T>>,
);

/// A wrapper type for all different new pooled transaction types
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NewPooledTransactionHashes {
    /// A list of transaction hashes valid for [66-68)
    Eth66(NewPooledTransactionHashes66),
    /// A list of transaction hashes valid for [68-72)
    ///
    /// Note: it is assumed that the payload is valid (all vectors have the same length)
    Eth68(NewPooledTransactionHashes68),
    /// A list of transaction hashes valid from [72..]
    ///
    /// This extends the eth/68 announcement payload with the `cell_mask` field introduced by
    /// [EIP-8070](https://eips.ethereum.org/EIPS/eip-8070).
    ///
    /// Note: it is assumed that the payload is valid (all vectors have the same length)
    Eth72(NewPooledTransactionHashes72),
}

// === impl NewPooledTransactionHashes ===

impl NewPooledTransactionHashes {
    /// Returns the message [`EthVersion`].
    pub const fn version(&self) -> EthVersion {
        match self {
            Self::Eth66(_) => EthVersion::Eth66,
            Self::Eth68(_) => EthVersion::Eth68,
            Self::Eth72(_) => EthVersion::Eth72,
        }
    }

    /// Returns `true` if the payload is valid for the given version
    pub const fn is_valid_for_version(&self, version: EthVersion) -> bool {
        match self {
            Self::Eth66(_) => {
                matches!(version, EthVersion::Eth67 | EthVersion::Eth66)
            }
            Self::Eth68(_) => {
                matches!(
                    version,
                    EthVersion::Eth68 | EthVersion::Eth69 | EthVersion::Eth70 | EthVersion::Eth71
                )
            }
            Self::Eth72(_) => {
                matches!(version, EthVersion::Eth72)
            }
        }
    }

    /// Returns an iterator over all transaction hashes.
    pub fn iter_hashes(&self) -> impl Iterator<Item = &B256> + '_ {
        match self {
            Self::Eth66(msg) => msg.iter(),
            Self::Eth68(msg) => msg.hashes.iter(),
            Self::Eth72(msg) => msg.hashes.iter(),
        }
    }

    /// Returns an immutable reference to transaction hashes.
    pub const fn hashes(&self) -> &Vec<B256> {
        match self {
            Self::Eth66(msg) => &msg.0,
            Self::Eth68(msg) => &msg.hashes,
            Self::Eth72(msg) => &msg.hashes,
        }
    }

    /// Returns a mutable reference to transaction hashes.
    pub const fn hashes_mut(&mut self) -> &mut Vec<B256> {
        match self {
            Self::Eth66(msg) => &mut msg.0,
            Self::Eth68(msg) => &mut msg.hashes,
            Self::Eth72(msg) => &mut msg.hashes,
        }
    }

    /// Consumes the type and returns all hashes
    pub fn into_hashes(self) -> Vec<B256> {
        match self {
            Self::Eth66(msg) => msg.0,
            Self::Eth68(msg) => msg.hashes,
            Self::Eth72(msg) => msg.hashes,
        }
    }

    /// Returns an iterator over all transaction hashes.
    pub fn into_iter_hashes(self) -> impl Iterator<Item = B256> {
        match self {
            Self::Eth66(msg) => msg.into_iter(),
            Self::Eth68(msg) => msg.hashes.into_iter(),
            Self::Eth72(msg) => msg.hashes.into_iter(),
        }
    }

    /// Shortens the number of hashes in the message, keeping the first `len` hashes and dropping
    /// the rest. If `len` is greater than the number of hashes, this has no effect.
    pub fn truncate(&mut self, len: usize) {
        match self {
            Self::Eth66(msg) => msg.truncate(len),
            Self::Eth68(msg) => {
                msg.types.truncate(len);
                msg.sizes.truncate(len);
                msg.hashes.truncate(len);
            }
            Self::Eth72(msg) => {
                msg.types.truncate(len);
                msg.sizes.truncate(len);
                msg.hashes.truncate(len);
            }
        }
    }

    /// Returns true if the message is empty
    pub const fn is_empty(&self) -> bool {
        match self {
            Self::Eth66(msg) => msg.0.is_empty(),
            Self::Eth68(msg) => msg.hashes.is_empty(),
            Self::Eth72(msg) => msg.hashes.is_empty(),
        }
    }

    /// Returns the number of hashes in the message
    pub const fn len(&self) -> usize {
        match self {
            Self::Eth66(msg) => msg.0.len(),
            Self::Eth68(msg) => msg.hashes.len(),
            Self::Eth72(msg) => msg.hashes.len(),
        }
    }

    /// Returns an immutable reference to the inner type if this is an eth68 announcement.
    pub const fn as_eth72(&self) -> Option<&NewPooledTransactionHashes72> {
        match self {
            Self::Eth66(_) | Self::Eth68(_) => None,
            Self::Eth72(msg) => Some(msg),
        }
    }

    /// Returns a mutable reference to the inner type if this is an eth68 announcement.
    pub const fn as_eth72_mut(&mut self) -> Option<&mut NewPooledTransactionHashes72> {
        match self {
            Self::Eth66(_) | Self::Eth68(_) => None,
            Self::Eth72(msg) => Some(msg),
        }
    }

    /// Returns an immutable reference to the inner type if this is an eth68 announcement.
    pub const fn as_eth68(&self) -> Option<&NewPooledTransactionHashes68> {
        match self {
            Self::Eth66(_) | Self::Eth72(_) => None,
            Self::Eth68(msg) => Some(msg),
        }
    }

    /// Returns a mutable reference to the inner type if this is an eth68 announcement.
    pub const fn as_eth68_mut(&mut self) -> Option<&mut NewPooledTransactionHashes68> {
        match self {
            Self::Eth66(_) | Self::Eth72(_) => None,
            Self::Eth68(msg) => Some(msg),
        }
    }

    /// Returns a mutable reference to the inner type if this is an eth66 announcement.
    pub const fn as_eth66_mut(&mut self) -> Option<&mut NewPooledTransactionHashes66> {
        match self {
            Self::Eth66(msg) => Some(msg),
            Self::Eth68(_) | Self::Eth72(_) => None,
        }
    }

    /// Returns the inner type if this is an eth68 announcement.
    pub fn take_eth68(&mut self) -> Option<NewPooledTransactionHashes68> {
        match self {
            Self::Eth66(_) | Self::Eth72(_) => None,
            Self::Eth68(msg) => Some(mem::take(msg)),
        }
    }

    /// Returns the inner type if this is an eth66 announcement.
    pub fn take_eth66(&mut self) -> Option<NewPooledTransactionHashes66> {
        match self {
            Self::Eth66(msg) => Some(mem::take(msg)),
            Self::Eth68(_) | Self::Eth72(_) => None,
        }
    }
}

impl<N: NetworkPrimitives> From<NewPooledTransactionHashes> for EthMessage<N> {
    fn from(value: NewPooledTransactionHashes) -> Self {
        match value {
            NewPooledTransactionHashes::Eth66(msg) => Self::NewPooledTransactionHashes66(msg),
            NewPooledTransactionHashes::Eth68(msg) => Self::NewPooledTransactionHashes68(msg),
            NewPooledTransactionHashes::Eth72(msg) => Self::NewPooledTransactionHashes72(msg),
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

impl From<NewPooledTransactionHashes72> for NewPooledTransactionHashes {
    fn from(hashes: NewPooledTransactionHashes72) -> Self {
        Self::Eth72(hashes)
    }
}

/// This informs peers of transaction hashes for transactions that have appeared on the network,
/// but have not been included in a block.
#[derive(Clone, Debug, PartialEq, Eq, Default, Deref, DerefMut, IntoIterator)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct NewPooledTransactionHashes66(
    /// Transaction hashes for new transactions that have appeared on the network.
    /// Clients should request the transactions with the given hashes using a
    /// [`GetPooledTransactions`](crate::GetPooledTransactions) message.
    pub Vec<B256>,
);

impl NewPooledTransactionHashes66 {
    /// Returns a new instance with capacity for `capacity` hashes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }
}

impl Encodable for NewPooledTransactionHashes66 {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        encode_b256_rlp_list(&self.0, out);
    }

    fn length(&self) -> usize {
        b256_rlp_list_length(&self.0)
    }
}

impl Decodable for NewPooledTransactionHashes66 {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        decode_b256_rlp_list(buf).map(Self)
    }
}

impl From<Vec<B256>> for NewPooledTransactionHashes66 {
    fn from(v: Vec<B256>) -> Self {
        Self(v)
    }
}

const RLP_B256_ENCODED_LEN: usize = 33;
const RLP_B256_STRING_PREFIX: u8 = EMPTY_STRING_CODE + 32;

const fn b256_rlp_list_payload_length(hashes: &[B256]) -> usize {
    RLP_B256_ENCODED_LEN * hashes.len()
}

const fn b256_rlp_list_length(hashes: &[B256]) -> usize {
    Header { list: true, payload_length: b256_rlp_list_payload_length(hashes) }
        .length_with_payload()
}

fn encode_b256_rlp_list(hashes: &[B256], out: &mut dyn bytes::BufMut) {
    Header { list: true, payload_length: b256_rlp_list_payload_length(hashes) }.encode(out);
    for hash in hashes {
        out.put_u8(RLP_B256_STRING_PREFIX);
        out.put_slice(hash.as_slice());
    }
}

fn decode_b256_rlp_list(buf: &mut &[u8]) -> alloy_rlp::Result<Vec<B256>> {
    let Header { list, payload_length } = Header::decode(buf)?;
    if !list {
        return Err(alloy_rlp::Error::UnexpectedString)
    }

    let (payload, rest) = buf.split_at(payload_length);
    let hashes =
        decode_canonical_b256_list(payload).map_or_else(|| decode_b256_list(payload), Ok)?;
    *buf = rest;

    Ok(hashes)
}

fn decode_canonical_b256_list(payload: &[u8]) -> Option<Vec<B256>> {
    let (chunks, remainder) = payload.as_chunks::<RLP_B256_ENCODED_LEN>();
    if !remainder.is_empty() {
        return None
    }

    if chunks.len() < 4 {
        let mut hashes = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            if chunk[0] != RLP_B256_STRING_PREFIX {
                return None
            }
            hashes.push(B256::from_slice(&chunk[1..]));
        }

        return Some(hashes)
    }

    let mut hashes = Vec::with_capacity(chunks.len());
    for (slot, chunk) in hashes.spare_capacity_mut().iter_mut().zip(chunks) {
        if chunk[0] != RLP_B256_STRING_PREFIX {
            return None
        }
        slot.write(B256::from_slice(&chunk[1..]));
    }
    // SAFETY: the loop above initialized exactly one slot for each chunk.
    unsafe {
        hashes.set_len(chunks.len());
    }

    Some(hashes)
}

fn decode_b256_list(mut payload: &[u8]) -> alloy_rlp::Result<Vec<B256>> {
    let mut hashes = Vec::new();
    while !payload.is_empty() {
        hashes.push(B256::decode(&mut payload)?);
    }
    Ok(hashes)
}

fn decode_usize_rlp_list_with_capacity(
    buf: &mut &[u8],
    capacity: usize,
) -> alloy_rlp::Result<Vec<usize>> {
    let Header { list, payload_length } = Header::decode(buf)?;
    if !list {
        return Err(alloy_rlp::Error::UnexpectedString)
    }

    let (mut payload, rest) = buf.split_at(payload_length);
    if let Some(values) = decode_fixed_width_usize_list(payload, capacity) {
        *buf = rest;
        return values
    }

    let mut values = Vec::with_capacity(capacity);
    while !payload.is_empty() {
        values.push(usize::decode(&mut payload)?);
    }
    *buf = rest;

    Ok(values)
}

fn decode_fixed_width_usize_list(
    payload: &[u8],
    capacity: usize,
) -> Option<alloy_rlp::Result<Vec<usize>>> {
    if capacity == 0 {
        return payload.is_empty().then(|| Ok(Vec::new()))
    }

    if payload.len() == capacity {
        let mut values = Vec::with_capacity(capacity);
        for byte in payload {
            match *byte {
                value @ 0..=0x7f => values.push(value as usize),
                EMPTY_STRING_CODE => values.push(0),
                _ => return None,
            }
        }

        return Some(Ok(values))
    }

    let encoded_len = payload.len().checked_div(capacity)?;
    if !payload.len().is_multiple_of(capacity) {
        return None
    }

    let value_len = encoded_len.checked_sub(1)?;
    if value_len == 0 || value_len > core::mem::size_of::<usize>() {
        return None
    }

    let prefix = EMPTY_STRING_CODE + value_len as u8;
    let mut values = Vec::with_capacity(capacity);
    if value_len == 1 {
        for chunk in payload.chunks_exact(encoded_len) {
            if chunk[0] != prefix {
                return None
            }
            let value = chunk[1];
            if value < EMPTY_STRING_CODE {
                return Some(Err(alloy_rlp::Error::NonCanonicalSingleByte))
            }
            values.push(value as usize);
        }

        return Some(Ok(values))
    }

    match value_len {
        2 => {
            for chunk in payload.chunks_exact(encoded_len) {
                if chunk[0] != prefix {
                    return None
                }
                let bytes = [chunk[1], chunk[2]];
                if bytes[0] == 0 {
                    return Some(Err(alloy_rlp::Error::LeadingZero))
                }
                values.push(u16::from_be_bytes(bytes) as usize);
            }

            return Some(Ok(values))
        }
        3 => {
            for chunk in payload.chunks_exact(encoded_len) {
                if chunk[0] != prefix {
                    return None
                }
                let bytes = &chunk[1..4];
                if bytes[0] == 0 {
                    return Some(Err(alloy_rlp::Error::LeadingZero))
                }
                values.push(
                    ((bytes[0] as usize) << 16) | ((bytes[1] as usize) << 8) | bytes[2] as usize,
                );
            }

            return Some(Ok(values))
        }
        4 => {
            for chunk in payload.chunks_exact(encoded_len) {
                if chunk[0] != prefix {
                    return None
                }
                let bytes = [chunk[1], chunk[2], chunk[3], chunk[4]];
                if bytes[0] == 0 {
                    return Some(Err(alloy_rlp::Error::LeadingZero))
                }
                values.push(u32::from_be_bytes(bytes) as usize);
            }

            return Some(Ok(values))
        }
        8 => {
            for chunk in payload.chunks_exact(encoded_len) {
                if chunk[0] != prefix {
                    return None
                }
                let bytes = [
                    chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7], chunk[8],
                ];
                if bytes[0] == 0 {
                    return Some(Err(alloy_rlp::Error::LeadingZero))
                }
                let Ok(value) = usize::try_from(u64::from_be_bytes(bytes)) else {
                    return Some(Err(alloy_rlp::Error::Overflow))
                };
                values.push(value);
            }

            return Some(Ok(values))
        }
        _ => {}
    }

    for chunk in payload.chunks_exact(encoded_len) {
        if chunk[0] != prefix {
            return None
        }

        let value_bytes = &chunk[1..];
        if value_len > 1 && value_bytes[0] == 0 {
            return Some(Err(alloy_rlp::Error::LeadingZero))
        }

        let mut value = 0usize;
        for byte in value_bytes {
            value = (value << 8) | *byte as usize;
        }
        values.push(value);
    }

    Some(Ok(values))
}

/// Same as [`NewPooledTransactionHashes66`] but extends that beside the transaction hashes,
/// the node sends the transaction types and their sizes (as defined in EIP-2718) as well.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NewPooledTransactionHashes68 {
    /// Transaction types for new transactions that have appeared on the network.
    ///
    /// ## Note on RLP encoding and decoding
    ///
    /// In the [eth/68 spec](https://eips.ethereum.org/EIPS/eip-5793#specification) this is defined
    /// the following way:
    ///  * `[type_0: B_1, type_1: B_1, ...]`
    ///
    /// This would make it seem like the [`Encodable`] and
    /// [`Decodable`] implementations should directly use a `Vec<u8>` for
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
    /// instead use the [`Encodable`] and [`Decodable`]
    /// implementations for `&[u8]` instead, which encodes into a RLP string, and expects an RLP
    /// string when decoding.
    pub types: Vec<u8>,
    /// Transaction sizes for new transactions that have appeared on the network.
    pub sizes: Vec<usize>,
    /// Transaction hashes for new transactions that have appeared on the network.
    pub hashes: Vec<B256>,
}

#[cfg(feature = "arbitrary")]
impl proptest::prelude::Arbitrary for NewPooledTransactionHashes68 {
    type Parameters = ();
    fn arbitrary_with(_args: ()) -> Self::Strategy {
        use proptest::{collection::vec, prelude::*};
        // Generate a single random length for all vectors
        let vec_length = any::<usize>().prop_map(|x| x % 100 + 1); // Lengths between 1 and 100

        vec_length
            .prop_flat_map(|len| {
                // Use the generated length to create vectors of TxType, usize, and B256
                let types_vec = vec(
                    proptest_arbitrary_interop::arb::<reth_ethereum_primitives::TxType>()
                        .prop_map(|ty| ty as u8),
                    len..=len,
                );

                // Map the usize values to the range 0..131072(0x20000)
                let sizes_vec = vec(proptest::num::usize::ANY.prop_map(|x| x % 131072), len..=len);
                let hashes_vec = vec(any::<B256>(), len..=len);

                (types_vec, sizes_vec, hashes_vec)
            })
            .prop_map(|(types, sizes, hashes)| Self { types, sizes, hashes })
            .boxed()
    }

    type Strategy = proptest::prelude::BoxedStrategy<Self>;
}

impl NewPooledTransactionHashes68 {
    /// Returns a new instance with capacity for `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            types: Vec::with_capacity(capacity),
            sizes: Vec::with_capacity(capacity),
            hashes: Vec::with_capacity(capacity),
        }
    }

    /// Returns an iterator over tx hashes zipped with corresponding metadata.
    pub fn metadata_iter(&self) -> impl Iterator<Item = (&B256, (u8, usize))> {
        self.hashes.iter().zip(self.types.iter().copied().zip(self.sizes.iter().copied()))
    }

    /// Appends a transaction
    pub fn push<T: SignedTransaction>(&mut self, tx: &T) {
        self.hashes.push(*tx.tx_hash());
        self.sizes.push(tx.encode_2718_len());
        self.types.push(tx.ty());
    }

    /// Appends the provided transactions
    pub fn extend<'a, T: SignedTransaction>(&mut self, txs: impl IntoIterator<Item = &'a T>) {
        for tx in txs {
            self.push(tx);
        }
    }

    /// Shrinks the capacity of the message vectors as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.hashes.shrink_to_fit();
        self.sizes.shrink_to_fit();
        self.types.shrink_to_fit()
    }

    /// Consumes and appends a transaction
    pub fn with_transaction<T: SignedTransaction>(mut self, tx: &T) -> Self {
        self.push(tx);
        self
    }

    /// Consumes and appends the provided transactions
    pub fn with_transactions<'a, T: SignedTransaction>(
        mut self,
        txs: impl IntoIterator<Item = &'a T>,
    ) -> Self {
        self.extend(txs);
        self
    }

    fn payload_length(&self) -> usize {
        self.types.as_slice().length() + self.sizes.length() + b256_rlp_list_length(&self.hashes)
    }
}

impl Encodable for NewPooledTransactionHashes68 {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        Header { list: true, payload_length: self.payload_length() }.encode(out);
        self.types.as_slice().encode(out);
        self.sizes.encode(out);
        encode_b256_rlp_list(&self.hashes, out);
    }

    fn length(&self) -> usize {
        Header { list: true, payload_length: self.payload_length() }.length_with_payload()
    }
}

impl Decodable for NewPooledTransactionHashes68 {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let Header { list, payload_length } = Header::decode(buf)?;
        if !list {
            return Err(alloy_rlp::Error::UnexpectedString)
        }

        let (mut payload, rest) = buf.split_at(payload_length);
        let types = Bytes::decode(&mut payload)?;
        let sizes = decode_usize_rlp_list_with_capacity(&mut payload, types.len())?;
        let hashes = decode_b256_rlp_list(&mut payload)?;
        if !payload.is_empty() {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: payload_length,
                got: payload_length - payload.len(),
            })
        }
        *buf = rest;

        let msg = Self { types: types.into(), sizes, hashes };

        if msg.hashes.len() != msg.types.len() {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: msg.hashes.len(),
                got: msg.types.len(),
            })
        }
        if msg.hashes.len() != msg.sizes.len() {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: msg.hashes.len(),
                got: msg.sizes.len(),
            })
        }

        Ok(msg)
    }
}

/// Same as [`NewPooledTransactionHashes68`] but adds the eth/72 `cell_mask` field from
/// [EIP-8070](https://eips.ethereum.org/EIPS/eip-8070).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NewPooledTransactionHashes72 {
    /// Transaction types for new transactions that have appeared on the network.
    ///
    /// ## Note on RLP encoding and decoding
    ///
    /// In the [eth/72 spec](https://eips.ethereum.org/EIPS/eip-8070#specification) this is defined
    /// the following way:
    ///  * `[types: B, [size_0: P, size_1: P, ...], [hash_0: B_32, hash_1: B_32, ...], cell_mask:
    ///    B_16]`
    pub types: Vec<u8>,
    /// Transaction sizes for new transactions that have appeared on the network.
    pub sizes: Vec<usize>,
    /// Transaction hashes for new transactions that have appeared on the network.
    pub hashes: Vec<B256>,
    /// Cell availability mask for type 3 (blob) transactions announced by this message.
    ///
    /// Per [EIP-8070](https://eips.ethereum.org/EIPS/eip-8070), this is a `B_16`
    /// bitarray over `CELLS_PER_EXT_BLOB`; bit `i` is set when the announcer has column
    /// `i` available for every type 3 transaction in the message. `None` encodes as RLP
    /// `nil` and must be used when no type 3 transactions are announced.
    pub cell_mask: Option<B128>,
}

#[cfg(feature = "arbitrary")]
impl proptest::prelude::Arbitrary for NewPooledTransactionHashes72 {
    type Parameters = ();
    fn arbitrary_with(_args: ()) -> Self::Strategy {
        use proptest::{collection::vec, prelude::*};
        // Generate a single random length for all vectors
        let vec_length = any::<usize>().prop_map(|x| x % 100 + 1); // Lengths between 1 and 100

        vec_length
            .prop_flat_map(|len| {
                // Use the generated length to create vectors of TxType, usize, and B256
                let types_vec = vec(
                    proptest_arbitrary_interop::arb::<reth_ethereum_primitives::TxType>()
                        .prop_map(|ty| ty as u8),
                    len..=len,
                );

                // Map the usize values to the range 0..131072(0x20000)
                let sizes_vec = vec(proptest::num::usize::ANY.prop_map(|x| x % 131072), len..=len);
                let hashes_vec = vec(any::<B256>(), len..=len);
                let cell_mask = any::<Option<B128>>();

                (types_vec, sizes_vec, hashes_vec, cell_mask)
            })
            .prop_map(|(types, sizes, hashes, cell_mask)| Self { types, sizes, hashes, cell_mask })
            .boxed()
    }

    type Strategy = proptest::prelude::BoxedStrategy<Self>;
}

impl NewPooledTransactionHashes72 {
    /// Returns a new instance with capacity for `capacity` entries and no cell mask.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            types: Vec::with_capacity(capacity),
            sizes: Vec::with_capacity(capacity),
            hashes: Vec::with_capacity(capacity),
            cell_mask: None,
        }
    }

    /// Returns an iterator over tx hashes zipped with corresponding metadata.
    pub fn metadata_iter(&self) -> impl Iterator<Item = (&B256, (u8, usize))> {
        self.hashes.iter().zip(self.types.iter().copied().zip(self.sizes.iter().copied()))
    }

    /// Appends a transaction
    pub fn push<T: SignedTransaction>(&mut self, tx: &T) {
        self.hashes.push(*tx.tx_hash());
        self.sizes.push(tx.encode_2718_len());
        self.types.push(tx.ty());
    }

    /// Appends the provided transactions
    pub fn extend<'a, T: SignedTransaction>(&mut self, txs: impl IntoIterator<Item = &'a T>) {
        for tx in txs {
            self.push(tx);
        }
    }

    /// Shrinks the capacity of the message vectors as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.hashes.shrink_to_fit();
        self.sizes.shrink_to_fit();
        self.types.shrink_to_fit()
    }

    /// Consumes and appends a transaction
    pub fn with_transaction<T: SignedTransaction>(mut self, tx: &T) -> Self {
        self.push(tx);
        self
    }

    /// Consumes and appends the provided transactions
    pub fn with_transactions<'a, T: SignedTransaction>(
        mut self,
        txs: impl IntoIterator<Item = &'a T>,
    ) -> Self {
        self.extend(txs);
        self
    }

    fn payload_length(&self) -> usize {
        self.types.as_slice().length() +
            self.sizes.length() +
            b256_rlp_list_length(&self.hashes) +
            self.cell_mask.as_ref().map_or(1, Encodable::length)
    }
}

impl Encodable for NewPooledTransactionHashes72 {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        Header { list: true, payload_length: self.payload_length() }.encode(out);
        self.types.as_slice().encode(out);
        self.sizes.encode(out);
        encode_b256_rlp_list(&self.hashes, out);
        if let Some(cell_mask) = &self.cell_mask {
            cell_mask.encode(out);
        } else {
            out.put_u8(alloy_rlp::EMPTY_STRING_CODE);
        }
    }

    fn length(&self) -> usize {
        Header { list: true, payload_length: self.payload_length() }.length_with_payload()
    }
}

impl Decodable for NewPooledTransactionHashes72 {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let Header { list, payload_length } = Header::decode(buf)?;
        if !list {
            return Err(alloy_rlp::Error::UnexpectedString)
        }
        if buf.len() < payload_length {
            return Err(alloy_rlp::Error::InputTooShort)
        }

        let (mut payload, rest) = buf.split_at(payload_length);
        let types = Bytes::decode(&mut payload)?;
        let sizes = decode_usize_rlp_list_with_capacity(&mut payload, types.len())?;
        let hashes = decode_b256_rlp_list(&mut payload)?;
        let Some(first_byte) = payload.first().copied() else {
            return Err(alloy_rlp::Error::InputTooShort)
        };
        let cell_mask = if first_byte == alloy_rlp::EMPTY_STRING_CODE {
            payload = &payload[1..];
            None
        } else {
            Some(B128::decode(&mut payload)?)
        };

        if !payload.is_empty() {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: payload_length,
                got: payload_length - payload.len(),
            })
        }

        let msg = Self { types: types.into(), sizes, hashes, cell_mask };

        if msg.hashes.len() != msg.types.len() {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: msg.hashes.len(),
                got: msg.types.len(),
            })
        }
        if msg.hashes.len() != msg.sizes.len() {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: msg.hashes.len(),
                got: msg.sizes.len(),
            })
        }

        *buf = rest;

        Ok(msg)
    }
}

/// Validation pass that checks for unique transaction hashes.
pub trait DedupPayload {
    /// Value type in [`PartiallyValidData`] map.
    type Value;

    /// The payload contains no entries.
    fn is_empty(&self) -> bool;

    /// Returns the number of entries.
    fn len(&self) -> usize;

    /// Consumes self, returning an iterator over hashes in payload.
    fn dedup(self) -> PartiallyValidData<Self::Value>;
}

/// Value in [`PartiallyValidData`] map obtained from an announcement.
pub type Eth68TxMetadata = Option<(u8, usize)>;

impl DedupPayload for NewPooledTransactionHashes {
    type Value = Eth68TxMetadata;

    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn len(&self) -> usize {
        self.len()
    }

    fn dedup(self) -> PartiallyValidData<Self::Value> {
        match self {
            Self::Eth66(msg) => msg.dedup(),
            Self::Eth68(msg) => msg.dedup(),
            Self::Eth72(msg) => msg.dedup(),
        }
    }
}

impl DedupPayload for NewPooledTransactionHashes72 {
    type Value = Eth68TxMetadata;

    fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    fn len(&self) -> usize {
        self.hashes.len()
    }

    fn dedup(self) -> PartiallyValidData<Self::Value> {
        let Self { hashes, mut sizes, mut types, .. } = self;

        let mut deduped_data = B256Map::with_capacity_and_hasher(hashes.len(), Default::default());

        for hash in hashes.into_iter().rev() {
            if let (Some(ty), Some(size)) = (types.pop(), sizes.pop()) {
                deduped_data.insert(hash, Some((ty, size)));
            }
        }

        PartiallyValidData::from_raw_data_eth72(deduped_data)
    }
}

impl DedupPayload for NewPooledTransactionHashes68 {
    type Value = Eth68TxMetadata;

    fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    fn len(&self) -> usize {
        self.hashes.len()
    }

    fn dedup(self) -> PartiallyValidData<Self::Value> {
        let Self { hashes, mut sizes, mut types } = self;

        let mut deduped_data = B256Map::with_capacity_and_hasher(hashes.len(), Default::default());

        for hash in hashes.into_iter().rev() {
            if let (Some(ty), Some(size)) = (types.pop(), sizes.pop()) {
                deduped_data.insert(hash, Some((ty, size)));
            }
        }

        PartiallyValidData::from_raw_data_eth68(deduped_data)
    }
}

impl DedupPayload for NewPooledTransactionHashes66 {
    type Value = Eth68TxMetadata;

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn dedup(self) -> PartiallyValidData<Self::Value> {
        let Self(hashes) = self;

        let mut deduped_data = B256Map::with_capacity_and_hasher(hashes.len(), Default::default());

        let noop_value: Eth68TxMetadata = None;

        for hash in hashes.into_iter().rev() {
            deduped_data.insert(hash, noop_value);
        }

        PartiallyValidData::from_raw_data_eth66(deduped_data)
    }
}

/// Interface for handling mempool message data. Used in various filters in pipelines in
/// `TransactionsManager` and in queries to `TransactionPool`.
pub trait HandleMempoolData {
    /// The announcement contains no entries.
    fn is_empty(&self) -> bool;

    /// Returns the number of entries.
    fn len(&self) -> usize;

    /// Retain only entries for which the hash in the entry satisfies a given predicate.
    fn retain_by_hash(&mut self, f: impl FnMut(&TxHash) -> bool);
}

/// Extension of [`HandleMempoolData`] interface, for mempool messages that are versioned.
pub trait HandleVersionedMempoolData {
    /// Returns the announcement version, either [`Eth66`](EthVersion::Eth66) or
    /// [`Eth68`](EthVersion::Eth68).
    fn msg_version(&self) -> EthVersion;
}

impl<T: SignedTransaction> HandleMempoolData for Vec<T> {
    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn len(&self) -> usize {
        self.len()
    }

    fn retain_by_hash(&mut self, mut f: impl FnMut(&TxHash) -> bool) {
        self.retain(|tx| f(tx.tx_hash()))
    }
}

macro_rules! handle_mempool_data_map_impl {
    ($data_ty:ty, $(<$generic:ident>)?) => {
        impl$(<$generic>)? HandleMempoolData for $data_ty {
            fn is_empty(&self) -> bool {
                self.data.is_empty()
            }

            fn len(&self) -> usize {
                self.data.len()
            }

            fn retain_by_hash(&mut self, mut f: impl FnMut(&TxHash) -> bool) {
                self.data.retain(|hash, _| f(hash));
            }
        }
    };
}

/// Data that has passed an initial validation pass that is not specific to any mempool message
/// type.
#[derive(Debug, Deref, DerefMut, IntoIterator)]
pub struct PartiallyValidData<V> {
    #[deref]
    #[deref_mut]
    #[into_iterator]
    data: B256Map<V>,
    version: Option<EthVersion>,
}

handle_mempool_data_map_impl!(PartiallyValidData<V>, <V>);

impl<V> PartiallyValidData<V> {
    /// Wraps raw data.
    pub const fn from_raw_data(data: B256Map<V>, version: Option<EthVersion>) -> Self {
        Self { data, version }
    }

    /// Wraps raw data with version [`EthVersion::Eth72`].
    pub const fn from_raw_data_eth72(data: B256Map<V>) -> Self {
        Self::from_raw_data(data, Some(EthVersion::Eth72))
    }

    /// Wraps raw data with version [`EthVersion::Eth68`].
    pub const fn from_raw_data_eth68(data: B256Map<V>) -> Self {
        Self::from_raw_data(data, Some(EthVersion::Eth68))
    }

    /// Wraps raw data with version [`EthVersion::Eth66`].
    pub const fn from_raw_data_eth66(data: B256Map<V>) -> Self {
        Self::from_raw_data(data, Some(EthVersion::Eth66))
    }

    /// Returns a new [`PartiallyValidData`] with empty data from an [`Eth72`](EthVersion::Eth72)
    /// announcement.
    pub fn empty_eth72() -> Self {
        Self::from_raw_data_eth72(B256Map::default())
    }

    /// Returns a new [`PartiallyValidData`] with empty data from an [`Eth68`](EthVersion::Eth68)
    /// announcement.
    pub fn empty_eth68() -> Self {
        Self::from_raw_data_eth68(B256Map::default())
    }

    /// Returns a new [`PartiallyValidData`] with empty data from an [`Eth66`](EthVersion::Eth66)
    /// announcement.
    pub fn empty_eth66() -> Self {
        Self::from_raw_data_eth66(B256Map::default())
    }

    /// Returns the version of the message this data was received in if different versions of the
    /// message exist.
    pub const fn msg_version(&self) -> Option<EthVersion> {
        self.version
    }

    /// Destructs returning the validated data.
    pub fn into_data(self) -> B256Map<V> {
        self.data
    }
}

/// Partially validated data from an announcement or a
/// [`PooledTransactions`](crate::PooledTransactions) response.
#[derive(Debug, Deref, DerefMut, IntoIterator, From)]
pub struct ValidAnnouncementData {
    #[deref]
    #[deref_mut]
    #[into_iterator]
    data: B256Map<Eth68TxMetadata>,
    version: EthVersion,
}

handle_mempool_data_map_impl!(ValidAnnouncementData,);

impl ValidAnnouncementData {
    /// Destructs returning only the valid hashes and the announcement message version. Caution! If
    /// this is [`Eth68`](EthVersion::Eth68) announcement data, this drops the metadata.
    pub fn into_request_hashes(self) -> (RequestTxHashes, EthVersion) {
        let hashes = self.data.into_keys().collect::<B256Set>();

        (RequestTxHashes::new(hashes), self.version)
    }

    /// Conversion from [`PartiallyValidData`] from an announcement. Note! [`PartiallyValidData`]
    /// from an announcement, should have some [`EthVersion`]. Panics if [`PartiallyValidData`] has
    /// version set to `None`.
    pub fn from_partially_valid_data(data: PartiallyValidData<Eth68TxMetadata>) -> Self {
        let PartiallyValidData { data, version } = data;

        let version = version.expect("should have eth version for conversion");

        Self { data, version }
    }

    /// Destructs returning the validated data.
    pub fn into_data(self) -> B256Map<Eth68TxMetadata> {
        self.data
    }
}

impl HandleVersionedMempoolData for ValidAnnouncementData {
    fn msg_version(&self) -> EthVersion {
        self.version
    }
}

/// Hashes to request from a peer.
#[derive(Debug, Default, Deref, DerefMut, IntoIterator, Constructor)]
pub struct RequestTxHashes {
    #[deref]
    #[deref_mut]
    #[into_iterator(owned, ref)]
    hashes: B256Set,
}

impl RequestTxHashes {
    /// Returns a new [`RequestTxHashes`] with given capacity for hashes. Caution! Make sure to
    /// call `shrink_to_fit` on [`RequestTxHashes`] when full, especially where it will
    /// be stored in its entirety like in the future waiting for a
    /// [`GetPooledTransactions`](crate::GetPooledTransactions) request to resolve.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(B256Set::with_capacity_and_hasher(capacity, Default::default()))
    }

    /// Returns a new empty instance.
    fn empty() -> Self {
        Self::new(B256Set::default())
    }

    /// Retains the given number of elements, returning an iterator over the rest.
    pub fn retain_count(&mut self, count: usize) -> Self {
        let rest_capacity = self.hashes.len().saturating_sub(count);
        if rest_capacity == 0 {
            return Self::empty()
        }
        let mut rest = Self::with_capacity(rest_capacity);

        let mut i = 0;
        self.hashes.retain(|hash| {
            if i >= count {
                rest.insert(*hash);
                return false
            }
            i += 1;

            true
        });

        rest
    }
}

impl FromIterator<(TxHash, Eth68TxMetadata)> for RequestTxHashes {
    fn from_iter<I: IntoIterator<Item = (TxHash, Eth68TxMetadata)>>(iter: I) -> Self {
        Self::new(iter.into_iter().map(|(hash, _)| hash).collect())
    }
}

/// The earliest block, the latest block and hash of the latest block which can be provided.
/// See [BlockRangeUpdate](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#blockrangeupdate-0x11).
#[derive(Clone, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct BlockRangeUpdate {
    /// The earliest block which is available.
    pub earliest: u64,
    /// The latest block which is available.
    pub latest: u64,
    /// Latest available block's hash.
    pub latest_hash: B256,
}

impl InMemorySize for NewPooledTransactionHashes {
    fn size(&self) -> usize {
        match self {
            Self::Eth66(msg) => msg.0.len() * core::mem::size_of::<B256>(),
            Self::Eth68(msg) => {
                msg.types.len() * core::mem::size_of::<u8>() +
                    msg.sizes.len() * core::mem::size_of::<usize>() +
                    msg.hashes.len() * core::mem::size_of::<B256>()
            }
            Self::Eth72(msg) => {
                msg.types.len() * core::mem::size_of::<u8>() +
                    msg.sizes.len() * core::mem::size_of::<usize>() +
                    msg.hashes.len() * core::mem::size_of::<B256>() +
                    core::mem::size_of::<B128>()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::TxHashRef, Typed2718};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{b256, hex, Signature, U256};
    use reth_ethereum_primitives::{Transaction, TransactionSigned};
    use std::str::FromStr;

    /// Takes as input a struct / encoded hex message pair, ensuring that we encode to the exact hex
    /// message, and decode to the exact struct.
    fn test_encoding_vector<T: Encodable + Decodable + PartialEq + core::fmt::Debug>(
        input: (T, &[u8]),
    ) {
        let (expected_decoded, expected_encoded) = input;
        let mut encoded = Vec::new();
        expected_decoded.encode(&mut encoded);

        assert_eq!(hex::encode(&encoded), hex::encode(expected_encoded));

        let decoded = T::decode(&mut encoded.as_ref()).unwrap();
        assert_eq!(expected_decoded, decoded);
    }

    #[test]
    fn can_return_latest_block() {
        let mut blocks = NewBlockHashes(vec![BlockHashNumber { hash: B256::random(), number: 0 }]);
        let latest = blocks.latest().unwrap();
        assert_eq!(latest.number, 0);

        blocks.push(BlockHashNumber { hash: B256::random(), number: 100 });
        blocks.push(BlockHashNumber { hash: B256::random(), number: 2 });
        let latest = blocks.latest().unwrap();
        assert_eq!(latest.number, 100);
    }

    #[test]
    fn eth_66_tx_hash_decode_fast_path() {
        let expected = NewPooledTransactionHashes66(vec![
            b256!("0x0000000000000000000000000000000000000000000000000000000000000001"),
            b256!("0x0000000000000000000000000000000000000000000000000000000000000002"),
            b256!("0x0000000000000000000000000000000000000000000000000000000000000003"),
            b256!("0x0000000000000000000000000000000000000000000000000000000000000004"),
        ]);
        let mut encoded = Vec::new();
        expected.encode(&mut encoded);
        encoded.push(alloy_rlp::EMPTY_STRING_CODE);

        let mut input = encoded.as_ref();
        let decoded = NewPooledTransactionHashes66::decode(&mut input).unwrap();

        assert_eq!(decoded, expected);
        assert_eq!(input, &[alloy_rlp::EMPTY_STRING_CODE]);
    }

    #[test]
    fn eth_66_tx_hash_decode_rejects_non_canonical_hash() {
        let mut encoded = vec![0xe2, 0xb8, 0x20];
        encoded.extend_from_slice(&[0x42; 32]);

        let result = NewPooledTransactionHashes66::decode(&mut encoded.as_ref());

        assert!(matches!(result, Err(alloy_rlp::Error::NonCanonicalSize)));
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
                    hashes: vec![
                        B256::from_str(
                            "0x0000000000000000000000000000000000000000000000000000000000000000",
                        )
                        .unwrap(),
                    ],
                },
                &hex!(
                    "e500c180e1a00000000000000000000000000000000000000000000000000000000000000000"
                )[..],
            ),
            (
                NewPooledTransactionHashes68 {
                    types: vec![0x00, 0x00],
                    sizes: vec![0x00, 0x00],
                    hashes: vec![
                        B256::from_str(
                            "0x0000000000000000000000000000000000000000000000000000000000000000",
                        )
                        .unwrap(),
                        B256::from_str(
                            "0x0000000000000000000000000000000000000000000000000000000000000000",
                        )
                        .unwrap(),
                    ],
                },
                &hex!(
                    "f84a820000c28080f842a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000"
                )[..],
            ),
            (
                NewPooledTransactionHashes68 {
                    types: vec![0x02],
                    sizes: vec![0xb6],
                    hashes: vec![
                        B256::from_str(
                            "0xfecbed04c7b88d8e7221a0a3f5dc33f220212347fc167459ea5cc9c3eb4c1124",
                        )
                        .unwrap(),
                    ],
                },
                &hex!(
                    "e602c281b6e1a0fecbed04c7b88d8e7221a0a3f5dc33f220212347fc167459ea5cc9c3eb4c1124"
                )[..],
            ),
            (
                NewPooledTransactionHashes68 {
                    types: vec![0xff, 0xff],
                    sizes: vec![0xffffffff, 0xffffffff],
                    hashes: vec![
                        B256::from_str(
                            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                        )
                        .unwrap(),
                        B256::from_str(
                            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                        )
                        .unwrap(),
                    ],
                },
                &hex!(
                    "f85282ffffca84ffffffff84fffffffff842a0ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa0ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                )[..],
            ),
            (
                NewPooledTransactionHashes68 {
                    types: vec![0xff, 0xff],
                    sizes: vec![0xffffffff, 0xffffffff],
                    hashes: vec![
                        B256::from_str(
                            "0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafe",
                        )
                        .unwrap(),
                        B256::from_str(
                            "0xbeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafe",
                        )
                        .unwrap(),
                    ],
                },
                &hex!(
                    "f85282ffffca84ffffffff84fffffffff842a0beefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafea0beefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafe"
                )[..],
            ),
            (
                NewPooledTransactionHashes68 {
                    types: vec![0x10, 0x10],
                    sizes: vec![0xdeadc0de, 0xdeadc0de],
                    hashes: vec![
                        B256::from_str(
                            "0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2",
                        )
                        .unwrap(),
                        B256::from_str(
                            "0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2",
                        )
                        .unwrap(),
                    ],
                },
                &hex!(
                    "f852821010ca84deadc0de84deadc0def842a03b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2a03b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2"
                )[..],
            ),
            (
                NewPooledTransactionHashes68 {
                    types: vec![0x6f, 0x6f],
                    sizes: vec![0x7fffffff, 0x7fffffff],
                    hashes: vec![
                        B256::from_str(
                            "0x0000000000000000000000000000000000000000000000000000000000000002",
                        )
                        .unwrap(),
                        B256::from_str(
                            "0x0000000000000000000000000000000000000000000000000000000000000002",
                        )
                        .unwrap(),
                    ],
                },
                &hex!(
                    "f852826f6fca847fffffff847ffffffff842a00000000000000000000000000000000000000000000000000000000000000002a00000000000000000000000000000000000000000000000000000000000000002"
                )[..],
            ),
        ];

        for vector in vectors {
            test_encoding_vector(vector);
        }
    }

    #[test]
    fn eth_68_tx_hash_decode_rejects_non_canonical_size_metadata() {
        let mut single_byte = vec![0xe6, 0x02, 0xc2, 0x81, 0x7f, 0xe1, 0xa0];
        single_byte.extend_from_slice(&[0x42; 32]);

        let result = NewPooledTransactionHashes68::decode(&mut single_byte.as_ref());
        assert_eq!(result.unwrap_err(), alloy_rlp::Error::NonCanonicalSingleByte);

        let mut leading_zero = vec![0xe7, 0x02, 0xc3, 0x82, 0x00, 0x80, 0xe1, 0xa0];
        leading_zero.extend_from_slice(&[0x42; 32]);

        let result = NewPooledTransactionHashes68::decode(&mut leading_zero.as_ref());
        assert_eq!(result.unwrap_err(), alloy_rlp::Error::LeadingZero);
    }

    #[test]
    fn eth_68_tx_hash_decode_single_byte_size_metadata() {
        let expected = NewPooledTransactionHashes68 {
            types: vec![0x02, 0x02],
            sizes: vec![0, 0x7f],
            hashes: vec![
                b256!("0x0000000000000000000000000000000000000000000000000000000000000001"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000002"),
            ],
        };
        let mut encoded = Vec::new();
        expected.encode(&mut encoded);

        let mut input = encoded.as_ref();
        let decoded = NewPooledTransactionHashes68::decode(&mut input).unwrap();

        assert_eq!(decoded, expected);
        assert!(input.is_empty());
    }

    #[test]
    fn eth_72_tx_hash_roundtrip() {
        let vectors = vec![
            (
                NewPooledTransactionHashes72 {
                    types: vec![],
                    sizes: vec![],
                    hashes: vec![],
                    cell_mask: None,
                },
                &hex!("c480c0c080")[..],
            ),
            (
                NewPooledTransactionHashes72 {
                    types: vec![],
                    sizes: vec![],
                    hashes: vec![],
                    cell_mask: Some(B128::repeat_byte(0x11)),
                },
                &hex!("d480c0c09011111111111111111111111111111111")[..],
            ),
        ];

        for vector in vectors {
            test_encoding_vector(vector);
        }
    }

    #[test]
    fn eth_72_rejects_missing_cell_mask() {
        let encoded_eth68_payload = hex!("c380c0c0");

        let result = NewPooledTransactionHashes72::decode(&mut encoded_eth68_payload.as_ref());

        assert!(matches!(result, Err(alloy_rlp::Error::InputTooShort)));
    }

    #[test]
    fn request_hashes_retain_count_keep_subset() {
        let mut hashes = RequestTxHashes::new(
            [
                b256!("0x0000000000000000000000000000000000000000000000000000000000000001"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000002"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000003"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000004"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000005"),
            ]
            .into_iter()
            .collect::<B256Set>(),
        );

        let rest = hashes.retain_count(3);

        assert_eq!(3, hashes.len());
        assert_eq!(2, rest.len());
    }

    #[test]
    fn request_hashes_retain_count_keep_all() {
        let mut hashes = RequestTxHashes::new(
            [
                b256!("0x0000000000000000000000000000000000000000000000000000000000000001"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000002"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000003"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000004"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000005"),
            ]
            .into_iter()
            .collect::<B256Set>(),
        );

        let _ = hashes.retain_count(6);

        assert_eq!(5, hashes.len());
    }

    #[test]
    fn split_request_hashes_keep_none() {
        let mut hashes = RequestTxHashes::new(
            [
                b256!("0x0000000000000000000000000000000000000000000000000000000000000001"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000002"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000003"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000004"),
                b256!("0x0000000000000000000000000000000000000000000000000000000000000005"),
            ]
            .into_iter()
            .collect::<B256Set>(),
        );

        let rest = hashes.retain_count(0);

        assert_eq!(0, hashes.len());
        assert_eq!(5, rest.len());
    }

    fn signed_transaction() -> impl SignedTransaction {
        TransactionSigned::new_unhashed(
            Transaction::Legacy(Default::default()),
            Signature::new(
                U256::from_str(
                    "0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12",
                )
                .unwrap(),
                U256::from_str(
                    "0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10",
                )
                .unwrap(),
                false,
            ),
        )
    }

    #[test]
    fn test_pooled_tx_hashes_68_push() {
        let tx = signed_transaction();
        let mut tx_hashes =
            NewPooledTransactionHashes68 { types: vec![], sizes: vec![], hashes: vec![] };
        tx_hashes.push(&tx);
        assert_eq!(tx_hashes.types.len(), 1);
        assert_eq!(tx_hashes.sizes.len(), 1);
        assert_eq!(tx_hashes.hashes.len(), 1);
        assert_eq!(tx_hashes.types[0], tx.ty());
        assert_eq!(tx_hashes.sizes[0], tx.encode_2718_len());
        assert_eq!(tx_hashes.hashes[0], *tx.tx_hash());
    }

    #[test]
    fn test_pooled_tx_hashes_68_extend() {
        let tx = signed_transaction();
        let txs = vec![tx.clone(), tx.clone()];
        let mut tx_hashes =
            NewPooledTransactionHashes68 { types: vec![], sizes: vec![], hashes: vec![] };
        tx_hashes.extend(&txs);
        assert_eq!(tx_hashes.types.len(), 2);
        assert_eq!(tx_hashes.sizes.len(), 2);
        assert_eq!(tx_hashes.hashes.len(), 2);
        assert_eq!(tx_hashes.types[0], tx.ty());
        assert_eq!(tx_hashes.sizes[0], tx.encode_2718_len());
        assert_eq!(tx_hashes.hashes[0], *tx.tx_hash());
        assert_eq!(tx_hashes.types[1], tx.ty());
        assert_eq!(tx_hashes.sizes[1], tx.encode_2718_len());
        assert_eq!(tx_hashes.hashes[1], *tx.tx_hash());
    }

    #[test]
    fn test_pooled_tx_hashes_68_with_transaction() {
        let tx = signed_transaction();
        let tx_hashes =
            NewPooledTransactionHashes68 { types: vec![], sizes: vec![], hashes: vec![] }
                .with_transaction(&tx);
        assert_eq!(tx_hashes.types.len(), 1);
        assert_eq!(tx_hashes.sizes.len(), 1);
        assert_eq!(tx_hashes.hashes.len(), 1);
        assert_eq!(tx_hashes.types[0], tx.ty());
        assert_eq!(tx_hashes.sizes[0], tx.encode_2718_len());
        assert_eq!(tx_hashes.hashes[0], *tx.tx_hash());
    }

    #[test]
    fn test_pooled_tx_hashes_68_with_transactions() {
        let tx = signed_transaction();
        let txs = vec![tx.clone(), tx.clone()];
        let tx_hashes =
            NewPooledTransactionHashes68 { types: vec![], sizes: vec![], hashes: vec![] }
                .with_transactions(&txs);
        assert_eq!(tx_hashes.types.len(), 2);
        assert_eq!(tx_hashes.sizes.len(), 2);
        assert_eq!(tx_hashes.hashes.len(), 2);
        assert_eq!(tx_hashes.types[0], tx.ty());
        assert_eq!(tx_hashes.sizes[0], tx.encode_2718_len());
        assert_eq!(tx_hashes.hashes[0], *tx.tx_hash());
        assert_eq!(tx_hashes.types[1], tx.ty());
        assert_eq!(tx_hashes.sizes[1], tx.encode_2718_len());
        assert_eq!(tx_hashes.hashes[1], *tx.tx_hash());
    }
}
