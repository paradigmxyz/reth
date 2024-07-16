//! Contains RPC handler implementations specific to tracing.

use crate::EthApi;
use reth_evm::ConfigureEvmCommit;
use reth_rpc_eth_api::helpers::{LoadState, Trace};

impl<Provider, Pool, Network, EvmConfig> Trace for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState,
    EvmConfig: ConfigureEvmCommit,
{
    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvmCommit {
        self.inner.evm_config()
    }
}
