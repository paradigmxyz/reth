//! Standalone crate for Optimism-specific RPC types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

use std::sync::Arc;

use reth_rpc::{
    call_impl, eth::api::EthApiInner, eth_call_impl, eth_fees_impl, eth_state_impl,
    eth_transactions_impl, load_block_impl, load_fee_impl, load_state_impl, load_transaction_impl,
    spawn_blocking_impl, trace_impl,
};

pub mod block;
pub mod error;
pub mod pending_block;
pub mod receipt;
pub mod transaction;

/// `Eth` API implementation for OP networks.
///
/// This type provides OP specific extension of default functionality for handling `eth_` related
/// requests. See [`EthApi`](reth_rpc::EthApi) for the default L1 implementation.
#[allow(missing_debug_implementations)]
#[allow(dead_code)]
#[derive(Clone)]
pub struct OptimismApi<Provider, Pool, Network, EvmConfig> {
    /// All nested fields bundled together.
    inner: Arc<EthApiInner<Provider, Pool, Network, EvmConfig>>,
}

eth_call_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
eth_fees_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
eth_state_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
eth_transactions_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);

load_block_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
load_fee_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
load_state_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
load_transaction_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);

call_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
spawn_blocking_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
trace_impl!(OptimismApi<Provider, Pool, Network, EvmConfig>);
