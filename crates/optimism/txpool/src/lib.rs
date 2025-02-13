//! OP-Reth Transaction pool.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(all(not(test), feature = "optimism"), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

mod validator;
pub use validator::{OpL1BlockInfo, OpTransactionValidator};

pub mod conditional;
mod transaction;
pub use transaction::OpPooledTransaction;

use reth_transaction_pool::{CoinbaseTipOrdering, Pool, TransactionValidationTaskExecutor};

/// Type alias for default optimism transaction pool
pub type OpTransactionPool<Client, S, T = OpPooledTransaction> = Pool<
    TransactionValidationTaskExecutor<OpTransactionValidator<Client, T>>,
    CoinbaseTipOrdering<T>,
    S,
>;
