//! Types for broadcasting new data.

use crate::{EthMessage, EthVersion};
use alloy_rlp::{
    Decodable, Encodable, RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper,
};

use derive_more::{Constructor, Deref, DerefMut, From, IntoIterator};
use reth_codecs::derive_arbitrary;
use reth_primitives::{
    Block, Bytes, PooledTransactionsElement, TransactionSigned, TxHash, B256, U128,
};

use std::{
    collections::{HashMap, HashSet},
    mem,
    sync::Arc,
};

#[cfg(feature = "arbitrary")]
use proptest::{collection::vec, prelude::*};

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
    pub hash: B256,
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
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive_arbitrary(rlp, 25)]
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

impl Transactions {
    /// Returns `true` if the list of transactions contains any blob transactions.
    pub fn has_eip4844(&self) -> bool {
        self.0.iter().any(|tx| tx.is_eip4844())
    }
}

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
    /// Returns the message [`EthVersion`].
    pub fn version(&self) -> EthVersion {
        match self {
            NewPooledTransactionHashes::Eth66(_) => EthVersion::Eth66,
            NewPooledTransactionHashes::Eth68(_) => EthVersion::Eth68,
        }
    }

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
    pub fn iter_hashes(&self) -> impl Iterator<Item = &B256> + '_ {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0.iter(),
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes.iter(),
        }
    }

    /// Returns an immutable reference to transaction hashes.
    pub fn hashes(&self) -> &Vec<B256> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => &msg.0,
            NewPooledTransactionHashes::Eth68(msg) => &msg.hashes,
        }
    }

    /// Returns a mutable reference to transaction hashes.
    pub fn hashes_mut(&mut self) -> &mut Vec<B256> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => &mut msg.0,
            NewPooledTransactionHashes::Eth68(msg) => &mut msg.hashes,
        }
    }

    /// Consumes the type and returns all hashes
    pub fn into_hashes(self) -> Vec<B256> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0,
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes,
        }
    }

    /// Returns an iterator over all transaction hashes.
    pub fn into_iter_hashes(self) -> impl Iterator<Item = B256> {
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

    /// Returns an immutable reference to the inner type if this an eth68 announcement.
    pub fn as_eth68(&self) -> Option<&NewPooledTransactionHashes68> {
        match self {
            NewPooledTransactionHashes::Eth66(_) => None,
            NewPooledTransactionHashes::Eth68(msg) => Some(msg),
        }
    }

    /// Returns a mutable reference to the inner type if this an eth68 announcement.
    pub fn as_eth68_mut(&mut self) -> Option<&mut NewPooledTransactionHashes68> {
        match self {
            NewPooledTransactionHashes::Eth66(_) => None,
            NewPooledTransactionHashes::Eth68(msg) => Some(msg),
        }
    }

    /// Returns a mutable reference to the inner type if this an eth66 announcement.
    pub fn as_eth66_mut(&mut self) -> Option<&mut NewPooledTransactionHashes66> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => Some(msg),
            NewPooledTransactionHashes::Eth68(_) => None,
        }
    }

    /// Returns the inner type if this an eth68 announcement.
    pub fn take_eth68(&mut self) -> Option<NewPooledTransactionHashes68> {
        match self {
            NewPooledTransactionHashes::Eth66(_) => None,
            NewPooledTransactionHashes::Eth68(msg) => Some(mem::take(msg)),
        }
    }

    /// Returns the inner type if this an eth66 announcement.
    pub fn take_eth66(&mut self) -> Option<NewPooledTransactionHashes66> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => Some(mem::take(msg)),
            NewPooledTransactionHashes::Eth68(_) => None,
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
    pub Vec<B256>,
);

impl From<Vec<B256>> for NewPooledTransactionHashes66 {
    fn from(v: Vec<B256>) -> Self {
        NewPooledTransactionHashes66(v)
    }
}

