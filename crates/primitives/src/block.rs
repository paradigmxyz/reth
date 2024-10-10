use crate::{
    GotExpected, Header, SealedHeader, TransactionSigned, TransactionSignedEcRecovered, Withdrawals,
};
use alloc::vec::Vec;
pub use alloy_eips::eip1898::{
    BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag, ForkBlock, RpcBlockHash,
};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Sealable, B256};
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use derive_more::{Deref, DerefMut};
#[cfg(any(test, feature = "arbitrary"))]
use proptest::prelude::prop_compose;
#[cfg(any(test, feature = "arbitrary"))]
pub use reth_primitives_traits::test_utils::{generate_valid_header, valid_header_strategy};
use reth_primitives_traits::Requests;
use serde::{Deserialize, Serialize};

// HACK(onbjerg): we need this to always set `requests` to `None` since we might otherwise generate
// a block with `None` withdrawals and `Some` requests, in which case we end up trying to decode the
// requests as withdrawals
#[cfg(any(feature = "arbitrary", test))]
prop_compose! {
    pub fn empty_requests_strategy()(_ in 0..1) -> Option<Requests> {
        None
    }
}

/// Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp, 25))]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Deref)]
pub struct Block {
    /// Block header.
    #[deref]
    pub header: Header,
    /// Block body.
    pub body: BlockBody,
}

impl Block {
    /// Calculate the header hash and seal the block so that it can't be changed.
    pub fn seal_slow(self) -> SealedBlock {
        let sealed = self.header.seal_slow();
        let (header, seal) = sealed.into_parts();
        SealedBlock { header: SealedHeader::new(header, seal), body: self.body }
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    pub fn seal(self, hash: B256) -> SealedBlock {
        SealedBlock { header: SealedHeader::new(self.header, hash), body: self.body }
    }

    /// Expensive operation that recovers transaction signer. See [`SealedBlockWithSenders`].
    pub fn senders(&self) -> Option<Vec<Address>> {
        self.body.recover_signers()
    }

    /// Transform into a [`BlockWithSenders`].
    ///
    /// # Panics
    ///
    /// If the number of senders does not match the number of transactions in the block
    /// and the signer recovery for one of the transactions fails.
    ///
    /// Note: this is expected to be called with blocks read from disk.
    #[track_caller]
    pub fn with_senders_unchecked(self, senders: Vec<Address>) -> BlockWithSenders {
        self.try_with_senders_unchecked(senders).expect("stored block is valid")
    }

    /// Transform into a [`BlockWithSenders`] using the given senders.
    ///
    /// If the number of senders does not match the number of transactions in the block, this falls
    /// back to manually recovery, but _without ensuring that the signature has a low `s` value_.
    /// See also [`TransactionSigned::recover_signer_unchecked`]
    ///
    /// Returns an error if a signature is invalid.
    #[track_caller]
    pub fn try_with_senders_unchecked(
        self,
        senders: Vec<Address>,
    ) -> Result<BlockWithSenders, Self> {
        let senders = if self.body.transactions.len() == senders.len() {
            senders
        } else {
            let Some(senders) = self.body.recover_signers() else { return Err(self) };
            senders
        };

        Ok(BlockWithSenders { block: self, senders })
    }

    /// **Expensive**. Transform into a [`BlockWithSenders`] by recovering senders in the contained
    /// transactions.
    ///
    /// Returns `None` if a transaction is invalid.
    pub fn with_recovered_senders(self) -> Option<BlockWithSenders> {
        let senders = self.senders()?;
        Some(BlockWithSenders { block: self, senders })
    }

    /// Calculates a heuristic for the in-memory size of the [`Block`].
    #[inline]
    pub fn size(&self) -> usize {
        self.header.size() + self.body.size()
    }
}

/// We need to implement RLP traits manually because we currently don't have a way to flatten
/// [`BlockBody`] into [`Block`].
mod block_rlp {
    use super::*;

    #[derive(RlpDecodable)]
    #[rlp(trailing)]
    struct Helper<H> {
        header: H,
        transactions: Vec<TransactionSigned>,
        ommers: Vec<Header>,
        withdrawals: Option<Withdrawals>,
        requests: Option<Requests>,
    }

    #[derive(RlpEncodable)]
    #[rlp(trailing)]
    struct HelperRef<'a, H> {
        header: &'a H,
        transactions: &'a Vec<TransactionSigned>,
        ommers: &'a Vec<Header>,
        withdrawals: Option<&'a Withdrawals>,
        requests: Option<&'a Requests>,
    }

