use crate::primitives::WormholeTransactionSigned;
use reth_transaction_pool::{CoinbaseTipOrdering, Pool, TransactionValidationTaskExecutor};

pub use reth_optimism_txpool::OpPooledTransaction;

/// Wormhole pooled transaction type
pub type WormholePooledTransaction = OpPooledTransaction<WormholeTransactionSigned>;

/// Wormhole transaction pool type
pub type WormholeTransactionPool<Client, S> = Pool<
    TransactionValidationTaskExecutor<
        reth_optimism_txpool::OpTransactionValidator<Client, WormholePooledTransaction>,
    >,
    CoinbaseTipOrdering<WormholePooledTransaction>,
    S,
>;
