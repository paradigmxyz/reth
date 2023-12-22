//! `eth` namespace handler implementation.

mod api;
pub mod bundle;
pub mod cache;
pub mod error;
mod filter;
pub mod gas_oracle;
mod id_provider;
mod logs_utils;
mod pubsub;
pub mod revm_utils;
mod signer;
pub(crate) mod utils;

pub use api::{
    fee_history::{fee_history_cache_new_blocks_task, FeeHistoryCache, FeeHistoryCacheConfig},
    EthApi, EthApiSpec, EthTransactions, TransactionSource, RPC_DEFAULT_GAS_CAP,
};

pub use bundle::EthBundle;
pub use filter::{EthFilter, EthFilterConfig};
pub use id_provider::EthSubscriptionIdProvider;
pub use pubsub::EthPubSub;
