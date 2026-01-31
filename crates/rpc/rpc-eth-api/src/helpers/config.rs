//! Loads chain configuration.

use alloy_consensus::BlockHeader;
use alloy_eips::{
    eip7840::BlobParams,
    eip7910::{EthConfig, EthForkConfig, SystemContract},
};
use alloy_evm::precompiles::Precompile;
use alloy_primitives::Address;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks, Hardforks, Head};
use reth_errors::{ProviderError, RethError};
use reth_evm::{precompiles::PrecompilesMap, ConfigureEvm, Evm};
use reth_node_api::NodePrimitives;
use reth_primitives_traits::header::HeaderMut;
use reth_revm::db::EmptyDB;
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::BlockReaderIdExt;
use std::collections::BTreeMap;

/// RPC endpoint support for [EIP-7910](https://eips.ethereum.org/EIPS/eip-7910)
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait EthConfigApi {
    /// Returns an object with data about recent and upcoming fork configurations.
    #[method(name = "config")]
    fn config(&self) -> RpcResult<EthConfig>;
}

/// Handler for the `eth_config` RPC endpoint.
///
/// Ref: <https://eips.ethereum.org/EIPS/eip-7910>
#[derive(Debug, Clone)]
pub struct EthConfigHandler<Provider, Evm> {
    provider: Provider,
    evm_config: Evm,
}

impl<Provider, Evm> EthConfigHandler<Provider, Evm>
where
    Provider: ChainSpecProvider<ChainSpec: Hardforks + EthereumHardforks>
        + BlockReaderIdExt<Header: HeaderMut>
        + 'static,
    Evm: ConfigureEvm<Primitives: NodePrimitives<BlockHeader = Provider::Header>> + 'static,
{
    /// Creates a new [`EthConfigHandler`].
    pub const fn new(provider: Provider, evm_config: Evm) -> Self {
        Self { provider, evm_config }
    }

    /// Returns fork config for specific timestamp.
    fn build_fork_config_at(
        &self,
        timestamp: u64,
        precompiles: BTreeMap<String, Address>,
    ) -> EthForkConfig {
        let chain_spec = self.provider.chain_spec();

        let mut system_contracts = BTreeMap::<SystemContract, Address>::default();

        if chain_spec.is_cancun_active_at_timestamp(timestamp) {
            system_contracts.extend(SystemContract::cancun());
        }

        if chain_spec.is_prague_active_at_timestamp(timestamp) {
            system_contracts
                .extend(SystemContract::prague(chain_spec.deposit_contract().map(|c| c.address)));
        }

        // Fork config only exists for timestamp-based hardforks.
        let fork_id = chain_spec
            .fork_id(&Head { timestamp, number: u64::MAX, ..Default::default() })
            .hash
            .0
            .into();

        EthForkConfig {
            activation_time: timestamp,
            blob_schedule: chain_spec
                .blob_params_at_timestamp(timestamp)
                // no blob support, so we set this to original cancun values as defined in eip-4844
                .unwrap_or_else(BlobParams::cancun),
            chain_id: chain_spec.chain().id(),
            fork_id,
            precompiles,
            system_contracts,
        }
    }

    fn config(&self) -> Result<EthConfig, RethError> {
        let chain_spec = self.provider.chain_spec();
        let latest = self
            .provider
            .latest_header()?
            .ok_or_else(|| ProviderError::BestBlockNotFound)?
            .into_header();

        let current_precompiles = evm_to_precompiles_map(
            self.evm_config.evm_for_block(EmptyDB::default(), &latest).map_err(RethError::other)?,
        );

        let mut fork_timestamps =
            chain_spec.forks_iter().filter_map(|(_, cond)| cond.as_timestamp()).collect::<Vec<_>>();
        fork_timestamps.sort_unstable();
        fork_timestamps.dedup();

        let (current_fork_idx, current_fork_timestamp) = fork_timestamps
            .iter()
            .position(|ts| &latest.timestamp() < ts)
            .and_then(|idx| idx.checked_sub(1))
            .or_else(|| fork_timestamps.len().checked_sub(1))
            .and_then(|idx| fork_timestamps.get(idx).map(|ts| (idx, *ts)))
            .ok_or_else(|| RethError::msg("no active timestamp fork found"))?;

        let current = self.build_fork_config_at(current_fork_timestamp, current_precompiles);

        let mut config = EthConfig { current, next: None, last: None };

        if let Some(next_fork_timestamp) = fork_timestamps.get(current_fork_idx + 1).copied() {
            let fake_header = {
                let mut header = latest.clone();
                header.set_timestamp(next_fork_timestamp);
                header
            };
            let next_precompiles = evm_to_precompiles_map(
                self.evm_config
                    .evm_for_block(EmptyDB::default(), &fake_header)
                    .map_err(RethError::other)?,
            );

            config.next = Some(self.build_fork_config_at(next_fork_timestamp, next_precompiles));
        } else {
            // If there is no fork scheduled, there is no "last" or "final" fork scheduled.
            return Ok(config);
        }

        let last_fork_timestamp = fork_timestamps.last().copied().unwrap();
        let fake_header = {
            let mut header = latest;
            header.set_timestamp(last_fork_timestamp);
            header
        };
        let last_precompiles = evm_to_precompiles_map(
            self.evm_config
                .evm_for_block(EmptyDB::default(), &fake_header)
                .map_err(RethError::other)?,
        );

        config.last = Some(self.build_fork_config_at(last_fork_timestamp, last_precompiles));

        Ok(config)
    }
}

impl<Provider, Evm> EthConfigApiServer for EthConfigHandler<Provider, Evm>
where
    Provider: ChainSpecProvider<ChainSpec: Hardforks + EthereumHardforks>
        + BlockReaderIdExt<Header: HeaderMut>
        + 'static,
    Evm: ConfigureEvm<Primitives: NodePrimitives<BlockHeader = Provider::Header>> + 'static,
{
    fn config(&self) -> RpcResult<EthConfig> {
        Ok(self.config().map_err(EthApiError::from)?)
    }
}

fn evm_to_precompiles_map(
    evm: impl Evm<Precompiles = PrecompilesMap>,
) -> BTreeMap<String, Address> {
    let precompiles = evm.precompiles();
    precompiles
        .addresses()
        .filter_map(|address| {
            Some((precompiles.get(address)?.precompile_id().name().to_string(), *address))
        })
        .collect()
}
