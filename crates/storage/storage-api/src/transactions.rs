use crate::{BlockNumReader, BlockReader};
use alloy_consensus::transaction::TransactionMeta;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, BlockNumber, TxHash, TxNumber};
use reth_primitives_traits::SignedTransaction;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::ops::{Range, RangeBounds, RangeInclusive};

/// Enum to control transaction hash inclusion.
///
/// This serves as a hint to the provider to include or omit exclude hashes because hashes are
/// stored separately and are not always needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionVariant {
    /// Indicates that transactions should be processed without including their hashes.
    NoHash,
    /// Indicates that transactions should be processed along with their hashes.
    #[default]
    WithHash,
}

///  Client trait for fetching transactions related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait TransactionsProvider: BlockNumReader + Send + Sync {
    /// The transaction type this provider reads.
    type Transaction: Send + Sync + SignedTransaction;

    /// Get internal transaction identifier by transaction hash.
    ///
    /// This is the inverse of [TransactionsProvider::transaction_by_id].
    /// Returns None if the transaction is not found.
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>>;

    /// Get transaction by id, computes hash every time so more expensive.
    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>>;

    /// Get transaction by id without computing the hash.
    fn transaction_by_id_unhashed(&self, id: TxNumber)
        -> ProviderResult<Option<Self::Transaction>>;

    /// Get transaction by transaction hash.
    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>>;

    /// Get transaction by transaction hash and additional metadata of the block the transaction was
    /// mined in
    fn transaction_by_hash_with_meta(
        &self,
        hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>>;

    /// Get transaction block number
    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>>;

    /// Get transactions by block id.
    fn transactions_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>>;

    /// Get transactions by block range.
    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>>;

    /// Get transactions by tx range.
    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>>;

    /// Get Senders from a tx range.
    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>>;

    /// Get transaction sender.
    ///
    /// Returns None if the transaction is not found.
    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>>;
}

/// A helper type alias to access [`TransactionsProvider::Transaction`].
pub type ProviderTx<P> = <P as TransactionsProvider>::Transaction;

///  Client trait for fetching additional transactions related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait TransactionsProviderExt: BlockReader {
    /// Get transactions range by block range.
    fn transaction_range_by_block_range(
        &self,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<RangeInclusive<TxNumber>> {
        let from = self
            .block_body_indices(*block_range.start())?
            .ok_or_else(|| ProviderError::BlockBodyIndicesNotFound(*block_range.start()))?
            .first_tx_num();

        let to = self
            .block_body_indices(*block_range.end())?
            .ok_or_else(|| ProviderError::BlockBodyIndicesNotFound(*block_range.end()))?
            .last_tx_num();

        Ok(from..=to)
    }

    /// Get transaction hashes from a transaction range.
    fn transaction_hashes_by_range(
        &self,
        tx_range: Range<TxNumber>,
    ) -> ProviderResult<Vec<(TxHash, TxNumber)>>;
}
