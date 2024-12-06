//! Contains RPC handler implementations specific to tracing.

use alloy_consensus::Header;
use reth_evm::ConfigureEvm;
use reth_provider::BlockReader;
use reth_rpc_eth_api::helpers::{LoadState, Trace};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> Trace for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState<Provider: BlockReader, Evm: ConfigureEvm<Header = Header>>,
    Provider: BlockReader,
{
}
