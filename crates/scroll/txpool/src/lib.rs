//! Transaction pool for Scroll node.

mod transaction;
pub use transaction::ScrollPooledTransaction;

mod validator;
pub use validator::{ScrollL1BlockInfo, ScrollTransactionValidator};

use reth_transaction_pool::{CoinbaseTipOrdering, Pool, TransactionValidationTaskExecutor};

/// Type alias for default scroll transaction pool
pub type ScrollTransactionPool<Client, S, T = ScrollPooledTransaction> = Pool<
    TransactionValidationTaskExecutor<ScrollTransactionValidator<Client, T>>,
    CoinbaseTipOrdering<T>,
    S,
>;
