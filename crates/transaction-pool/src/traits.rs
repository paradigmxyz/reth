/// General purpose abstraction fo a transaction-pool
#[async_trait::async_trait]
pub trait TransactionPool: Send + Sync {
    // TODO probably need associated `Transaction` type here
    // TODO needs transaction type

    // TODO add interfaces for adding new transactions
}
