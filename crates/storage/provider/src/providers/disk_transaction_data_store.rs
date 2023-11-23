use reth_interfaces::provider::TransactionDataStoreError;
use reth_nippy_jar::compression::{Compression, Lz4};
use reth_primitives::{
    transaction::StoredTransactionData, Bytes, StoredTransaction, Transaction, TransactionSigned,
    TransactionSignedNoHash, TxEip1559, TxEip2930, TxEip4844, TxHash, TxLegacy,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::traits::TransactionDataStore;

/// File backed transaction data store.
/// This implementation naively saves, loads and deletes transaction data from disk using filesystem
/// functions from the standard library.
#[derive(Debug)]
pub struct DiskFileTransactionDataStore {
    path: PathBuf,
    compressor: Lz4,
}

impl DiskFileTransactionDataStore {
    /// Create new transaction data store from path.
    pub fn new(path: PathBuf) -> Result<Self, TransactionDataStoreError> {
        fs::create_dir_all(&path)?;
        Ok(Self { path, compressor: Lz4::default() })
    }

    /// Returns path to directory where transaction data is stored.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn filepath(&self, hash: TxHash) -> PathBuf {
        self.path.join(format!("{hash}.data"))
    }
}

impl TransactionDataStore for DiskFileTransactionDataStore {
    /// Saves transactions data to a separate file.
    fn save(&self, hash: TxHash, data: Bytes) -> Result<(), TransactionDataStoreError> {
        let filepath = self.filepath(hash);
        let compressed = self.compressor.compress(&data[..])?;
        tracing::trace!(
            target: "provider::txdata",
            %hash,
            ?filepath,
            size = data.len(),
            compressed_size = compressed.len(),
            "Saving transaction data to disk"
        );
        fs::write(filepath, compressed)?;
        Ok(())
    }

    /// Loads transactions data from file by specified hash.
    fn load(&self, hash: TxHash) -> Result<Option<Bytes>, TransactionDataStoreError> {
        let filepath = self.filepath(hash);
        let exists = filepath.try_exists()?;
        tracing::trace!(target: "provider::txdata", %hash, ?filepath, exists, "Loading transaction data from disk");
        Ok(if exists {
            let raw = fs::read(filepath)?;
            Some(self.compressor.decompress(&raw)?.into())
        } else {
            None
        })
    }

    /// Removes transactions data by specified hash.
    fn remove(&self, hash: TxHash) -> Result<(), TransactionDataStoreError> {
        let filepath = self.filepath(hash);
        tracing::trace!(target: "provider::txdata", %hash, ?filepath, "Removing transaction data from disk");
        Ok(fs::remove_file(filepath)?)
    }

    /// Converts stored transaction into [TransactionSignedNoHash].
    /// Returns error if the transaction data was not found or  
    fn stored_tx_into_signed_no_hash(
        &self,
        tx: StoredTransaction,
    ) -> Result<TransactionSignedNoHash, TransactionDataStoreError> {
        let (mut transaction, stored_transaction_data) = tx.into_inner();
        if let StoredTransactionData::Excluded(hash) = stored_transaction_data {
            match &mut transaction.transaction {
                Transaction::Legacy(TxLegacy { input, .. }) |
                Transaction::Eip2930(TxEip2930 { input, .. }) |
                Transaction::Eip1559(TxEip1559 { input, .. }) |
                Transaction::Eip4844(TxEip4844 { input, .. }) => {
                    *input = self
                        .load(hash)?
                        .ok_or(TransactionDataStoreError::MissingTransactionData(hash))?;
                }
            };
        }
        Ok(transaction)
    }

    /// Converts stored transaction into [TransactionSigned].
    /// Returns error if the transaction data was not found or  
    fn stored_tx_into_signed(
        &self,
        tx: StoredTransaction,
    ) -> Result<TransactionSigned, TransactionDataStoreError> {
        let mut tx_hash = None;
        let (mut transaction, stored_transaction_data) = tx.into_inner();
        if let StoredTransactionData::Excluded(hash) = stored_transaction_data {
            tx_hash = Some(hash);
            match &mut transaction.transaction {
                Transaction::Legacy(TxLegacy { input, .. }) |
                Transaction::Eip2930(TxEip2930 { input, .. }) |
                Transaction::Eip1559(TxEip1559 { input, .. }) |
                Transaction::Eip4844(TxEip4844 { input, .. }) => {
                    *input = self
                        .load(hash)?
                        .ok_or(TransactionDataStoreError::MissingTransactionData(hash))?;
                }
            };
        }

        let hash = tx_hash.unwrap_or_else(|| transaction.hash());
        Ok(TransactionSigned {
            transaction: transaction.transaction,
            signature: transaction.signature,
            hash,
        })
    }
}
