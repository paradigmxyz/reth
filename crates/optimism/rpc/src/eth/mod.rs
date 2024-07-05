//! OP-Reth `eth_` endpoint implementation.

use alloy_primitives::{Address, U64};
use reth_chainspec::ChainInfo;
use reth_errors::RethResult;
use reth_rpc_eth_api::helpers::EthApiSpec;
use reth_rpc_types::SyncStatus;
use std::future::Future;

/// OP-Reth `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
///
/// This wraps a default `Eth` implementation, and provides additional functionality where the
/// optimism spec deviates from the default (ethereum) spec, e.g. transaction forwarding to the
/// sequencer, receipts, additional RPC fields for transaction receipts.
///
/// This type implements the [`FullEthApi`](reth_rpc_eth_api::helpers::FullEthApi) by implemented
/// all the `Eth` helper traits and prerequisite traits.
#[derive(Debug, Clone)]
pub struct OpEthApi<Eth> {
    inner: Eth,
}

impl<Eth> OpEthApi<Eth> {
    /// Creates a new `OpEthApi` from the provided `Eth` implementation.
    pub const fn new(inner: Eth) -> Self {
        Self { inner }
    }
}

impl<Eth: EthApiSpec> EthApiSpec for OpEthApi<Eth> {
    fn protocol_version(&self) -> impl Future<Output = RethResult<U64>> + Send {
        self.inner.protocol_version()
    }

    fn chain_id(&self) -> U64 {
        self.inner.chain_id()
    }

    fn chain_info(&self) -> RethResult<ChainInfo> {
        self.inner.chain_info()
    }

    fn accounts(&self) -> Vec<Address> {
        self.inner.accounts()
    }

    fn is_syncing(&self) -> bool {
        self.inner.is_syncing()
    }

    fn sync_status(&self) -> RethResult<SyncStatus> {
        self.inner.sync_status()
    }
}
