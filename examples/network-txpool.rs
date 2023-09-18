//! Example of how to use the network as a standalone component together with a transaction pool and
//! a custom pool validator.
//!
//! Run with
//!
//! ```not_rust
//! cargo run --example network-txpool
//! ```

use reth_network::{config::rng_secret_key, NetworkConfig, NetworkManager};
use reth_provider::test_utils::NoopProvider;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, validate::ValidTransaction, CoinbaseTipOrdering,
    EthPooledTransaction, PoolTransaction, TransactionListenerKind, TransactionOrigin,
    TransactionPool, TransactionValidationOutcome, TransactionValidator,
};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // This block provider implementation is used for testing purposes.
    // NOTE: This also means that we don't have access to the blockchain and are not able to serve
    // any requests for headers or bodies which can result in dropped connections initiated by
    // remote or able to validate transaction against the latest state.
    let client = NoopProvider::default();

    let pool = reth_transaction_pool::Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // The key that's used for encrypting sessions and to identify our node.
    let local_key = rng_secret_key();

    // Configure the network
    let config = NetworkConfig::builder(local_key).mainnet_boot_nodes().build(client);

    // create the network instance
    let (_handle, network, txpool, _) =
        NetworkManager::builder(config).await?.transactions(pool.clone()).split_with_handle();

    // spawn the network task
    tokio::task::spawn(network);
    // spawn the pool task
    tokio::task::spawn(txpool);

    // listen for new transactions
    let mut txs = pool.pending_transactions_listener_for(TransactionListenerKind::All);

    while let Some(tx) = txs.recv().await {
        println!("Received new transaction: {:?}", tx);
    }

    Ok(())
}

/// A transaction validator that determines all transactions to be valid.
///
/// An actual validator impl like
/// [TransactionValidationTaskExecutor](reth_transaction_pool::TransactionValidationTaskExecutor)
/// would require up to date db access.
///
/// CAUTION: This validator is not safe to use since it doesn't actually validate the transaction's
/// properties such as chain id, balance, nonce, etc.
#[derive(Default)]
#[non_exhaustive]
struct OkValidator;

#[async_trait::async_trait]
impl TransactionValidator for OkValidator {
    type Transaction = EthPooledTransaction;

    async fn validate_transaction(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        // Always return valid
        TransactionValidationOutcome::Valid {
            balance: transaction.cost(),
            state_nonce: transaction.nonce(),
            transaction: ValidTransaction::Valid(transaction),
            propagate: false,
        }
    }
}
