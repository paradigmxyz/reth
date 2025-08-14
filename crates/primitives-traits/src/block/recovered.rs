//! Recovered Block variant.

use crate::{
    block::{error::SealedBlockRecoveryError, SealedBlock},
    transaction::signed::{RecoveryError, SignedTransaction},
    Block, BlockBody, InMemorySize, SealedHeader,
};
use alloc::vec::Vec;
use alloy_consensus::{
    transaction::{Recovered, TransactionMeta},
    BlockHeader,
};
use alloy_eips::{eip1898::BlockWithParent, BlockNumHash, Encodable2718};
use alloy_primitives::{
    Address, BlockHash, BlockNumber, Bloom, Bytes, Sealed, TxHash, B256, B64, U256,
};
use derive_more::Deref;

/// A block with senders recovered from the block's transactions.
///
/// This type represents a [`SealedBlock`] where all transaction senders have been
/// recovered and verified. Recovery is an expensive operation that extracts the
/// sender address from each transaction's signature.
///
/// # Construction
///
/// - [`RecoveredBlock::new`] / [`RecoveredBlock::new_unhashed`] - Create with pre-recovered senders
///   (unchecked)
/// - [`RecoveredBlock::try_new`] / [`RecoveredBlock::try_new_unhashed`] - Create with validation
/// - [`RecoveredBlock::try_recover`] - Recover from a block
/// - [`RecoveredBlock::try_recover_sealed`] - Recover from a sealed block
///
/// # Performance
///
/// Sender recovery is computationally expensive. Cache recovered blocks when possible
/// to avoid repeated recovery operations.
///
/// ## Sealing
///
/// This type uses lazy sealing to avoid hashing the header until it is needed:
///
/// [`RecoveredBlock::new_unhashed`] creates a recovered block without hashing the header.
/// [`RecoveredBlock::new`] creates a recovered block with the corresponding block hash.
///
/// ## Recovery
///
/// Sender recovery is fallible and can fail if any of the transactions fail to recover the sender.
/// A [`SealedBlock`] can be upgraded to a [`RecoveredBlock`] using the
/// [`RecoveredBlock::try_recover`] or [`SealedBlock::try_recover`] method.
#[derive(Debug, Clone, Deref)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RecoveredBlock<B: Block> {
    /// Block
    #[deref]
    #[cfg_attr(
        feature = "serde",
        serde(bound = "SealedBlock<B>: serde::Serialize + serde::de::DeserializeOwned")
    )]
    block: SealedBlock<B>,
    /// List of senders that match the transactions in the block
    senders: Vec<Address>,
}

impl<B: Block> RecoveredBlock<B> {
    /// Creates a new recovered block instance with the given senders as provided and the block
    /// hash.
    ///
    /// Note: This expects that the given senders match the transactions in the block.
    pub fn new(block: B, senders: Vec<Address>, hash: BlockHash) -> Self {
        Self { block: SealedBlock::new_unchecked(block, hash), senders }
    }

    /// Creates a new recovered block instance with the given senders as provided.
    ///
    /// Note: This expects that the given senders match the transactions in the block.
    pub fn new_unhashed(block: B, senders: Vec<Address>) -> Self {
        Self { block: SealedBlock::new_unhashed(block), senders }
    }

    /// Returns the recovered senders.
    pub fn senders(&self) -> &[Address] {
        &self.senders
    }

    /// Returns an iterator over the recovered senders.
    pub fn senders_iter(&self) -> impl Iterator<Item = &Address> {
        self.senders.iter()
    }

    /// Consumes the type and returns the inner block.
    pub fn into_block(self) -> B {
        self.block.into_block()
    }

    /// Returns a reference to the sealed block.
    pub const fn sealed_block(&self) -> &SealedBlock<B> {
        &self.block
    }

    /// Creates a new recovered block instance with the given [`SealedBlock`] and senders as
    /// provided
    pub const fn new_sealed(block: SealedBlock<B>, senders: Vec<Address>) -> Self {
        Self { block, senders }
    }

