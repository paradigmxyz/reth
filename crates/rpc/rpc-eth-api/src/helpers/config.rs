//! Loads chain configuration.

use alloy_consensus::BlockHeader as _;
use alloy_eips::eip7910::{EthBaseForkConfig, EthConfig, EthForkConfig, SystemContract};
use alloy_evm::precompiles::Precompile;
use alloy_primitives::Address;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks, Hardforks};
use reth_errors::{RethError, RethResult};
use reth_evm::{precompiles::PrecompilesMap, ConfigureEvm};
use reth_storage_api::{BlockNumReader, BlockReaderIdExt};
use revm::precompile::PrecompileId;
use std::{borrow::Borrow, collections::BTreeMap};

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

        let mut config = EthConfig { current, next: None, last: None };

        if let Some(last_fork_idx) = current_fork_idx.checked_sub(1) {
            if let Some(last_fork_timestamp) = fork_timestamps.get(last_fork_idx).copied() {
                if let Some(last_base_config) =
                    chain_spec.fork_config_at_timestamp(last_fork_timestamp)
                {
                    let last_precompiles =
                        evm_config.precompiles_for_timestamp(last_fork_timestamp);
                    let last = base_to_fork_config(&chain_spec, last_base_config, last_precompiles);
                    config.last = Some(last);
                }
            }
        }

        if let Some(next_fork_timestamp) = fork_timestamps.get(current_fork_idx + 1).copied() {
            if let Some(next_base_config) = chain_spec.fork_config_at_timestamp(next_fork_timestamp)
            {
                let next_precompiles = evm_config.precompiles_for_timestamp(next_fork_timestamp);
                let next = base_to_fork_config(&chain_spec, next_base_config, next_precompiles);
                config.next = Some(next);
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
    let mut system_contracts = BTreeMap::<SystemContract, Address>::default();

    if chain_spec.is_cancun_active_at_timestamp(config.activation_time) {
        system_contracts.extend(SystemContract::cancun());
    }

    if chain_spec.is_prague_active_at_timestamp(config.activation_time) {
        system_contracts
            .extend(SystemContract::prague(chain_spec.deposit_contract().map(|c| c.address)));
    }

    let precompiles = precompiles
        .addresses()
        .filter_map(|address| {
            Some((precompile_to_str(precompiles.get(address)?.precompile_id()), *address))
        })
        .collect();

    EthForkConfig {
        activation_time: config.activation_time,
        blob_schedule: config.blob_schedule,
        chain_id: config.chain_id,
        fork_id: config.fork_id,
        precompiles,
        system_contracts,
    }
}

// TODO: move
fn precompile_to_str(id: &PrecompileId) -> String {
    let str = match id {
        PrecompileId::EcRec => "ECREC",
        PrecompileId::Sha256 => "SHA256",
        PrecompileId::Ripemd160 => "RIPEMD160",
        PrecompileId::Identity => "ID",
        PrecompileId::ModExp => "MODEXP",
        PrecompileId::Bn128Add => "BN254_ADD",
        PrecompileId::Bn128Mul => "BN254_MUL",
        PrecompileId::Bn128Pairing => "BN254_PAIRING",
        PrecompileId::Blake2F => "BLAKE2F",
        PrecompileId::KzgPointEvaluation => "KZG_POINT_EVALUATION",
        PrecompileId::Bls12G1Add => "BLS12_G1ADD",
        PrecompileId::Bls12G1Msm => "BLS12_G1MSM",
        PrecompileId::Bls12G2Add => "BLS12_G2ADD",
        PrecompileId::Bls12G2Msm => "BLS12_G2MSM",
        PrecompileId::Bls12Pairing => "BLS12_PAIRING_CHECK",
        PrecompileId::Bls12MapFpToGp1 => "BLS12_MAP_FP_TO_G1",
        PrecompileId::Bls12MapFp2ToGp2 => "BLS12_MAP_FP2_TO_G2",
        PrecompileId::P256Verify => "P256_VERIFY",
        PrecompileId::Custom(custom) => custom.borrow(),
    };
    str.to_owned()
}
