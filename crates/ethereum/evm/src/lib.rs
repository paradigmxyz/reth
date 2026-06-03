//! EVM config for vanilla ethereum.
//!
//! # EVM features
//!
//! This crate does __not__ enforce specific EVM features such as `blst` or `c-kzg`, which are
//! critical for EVM internals, it is the responsibility of the implementer to ensure the
//! proper features are selected.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{
    borrow::Cow,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use alloy_consensus::{BlockHeader, Header};
use core::convert::Infallible;
use reth_chainspec::{ChainSpec, EthChainSpec, EthExecutorSpec, MAINNET};
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm::{
    context::{BlobExcessGasAndPrice, BlockEnv, CfgEnv},
    eth::EthBlockExecutionCtx,
    evm2::{
        precompiles_for_spec, spec_id_from_revm, Evm2RethBlockExecutorFactory,
        RethEvm2ReceiptBuilder,
    },
    hardfork::SpecId,
    ConfigureEvm, EvmEnv, NextBlockEnvAttributes,
};
use reth_primitives_traits::{SealedBlock, SealedHeader};

#[cfg(feature = "std")]
use reth_evm::{ConfigureEngineEvm, ExecutableTxIterator};
#[allow(unused_imports)]
use {
    alloy_eips::Decodable2718,
    alloy_primitives::{Address, Bytes, U256},
    alloy_rpc_types_engine::ExecutionData,
    reth_chainspec::EthereumHardforks,
    reth_evm::{EvmEnvFor, ExecutionCtxFor},
    reth_primitives_traits::{constants::MAX_TX_GAS_LIMIT_OSAKA, SignedTransaction, TxTy},
    reth_storage_errors::any::AnyError,
};

mod config;
pub use config::{revm_spec, revm_spec_by_timestamp_and_block_number};
use reth_ethereum_forks::{EthereumHardfork, Hardforks};
pub use reth_evm::EthEvm;

mod build;
pub use build::EthBlockAssembler;

#[cfg(feature = "test-utils")]
mod test_utils;
#[cfg(feature = "test-utils")]
pub use test_utils::*;

/// Ethereum-related EVM configuration backed by evm2 execution.
#[derive(Debug, Clone)]
pub struct EthEvm2Config<C = ChainSpec> {
    /// Inner evm2 [`BlockExecutorFactory`](reth_evm::execute::BlockExecutorFactory).
    pub executor_factory: Evm2RethBlockExecutorFactory<RethEvm2ReceiptBuilder, Arc<C>>,
    /// Ethereum block assembler.
    pub block_assembler: EthBlockAssembler<C>,
}

impl EthEvm2Config {
    /// Creates a new evm2 Ethereum EVM configuration for ethereum mainnet.
    pub fn mainnet() -> Self {
        Self::ethereum(MAINNET.clone())
    }
}

impl<ChainSpec> EthEvm2Config<ChainSpec>
where
    ChainSpec: Hardforks,
{
    /// Creates a new evm2 Ethereum EVM configuration with the given chain spec.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self::ethereum(chain_spec)
    }

    /// Creates a new evm2 Ethereum EVM configuration.
    pub fn ethereum(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            block_assembler: EthBlockAssembler::new(chain_spec.clone()),
            executor_factory: Evm2RethBlockExecutorFactory::new(
                RethEvm2ReceiptBuilder,
                chain_spec.clone(),
            )
            .with_dao_fork_block(chain_spec.fork(EthereumHardfork::Dao).block_number()),
        }
    }

    /// Returns the chain spec associated with this configuration.
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        self.executor_factory.spec()
    }
}