    /// A safer variant of [`Self::new_unhashed`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_new(
        block: B,
        senders: Vec<Address>,
        hash: BlockHash,
    ) -> Result<Self, SealedBlockRecoveryError<B>> {
        let senders = if block.body().transaction_count() == senders.len() {
            senders
        } else {
            let Ok(senders) = block.body().try_recover_signers() else {
                return Err(SealedBlockRecoveryError::new(SealedBlock::new_unchecked(block, hash)));
            };
            senders
        };
        Ok(Self::new(block, senders, hash))
    }

    /// A safer variant of [`Self::new`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_new_unchecked(
        block: B,
        senders: Vec<Address>,
        hash: BlockHash,
    ) -> Result<Self, SealedBlockRecoveryError<B>> {
        let senders = if block.body().transaction_count() == senders.len() {
            senders
        } else {
            let Ok(senders) = block.body().try_recover_signers_unchecked() else {
                return Err(SealedBlockRecoveryError::new(SealedBlock::new_unchecked(block, hash)));
            };
            senders
        };
        Ok(Self::new(block, senders, hash))
    }

    /// A safer variant of [`Self::new_unhashed`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_new_unhashed(block: B, senders: Vec<Address>) -> Result<Self, RecoveryError> {
        let senders = if block.body().transaction_count() == senders.len() {
            senders
        } else {
            block.body().try_recover_signers()?
        };
        Ok(Self::new_unhashed(block, senders))
    }

    /// A safer variant of [`Self::new_unhashed`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_new_unhashed_unchecked(
        block: B,
        senders: Vec<Address>,
    ) -> Result<Self, RecoveryError> {
        let senders = if block.body().transaction_count() == senders.len() {
            senders
        } else {
            block.body().try_recover_signers_unchecked()?
        };
        Ok(Self::new_unhashed(block, senders))
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer`](crate::transaction::signed::SignedTransaction).
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover(block: B) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers()?;
        Ok(Self::new_unhashed(block, senders))
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction).
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_unchecked(block: B) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers_unchecked()?;
        Ok(Self::new_unhashed(block, senders))
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer`](crate::transaction::signed::SignedTransaction).
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_sealed(block: SealedBlock<B>) -> Result<Self, SealedBlockRecoveryError<B>> {
        let Ok(senders) = block.body().try_recover_signers() else {
            return Err(SealedBlockRecoveryError::new(block));
        };
        let (block, hash) = block.split();
        Ok(Self::new(block, senders, hash))
    }

    /// Recovers the senders from the transactions in the sealed block using
    /// [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction).
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_sealed_unchecked(
        block: SealedBlock<B>,
    ) -> Result<Self, SealedBlockRecoveryError<B>> {
        let Ok(senders) = block.body().try_recover_signers_unchecked() else {
            return Err(SealedBlockRecoveryError::new(block));
        };
        let (block, hash) = block.split();
        Ok(Self::new(block, senders, hash))
    }

    /// A safer variant of [`Self::new_unhashed`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_sealed_with_senders(
        block: SealedBlock<B>,
        senders: Vec<Address>,
    ) -> Result<Self, SealedBlockRecoveryError<B>> {
        let (block, hash) = block.split();
        Self::try_new(block, senders, hash)
    }

    /// A safer variant of [`Self::new`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_recover_sealed_with_senders_unchecked(
        block: SealedBlock<B>,
        senders: Vec<Address>,
    ) -> Result<Self, SealedBlockRecoveryError<B>> {
        let (block, hash) = block.split();
        Self::try_new_unchecked(block, senders, hash)
    }

    /// Returns the block hash.
    pub fn hash_ref(&self) -> &BlockHash {
        self.block.hash_ref()
    }

    /// Returns a copy of the block hash.
    pub fn hash(&self) -> BlockHash {
        *self.hash_ref()
    }

    /// Return the number hash tuple.
    pub fn num_hash(&self) -> BlockNumHash {
        BlockNumHash::new(self.header().number(), self.hash())
    }

    /// Return a [`BlockWithParent`] for this header.
    pub fn block_with_parent(&self) -> BlockWithParent {
        BlockWithParent { parent: self.header().parent_hash(), block: self.num_hash() }
    }

    /// Clone the header.
    pub fn clone_header(&self) -> B::Header {
        self.header().clone()
    }

    /// Clones the internal header and returns a [`SealedHeader`] sealed with the hash.
    pub fn clone_sealed_header(&self) -> SealedHeader<B::Header> {
        SealedHeader::new(self.clone_header(), self.hash())
    }

    /// Clones the wrapped block and returns the [`SealedBlock`] sealed with the hash.
    pub fn clone_sealed_block(&self) -> SealedBlock<B> {
        self.block.clone()
    }

    /// Consumes the block and returns the block's header.
    pub fn into_header(self) -> B::Header {
        self.block.into_header()
    }

    /// Consumes the block and returns the block's body.
    pub fn into_body(self) -> B::Body {
        self.block.into_body()
    }

    /// Consumes the block and returns the [`SealedBlock`] and drops the recovered senders.
    pub fn into_sealed_block(self) -> SealedBlock<B> {
        self.block
    }

    /// Consumes the type and returns its components.
    pub fn split_sealed(self) -> (SealedBlock<B>, Vec<Address>) {
        (self.block, self.senders)
    }

    /// Consumes the type and returns its components.
    #[doc(alias = "into_components")]
    pub fn split(self) -> (B, Vec<Address>) {
        (self.block.into_block(), self.senders)
    }

    /// Returns the `Recovered<&T>` transaction at the given index.
    pub fn recovered_transaction(
        &self,
        idx: usize,
    ) -> Option<Recovered<&<B::Body as BlockBody>::Transaction>> {
        let sender = self.senders.get(idx).copied()?;
        self.block.body().transactions().get(idx).map(|tx| Recovered::new_unchecked(tx, sender))
    }

    /// Finds a transaction by hash and returns it with its index and block context.
    pub fn find_indexed(&self, tx_hash: TxHash) -> Option<IndexedTx<'_, B>> {
        self.body()
            .transactions_iter()
            .enumerate()
            .find(|(_, tx)| tx.trie_hash() == tx_hash)
            .map(|(index, tx)| IndexedTx { block: self, tx, index })
    }

    /// Returns an iterator over all transactions and their sender.
    #[inline]
    pub fn transactions_with_sender(
        &self,
    ) -> impl Iterator<Item = (&Address, &<B::Body as BlockBody>::Transaction)> + '_ {
        self.senders.iter().zip(self.block.body().transactions())
    }

    /// Returns an iterator over cloned `Recovered<Transaction>`
    #[inline]
    pub fn clone_transactions_recovered(
        &self,
    ) -> impl Iterator<Item = Recovered<<B::Body as BlockBody>::Transaction>> + '_ {
        self.transactions_with_sender()
            .map(|(sender, tx)| Recovered::new_unchecked(tx.clone(), *sender))
    }

    /// Returns an iterator over `Recovered<&Transaction>`
    #[inline]
    pub fn transactions_recovered(
        &self,
    ) -> impl Iterator<Item = Recovered<&'_ <B::Body as BlockBody>::Transaction>> + '_ {
        self.transactions_with_sender().map(|(sender, tx)| Recovered::new_unchecked(tx, *sender))
    }

    /// Consumes the type and returns an iterator over all [`Recovered`] transactions in the block.
    #[inline]
    pub fn into_transactions_recovered(
        self,
    ) -> impl Iterator<Item = Recovered<<B::Body as BlockBody>::Transaction>> {
        self.block
            .split()
            .0
            .into_body()
            .into_transactions()
            .into_iter()
            .zip(self.senders)
            .map(|(tx, sender)| tx.with_signer(sender))
    }

    /// Consumes the block and returns the transactions of the block.
    #[inline]
    pub fn into_transactions(self) -> Vec<<B::Body as BlockBody>::Transaction> {
        self.block.split().0.into_body().into_transactions()
    }
}

