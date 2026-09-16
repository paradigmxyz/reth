//! Runtime settings shared by `eth` RPC helpers.

use crate::{builder::config::PendingBlockKind, RPC_DEFAULT_GAS_CAP};
use reth_rpc_server_types::constants::{
    DEFAULT_ETH_PROOF_WINDOW, DEFAULT_MAX_SIMULATE_BLOCKS,
    RPC_DEFAULT_SEND_RAW_TX_SYNC_TIMEOUT_SECS,
};
use std::time::Duration;

/// Settings used when serving `eth` RPC requests.
///
/// These settings are shared by the API's helper traits so additional settings can be exposed
/// without adding individual trait methods.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EthApiSettings {
    /// Maximum gas limit for `eth_call` and call tracing RPC methods.
    pub gas_cap: u64,
    /// Maximum number of blocks for `eth_simulateV1`.
    pub max_simulate_blocks: u64,
    /// Whether to compute state roots for `eth_simulateV1`.
    pub compute_state_root_for_eth_simulate: bool,
    /// Maximum number of blocks into the past for generating state proofs.
    pub eth_proof_window: u64,
    /// Configuration for pending block construction.
    pub pending_block_kind: PendingBlockKind,
    /// Timeout duration for `send_raw_transaction_sync` RPC method.
    pub send_raw_transaction_sync_timeout: Duration,
    /// Maximum memory the EVM can allocate per RPC request.
    pub evm_memory_limit: u64,
    /// Whether to force upcasting EIP-4844 blob sidecars to EIP-7594 format when Osaka is active.
    pub force_blob_sidecar_upcasting: bool,
}

impl Default for EthApiSettings {
    fn default() -> Self {
        Self {
            gas_cap: RPC_DEFAULT_GAS_CAP.into(),
            max_simulate_blocks: DEFAULT_MAX_SIMULATE_BLOCKS,
            compute_state_root_for_eth_simulate: false,
            eth_proof_window: DEFAULT_ETH_PROOF_WINDOW,
            pending_block_kind: PendingBlockKind::default(),
            send_raw_transaction_sync_timeout: RPC_DEFAULT_SEND_RAW_TX_SYNC_TIMEOUT_SECS,
            evm_memory_limit: (1 << 32) - 1,
            force_blob_sidecar_upcasting: false,
        }
    }
}