    impl<'a> From<&'a Block> for HelperRef<'a, Header> {
        fn from(block: &'a Block) -> Self {
            let Block { header, body: BlockBody { transactions, ommers, withdrawals, requests } } =
                block;
            Self {
                header,
                transactions,
                ommers,
                withdrawals: withdrawals.as_ref(),
                requests: requests.as_ref(),
            }
        }
    }

    impl<'a> From<&'a SealedBlock> for HelperRef<'a, SealedHeader> {
        fn from(block: &'a SealedBlock) -> Self {
            let SealedBlock {
                header,
                body: BlockBody { transactions, ommers, withdrawals, requests },
            } = block;
            Self {
                header,
                transactions,
                ommers,
                withdrawals: withdrawals.as_ref(),
                requests: requests.as_ref(),
            }
        }
    }

    impl Decodable for Block {
        fn decode(b: &mut &[u8]) -> alloy_rlp::Result<Self> {
            let Helper { header, transactions, ommers, withdrawals, requests } = Helper::decode(b)?;
            Ok(Self { header, body: BlockBody { transactions, ommers, withdrawals, requests } })
        }
    }

    impl Decodable for SealedBlock {
        fn decode(b: &mut &[u8]) -> alloy_rlp::Result<Self> {
            let Helper { header, transactions, ommers, withdrawals, requests } = Helper::decode(b)?;
            Ok(Self { header, body: BlockBody { transactions, ommers, withdrawals, requests } })
        }
    }

    impl Encodable for Block {
        fn length(&self) -> usize {
            let helper: HelperRef<'_, _> = self.into();
            helper.length()
        }

        fn encode(&self, out: &mut dyn bytes::BufMut) {
            let helper: HelperRef<'_, _> = self.into();
            helper.encode(out)
        }
    }

    impl Encodable for SealedBlock {
        fn length(&self) -> usize {
            let helper: HelperRef<'_, _> = self.into();
            helper.length()
        }

        fn encode(&self, out: &mut dyn bytes::BufMut) {
            let helper: HelperRef<'_, _> = self.into();
            helper.encode(out)
        }
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for Block {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        // first generate up to 100 txs
        let transactions = (0..100)
            .map(|_| TransactionSigned::arbitrary(u))
            .collect::<arbitrary::Result<Vec<_>>>()?;

        // then generate up to 2 ommers
        let ommers = (0..2).map(|_| Header::arbitrary(u)).collect::<arbitrary::Result<Vec<_>>>()?;

        Ok(Self {
            header: u.arbitrary()?,
            body: BlockBody {
                transactions,
                ommers,
                // for now just generate empty requests, see HACK above
                requests: u.arbitrary()?,
                withdrawals: u.arbitrary()?,
            },
        })
    }
}

/// Sealed block with senders recovered from transactions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deref, DerefMut)]
pub struct BlockWithSenders {
    /// Block
    #[deref]
    #[deref_mut]
    pub block: Block,
    /// List of senders that match the transactions in the block
    pub senders: Vec<Address>,
}

impl BlockWithSenders {
    /// New block with senders. Return none if len of tx and senders does not match
    pub fn new(block: Block, senders: Vec<Address>) -> Option<Self> {
        (block.body.transactions.len() == senders.len()).then_some(Self { block, senders })
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    #[inline]
    pub fn seal(self, hash: B256) -> SealedBlockWithSenders {
        let Self { block, senders } = self;
        SealedBlockWithSenders { block: block.seal(hash), senders }
    }

    /// Calculate the header hash and seal the block with senders so that it can't be changed.
    #[inline]
    pub fn seal_slow(self) -> SealedBlockWithSenders {
        SealedBlockWithSenders { block: self.block.seal_slow(), senders: self.senders }
    }

    /// Split Structure to its components
    #[inline]
    pub fn into_components(self) -> (Block, Vec<Address>) {
        (self.block, self.senders)
    }

    /// Returns an iterator over all transactions and their sender.
    #[inline]
    pub fn transactions_with_sender(
        &self,
    ) -> impl Iterator<Item = (&Address, &TransactionSigned)> + '_ {
        self.senders.iter().zip(self.block.body.transactions())
    }