impl<B: Block> BlockHeader for RecoveredBlock<B> {
    fn parent_hash(&self) -> B256 {
        self.header().parent_hash()
    }

    fn ommers_hash(&self) -> B256 {
        self.header().ommers_hash()
    }

    fn beneficiary(&self) -> Address {
        self.header().beneficiary()
    }

    fn state_root(&self) -> B256 {
        self.header().state_root()
    }

    fn transactions_root(&self) -> B256 {
        self.header().transactions_root()
    }

    fn receipts_root(&self) -> B256 {
        self.header().receipts_root()
    }

    fn withdrawals_root(&self) -> Option<B256> {
        self.header().withdrawals_root()
    }

    fn logs_bloom(&self) -> Bloom {
        self.header().logs_bloom()
    }

    fn difficulty(&self) -> U256 {
        self.header().difficulty()
    }

    fn number(&self) -> BlockNumber {
        self.header().number()
    }

    fn gas_limit(&self) -> u64 {
        self.header().gas_limit()
    }

    fn gas_used(&self) -> u64 {
        self.header().gas_used()
    }

    fn timestamp(&self) -> u64 {
        self.header().timestamp()
    }

    fn mix_hash(&self) -> Option<B256> {
        self.header().mix_hash()
    }

    fn nonce(&self) -> Option<B64> {
        self.header().nonce()
    }

