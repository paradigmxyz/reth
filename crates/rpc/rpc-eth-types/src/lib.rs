//! Reth RPC server types, used in server implementation of `eth` namespace API.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

pub mod builder;
pub mod cache;
pub mod error;
pub mod fee_history;
pub mod gas_oracle;
pub mod id_provider;
pub mod logs_utils;
pub mod pending_block;
pub mod receipt;
pub mod revm_utils;
pub mod simulate;
pub mod transaction;
pub mod utils;

pub use builder::{
    config::{EthConfig, EthFilterConfig},
    ctx::EthApiBuilderCtx,
};
pub use cache::{
    config::EthStateCacheConfig, db::StateCacheDb, multi_consumer::MultiConsumerLruCache,
    EthStateCache,
};
pub use error::{EthApiError, EthResult, RevertError, RpcInvalidTransactionError, SignError};
pub use fee_history::{FeeHistoryCache, FeeHistoryCacheConfig, FeeHistoryEntry};
pub use gas_oracle::{
    GasCap, GasPriceOracle, GasPriceOracleConfig, GasPriceOracleResult, RPC_DEFAULT_GAS_CAP,
};
pub use id_provider::EthSubscriptionIdProvider;
pub use pending_block::{PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin};
pub use receipt::ReceiptBuilder;
pub use transaction::TransactionSource;