    /// Returns an iterator over all transactions in the chain.
    #[inline]
    pub fn into_transactions_ecrecovered(
        self,
    ) -> impl Iterator<Item = TransactionSignedEcRecovered> {
        self.block
            .body
            .transactions
            .into_iter()
            .zip(self.senders)
            .map(|(tx, sender)| tx.with_signer(sender))
    }

    /// Consumes the block and returns the transactions of the block.
    #[inline]
    pub fn into_transactions(self) -> Vec<TransactionSigned> {
        self.block.body.transactions
    }
}

/// Sealed Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp, 32))]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Deref, DerefMut)]
pub struct SealedBlock {
    /// Locked block header.
    #[deref]
    #[deref_mut]
    pub header: SealedHeader,
    /// Block body.
    pub body: BlockBody,
}

impl SealedBlock {
    /// Create a new sealed block instance using the sealed header and block body.
    #[inline]
    pub const fn new(header: SealedHeader, body: BlockBody) -> Self {
        Self { header, body }
    }

    /// Header hash.
    #[inline]
    pub const fn hash(&self) -> B256 {
        self.header.hash()
    }

    /// Splits the sealed block into underlying components
    #[inline]
    pub fn split(self) -> (SealedHeader, Vec<TransactionSigned>, Vec<Header>) {
        (self.header, self.body.transactions, self.body.ommers)
    }

    /// Splits the [`BlockBody`] and [`SealedHeader`] into separate components
    #[inline]
    pub fn split_header_body(self) -> (SealedHeader, BlockBody) {
        (self.header, self.body)
    }

    /// Returns an iterator over all blob transactions of the block
    #[inline]
    pub fn blob_transactions_iter(&self) -> impl Iterator<Item = &TransactionSigned> + '_ {
        self.body.blob_transactions_iter()
    }

    /// Returns only the blob transactions, if any, from the block body.
    #[inline]
    pub fn blob_transactions(&self) -> Vec<&TransactionSigned> {
        self.blob_transactions_iter().collect()
    }

    /// Returns an iterator over all blob versioned hashes from the block body.
    #[inline]
    pub fn blob_versioned_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.blob_transactions_iter()
            .filter_map(|tx| tx.as_eip4844().map(|blob_tx| &blob_tx.blob_versioned_hashes))
            .flatten()
    }

    /// Returns all blob versioned hashes from the block body.
    #[inline]
    pub fn blob_versioned_hashes(&self) -> Vec<&B256> {
        self.blob_versioned_hashes_iter().collect()
    }

    /// Expensive operation that recovers transaction signer. See [`SealedBlockWithSenders`].
    pub fn senders(&self) -> Option<Vec<Address>> {
        self.body.recover_signers()
    }

    /// Seal sealed block with recovered transaction senders.
    pub fn seal_with_senders(self) -> Option<SealedBlockWithSenders> {
        self.try_seal_with_senders().ok()
    }

    /// Seal sealed block with recovered transaction senders.
    pub fn try_seal_with_senders(self) -> Result<SealedBlockWithSenders, Self> {
        match self.senders() {
            Some(senders) => Ok(SealedBlockWithSenders { block: self, senders }),
            None => Err(self),
        }
    }

    /// Transform into a [`SealedBlockWithSenders`].
    ///
    /// # Panics
    ///
    /// If the number of senders does not match the number of transactions in the block
    /// and the signer recovery for one of the transactions fails.
    #[track_caller]
    pub fn with_senders_unchecked(self, senders: Vec<Address>) -> SealedBlockWithSenders {
        self.try_with_senders_unchecked(senders).expect("stored block is valid")
    }

    /// Transform into a [`SealedBlockWithSenders`] using the given senders.
    ///
    /// If the number of senders does not match the number of transactions in the block, this falls
    /// back to manually recovery, but _without ensuring that the signature has a low `s` value_.
    /// See also [`TransactionSigned::recover_signer_unchecked`]
    ///
    /// Returns an error if a signature is invalid.
    #[track_caller]
    pub fn try_with_senders_unchecked(
        self,
        senders: Vec<Address>,
    ) -> Result<SealedBlockWithSenders, Self> {
        let senders = if self.body.transactions.len() == senders.len() {
            senders
        } else {
            let Some(senders) = self.body.recover_signers() else { return Err(self) };
            senders
        };

        Ok(SealedBlockWithSenders { block: self, senders })
    }

    /// Unseal the block
    pub fn unseal(self) -> Block {
        Block { header: self.header.unseal(), body: self.body }
    }

    /// Calculates a heuristic for the in-memory size of the [`SealedBlock`].
    #[inline]
    pub fn size(&self) -> usize {
        self.header.size() + self.body.size()
    }

    /// Calculates the total gas used by blob transactions in the sealed block.
    pub fn blob_gas_used(&self) -> u64 {
        self.blob_transactions().iter().filter_map(|tx| tx.blob_gas_used()).sum()
    }

    /// Returns whether or not the block contains any blob transactions.
    #[inline]
    pub fn has_blob_transactions(&self) -> bool {
        self.body.has_blob_transactions()
    }

    /// Returns whether or not the block contains any eip-7702 transactions.
    #[inline]
    pub fn has_eip7702_transactions(&self) -> bool {
        self.body.has_eip7702_transactions()
    }

    /// Ensures that the transaction root in the block header is valid.
    ///
    /// The transaction root is the Keccak 256-bit hash of the root node of the trie structure
    /// populated with each transaction in the transactions list portion of the block.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the calculated transaction root matches the one stored in the header,
    /// indicating that the transactions in the block are correctly represented in the trie.
    ///
    /// Returns `Err(error)` if the transaction root validation fails, providing a `GotExpected`
    /// error containing the calculated and expected roots.
    pub fn ensure_transaction_root_valid(&self) -> Result<(), GotExpected<B256>> {
        let calculated_root = self.body.calculate_tx_root();

        if self.header.transactions_root != calculated_root {
            return Err(GotExpected {
                got: calculated_root,
                expected: self.header.transactions_root,
            })
        }

        Ok(())
    }

    /// Returns a vector of transactions RLP encoded with
    /// [`alloy_eips::eip2718::Encodable2718::encoded_2718`].
    pub fn raw_transactions(&self) -> Vec<Bytes> {
        self.body.transactions().map(|tx| tx.encoded_2718().into()).collect()
    }
}

