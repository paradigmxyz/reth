use futures::StreamExt;
use reth::transaction_pool::{Pool, PoolConfig, TransactionPoolExt};
use reth::maintain::maintain_transaction_pool_future;
use reth_provider::{CanonStateNotification, StateProviderFactory, BlockReaderIdExt};

async fn stream_mempool<Client>(client: Client, pool: Pool)
where
    Client: StateProviderFactory + BlockReaderIdExt + Send + 'static,
{
    // Create a stream of transaction events
    let mut stream = pool.transactions_listener();

    // Maintain the state of the transaction pool
    let maintain_future = maintain_transaction_pool_future(client, pool, stream);

    // Spawn the maintain future
    tokio::spawn(maintain_future);

    // Stream the mempool
    while let Some(event) = stream.next().await {
        println!("New transaction: {:?}", event);
    }
}
