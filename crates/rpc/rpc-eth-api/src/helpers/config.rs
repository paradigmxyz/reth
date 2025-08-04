//! Loads chain configuration.

use alloy_consensus::BlockHeader as _;
use alloy_eips::{
    eip2124::Head,
    eip7910::{EthBaseForkConfig, EthConfig, EthForkConfig, SystemContract},
};
use alloy_primitives::Address;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks, Hardforks};
use reth_errors::{RethError, RethResult};
use reth_evm::{precompiles::PrecompilesMap, ConfigureEvm};
use reth_storage_api::{BlockNumReader, BlockReaderIdExt};
use std::collections::BTreeMap;

use crate::RpcNodeCore;

/// Chain configuration fetcher for the [`EthApiServer`](crate::EthApiServer) trait.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthConfigSpec:
    RpcNodeCore<Provider: ChainSpecProvider<ChainSpec: Hardforks + EthereumHardforks> + BlockNumReader>
{
    /// Returns provider fork configuration
    fn chain_config(&self) -> RethResult<EthConfig> {
        let chain_spec = self.provider().chain_spec();
        let evm_config = self.evm_config();
        let head_timestamp = self.provider().latest_header()?.map_or(0, |h| h.timestamp());

        // Short-circuit if Cancun is not active.
        if !chain_spec.is_cancun_active_at_timestamp(head_timestamp) {
            return Err(RethError::msg("cancun has not been activated"))
        }

        let fork_timestamps = chain_spec.fork_timestamps();
        let (current_fork_idx, current_fork_timestamp) = fork_timestamps
            .iter()
            .position(|ts| &head_timestamp < ts)
            .and_then(|idx| idx.checked_sub(1))
            .or_else(|| fork_timestamps.len().checked_sub(1))
            .and_then(|idx| fork_timestamps.get(idx).map(|ts| (idx, *ts)))
            .ok_or_else(|| RethError::msg("no active timestamp fork found"))?;

        let current_base_config = chain_spec
            .fork_config_at_timestamp(current_fork_timestamp)
            .ok_or_else(|| RethError::msg("no base config for current fork"))?;
        let current_precompiles = evm_config.precompiles_for_timestamp(current_fork_timestamp);
        let current = base_to_fork_config(&chain_spec, current_base_config, current_precompiles);
        let current_hash = current.fork_hash().map_err(RethError::other)?;
        let current_fork_id = chain_spec.fork_id(&Head {
            number: u64::MAX,
            timestamp: current_fork_timestamp,
            ..Default::default()
        });

        let mut config = EthConfig {
            current,
            current_hash,
            current_fork_id: current_fork_id.hash,
            next: None,
            next_hash: None,
            next_fork_id: None,
            last: None,
            last_hash: None,
            last_fork_id: None,
        };

        if let Some(last_fork_idx) = current_fork_idx.checked_sub(1) {
            if let Some(last_fork_timestamp) = fork_timestamps.get(last_fork_idx).copied() {
                if let Some(last_base_config) =
                    chain_spec.fork_config_at_timestamp(last_fork_timestamp)
                {
                    let last_precompiles =
                        evm_config.precompiles_for_timestamp(last_fork_timestamp);
                    let last = base_to_fork_config(&chain_spec, last_base_config, last_precompiles);
                    let last_hash = last.fork_hash().map_err(RethError::other)?;
                    let last_fork_id = chain_spec.fork_id(&Head {
                        number: u64::MAX,
                        timestamp: last_fork_timestamp,
                        ..Default::default()
                    });
                    config.last = Some(last);
                    config.last_hash = Some(last_hash);
                    config.last_fork_id = Some(last_fork_id.hash);
                }
            }
        }

        if let Some(next_fork_timestamp) = fork_timestamps.get(current_fork_idx + 1).copied() {
            if let Some(next_base_config) = chain_spec.fork_config_at_timestamp(next_fork_timestamp)
            {
                let next_precompiles = evm_config.precompiles_for_timestamp(next_fork_timestamp);
                let next = base_to_fork_config(&chain_spec, next_base_config, next_precompiles);
                let next_hash = next.fork_hash().map_err(RethError::other)?;
                let next_fork_id = chain_spec.fork_id(&Head {
                    number: u64::MAX,
                    timestamp: next_fork_timestamp,
                    ..Default::default()
                });
                config.next = Some(next);
                config.next_hash = Some(next_hash);
                config.next_fork_id = Some(next_fork_id.hash);
            }
        }

        Ok(config)
    }
}

// TODO: move
fn base_to_fork_config<ChainSpec: EthChainSpec + EthereumHardforks>(
    chain_spec: &ChainSpec,
    config: EthBaseForkConfig,
    precompiles: PrecompilesMap,
) -> EthForkConfig {
    let mut precompiles = Default::default();
    let mut system_contracts = BTreeMap::<SystemContract, Address>::default();

    if chain_spec.is_cancun_active_at_timestamp(config.activation_time) {
        system_contracts.extend(SystemContract::cancun());

        // TODO: this is ugly af and is only intended to make the impl work.
        // what we should do instead is to introduce an enum for precompiles to resolve their names.
        // precompiles.extend([
        //     (revm_precompile::secp256k1::ECRECOVER.address(), String::from("ECREC")),
        //     (), // TODO:
        // ])
    }

    if chain_spec.is_prague_active_at_timestamp(config.activation_time) {
        system_contracts
            .extend(SystemContract::prague(chain_spec.deposit_contract().map(|c| c.address)));

        // TODO: this is ugly af and is only intended to make the impl work.
        // what we should do instead is to introduce an enum for precompiles to resolve their names.
    }

    EthForkConfig {
        activation_time: config.activation_time,
        blob_schedule: config.blob_schedule,
        chain_id: config.chain_id,
        precompiles,
        system_contracts,
    }
}