impl From<SealedBlock> for Block {
    fn from(block: SealedBlock) -> Self {
        block.unseal()
    }
}

/// Sealed block with senders recovered from transactions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Deref, DerefMut)]
pub struct SealedBlockWithSenders {
    /// Sealed block
    #[deref]
    #[deref_mut]
    pub block: SealedBlock,
    /// List of senders that match transactions from block.
    pub senders: Vec<Address>,
}

impl SealedBlockWithSenders {
    /// New sealed block with sender. Return none if len of tx and senders does not match
    pub fn new(block: SealedBlock, senders: Vec<Address>) -> Option<Self> {
        (block.body.transactions.len() == senders.len()).then_some(Self { block, senders })
    }

    /// Split Structure to its components
    #[inline]
    pub fn into_components(self) -> (SealedBlock, Vec<Address>) {
        (self.block, self.senders)
    }

    /// Returns the unsealed [`BlockWithSenders`]
    #[inline]
    pub fn unseal(self) -> BlockWithSenders {
        let Self { block, senders } = self;
        BlockWithSenders { block: block.unseal(), senders }
    }

    /// Returns an iterator over all transactions in the block.
    #[inline]
    pub fn transactions(&self) -> impl Iterator<Item = &TransactionSigned> + '_ {
        self.block.body.transactions()
    }

    /// Returns an iterator over all transactions and their sender.
    #[inline]
    pub fn transactions_with_sender(
        &self,
    ) -> impl Iterator<Item = (&Address, &TransactionSigned)> + '_ {
        self.senders.iter().zip(self.block.body.transactions())
    }

    /// Consumes the block and returns the transactions of the block.
    #[inline]
    pub fn into_transactions(self) -> Vec<TransactionSigned> {
        self.block.body.transactions
    }

    /// Returns an iterator over all transactions in the chain.
    #[inline]
    pub fn into_transactions_ecrecovered(
        self,
    ) -> impl Iterator<Item = TransactionSignedEcRecovered> {
        self.block
            .body
            .transactions
            .into_iter()
            .zip(self.senders)
            .map(|(tx, sender)| tx.with_signer(sender))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for SealedBlockWithSenders {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let block = SealedBlock::arbitrary(u)?;

        let senders = block
            .body
            .transactions
            .iter()
            .map(|tx| tx.recover_signer().unwrap())
            .collect::<Vec<_>>();

        Ok(Self { block, senders })
    }
}