impl<ChainSpec> ConfigureEvm for EthEvm2Config<ChainSpec>
where
    ChainSpec: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory =
        Evm2RethBlockExecutorFactory<RethEvm2ReceiptBuilder, Arc<ChainSpec>>;
    type BlockAssembler = EthBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnv<SpecId>, Self::Error> {
        Ok(EvmEnv::for_eth_block(
            header,
            self.chain_spec(),
            self.chain_spec().chain().id(),
            self.chain_spec().blob_params_at_timestamp(header.timestamp),
        ))
    }

    fn precompiles(&self, header: &Header) -> Result<Vec<(String, Address)>, Self::Error> {
        let evm_env = self.evm_env(header)?;
        let precompiles = precompiles_for_spec(spec_id_from_revm(evm_env.cfg_env.spec));
        let precompiles = precompiles.as_map();
        Ok(precompiles
            .addresses()
            .filter_map(|address| {
                Some((precompiles.get(&address)?.id().name().to_string(), address))
            })
            .collect())
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnv, Self::Error> {
        let number = parent.number + 1;
        let timestamp = attributes.timestamp;
        let blob_params = self.chain_spec().blob_params_at_timestamp(timestamp);
        let spec = revm_spec_by_timestamp_and_block_number(self.chain_spec(), timestamp, number);

        let mut cfg_env = CfgEnv::new()
            .with_chain_id(self.chain_spec().chain().id())
            .with_spec_and_mainnet_gas_params(spec);

        if let Some(blob_params) = &blob_params {
            cfg_env.set_max_blobs_per_tx(blob_params.max_blobs_per_tx);
        }

        if self.chain_spec().is_osaka_active_at_timestamp(timestamp) {
            cfg_env.tx_gas_limit_cap = Some(MAX_TX_GAS_LIMIT_OSAKA);
        }

        let blob_excess_gas_and_price = parent
            .maybe_next_block_excess_blob_gas(blob_params)
            .or_else(|| blob_params.map(|_| 0))
            .zip(blob_params)
            .map(|(excess_blob_gas, params)| {
                let blob_gasprice = params.calc_blob_fee(excess_blob_gas);
                BlobExcessGasAndPrice { excess_blob_gas, blob_gasprice }
            });

        let block_env = BlockEnv {
            number: U256::from(number),
            beneficiary: attributes.suggested_fee_recipient,
            timestamp: U256::from(timestamp),
            difficulty: U256::ZERO,
            prevrandao: Some(attributes.prev_randao),
            gas_limit: attributes.gas_limit,
            basefee: self.chain_spec().next_block_base_fee(parent, timestamp).unwrap_or_default(),
            blob_excess_gas_and_price,
            slot_num: attributes.slot_number.unwrap_or_default(),
        };

        Ok(EvmEnv { cfg_env, block_env })
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block>,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error> {
        Ok(EthBlockExecutionCtx {
            tx_count_hint: Some(block.transaction_count()),
            parent_hash: block.header().parent_hash,
            parent_beacon_block_root: block.header().parent_beacon_block_root,
            ommers: &block.body().ommers,
            withdrawals: block.body().withdrawals.as_ref().map(|w| Cow::Borrowed(w.as_slice())),
            extra_data: block.header().extra_data.clone(),
            slot_number: block.header().slot_number,
        })
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<EthBlockExecutionCtx<'_>, Self::Error> {
        Ok(EthBlockExecutionCtx {
            tx_count_hint: None,
            parent_hash: parent.hash(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
            ommers: &[],
            withdrawals: attributes.withdrawals.map(|w| Cow::Owned(w.into_inner())),
            extra_data: attributes.extra_data,
            slot_number: attributes.slot_number,
        })
    }
}

#[cfg(feature = "std")]
impl<ChainSpec> ConfigureEngineEvm<ExecutionData> for EthEvm2Config<ChainSpec>
where
    ChainSpec: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
{
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        let timestamp = payload.payload.timestamp();
        let block_number = payload.payload.block_number();

        let blob_params = self.chain_spec().blob_params_at_timestamp(timestamp);
        let spec =
            revm_spec_by_timestamp_and_block_number(self.chain_spec(), timestamp, block_number);

        let mut cfg_env = CfgEnv::new()
            .with_chain_id(self.chain_spec().chain().id())
            .with_spec_and_mainnet_gas_params(spec);

        if let Some(blob_params) = &blob_params {
            cfg_env.set_max_blobs_per_tx(blob_params.max_blobs_per_tx);
        }

        if self.chain_spec().is_osaka_active_at_timestamp(timestamp) {
            cfg_env.tx_gas_limit_cap = Some(MAX_TX_GAS_LIMIT_OSAKA);
        }

        let blob_excess_gas_and_price =
            payload.payload.excess_blob_gas().zip(blob_params).map(|(excess_blob_gas, params)| {
                let blob_gasprice = params.calc_blob_fee(excess_blob_gas);
                BlobExcessGasAndPrice { excess_blob_gas, blob_gasprice }
            });

        let block_env = BlockEnv {
            number: U256::from(block_number),
            beneficiary: payload.payload.fee_recipient(),
            timestamp: U256::from(timestamp),
            difficulty: if spec >= SpecId::MERGE {
                U256::ZERO
            } else {
                payload.payload.as_v1().prev_randao.into()
            },
            prevrandao: (spec >= SpecId::MERGE).then(|| payload.payload.as_v1().prev_randao),
            gas_limit: payload.payload.gas_limit(),
            basefee: payload.payload.saturated_base_fee_per_gas(),
            blob_excess_gas_and_price,
            slot_num: payload.payload.as_v4().map(|v4| v4.slot_number).unwrap_or_default(),
        };

        Ok(EvmEnv { cfg_env, block_env })
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        Ok(EthBlockExecutionCtx {
            tx_count_hint: Some(payload.payload.transactions().len()),
            parent_hash: payload.parent_hash(),
            parent_beacon_block_root: payload.sidecar.parent_beacon_block_root(),
            ommers: &[],
            withdrawals: payload.payload.withdrawals().map(|w| Cow::Borrowed(w.as_slice())),
            extra_data: payload.payload.as_v1().extra_data.clone(),
            slot_number: payload.payload.as_v4().map(|v4| v4.slot_number),
        })
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let txs = payload.payload.transactions().clone();
        let convert = |tx: Bytes| {
            let tx =
                TxTy::<Self::Primitives>::decode_2718_exact(tx.as_ref()).map_err(AnyError::new)?;
            let signer = tx.try_recover().map_err(AnyError::new)?;
            Ok::<_, AnyError>(tx.with_signer(signer))
        };

        Ok((txs, convert))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_genesis::Genesis;
    use reth_chainspec::{Chain, ChainSpec};
    use reth_evm::{
        context::{BlockEnv, CfgEnv},
        database::EmptyDBTyped,
        execute::ProviderError,
        inspector::NoOpInspector,
        Evm, EvmEnv, State,
    };

    fn test_db() -> State<EmptyDBTyped<ProviderError>> {
        State::builder().with_database(EmptyDBTyped::<ProviderError>::default()).build()
    }

    #[test]
    fn test_fill_cfg_and_block_env() {
        // Create a default header
        let header = Header::default();

        // Build the ChainSpec for Ethereum mainnet, activating London, Paris, and Shanghai
        // hardforks
        let chain_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .london_activated()
            .paris_activated()
            .shanghai_activated()
            .build();

        // Use the `EthEvm2Config` to fill the `cfg_env` and `block_env` based on the ChainSpec,
        // Header, and total difficulty
        let EvmEnv { cfg_env, .. } =
            EthEvm2Config::new(Arc::new(chain_spec.clone())).evm_env(&header).unwrap();

        // Assert that the chain ID in the `cfg_env` is correctly set to the chain ID of the
        // ChainSpec
        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }

    #[test]
    fn test_evm_with_env_default_spec() {
        let evm_config = EthEvm2Config::mainnet();

        let db = test_db();

        let evm_env = EvmEnv::default();

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        // Check that the EVM environment
        assert_eq!(evm.block(), &evm_env.block_env);
        assert_eq!(evm.cfg_env(), &evm_env.cfg_env);
    }

    #[test]
    fn test_evm_with_env_custom_cfg() {
        let evm_config = EthEvm2Config::mainnet();

        let db = test_db();

        // Create a custom configuration environment with a chain ID of 111
        let cfg = CfgEnv::default().with_chain_id(111);

        let evm_env = EvmEnv { cfg_env: cfg.clone(), ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env);

        // Check that the EVM environment is initialized with the custom environment
        assert_eq!(evm.cfg_env(), &cfg);
    }

    #[test]
    fn test_evm_with_env_custom_block_and_tx() {
        let evm_config = EthEvm2Config::mainnet();

        let db = test_db();

        // Create customs block and tx env
        let block = BlockEnv {
            basefee: 1000,
            gas_limit: 10_000_000,
            number: U256::from(42),
            ..Default::default()
        };

        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.block(), &evm_env.block_env);

        // Default spec ID
        assert_eq!(evm.cfg_env().spec, SpecId::default());
    }

    #[test]
    fn test_evm_with_spec_id() {
        let evm_config = EthEvm2Config::mainnet();

        let db = test_db();

        let evm_env = EvmEnv {
            cfg_env: CfgEnv::new().with_spec_and_mainnet_gas_params(SpecId::PETERSBURG),
            ..Default::default()
        };

        let evm = evm_config.evm_with_env(db, evm_env);

        // Check that the spec ID is setup properly
        assert_eq!(evm.cfg_env().spec, SpecId::PETERSBURG);
    }

    #[test]
    fn test_evm_with_env_and_default_inspector() {
        let evm_config = EthEvm2Config::mainnet();
        let db = test_db();

        let evm_env = EvmEnv::default();

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Check that the EVM environment is set to default values
        assert_eq!(evm.block(), &evm_env.block_env);
        assert_eq!(evm.cfg_env(), &evm_env.cfg_env);
    }

    #[test]
    fn test_evm_with_env_inspector_and_custom_cfg() {
        let evm_config = EthEvm2Config::mainnet();
        let db = test_db();

        let cfg_env = CfgEnv::default().with_chain_id(111);
        let block = BlockEnv::default();
        let evm_env = EvmEnv { cfg_env: cfg_env.clone(), block_env: block };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env, NoOpInspector {});

        // Check that the EVM environment is set with custom configuration
        assert_eq!(evm.cfg_env(), &cfg_env);
        assert_eq!(evm.cfg_env().spec, SpecId::default());
    }

    #[test]
    fn test_evm_with_env_inspector_and_custom_block_tx() {
        let evm_config = EthEvm2Config::mainnet();
        let db = test_db();

        // Create custom block and tx environment
        let block = BlockEnv {
            basefee: 1000,
            gas_limit: 10_000_000,
            number: U256::from(42),
            ..Default::default()
        };
        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.block(), &evm_env.block_env);
        assert_eq!(evm.cfg_env().spec, SpecId::default());
    }

    #[test]
    fn test_evm_with_env_inspector_and_spec_id() {
        let evm_config = EthEvm2Config::mainnet();
        let db = test_db();

        let evm_env = EvmEnv {
            cfg_env: CfgEnv::new().with_spec_and_mainnet_gas_params(SpecId::PETERSBURG),
            ..Default::default()
        };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Check that the spec ID is set properly
        assert_eq!(evm.block(), &evm_env.block_env);
        assert_eq!(evm.cfg_env(), &evm_env.cfg_env);
        assert_eq!(evm.ctx().tx, Default::default());
    }
}
