use reth_interfaces::provider::TransactionDataStoreError;
use reth_primitives::{
    Bytes, StoredTransaction, TransactionSigned, TransactionSignedNoHash, TxHash,
};

/// The abstraction for storing and looking up transaction data
/// in some storage separate from the database.
pub trait TransactionDataStore: std::fmt::Debug + Send + Sync {
    /// Saves transaction data to the store.
    fn save(&self, hash: TxHash, data: Bytes) -> Result<(), TransactionDataStoreError>;

    /// Loads transaction data from file by specified hash.
    fn load(&self, hash: TxHash) -> Result<Option<Bytes>, TransactionDataStoreError>;

    /// Removes transactions data by specified hash.
    fn remove(&self, hash: TxHash) -> Result<(), TransactionDataStoreError>;

    /// Converts stored transaction into [TransactionSignedNoHash].
    /// Returns error if the transaction data was not found.
    fn stored_tx_into_signed_no_hash(
        &self,
        tx: StoredTransaction,
    ) -> Result<TransactionSignedNoHash, TransactionDataStoreError>;

    /// Converts stored transaction into [TransactionSigned].
    /// Returns error if the transaction data was not found.
    fn stored_tx_into_signed(
        &self,
        tx: StoredTransaction,
    ) -> Result<TransactionSigned, TransactionDataStoreError>;
}
