//! `eth` namespace handler implementation.

mod api;
pub mod cache;
pub mod error;
mod filter;
mod id_provider;
mod logs_utils;
mod pubsub;
pub(crate) mod revm_utils;
mod signer;
pub(crate) mod utils;

pub use api::{EthApi, EthApiSpec, EthTransactions, TransactionSource};
pub use filter::EthFilter;
pub use id_provider::EthSubscriptionIdProvider;
pub use pubsub::EthPubSub;

/// The block number at which Byzantium fork is enabled.
pub const BYZANTIUM_FORK_BLKNUM: u64 = 4370000;
