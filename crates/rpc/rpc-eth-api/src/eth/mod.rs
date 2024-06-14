//! `eth` namespace handler implementation.

pub mod api;
pub mod cache;
pub mod error;
pub mod gas_oracle;
pub mod revm_utils;
mod signer;
pub mod utils;

pub use api::{
    fee_history::{fee_history_cache_new_blocks_task, FeeHistoryCache, FeeHistoryCacheConfig},
    EthApi, EthApiSpec, PendingBlock, TransactionSource, RPC_DEFAULT_GAS_CAP,
};

pub use signer::EthSigner;
