//! Reth RPC `eth_` API implementation
//!
//! ## Feature Flags
//!
//! - `client`: Enables JSON-RPC client support.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod api;
pub mod cache;
pub mod error;
pub mod fee_history;
pub mod gas_oracle;
pub mod id_provider;
pub mod logs_utils;
pub mod pending_block;
pub mod receipt;
pub mod result;
pub mod revm_utils;
pub mod transaction;
pub mod utils;

#[cfg(feature = "client")]
pub use api::{
    bundle::{EthBundleApiClient, EthCallBundleApiClient},
    filter::EthFilterApiClient,
    EthApiClient,
};
pub use api::{
    bundle::{EthBundleApiServer, EthCallBundleApiServer},
    filter::EthFilterApiServer,
    pubsub::EthPubSubApiServer,
    servers::{
        self,
        bundle::EthBundle,
        filter::{EthFilter, EthFilterConfig},
        pubsub::EthPubSub,
        EthApi,
    },
    EthApiServer, FullEthApiServer,
};
pub use cache::{
    config::EthStateCacheConfig, db::StateCacheDb, multi_consumer::MultiConsumerLruCache,
    EthStateCache,
};
pub use error::{EthApiError, EthResult, RevertError, RpcInvalidTransactionError, SignError};
pub use fee_history::{FeeHistoryCache, FeeHistoryCacheConfig, FeeHistoryEntry};
pub use gas_oracle::{
    GasCap, GasPriceOracle, GasPriceOracleConfig, GasPriceOracleResult, ESTIMATE_GAS_ERROR_RATIO,
    MIN_TRANSACTION_GAS, RPC_DEFAULT_GAS_CAP,
};
pub use id_provider::EthSubscriptionIdProvider;
pub use pending_block::{PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin};
pub use receipt::ReceiptBuilder;
pub use result::ToRpcResult;
pub use transaction::TransactionSource;
