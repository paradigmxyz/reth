use crate::BlockNumReader;
use reth_interfaces::RethResult;
use reth_primitives::{
    Address, BlockHashOrNumber, BlockNumber, TransactionMeta, TransactionSigned,
    TransactionSignedNoHash, TxHash, TxNumber,
};
use std::ops::RangeBounds;

///  Client trait for fetching [TransactionSigned] related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait TransactionsProvider: BlockNumReader + Send + Sync {
    /// Get internal transaction identifier by transaction hash.
    ///
    /// This is the inverse of [TransactionsProvider::transaction_by_id].
    /// Returns None if the transaction is not found.
    fn transaction_id(&self, tx_hash: TxHash) -> RethResult<Option<TxNumber>>;

    /// Get transaction by id, computes hash everytime so more expensive.
    fn transaction_by_id(&self, id: TxNumber) -> RethResult<Option<TransactionSigned>>;

    /// Get transaction by id without computing the hash.
    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> RethResult<Option<TransactionSignedNoHash>>;

    /// Get transaction by transaction hash.
    fn transaction_by_hash(&self, hash: TxHash) -> RethResult<Option<TransactionSigned>>;

    /// Get transaction by transaction hash and additional metadata of the block the transaction was
    /// mined in
    fn transaction_by_hash_with_meta(
        &self,
        hash: TxHash,
    ) -> RethResult<Option<(TransactionSigned, TransactionMeta)>>;

    /// Get transaction block number
    fn transaction_block(&self, id: TxNumber) -> RethResult<Option<BlockNumber>>;

    /// Get transactions by block id.
    fn transactions_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> RethResult<Option<Vec<TransactionSigned>>>;

    /// Get transactions by block range.
    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<Vec<TransactionSigned>>>;

    /// Get transactions by tx range.
    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> RethResult<Vec<TransactionSignedNoHash>>;

    /// Get Senders from a tx range.
    fn senders_by_tx_range(&self, range: impl RangeBounds<TxNumber>) -> RethResult<Vec<Address>>;

    /// Get transaction sender.
    ///
    /// Returns None if the transaction is not found.
    fn transaction_sender(&self, id: TxNumber) -> RethResult<Option<Address>>;
}
