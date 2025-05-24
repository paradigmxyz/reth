use crate::{
    error::PoolError, maintain::LocalTransactionBackupConfig, traits::TransactionPool,
    PoolTransaction,
};
use alloy_rlp::Encodable;
use reth_fs_util::FsPathError;
use reth_primitives_traits::SignedTransaction;
use std::path::Path;
use tracing::{debug, error, info, trace, warn};

/// Loads transactions from a file, decodes them from the RLP format, and inserts them
/// into the transaction pool on node boot up.
/// The file is removed after the transactions have been successfully processed.
pub(crate) async fn load_and_reinsert_transactions<P>(
    pool: P,
    file_path: &Path,
) -> Result<(), TransactionsBackupError>
where
    P: TransactionPool<Transaction: PoolTransaction<Consensus: SignedTransaction>>,
{
    if !file_path.exists() {
        return Ok(())
    }

    debug!(target: "txpool", txs_file =?file_path, "Check local persistent storage for saved transactions");
    let data = reth_fs_util::read(file_path)?;

    if data.is_empty() {
        return Ok(())
    }

    let txs_signed: Vec<<P::Transaction as PoolTransaction>::Consensus> =
        alloy_rlp::Decodable::decode(&mut data.as_slice())?;

    let pool_transactions = txs_signed
        .into_iter()
        .filter_map(|tx| tx.try_clone_into_recovered().ok())
        .filter_map(|tx| {
            // Filter out errors
            <P::Transaction as PoolTransaction>::try_from_consensus(tx).ok()
        })
        .collect();

    let outcome = pool.add_transactions(crate::TransactionOrigin::Local, pool_transactions).await;

    info!(target: "txpool", txs_file =?file_path, num_txs=%outcome.len(), "Successfully reinserted local transactions from file");
    reth_fs_util::remove_file(file_path)?;
    Ok(())
}

fn save_local_txs_backup<P>(pool: P, file_path: &Path)
where
    P: TransactionPool<Transaction: PoolTransaction<Consensus: Encodable>>,
{
    let local_transactions = pool.get_local_transactions();
    if local_transactions.is_empty() {
        trace!(target: "txpool", "no local transactions to save");
        return
    }

    let local_transactions = local_transactions
        .into_iter()
        .map(|tx| tx.transaction.clone_into_consensus().into_inner())
        .collect::<Vec<_>>();

    let num_txs = local_transactions.len();
    let mut buf = Vec::new();
    alloy_rlp::encode_list(&local_transactions, &mut buf);
    info!(target: "txpool", txs_file =?file_path, num_txs=%num_txs, "Saving current local transactions");
    let parent_dir = file_path.parent().map(std::fs::create_dir_all).transpose();

    match parent_dir.map(|_| reth_fs_util::write(file_path, buf)) {
        Ok(_) => {
            info!(target: "txpool", txs_file=?file_path, "Wrote local transactions to file");
        }
        Err(err) => {
            warn!(target: "txpool", %err, txs_file=?file_path, "Failed to write local transactions to file");
        }
    }
}

/// Errors possible during txs backup load and decode
#[derive(thiserror::Error, Debug)]
pub(crate) enum TransactionsBackupError {
    /// Error during RLP decoding of transactions
    #[error("failed to apply transactions backup. Encountered RLP decode error: {0}")]
    Decode(#[from] alloy_rlp::Error),
    /// Error during file upload
    #[error("failed to apply transactions backup. Encountered file error: {0}")]
    FsPath(#[from] FsPathError),
    /// Error adding transactions to the transaction pool
    #[error("failed to insert transactions to the transactions pool. Encountered pool error: {0}")]
    Pool(#[from] PoolError),
}

/// Task which manages saving local transactions to the persistent file in case of shutdown.
/// Reloads the transactions from the file on the boot up and inserts them into the pool.
pub async fn backup_local_transactions_task<P>(
    shutdown: reth_tasks::shutdown::GracefulShutdown,
    pool: P,
    config: LocalTransactionBackupConfig,
) where
    P: TransactionPool<Transaction: PoolTransaction<Consensus: SignedTransaction>> + Clone,
{
    let Some(transactions_path) = config.transactions_path else {
        // nothing to do
        return
    };

    if let Err(err) = load_and_reinsert_transactions(pool.clone(), &transactions_path).await {
        error!(target: "txpool", "{}", err)
    }

    let graceful_guard = shutdown.await;

    // write transactions to disk
    save_local_txs_backup(pool, &transactions_path);

    drop(graceful_guard)
}