    fn base_fee_per_gas(&self) -> Option<u64> {
        self.header().base_fee_per_gas()
    }

    fn blob_gas_used(&self) -> Option<u64> {
        self.header().blob_gas_used()
    }

    fn excess_blob_gas(&self) -> Option<u64> {
        self.header().excess_blob_gas()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.header().parent_beacon_block_root()
    }

    fn requests_hash(&self) -> Option<B256> {
        self.header().requests_hash()
    }

    fn extra_data(&self) -> &Bytes {
        self.header().extra_data()
    }
}

impl<B: Block> Eq for RecoveredBlock<B> {}

impl<B: Block> PartialEq for RecoveredBlock<B> {
    fn eq(&self, other: &Self) -> bool {
        self.hash_ref().eq(other.hash_ref()) &&
            self.block.eq(&other.block) &&
            self.senders.eq(&other.senders)
    }
}

impl<B: Block + Default> Default for RecoveredBlock<B> {
    #[inline]
    fn default() -> Self {
        Self::new_unhashed(B::default(), Default::default())
    }
}

impl<B: Block> InMemorySize for RecoveredBlock<B> {
    #[inline]
    fn size(&self) -> usize {
        self.block.size() + self.senders.len() * core::mem::size_of::<Address>()
    }
}

impl<B: Block> From<RecoveredBlock<B>> for Sealed<B> {
    fn from(value: RecoveredBlock<B>) -> Self {
        value.block.into()
    }
}

/// Converts a block with recovered transactions into a [`RecoveredBlock`].
///
/// This implementation takes an `alloy_consensus::Block` where transactions are of type
/// `Recovered<T>` (transactions with their recovered senders) and converts it into a
/// [`RecoveredBlock`] which stores transactions and senders separately for efficiency.
impl<T, H> From<alloy_consensus::Block<Recovered<T>, H>>
    for RecoveredBlock<alloy_consensus::Block<T, H>>