/// A response to `GetBlockBodies`, containing bodies if any bodies were found.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp, 10))]
#[derive(
    Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, RlpEncodable, RlpDecodable,
)]
#[rlp(trailing)]
pub struct BlockBody {
    /// Transactions in the block
    pub transactions: Vec<TransactionSigned>,
    /// Uncle headers for the given block
    pub ommers: Vec<Header>,
    /// Withdrawals in the block.
    pub withdrawals: Option<Withdrawals>,
    /// Requests in the block.
    pub requests: Option<Requests>,
}

impl BlockBody {
    /// Create a [`Block`] from the body and its header.
    pub const fn into_block(self, header: Header) -> Block {
        Block { header, body: self }
    }

    /// Calculate the transaction root for the block body.
    pub fn calculate_tx_root(&self) -> B256 {
        crate::proofs::calculate_transaction_root(&self.transactions)
    }

    /// Calculate the ommers root for the block body.
    pub fn calculate_ommers_root(&self) -> B256 {
        crate::proofs::calculate_ommers_root(&self.ommers)
    }

    /// Calculate the withdrawals root for the block body, if withdrawals exist. If there are no
    /// withdrawals, this will return `None`.
    pub fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.withdrawals.as_ref().map(|w| crate::proofs::calculate_withdrawals_root(w))
    }

    /// Calculate the requests root for the block body, if requests exist. If there are no
    /// requests, this will return `None`.
    pub fn calculate_requests_root(&self) -> Option<B256> {
        self.requests.as_ref().map(|r| crate::proofs::calculate_requests_root(&r.0))
    }

    /// Recover signer addresses for all transactions in the block body.
    pub fn recover_signers(&self) -> Option<Vec<Address>> {
        TransactionSigned::recover_signers(&self.transactions, self.transactions.len())
    }

    /// Returns whether or not the block body contains any blob transactions.
    #[inline]
    pub fn has_blob_transactions(&self) -> bool {
        self.transactions.iter().any(|tx| tx.is_eip4844())
    }

    /// Returns whether or not the block body contains any EIP-7702 transactions.
    #[inline]
    pub fn has_eip7702_transactions(&self) -> bool {
        self.transactions.iter().any(|tx| tx.is_eip7702())
    }

    /// Returns an iterator over all blob transactions of the block
    #[inline]
    pub fn blob_transactions_iter(&self) -> impl Iterator<Item = &TransactionSigned> + '_ {
        self.transactions.iter().filter(|tx| tx.is_eip4844())
    }

    /// Returns only the blob transactions, if any, from the block body.
    #[inline]
    pub fn blob_transactions(&self) -> Vec<&TransactionSigned> {
        self.blob_transactions_iter().collect()
    }

    /// Returns an iterator over all blob versioned hashes from the block body.
    #[inline]
    pub fn blob_versioned_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.blob_transactions_iter()
            .filter_map(|tx| tx.as_eip4844().map(|blob_tx| &blob_tx.blob_versioned_hashes))
            .flatten()
    }

    /// Returns all blob versioned hashes from the block body.
    #[inline]
    pub fn blob_versioned_hashes(&self) -> Vec<&B256> {
        self.blob_versioned_hashes_iter().collect()
    }

    /// Returns an iterator over all transactions.
    #[inline]
    pub fn transactions(&self) -> impl Iterator<Item = &TransactionSigned> + '_ {
        self.transactions.iter()
    }

    /// Calculates a heuristic for the in-memory size of the [`BlockBody`].
    #[inline]
    pub fn size(&self) -> usize {
        self.transactions.iter().map(TransactionSigned::size).sum::<usize>() +
            self.transactions.capacity() * core::mem::size_of::<TransactionSigned>() +
            self.ommers.iter().map(Header::size).sum::<usize>() +
            self.ommers.capacity() * core::mem::size_of::<Header>() +
            self.withdrawals
                .as_ref()
                .map_or(core::mem::size_of::<Option<Withdrawals>>(), Withdrawals::total_size)
    }
}

impl From<Block> for BlockBody {
    fn from(block: Block) -> Self {
        Self {
            transactions: block.body.transactions,
            ommers: block.body.ommers,
            withdrawals: block.body.withdrawals,
            requests: block.body.requests,
        }
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for BlockBody {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        // first generate up to 100 txs
        let transactions = (0..100)
            .map(|_| TransactionSigned::arbitrary(u))
            .collect::<arbitrary::Result<Vec<_>>>()?;

        // then generate up to 2 ommers
        let ommers = (0..2)
            .map(|_| {
                let header = Header::arbitrary(u)?;

                Ok(header)
            })
            .collect::<arbitrary::Result<Vec<_>>>()?;

        // for now just generate empty requests, see HACK above
        Ok(Self { transactions, ommers, requests: None, withdrawals: u.arbitrary()? })
    }
}

/// Bincode-compatible block type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub(super) mod serde_bincode_compat {
    use alloc::{borrow::Cow, vec::Vec};
    use alloy_consensus::serde_bincode_compat::Header;
    use alloy_primitives::Address;
    use reth_primitives_traits::{serde_bincode_compat::SealedHeader, Requests, Withdrawals};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    use crate::transaction::serde_bincode_compat::TransactionSigned;