/// Same as [`NewPooledTransactionHashes66`] but extends that that beside the transaction hashes,
/// the node sends the transaction types and their sizes (as defined in EIP-2718) as well.
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
impl Arbitrary for NewPooledTransactionHashes68 {
    type Parameters = ();
    fn arbitrary_with(_args: ()) -> Self::Strategy {
        // Generate a single random length for all vectors
        let vec_length = any::<usize>().prop_map(|x| x % 100 + 1); // Lengths between 1 and 100

        vec_length
            .prop_flat_map(|len| {
                // Use the generated length to create vectors of TxType, usize, and B256
                let types_vec =
                    vec(any::<reth_primitives::TxType>().prop_map(|ty| ty as u8), len..=len);

                // Map the usize values to the range 0..131072(0x20000)
                let sizes_vec = vec(proptest::num::usize::ANY.prop_map(|x| x % 131072), len..=len);
                let hashes_vec = vec(any::<B256>(), len..=len);

                (types_vec, sizes_vec, hashes_vec)
            })
            .prop_map(|(types, sizes, hashes)| NewPooledTransactionHashes68 {
                types,
                sizes,
                hashes,
            })
            .boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}

impl NewPooledTransactionHashes68 {
    /// Returns an iterator over tx hashes zipped with corresponding metadata.
    pub fn metadata_iter(&self) -> impl Iterator<Item = (&B256, (u8, usize))> {
        self.hashes.iter().zip(self.types.iter().copied().zip(self.sizes.iter().copied()))
    }
}

impl Encodable for NewPooledTransactionHashes68 {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        #[derive(RlpEncodable)]
        struct EncodableNewPooledTransactionHashes68<'a> {
            types: &'a [u8],
            sizes: &'a Vec<usize>,
            hashes: &'a Vec<B256>,
        }

        let encodable = EncodableNewPooledTransactionHashes68 {
            types: &self.types[..],
            sizes: &self.sizes,
            hashes: &self.hashes,
        };

        encodable.encode(out);
    }
    fn length(&self) -> usize {
        #[derive(RlpEncodable)]
        struct EncodableNewPooledTransactionHashes68<'a> {
            types: &'a [u8],
            sizes: &'a Vec<usize>,
            hashes: &'a Vec<B256>,
        }

        let encodable = EncodableNewPooledTransactionHashes68 {
            types: &self.types[..],
            sizes: &self.sizes,
            hashes: &self.hashes,
        };

        encodable.length()
    }
}

impl Decodable for NewPooledTransactionHashes68 {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        #[derive(RlpDecodable)]
        struct EncodableNewPooledTransactionHashes68 {
            types: Bytes,
            sizes: Vec<usize>,
            hashes: Vec<B256>,
        }

        let encodable = EncodableNewPooledTransactionHashes68::decode(buf)?;
        let msg = Self {
            types: encodable.types.into(),
            sizes: encodable.sizes,
            hashes: encodable.hashes,
        };

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
            NewPooledTransactionHashes::Eth66(msg) => msg.dedup(),
            NewPooledTransactionHashes::Eth68(msg) => msg.dedup(),
        }
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

        let mut deduped_data = HashMap::with_capacity(hashes.len());

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

        let mut deduped_data = HashMap::with_capacity(hashes.len());

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

impl HandleMempoolData for Vec<PooledTransactionsElement> {
    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn len(&self) -> usize {
        self.len()
    }

    fn retain_by_hash(&mut self, mut f: impl FnMut(&TxHash) -> bool) {
        self.retain(|tx| f(tx.hash()))
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
    data: HashMap<TxHash, V>,
    version: Option<EthVersion>,
}

handle_mempool_data_map_impl!(PartiallyValidData<V>, <V>);

impl<V> PartiallyValidData<V> {
    /// Wraps raw data.
    pub fn from_raw_data(data: HashMap<TxHash, V>, version: Option<EthVersion>) -> Self {
        Self { data, version }
    }

    /// Wraps raw data with version [`EthVersion::Eth68`].
    pub fn from_raw_data_eth68(data: HashMap<TxHash, V>) -> Self {
        Self::from_raw_data(data, Some(EthVersion::Eth68))
    }

    /// Wraps raw data with version [`EthVersion::Eth66`].
    pub fn from_raw_data_eth66(data: HashMap<TxHash, V>) -> Self {
        Self::from_raw_data(data, Some(EthVersion::Eth66))
    }

    /// Returns a new [`PartiallyValidData`] with empty data from an [`Eth68`](EthVersion::Eth68)
    /// announcement.
    pub fn empty_eth68() -> Self {
        Self::from_raw_data_eth68(HashMap::new())
    }

    /// Returns a new [`PartiallyValidData`] with empty data from an [`Eth66`](EthVersion::Eth66)
    /// announcement.
    pub fn empty_eth66() -> Self {
        Self::from_raw_data_eth66(HashMap::new())
    }

    /// Returns the version of the message this data was received in if different versions of the
    /// message exists, either [`Eth66`](EthVersion::Eth66) or [`Eth68`](EthVersion::Eth68).
    pub fn msg_version(&self) -> Option<EthVersion> {
        self.version
    }

