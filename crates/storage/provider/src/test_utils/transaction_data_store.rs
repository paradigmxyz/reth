use crate::{providers::DiskFileTransactionDataStore, TransactionDataStore};
use reth_interfaces::provider::TransactionDataStoreError;
use reth_primitives::{
    Bytes, StoredTransaction, TransactionSigned, TransactionSignedNoHash, TxHash,
};

/// Transaction data store backed by temporary directory.
/// The directory is removed on drops.
#[derive(Debug)]
pub struct TempTransactionDataStore {
    store: DiskFileTransactionDataStore,
}

impl Default for TempTransactionDataStore {
    fn default() -> Self {
        let path = tempfile::TempDir::new().expect("error creating temp dir").into_path();
        Self { store: DiskFileTransactionDataStore::new(path) }
    }
}

impl Drop for TempTransactionDataStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.store.path());
    }
}

impl TransactionDataStore for TempTransactionDataStore {
    fn load(&self, hash: TxHash) -> Result<Option<Bytes>, TransactionDataStoreError> {
        self.store.load(hash)
    }

    fn save(&self, hash: TxHash, data: Bytes) -> Result<(), TransactionDataStoreError> {
        self.store.save(hash, data)
    }

    fn remove(&self, hash: TxHash) -> Result<(), TransactionDataStoreError> {
        self.store.remove(hash)
    }

    fn stored_tx_into_signed_no_hash(
        &self,
        tx: StoredTransaction,
    ) -> Result<TransactionSignedNoHash, TransactionDataStoreError> {
        self.store.stored_tx_into_signed_no_hash(tx)
    }

    fn stored_tx_into_signed(
        &self,
        tx: StoredTransaction,
    ) -> Result<TransactionSigned, TransactionDataStoreError> {
        self.store.stored_tx_into_signed(tx)
    }
}