    /// Bincode-compatible [`super::BlockBody`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives::{serde_bincode_compat, BlockBody};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::BlockBody")]
    ///     body: BlockBody,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct BlockBody<'a> {
        transactions: Vec<TransactionSigned<'a>>,
        ommers: Vec<Header<'a>>,
        withdrawals: Cow<'a, Option<Withdrawals>>,
        requests: Cow<'a, Option<Requests>>,
    }

    impl<'a> From<&'a super::BlockBody> for BlockBody<'a> {
        fn from(value: &'a super::BlockBody) -> Self {
            Self {
                transactions: value.transactions.iter().map(Into::into).collect(),
                ommers: value.ommers.iter().map(Into::into).collect(),
                withdrawals: Cow::Borrowed(&value.withdrawals),
                requests: Cow::Borrowed(&value.requests),
            }
        }
    }

    impl<'a> From<BlockBody<'a>> for super::BlockBody {
        fn from(value: BlockBody<'a>) -> Self {
            Self {
                transactions: value.transactions.into_iter().map(Into::into).collect(),
                ommers: value.ommers.into_iter().map(Into::into).collect(),
                withdrawals: value.withdrawals.into_owned(),
                requests: value.requests.into_owned(),
            }
        }
    }

    impl SerializeAs<super::BlockBody> for BlockBody<'_> {
        fn serialize_as<S>(source: &super::BlockBody, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            BlockBody::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::BlockBody> for BlockBody<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::BlockBody, D::Error>
        where
            D: Deserializer<'de>,
        {
            BlockBody::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::SealedBlock`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives::{serde_bincode_compat, SealedBlock};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::SealedBlock")]
    ///     block: SealedBlock,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct SealedBlock<'a> {
        header: SealedHeader<'a>,
        body: BlockBody<'a>,
    }

    impl<'a> From<&'a super::SealedBlock> for SealedBlock<'a> {
        fn from(value: &'a super::SealedBlock) -> Self {
            Self { header: SealedHeader::from(&value.header), body: BlockBody::from(&value.body) }
        }
    }

    impl<'a> From<SealedBlock<'a>> for super::SealedBlock {
        fn from(value: SealedBlock<'a>) -> Self {
            Self { header: value.header.into(), body: value.body.into() }
        }
    }

    impl SerializeAs<super::SealedBlock> for SealedBlock<'_> {
        fn serialize_as<S>(source: &super::SealedBlock, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            SealedBlock::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::SealedBlock> for SealedBlock<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::SealedBlock, D::Error>
        where
            D: Deserializer<'de>,
        {
            SealedBlock::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::SealedBlockWithSenders`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives::{serde_bincode_compat, SealedBlockWithSenders};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::SealedBlockWithSenders")]
    ///     block: SealedBlockWithSenders,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct SealedBlockWithSenders<'a> {
        block: SealedBlock<'a>,
        senders: Cow<'a, Vec<Address>>,
    }

    impl<'a> From<&'a super::SealedBlockWithSenders> for SealedBlockWithSenders<'a> {
        fn from(value: &'a super::SealedBlockWithSenders) -> Self {
            Self { block: SealedBlock::from(&value.block), senders: Cow::Borrowed(&value.senders) }
        }
    }

    impl<'a> From<SealedBlockWithSenders<'a>> for super::SealedBlockWithSenders {
        fn from(value: SealedBlockWithSenders<'a>) -> Self {
            Self { block: value.block.into(), senders: value.senders.into_owned() }
        }
    }

    impl SerializeAs<super::SealedBlockWithSenders> for SealedBlockWithSenders<'_> {
        fn serialize_as<S>(
            source: &super::SealedBlockWithSenders,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            SealedBlockWithSenders::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::SealedBlockWithSenders> for SealedBlockWithSenders<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::SealedBlockWithSenders, D::Error>
        where
            D: Deserializer<'de>,
        {
            SealedBlockWithSenders::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{serde_bincode_compat, BlockBody, SealedBlock, SealedBlockWithSenders};

        use arbitrary::Arbitrary;
        use rand::Rng;
        use reth_testing_utils::generators;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_block_body_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::BlockBody")]
                block_body: BlockBody,
            }

            let mut bytes = [0u8; 1024];
            generators::rng().fill(bytes.as_mut_slice());
            let data = Data {
                block_body: BlockBody::arbitrary(&mut arbitrary::Unstructured::new(&bytes))
                    .unwrap(),
            };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_sealed_block_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::SealedBlock")]
                block: SealedBlock,
            }

            let mut bytes = [0u8; 1024];
            generators::rng().fill(bytes.as_mut_slice());
            let data = Data {
                block: SealedBlock::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap(),
            };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_sealed_block_with_senders_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::SealedBlockWithSenders")]
                block: SealedBlockWithSenders,
            }

            let mut bytes = [0u8; 1024];
            generators::rng().fill(bytes.as_mut_slice());
            let data = Data {
                block: SealedBlockWithSenders::arbitrary(&mut arbitrary::Unstructured::new(&bytes))
                    .unwrap(),
            };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockNumberOrTag::*, *};
    use alloy_eips::eip1898::HexStringMissingPrefixError;
    use alloy_primitives::hex_literal::hex;
    use alloy_rlp::{Decodable, Encodable};
    use std::str::FromStr;

    /// Check parsing according to EIP-1898.
    #[test]
    fn can_parse_blockid_u64() {
        let num = serde_json::json!(
            {"blockNumber": "0xaf"}
        );

        let id = serde_json::from_value::<BlockId>(num);
        assert_eq!(id.unwrap(), BlockId::from(175));
    }
    #[test]
    fn can_parse_block_hash() {
        let block_hash =
            B256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        let block_hash_json = serde_json::json!(
            { "blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}
        );
        let id = serde_json::from_value::<BlockId>(block_hash_json).unwrap();
        assert_eq!(id, BlockId::from(block_hash,));
    }
    #[test]
    fn can_parse_block_hash_with_canonical() {
        let block_hash =
            B256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        let block_id = BlockId::Hash(RpcBlockHash::from_hash(block_hash, Some(true)));
        let block_hash_json = serde_json::json!(
            { "blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3", "requireCanonical": true }
        );
        let id = serde_json::from_value::<BlockId>(block_hash_json).unwrap();
        assert_eq!(id, block_id)
    }
    #[test]
    fn can_parse_blockid_tags() {
        let tags =
            [("latest", Latest), ("finalized", Finalized), ("safe", Safe), ("pending", Pending)];
        for (value, tag) in tags {
            let num = serde_json::json!({ "blockNumber": value });
            let id = serde_json::from_value::<BlockId>(num);
            assert_eq!(id.unwrap(), BlockId::from(tag))
        }
    }
    #[test]
    fn repeated_keys_is_err() {
        let num = serde_json::json!({"blockNumber": 1, "requireCanonical": true, "requireCanonical": false});
        assert!(serde_json::from_value::<BlockId>(num).is_err());
        let num =
            serde_json::json!({"blockNumber": 1, "requireCanonical": true, "blockNumber": 23});
        assert!(serde_json::from_value::<BlockId>(num).is_err());
    }
    /// Serde tests
    #[test]
    fn serde_blockid_tags() {
        let block_ids = [Latest, Finalized, Safe, Pending].map(BlockId::from);
        for block_id in &block_ids {
            let serialized = serde_json::to_string(&block_id).unwrap();
            let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, *block_id)
        }
    }

    #[test]
    fn serde_blockid_number() {
        let block_id = BlockId::from(100u64);
        let serialized = serde_json::to_string(&block_id).unwrap();
        let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, block_id)
    }

    #[test]
    fn serde_blockid_hash() {
        let block_id = BlockId::from(B256::default());
        let serialized = serde_json::to_string(&block_id).unwrap();
        let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, block_id)
    }

    #[test]
    fn serde_blockid_hash_from_str() {
        let val = "\"0x898753d8fdd8d92c1907ca21e68c7970abd290c647a202091181deec3f30a0b2\"";
        let block_hash: B256 = serde_json::from_str(val).unwrap();
        let block_id: BlockId = serde_json::from_str(val).unwrap();
        assert_eq!(block_id, BlockId::Hash(block_hash.into()));
    }

    #[test]
    fn serde_rpc_payload_block_tag() {
        let payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},"latest"],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap();
        let block_id: BlockId = serde_json::from_value::<BlockId>(block_id_param.clone()).unwrap();
        assert_eq!(BlockId::Number(BlockNumberOrTag::Latest), block_id);
    }
    #[test]
    fn serde_rpc_payload_block_object() {
        let example_payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},{"blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(example_payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap().to_string();
        let block_id: BlockId = serde_json::from_str::<BlockId>(&block_id_param).unwrap();
        let hash =
            B256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        assert_eq!(BlockId::from(hash), block_id);
        let serialized = serde_json::to_string(&BlockId::from(hash)).unwrap();
        assert_eq!("{\"blockHash\":\"0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3\"}", serialized)
    }
    #[test]
    fn serde_rpc_payload_block_number() {
        let example_payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},{"blockNumber": "0x0"}],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(example_payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap().to_string();
        let block_id: BlockId = serde_json::from_str::<BlockId>(&block_id_param).unwrap();
        assert_eq!(BlockId::from(0u64), block_id);
        let serialized = serde_json::to_string(&BlockId::from(0u64)).unwrap();
        assert_eq!("\"0x0\"", serialized)
    }
    #[test]
    #[should_panic]
    fn serde_rpc_payload_block_number_duplicate_key() {
        let payload = r#"{"blockNumber": "0x132", "blockNumber": "0x133"}"#;
        let parsed_block_id = serde_json::from_str::<BlockId>(payload);
        parsed_block_id.unwrap();
    }
    #[test]
    fn serde_rpc_payload_block_hash() {
        let payload = r#"{"blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}"#;
        let parsed = serde_json::from_str::<BlockId>(payload).unwrap();
        let expected = BlockId::from(
            B256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap(),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn encode_decode_raw_block() {
        let bytes = hex!("f90288f90218a0fe21bb173f43067a9f90cfc59bbb6830a7a2929b5de4a61f372a9db28e87f9aea01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347940000000000000000000000000000000000000000a061effbbcca94f0d3e02e5bd22e986ad57142acabf0cb3d129a6ad8d0f8752e94a0d911c25e97e27898680d242b7780b6faef30995c355a2d5de92e6b9a7212ad3aa0056b23fbba480696b65fe5a59b8f2148a1299103c4f57df839233af2cf4ca2d2b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008003834c4b408252081e80a00000000000000000000000000000000000000000000000000000000000000000880000000000000000842806be9da056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421f869f86702842806be9e82520894658bdf435d810c91414ec09147daa6db624063798203e880820a95a040ce7918eeb045ebf8c8b1887ca139d076bda00fa828a07881d442a72626c42da0156576a68e456e295e4c9cf67cf9f53151f329438916e0f24fc69d6bbb7fbacfc0c0");
        let bytes_buf = &mut bytes.as_ref();
        let block = Block::decode(bytes_buf).unwrap();
        let mut encoded_buf = Vec::with_capacity(bytes.len());
        block.encode(&mut encoded_buf);
        assert_eq!(bytes[..], encoded_buf);
    }

    #[test]
    fn serde_blocknumber_non_0xprefix() {
        let s = "\"2\"";
        let err = serde_json::from_str::<BlockNumberOrTag>(s).unwrap_err();
        assert_eq!(err.to_string(), HexStringMissingPrefixError::default().to_string());
    }

    #[test]
    fn block_with_senders() {
        let mut block = Block::default();
        let sender = Address::random();
        block.body.transactions.push(TransactionSigned::default());
        assert_eq!(BlockWithSenders::new(block.clone(), vec![]), None);
        assert_eq!(
            BlockWithSenders::new(block.clone(), vec![sender]),
            Some(BlockWithSenders { block: block.clone(), senders: vec![sender] })
        );
        let sealed = block.seal_slow();
        assert_eq!(SealedBlockWithSenders::new(sealed.clone(), vec![]), None);
        assert_eq!(
            SealedBlockWithSenders::new(sealed.clone(), vec![sender]),
            Some(SealedBlockWithSenders { block: sealed, senders: vec![sender] })
        );
    }

    #[test]
    fn test_default_seal() {
        let block = SealedBlock::default();
        let sealed = block.hash();
        let block = block.unseal();
        let block = block.seal_slow();
        assert_eq!(sealed, block.hash());
    }
}
