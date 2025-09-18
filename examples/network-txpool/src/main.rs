//! Example of how to use the network as a standalone component together with a transaction pool and
//! a custom pool validator.
//!
//! Run with
//!
//! ```sh
//! cargo run --release -p network-txpool -- node
//! ```

#![warn(unused_crate_dependencies)]

use reth_ethereum::{
    network::{config::rng_secret_key, EthNetworkPrimitives, NetworkConfig, NetworkManager},
    pool::{
        blobstore::InMemoryBlobStore, test_utils::OkValidator, CoinbaseTipOrdering,
        EthPooledTransaction, Pool, TransactionListenerKind, TransactionPool,
    },
    provider::test_utils::NoopProvider,
};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // This block provider implementation is used for testing purposes.
    // NOTE: This also means that we don't have access to the blockchain and are not able to serve
    // any requests for headers or bodies which can result in dropped connections initiated by
    // remote or able to validate transaction against the latest state.
    let client = NoopProvider::default();

    let pool: Pool<
        OkValidator<EthPooledTransaction>,
        CoinbaseTipOrdering<EthPooledTransaction>,
        InMemoryBlobStore,
    > = reth_ethereum::pool::Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // The key that's used for encrypting sessions and to identify our node.
    let local_key = rng_secret_key();

    // Configure the network
    let config = NetworkConfig::<_, EthNetworkPrimitives>::builder(local_key)
        .mainnet_boot_nodes()
        .build(client);
    let transactions_manager_config = config.transactions_manager_config.clone();
    // create the network instance
    let (_handle, network, txpool, _) = NetworkManager::builder(config)
        .await?
        .transactions(pool.clone(), transactions_manager_config)
        .split_with_handle();

    // this can be used to interact with the `txpool` service directly
    let _txs_handle = txpool.handle();

    // spawn the network task
    tokio::task::spawn(network);
    // spawn the pool task
    tokio::task::spawn(txpool);

    // listen for new transactions
    let mut txs = pool.pending_transactions_listener_for(TransactionListenerKind::All);

    while let Some(tx) = txs.recv().await {
        println!("Received new transaction: {tx:?}");
    }

    Ok(())
}
