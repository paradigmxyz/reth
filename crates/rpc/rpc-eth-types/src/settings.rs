//! Settings for initializing and serving the `eth` RPC API.

use crate::{builder::config::PendingBlockKind, RPC_DEFAULT_GAS_CAP};
use reth_rpc_server_types::constants::{
    DEFAULT_ETH_PROOF_WINDOW, DEFAULT_MAX_BLOCKING_IO_REQUEST, DEFAULT_MAX_SIMULATE_BLOCKS,
    DEFAULT_PROOF_PERMITS, RPC_DEFAULT_SEND_RAW_TX_SYNC_TIMEOUT_SECS,
};
use std::time::Duration;

/// Settings used when initializing the API and serving `eth` RPC requests.
///
/// These settings are shared by the API's helper traits so additional settings can be exposed
/// without adding individual trait methods. Concurrency and batching limits take effect when the
/// API is constructed.
///
/// Configure individual settings with [`Self::default`] and the `with_*` methods.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EthApiSettings {
    /// Maximum number of concurrent proof requests.
    pub proof_permits: usize,
    /// Maximum batch size for transaction insertions.
    pub max_batch_size: usize,
    /// Maximum number of concurrent blocking IO requests.
    pub max_blocking_io_requests: usize,

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

impl EthApiSettings {
    /// Sets the maximum number of concurrent proof requests.
    #[must_use]
    pub const fn with_proof_permits(mut self, proof_permits: usize) -> Self {
        self.proof_permits = proof_permits;
        self
    }

    /// Sets the maximum batch size for transaction insertions.
    #[must_use]
    pub const fn with_max_batch_size(mut self, max_batch_size: usize) -> Self {
        self.max_batch_size = max_batch_size;
        self
    }

    /// Sets the maximum number of concurrent blocking IO requests.
    #[must_use]
    pub const fn with_max_blocking_io_requests(mut self, max_blocking_io_requests: usize) -> Self {
        self.max_blocking_io_requests = max_blocking_io_requests;
        self
    }

    /// Sets the maximum gas limit for calls and tracing.
    #[must_use]
    pub const fn with_gas_cap(mut self, gas_cap: u64) -> Self {
        self.gas_cap = gas_cap;
        self
    }

    /// Sets the maximum number of blocks for `eth_simulateV1`.
    #[must_use]
    pub const fn with_max_simulate_blocks(mut self, max_simulate_blocks: u64) -> Self {
        self.max_simulate_blocks = max_simulate_blocks;
        self
    }

    /// Sets whether `eth_simulateV1` computes state roots.
    #[must_use]
    pub const fn with_compute_state_root_for_eth_simulate(
        mut self,
        compute_state_root_for_eth_simulate: bool,
    ) -> Self {
        self.compute_state_root_for_eth_simulate = compute_state_root_for_eth_simulate;
        self
    }

    /// Sets the maximum number of blocks into the past for state proofs.
    #[must_use]
    pub const fn with_eth_proof_window(mut self, eth_proof_window: u64) -> Self {
        self.eth_proof_window = eth_proof_window;
        self
    }

    /// Sets the pending block construction mode.
    #[must_use]
    pub const fn with_pending_block_kind(mut self, pending_block_kind: PendingBlockKind) -> Self {
        self.pending_block_kind = pending_block_kind;
        self
    }

    /// Sets the timeout for `send_raw_transaction_sync`.
    #[must_use]
    pub const fn with_send_raw_transaction_sync_timeout(
        mut self,
        send_raw_transaction_sync_timeout: Duration,
    ) -> Self {
        self.send_raw_transaction_sync_timeout = send_raw_transaction_sync_timeout;
        self
    }

    /// Sets the maximum EVM memory per RPC request.
    #[must_use]
    pub const fn with_evm_memory_limit(mut self, evm_memory_limit: u64) -> Self {
        self.evm_memory_limit = evm_memory_limit;
        self
    }

    /// Sets whether to force blob sidecar upcasting when Osaka is active.
    #[must_use]
    pub const fn with_force_blob_sidecar_upcasting(
        mut self,
        force_blob_sidecar_upcasting: bool,
    ) -> Self {
        self.force_blob_sidecar_upcasting = force_blob_sidecar_upcasting;
        self
    }
}

impl Default for EthApiSettings {
    fn default() -> Self {
        Self {
            proof_permits: DEFAULT_PROOF_PERMITS,
            max_batch_size: 1,
            max_blocking_io_requests: DEFAULT_MAX_BLOCKING_IO_REQUEST,
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