where
    T: SignedTransaction,
    H: crate::block::header::BlockHeader,
{
    fn from(block: alloy_consensus::Block<Recovered<T>, H>) -> Self {
        let header = block.header;

        // Split the recovered transactions into transactions and senders
        let (transactions, senders): (Vec<T>, Vec<Address>) = block
            .body
            .transactions
            .into_iter()
            .map(|recovered| {
                let (tx, sender) = recovered.into_parts();
                (tx, sender)
            })
            .unzip();

        // Reconstruct the block with regular transactions
        let body = alloy_consensus::BlockBody {
            transactions,
            ommers: block.body.ommers,
            withdrawals: block.body.withdrawals,
        };

        let block = alloy_consensus::Block::new(header, body);

        Self::new_unhashed(block, senders)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, B> arbitrary::Arbitrary<'a> for RecoveredBlock<B>
where
    B: Block + arbitrary::Arbitrary<'a>,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let block = B::arbitrary(u)?;
        Ok(Self::try_recover(block).unwrap())
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<B: Block> RecoveredBlock<B> {
    /// Returns a mutable reference to the recovered senders.
    pub const fn senders_mut(&mut self) -> &mut Vec<Address> {
        &mut self.senders
    }

    /// Appends the sender to the list of senders.
    pub fn push_sender(&mut self, sender: Address) {
        self.senders.push(sender);
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<B> core::ops::DerefMut for RecoveredBlock<B>
where
    B: Block,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.block
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<B: crate::test_utils::TestBlock> RecoveredBlock<B> {
    /// Updates the block header.
    pub fn set_header(&mut self, header: B::Header) {
        *self.header_mut() = header
    }

    /// Updates the block hash.
    pub fn set_hash(&mut self, hash: BlockHash) {
        self.block.set_hash(hash)
    }

    /// Returns a mutable reference to the header.
    pub const fn header_mut(&mut self) -> &mut B::Header {
        self.block.header_mut()
    }

    /// Returns a mutable reference to the body.
    pub const fn block_mut(&mut self) -> &mut B::Body {
        self.block.body_mut()
    }

    /// Updates the parent block hash.
    pub fn set_parent_hash(&mut self, hash: BlockHash) {
        self.block.set_parent_hash(hash);
    }

    /// Updates the block number.
    pub fn set_block_number(&mut self, number: alloy_primitives::BlockNumber) {
        self.block.set_block_number(number);
    }

    /// Updates the block state root.
    pub fn set_state_root(&mut self, state_root: alloy_primitives::B256) {
        self.block.set_state_root(state_root);
    }

    /// Updates the block difficulty.
    pub fn set_difficulty(&mut self, difficulty: alloy_primitives::U256) {
        self.block.set_difficulty(difficulty);
    }
}

/// Transaction with its index and block reference for efficient metadata access.
#[derive(Debug)]
pub struct IndexedTx<'a, B: Block> {
    /// Recovered block containing the transaction
    block: &'a RecoveredBlock<B>,
    /// Transaction matching the hash
    tx: &'a <B::Body as BlockBody>::Transaction,
    /// Index of the transaction in the block
    index: usize,
}

impl<'a, B: Block> IndexedTx<'a, B> {
    /// Returns the transaction.
    pub const fn tx(&self) -> &<B::Body as BlockBody>::Transaction {
        self.tx
    }

    /// Returns the transaction hash.
    pub fn tx_hash(&self) -> TxHash {
        self.tx.trie_hash()
    }

    /// Returns the block hash.
    pub fn block_hash(&self) -> B256 {
        self.block.hash()
    }

    /// Returns the index of the transaction in the block.
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Builds a [`TransactionMeta`] for the indexed transaction.
    pub fn meta(&self) -> TransactionMeta {
        TransactionMeta {
            tx_hash: self.tx.trie_hash(),
            index: self.index as u64,
            block_hash: self.block.hash(),
            block_number: self.block.number(),
            base_fee: self.block.base_fee_per_gas(),
            timestamp: self.block.timestamp(),
            excess_blob_gas: self.block.excess_blob_gas(),
        }
    }
}

#[cfg(feature = "rpc-compat")]
mod rpc_compat {
    use super::{
        Block as BlockTrait, BlockBody as BlockBodyTrait, RecoveredBlock, SignedTransaction,
    };
    use crate::{block::error::BlockRecoveryError, SealedHeader};
    use alloc::vec::Vec;
    use alloy_consensus::{
        transaction::Recovered, Block as CBlock, BlockBody, BlockHeader, Sealable,
    };
    use alloy_rpc_types_eth::{Block, BlockTransactions, BlockTransactionsKind, TransactionInfo};

    impl<B> RecoveredBlock<B>
    where
        B: BlockTrait,
    {
        /// Converts the block into an RPC [`Block`] with the given [`BlockTransactionsKind`].
        ///
        /// The `tx_resp_builder` closure transforms each transaction into the desired response
        /// type.
        ///
        /// `header_builder` transforms the block header into RPC representation. It takes the
        /// consensus header and RLP length of the block which is a common dependency of RPC
        /// headers.
        pub fn into_rpc_block<T, RpcH, F, E>(
            self,
            kind: BlockTransactionsKind,
            tx_resp_builder: F,
            header_builder: impl FnOnce(SealedHeader<B::Header>, usize) -> Result<RpcH, E>,
        ) -> Result<Block<T, RpcH>, E>
        where
            F: Fn(
                Recovered<<<B as BlockTrait>::Body as BlockBodyTrait>::Transaction>,
                TransactionInfo,
            ) -> Result<T, E>,
        {
            match kind {
                BlockTransactionsKind::Hashes => self.into_rpc_block_with_tx_hashes(header_builder),
                BlockTransactionsKind::Full => {
                    self.into_rpc_block_full(tx_resp_builder, header_builder)
                }
            }
        }

        /// Converts the block to an RPC [`Block`] without consuming self.
        ///
        /// For transaction hashes, only necessary parts are cloned for efficiency.
        /// For full transactions, the entire block is cloned.
        ///
        /// The `tx_resp_builder` closure transforms each transaction into the desired response
        /// type.
        ///
        /// `header_builder` transforms the block header into RPC representation. It takes the
        /// consensus header and RLP length of the block which is a common dependency of RPC
        /// headers.
        pub fn clone_into_rpc_block<T, RpcH, F, E>(
            &self,
            kind: BlockTransactionsKind,
            tx_resp_builder: F,
            header_builder: impl FnOnce(SealedHeader<B::Header>, usize) -> Result<RpcH, E>,
        ) -> Result<Block<T, RpcH>, E>
        where
            F: Fn(
                Recovered<<<B as BlockTrait>::Body as BlockBodyTrait>::Transaction>,
                TransactionInfo,
            ) -> Result<T, E>,
        {
            match kind {
                BlockTransactionsKind::Hashes => self.to_rpc_block_with_tx_hashes(header_builder),
                BlockTransactionsKind::Full => {
                    self.clone().into_rpc_block_full(tx_resp_builder, header_builder)
                }
            }
        }

        /// Creates an RPC [`Block`] with transaction hashes from a reference.
        ///
        /// Returns [`BlockTransactions::Hashes`] containing only transaction hashes.
        /// Efficiently clones only necessary parts, not the entire block.
        pub fn to_rpc_block_with_tx_hashes<T, RpcH, E>(
            &self,
            header_builder: impl FnOnce(SealedHeader<B::Header>, usize) -> Result<RpcH, E>,
        ) -> Result<Block<T, RpcH>, E> {
            let transactions = self.body().transaction_hashes_iter().copied().collect();
            let rlp_length = self.rlp_length();
            let header = self.clone_sealed_header();
            let withdrawals = self.body().withdrawals().cloned();

            let transactions = BlockTransactions::Hashes(transactions);
            let uncles =
                self.body().ommers().unwrap_or(&[]).iter().map(|h| h.hash_slow()).collect();
            let header = header_builder(header, rlp_length)?;

            Ok(Block { header, uncles, transactions, withdrawals })
        }

        /// Converts the block into an RPC [`Block`] with transaction hashes.
        ///
        /// Consumes self and returns [`BlockTransactions::Hashes`] containing only transaction
        /// hashes.
        pub fn into_rpc_block_with_tx_hashes<T, E, RpcHeader>(
            self,
            f: impl FnOnce(SealedHeader<B::Header>, usize) -> Result<RpcHeader, E>,
        ) -> Result<Block<T, RpcHeader>, E> {
            let transactions = self.body().transaction_hashes_iter().copied().collect();
            let rlp_length = self.rlp_length();
            let (header, body) = self.into_sealed_block().split_sealed_header_body();
            let BlockBody { ommers, withdrawals, .. } = body.into_ethereum_body();

            let transactions = BlockTransactions::Hashes(transactions);
            let uncles = ommers.into_iter().map(|h| h.hash_slow()).collect();
            let header = f(header, rlp_length)?;

            Ok(Block { header, uncles, transactions, withdrawals })
        }

        /// Converts the block into an RPC [`Block`] with full transaction objects.
        ///
        /// Returns [`BlockTransactions::Full`] with complete transaction data.
        /// The `tx_resp_builder` closure transforms each transaction with its metadata.
        pub fn into_rpc_block_full<T, RpcHeader, F, E>(
            self,
            tx_resp_builder: F,
            header_builder: impl FnOnce(SealedHeader<B::Header>, usize) -> Result<RpcHeader, E>,
        ) -> Result<Block<T, RpcHeader>, E>
        where
            F: Fn(
                Recovered<<<B as BlockTrait>::Body as BlockBodyTrait>::Transaction>,
                TransactionInfo,
            ) -> Result<T, E>,
        {
            let block_number = self.header().number();
            let base_fee = self.header().base_fee_per_gas();
            let block_length = self.rlp_length();
            let block_hash = Some(self.hash());

            let (block, senders) = self.split_sealed();
            let (header, body) = block.split_sealed_header_body();
            let BlockBody { transactions, ommers, withdrawals } = body.into_ethereum_body();

            let transactions = transactions
                .into_iter()
                .zip(senders)
                .enumerate()
                .map(|(idx, (tx, sender))| {
                    let tx_info = TransactionInfo {
                        hash: Some(*tx.tx_hash()),
                        block_hash,
                        block_number: Some(block_number),
                        base_fee,
                        index: Some(idx as u64),
                    };

                    tx_resp_builder(Recovered::new_unchecked(tx, sender), tx_info)
                })
                .collect::<Result<Vec<_>, E>>()?;

            let transactions = BlockTransactions::Full(transactions);
            let uncles = ommers.into_iter().map(|h| h.hash_slow()).collect();
            let header = header_builder(header, block_length)?;

            let block = Block { header, uncles, transactions, withdrawals };

            Ok(block)
        }
    }

    impl<T> RecoveredBlock<CBlock<T>>
    where
        T: SignedTransaction,
    {
        /// Creates a `RecoveredBlock` from an RPC block.
        ///
        /// Converts the RPC block to consensus format and recovers transaction senders.
        /// Works with any transaction type `U` that can be converted to `T`.
        ///
        /// # Examples
        /// ```ignore
        /// let rpc_block: alloy_rpc_types_eth::Block = get_rpc_block();
        /// let recovered = RecoveredBlock::from_rpc_block(rpc_block)?;
        /// ```
        pub fn from_rpc_block<U>(
            block: alloy_rpc_types_eth::Block<U>,
        ) -> Result<Self, BlockRecoveryError<alloy_consensus::Block<T>>>
        where
            T: From<U>,
        {
            // Convert to consensus block and then convert transactions
            let consensus_block = block.into_consensus().convert_transactions();

            // Try to recover the block
            consensus_block.try_into_recovered()
        }
    }

    impl<T, U> TryFrom<alloy_rpc_types_eth::Block<U>> for RecoveredBlock<CBlock<T>>
    where
        T: SignedTransaction + From<U>,
    {
        type Error = BlockRecoveryError<alloy_consensus::Block<T>>;

        fn try_from(block: alloy_rpc_types_eth::Block<U>) -> Result<Self, Self::Error> {
            Self::from_rpc_block(block)
        }
    }
}

/// Bincode-compatible [`RecoveredBlock`] serde implementation.
#[cfg(feature = "serde-bincode-compat")]
pub(super) mod serde_bincode_compat {
    use crate::{
        serde_bincode_compat::{self, SerdeBincodeCompat},
        Block,
    };
    use alloc::{borrow::Cow, vec::Vec};
    use alloy_primitives::Address;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::RecoveredBlock`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives_traits::{
    ///     block::RecoveredBlock,
    ///     serde_bincode_compat::{self, SerdeBincodeCompat},
    ///     Block,
    /// };
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data<T: Block<Header: SerdeBincodeCompat, Body: SerdeBincodeCompat> + 'static> {
    ///     #[serde_as(as = "serde_bincode_compat::RecoveredBlock<'_, T>")]
    ///     block: RecoveredBlock<T>,
    /// }
    /// ```
    #[derive(derive_more::Debug, Serialize, Deserialize)]
    pub struct RecoveredBlock<
        'a,
        T: Block<Header: SerdeBincodeCompat, Body: SerdeBincodeCompat> + 'static,
    > {
        #[serde(
            bound = "serde_bincode_compat::SealedBlock<'a, T>: Serialize + serde::de::DeserializeOwned"
        )]
        block: serde_bincode_compat::SealedBlock<'a, T>,
        #[expect(clippy::owned_cow)]
        senders: Cow<'a, Vec<Address>>,
    }

    impl<'a, T: Block<Header: SerdeBincodeCompat, Body: SerdeBincodeCompat> + 'static>
        From<&'a super::RecoveredBlock<T>> for RecoveredBlock<'a, T>
    {
        fn from(value: &'a super::RecoveredBlock<T>) -> Self {
            Self { block: (&value.block).into(), senders: Cow::Borrowed(&value.senders) }
        }
    }

    impl<'a, T: Block<Header: SerdeBincodeCompat, Body: SerdeBincodeCompat> + 'static>
        From<RecoveredBlock<'a, T>> for super::RecoveredBlock<T>
    {
        fn from(value: RecoveredBlock<'a, T>) -> Self {
            Self::new_sealed(value.block.into(), value.senders.into_owned())
        }
    }

    impl<T: Block<Header: SerdeBincodeCompat, Body: SerdeBincodeCompat> + 'static>
        SerializeAs<super::RecoveredBlock<T>> for RecoveredBlock<'_, T>
    {
        fn serialize_as<S>(
            source: &super::RecoveredBlock<T>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            RecoveredBlock::from(source).serialize(serializer)
        }
    }

    impl<'de, T: Block<Header: SerdeBincodeCompat, Body: SerdeBincodeCompat> + 'static>
        DeserializeAs<'de, super::RecoveredBlock<T>> for RecoveredBlock<'de, T>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<super::RecoveredBlock<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            RecoveredBlock::deserialize(deserializer).map(Into::into)
        }
    }

    impl<T: Block<Header: SerdeBincodeCompat, Body: SerdeBincodeCompat> + 'static>
        SerdeBincodeCompat for super::RecoveredBlock<T>
    {
        type BincodeRepr<'a> = RecoveredBlock<'a, T>;

        fn as_repr(&self) -> Self::BincodeRepr<'_> {
            self.into()
        }

        fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
            repr.into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Header, TxLegacy};
    use alloy_primitives::{bytes, Signature, TxKind};

    #[test]
    fn test_from_block_with_recovered_transactions() {
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce: 0,
            gas_price: 21_000_000_000,
            gas_limit: 21_000,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: bytes!(),
        };

        let signature = Signature::new(U256::from(1), U256::from(2), false);
        let sender = Address::from([0x01; 20]);

        let signed_tx = alloy_consensus::TxEnvelope::Legacy(
            alloy_consensus::Signed::new_unchecked(tx, signature, B256::ZERO),
        );

        let recovered_tx = Recovered::new_unchecked(signed_tx, sender);

        let header = Header::default();
        let body = alloy_consensus::BlockBody {
            transactions: vec![recovered_tx],
            ommers: vec![],
            withdrawals: None,
        };
        let block_with_recovered = alloy_consensus::Block::new(header, body);

        let recovered_block: RecoveredBlock<
            alloy_consensus::Block<alloy_consensus::TxEnvelope, Header>,
        > = block_with_recovered.into();

        assert_eq!(recovered_block.senders().len(), 1);
        assert_eq!(recovered_block.senders()[0], sender);
        assert_eq!(recovered_block.body().transactions().count(), 1);
    }
}