    /// Destructs returning the validated data.
    pub fn into_data(self) -> HashMap<TxHash, V> {
        self.data
    }
}

/// Partially validated data from an announcement or a
/// [`PooledTransactions`](crate::PooledTransactions) response.
#[derive(Debug, Deref, DerefMut, IntoIterator, From)]
#[from(PartiallyValidData<Eth68TxMetadata>)]
pub struct ValidAnnouncementData {
    #[deref]
    #[deref_mut]
    #[into_iterator]
    data: HashMap<TxHash, Eth68TxMetadata>,
    version: EthVersion,
}

handle_mempool_data_map_impl!(ValidAnnouncementData,);

impl ValidAnnouncementData {
    /// Destructs returning only the valid hashes and the announcement message version. Caution! If
    /// this is [`Eth68`](EthVersion::Eth68) announcement data, this drops the metadata.
    pub fn into_request_hashes(self) -> (RequestTxHashes, EthVersion) {
        let hashes = self.data.into_keys().collect::<HashSet<_>>();

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
    pub fn into_data(self) -> HashMap<TxHash, Eth68TxMetadata> {
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
    hashes: HashSet<TxHash>,
}

impl RequestTxHashes {
    /// Returns a new [`RequestTxHashes`] with given capacity for hashes. Caution! Make sure to
    /// call [`HashSet::shrink_to_fit`] on [`RequestTxHashes`] when full, especially where it will
    /// be stored in its entirety like in the future waiting for a
    /// [`GetPooledTransactions`](crate::GetPooledTransactions) request to resolve.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(HashSet::with_capacity(capacity))
    }

    /// Returns an new empty instance.
    fn empty() -> Self {
        Self::new(HashSet::new())
    }

    /// Retains the given number of elements, returning and iterator over the rest.
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
        RequestTxHashes::new(iter.into_iter().map(|(hash, _)| hash).collect::<HashSet<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{b256, hex};
    use std::str::FromStr;

    /// Takes as input a struct / encoded hex message pair, ensuring that we encode to the exact hex
    /// message, and decode to the exact struct.
    fn test_encoding_vector<T: Encodable + Decodable + PartialEq + std::fmt::Debug>(
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

        blocks.0.push(BlockHashNumber { hash: B256::random(), number: 100 });
        blocks.0.push(BlockHashNumber { hash: B256::random(), number: 2 });
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
                hashes: vec![B256::from_str(
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
            &hex!("f84a820000c28080f842a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000")[..],
            ),
            (
            NewPooledTransactionHashes68 {
                types: vec![0x02],
                sizes: vec![0xb6],
                hashes: vec![B256::from_str(
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
            &hex!("f85282ffffca84ffffffff84fffffffff842a0ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa0ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")[..],
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
            &hex!("f85282ffffca84ffffffff84fffffffff842a0beefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafea0beefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafe")[..],
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
            &hex!("f852821010ca84deadc0de84deadc0def842a03b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2a03b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e4d4e2")[..],
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
            &hex!("f852826f6fca847fffffff847ffffffff842a00000000000000000000000000000000000000000000000000000000000000002a00000000000000000000000000000000000000000000000000000000000000002")[..],
            ),
        ];

        for vector in vectors {
            test_encoding_vector(vector);
        }
    }

    #[test]
    fn request_hashes_retain_count_keep_subset() {
        let mut hashes = RequestTxHashes::new(
            [
                b256!("0000000000000000000000000000000000000000000000000000000000000001"),
                b256!("0000000000000000000000000000000000000000000000000000000000000002"),
                b256!("0000000000000000000000000000000000000000000000000000000000000003"),
                b256!("0000000000000000000000000000000000000000000000000000000000000004"),
                b256!("0000000000000000000000000000000000000000000000000000000000000005"),
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
        );

        let rest = hashes.retain_count(3);

        assert_eq!(3, hashes.len());
        assert_eq!(2, rest.len());
    }

    #[test]
    fn request_hashes_retain_count_keep_all() {
        let mut hashes = RequestTxHashes::new(
            [
                b256!("0000000000000000000000000000000000000000000000000000000000000001"),
                b256!("0000000000000000000000000000000000000000000000000000000000000002"),
                b256!("0000000000000000000000000000000000000000000000000000000000000003"),
                b256!("0000000000000000000000000000000000000000000000000000000000000004"),
                b256!("0000000000000000000000000000000000000000000000000000000000000005"),
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
        );

        let _ = hashes.retain_count(6);

        assert_eq!(5, hashes.len());
    }

    #[test]
    fn split_request_hashes_keep_none() {
        let mut hashes = RequestTxHashes::new(
            [
                b256!("0000000000000000000000000000000000000000000000000000000000000001"),
                b256!("0000000000000000000000000000000000000000000000000000000000000002"),
                b256!("0000000000000000000000000000000000000000000000000000000000000003"),
                b256!("0000000000000000000000000000000000000000000000000000000000000004"),
                b256!("0000000000000000000000000000000000000000000000000000000000000005"),
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
        );

        let rest = hashes.retain_count(0);

        assert_eq!(0, hashes.len());
        assert_eq!(5, rest.len());
    }
}
