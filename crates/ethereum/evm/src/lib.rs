//! EVM config for vanilla Ethereum.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{borrow::Cow, sync::Arc};
use alloy_consensus::Header;
use alloy_eips::eip4895::Withdrawal;
#[cfg(feature = "std")]
use alloy_eips::Decodable2718;
use alloy_primitives::{Address, Bytes, B256};
#[cfg(feature = "std")]
use alloy_rpc_types_engine::ExecutionData;
#[cfg(feature = "jit")]
use core::any::Any;
use core::{convert::Infallible, fmt::Debug, marker::PhantomData};
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks, MAINNET};
use reth_ethereum_forks::Hardforks;
#[cfg(feature = "std")]
use reth_ethereum_primitives::TransactionSigned;
use reth_ethereum_primitives::{Block, EthPrimitives};
#[cfg(feature = "std")]
use reth_evm::{ConfigureEngineEvm, ExecutableTxIterator};
use reth_evm::{ConfigureEvm, EvmEnv, EvmEnvFor, NextBlockEnvAttributes};
#[cfg(feature = "std")]
use reth_primitives_traits::SignedTransaction;
use reth_primitives_traits::{SealedBlock, SealedHeader};
#[cfg(feature = "std")]
use reth_storage_errors::any::AnyError;

use convert::{block_env_with_blob_params, spec_id};
#[cfg(feature = "std")]
use convert::{payload_block_env, spec_id_by_timestamp_and_block_number};

/// Legacy Ethereum EVM type placeholder.
pub type EthEvm<DB = (), I = (), P = ()> = PhantomData<(DB, I, P)>;

/// Configured Ethereum EVM environment.
#[derive(Debug, Clone)]
pub struct EthEvmEnv {
    /// Active EVM spec.
    pub spec: evm2::SpecId,
    /// Runtime configuration for the active EVM spec.
    pub version: evm2::Version,
    /// EVM block environment.
    pub block: evm2::env::BlockEnv,
}

impl EthEvmEnv {
    /// Creates a new Ethereum EVM environment.
    pub const fn new(spec: evm2::SpecId, block: evm2::env::BlockEnv, chain_id: u64) -> Self {
        let mut version = evm2::Version::new(spec);
        version.chain_id = chain_id;
        Self { spec, version, block }
    }
}

impl Default for EthEvmEnv {
    fn default() -> Self {
        let spec = evm2::SpecId::default();
        Self { spec, version: evm2::Version::new(spec), block: Default::default() }
    }
}

impl EvmEnv for EthEvmEnv {
    fn block_env(&self) -> evm2::env::BlockEnv {
        self.block
    }

    fn transaction_validation_limits(&self) -> reth_evm::EvmTransactionValidationLimits {
        let tx_gas_limit_cap = if self.version.feature(evm2::EvmFeatures::EIP8037) ||
            self.version.tx_gas_limit_cap == u64::MAX
        {
            0
        } else {
            self.version.tx_gas_limit_cap
        };

        reth_evm::EvmTransactionValidationLimits {
            max_initcode_size: self.version.max_initcode_size,
            tx_gas_limit_cap,
        }
    }

    fn with_nonce_check_disabled(mut self) -> Self {
        self.version.features.remove(evm2::EvmFeatures::NONCE_CHECK);
        self
    }

    fn with_balance_check_disabled(mut self) -> Self {
        self.version.features.remove(evm2::EvmFeatures::BALANCE_CHECK);
        self
    }
}

/// Ethereum block execution context.
#[derive(Debug, Clone)]
pub struct EthBlockExecutionCtx<'a> {
    /// Optional transaction count hint.
    pub tx_count_hint: Option<usize>,
    /// Parent block hash.
    pub parent_hash: B256,
    /// Parent beacon block root.
    pub parent_beacon_block_root: Option<B256>,
    /// Ommer headers.
    pub ommers: &'a [Header],
    /// Withdrawals.
    pub withdrawals: Option<Cow<'a, [Withdrawal]>>,
    /// Extra data for the built block.
    pub extra_data: Bytes,
    /// Optional slot number for post-Amsterdam payloads.
    pub slot_number: Option<u64>,
}

impl EthBlockExecutionCtx<'_> {
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    fn block_execution_context(
        &self,
        chain_id: u64,
        deposit_contract_address: Option<Address>,
    ) -> crate::execution::BlockExecutionContext<'_> {
        crate::execution::BlockExecutionContext {
            chain_id,
            system_calls: Some(crate::execution::BlockSystemCalls {
                parent_hash: self.parent_hash,
                parent_beacon_block_root: self.parent_beacon_block_root,
            }),
            ommers: Some(self.ommers),
            withdrawals: self.withdrawals.as_deref(),
            deposit_contract_address,
        }
    }
}

/// Helper type with backwards compatible methods to obtain Ethereum executor providers.
#[doc(hidden)]
pub mod execute {
    use crate::EthEvmConfig;

    #[deprecated(note = "Use `EthEvmConfig` instead")]
    pub type EthExecutorProvider = EthEvmConfig;
}

mod build;
pub use build::EthBlockAssembler;

mod executor;
pub use executor::EthBlockExecutor;

/// Ethereum block executor and EVM factory implementations.
pub mod factory;
pub use factory::EthBlockExecutorFactory;
#[cfg(not(feature = "jit"))]
pub use factory::RethEvmFactory;
#[cfg(feature = "jit")]
pub use factory::{
    maybe_run_jit_helper, CompilationEvent, CompilationKind, CompileTimings,
    JitBackend as Evm2JitBackend, JitMetrics, JitMode, RethEvmFactory, RuntimeConfig,
    RuntimeStatsSnapshot, RuntimeTuning,
};

mod convert;
pub use convert::{EthTxEnv, ExecutableRecoveredTx};

mod execution;
pub use execution::{
    BlockExecutionContext, BlockSystemCalls, EthExecutionError, PayloadExecutionError,
};
pub use reth_evm::execute::HashedStateMode;

mod receipt;
pub use receipt::RethReceiptBuilder;

#[cfg(feature = "test-utils")]
mod test_utils;
#[cfg(feature = "test-utils")]
pub use test_utils::*;

/// Ethereum-related EVM configuration.
#[derive(Debug, Clone)]
pub struct EthEvmConfig<C = ChainSpec, EvmFactory = ()> {
    /// Inner Ethereum block executor factory.
    pub executor_factory: EthBlockExecutorFactory<C, EvmFactory>,
    /// Ethereum block assembler placeholder.
    pub block_assembler: EthBlockAssembler<C>,
}

impl EthEvmConfig {
    /// Creates a new Ethereum EVM configuration for Ethereum mainnet.
    pub fn mainnet() -> Self {
        Self::ethereum(MAINNET.clone())
    }
}

impl<ChainSpec> EthEvmConfig<ChainSpec> {
    /// Creates a new Ethereum EVM configuration with the given chain spec.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self::ethereum(chain_spec)
    }

    /// Creates a new Ethereum EVM configuration.
    pub fn ethereum(chain_spec: Arc<ChainSpec>) -> Self {
        Self::new_with_evm_factory(chain_spec, ())
    }
}

impl<ChainSpec, EvmFactory> EthEvmConfig<ChainSpec, EvmFactory> {
    /// Creates a new Ethereum EVM configuration with the given EVM factory configuration.
    pub fn new_with_evm_factory(chain_spec: Arc<ChainSpec>, evm_factory: EvmFactory) -> Self {
        Self {
            block_assembler: EthBlockAssembler::new(chain_spec.clone()),
            executor_factory: EthBlockExecutorFactory::new_with_evm_factory(
                chain_spec,
                evm_factory,
            ),
        }
    }

    /// Returns the chain spec associated with this configuration.
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        self.executor_factory.chain_spec()
    }
}

impl<ChainSpec, EvmF> ConfigureEvm for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec: EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
    EvmF: Clone + Debug + Send + Sync + Unpin + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type TxEnv = EthTxEnv;
    type BlockExecutorFactory = EthBlockExecutorFactory<ChainSpec, EvmF>;
    #[cfg(feature = "std")]
    type BlockAssembler = EthBlockAssembler<ChainSpec>;

    #[cfg(feature = "std")]
    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    #[cfg(feature = "std")]
    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn with_jit_support_enabled(self, enabled: bool) -> Self
    where
        Self: Sized,
    {
        #[cfg(feature = "jit")]
        {
            let mut this = self;
            if let Some(factory) = (this.executor_factory.evm_factory_mut() as &mut dyn Any)
                .downcast_mut::<factory::RethEvmFactory>()
            {
                factory.set_jit_support(enabled);
            }
            this
        }

        #[cfg(not(feature = "jit"))]
        {
            let _ = enabled;
            self
        }
    }

    fn jit_backend(&self) -> Option<&dyn reth_evm::JitBackend> {
        #[cfg(feature = "jit")]
        if let Some(factory) = (self.executor_factory.evm_factory() as &dyn Any)
            .downcast_ref::<factory::RethEvmFactory>()
        {
            return Some(factory);
        }

        None
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnvFor<Self>, Self::Error> {
        let spec = spec_id(self.chain_spec().as_ref(), header);
        Ok(EthEvmEnv::new(
            spec,
            block_env_with_blob_params(
                header,
                self.chain_spec().as_ref().blob_params_at_timestamp(header.timestamp),
            ),
            self.chain_spec().chain_id(),
        ))
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        let base_fee = self
            .chain_spec()
            .as_ref()
            .next_block_base_fee(parent, attributes.timestamp)
            .unwrap_or_default();
        let header = Header {
            parent_hash: parent.hash_slow(),
            beneficiary: attributes.suggested_fee_recipient,
            timestamp: attributes.timestamp,
            number: parent.number + 1,
            gas_limit: attributes.gas_limit,
            base_fee_per_gas: Some(base_fee),
            mix_hash: attributes.prev_randao,
            slot_number: attributes.slot_number,
            ..Default::default()
        };

        let spec = spec_id(self.chain_spec().as_ref(), &header);
        Ok(EthEvmEnv::new(
            spec,
            block_env_with_blob_params(
                &header,
                self.chain_spec().as_ref().blob_params_at_timestamp(attributes.timestamp),
            ),
            self.chain_spec().chain_id(),
        ))
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block>,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error>
    where
        Self: 'a,
    {
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

    fn context_for_next_block<'a>(
        &'a self,
        parent: &'a SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error>
    where
        Self: 'a,
    {
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

    #[cfg(feature = "std")]
    fn pre_block_state_changes<'a, DB>(
        &self,
        db: DB,
        env: EthEvmEnv,
        block_number: u64,
        ctx: EthBlockExecutionCtx<'a>,
    ) -> Result<evm2::BlockStateAccumulator, alloc::boxed::Box<dyn core::error::Error + Send + Sync>>
    where
        Self: 'a,
        DB: evm2::evm::Database + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        let mut evm = self.evm_with_env(evm2::evm::Db::new(db), env);
        crate::execution::apply_pre_execution_system_calls(
            &mut evm,
            block_number,
            ctx.block_execution_context(
                self.chain_spec().chain_id(),
                self.chain_spec().deposit_contract().map(|contract| contract.address),
            ),
        )
        .map_err(Into::into)
    }
}

#[cfg(feature = "std")]
impl<ChainSpec, EvmF> ConfigureEngineEvm<ExecutionData> for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec: EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
    EvmF: Clone + Debug + Send + Sync + Unpin + 'static,
{
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        let spec = spec_id_by_timestamp_and_block_number(
            self.chain_spec().as_ref(),
            payload.payload.timestamp(),
            payload.payload.block_number(),
        );
        Ok(EthEvmEnv::new(
            spec,
            payload_block_env(
                payload,
                self.chain_spec().as_ref().blob_params_at_timestamp(payload.payload.timestamp()),
            ),
            self.chain_spec().chain_id(),
        ))
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<reth_evm::ExecutionCtxFor<'a, Self>, Self::Error> {
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
            let tx = TransactionSigned::decode_2718_exact(tx.as_ref()).map_err(AnyError::new)?;
            let signer = tx.try_recover().map_err(AnyError::new)?;
            Ok::<_, AnyError>(ExecutableRecoveredTx::new(tx.with_signer(signer)))
        };

        Ok((txs, convert))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_genesis::Genesis;
    use alloy_primitives::U256;
    use reth_chainspec::{Chain, ChainSpec};

    #[test]
    fn test_evm_env_for_header_uses_chain_blob_params() {
        let chain_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .cancun_activated()
            .build();
        let blob_params = chain_spec.blob_params_at_timestamp(1).expect("cancun blob params");
        let excess_blob_gas = 1_000_000;
        let header =
            Header { timestamp: 1, excess_blob_gas: Some(excess_blob_gas), ..Default::default() };

        let env = EthEvmConfig::new(Arc::new(chain_spec)).evm_env(&header).unwrap().block;

        assert_eq!(env.blob_basefee, U256::from(blob_params.calc_blob_fee(excess_blob_gas)));
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_support_updates_reth_factory() {
        let evm_config =
            EthEvmConfig::new_with_evm_factory(MAINNET.clone(), RethEvmFactory::disabled());

        assert!(evm_config.jit_backend().is_some());
        assert!(!evm_config.executor_factory.evm_factory().jit_support_enabled());

        let evm_config = evm_config.with_jit_support();
        assert!(evm_config.executor_factory.evm_factory().jit_support_enabled());

        let evm_config = evm_config.with_jit_support_enabled(false);
        assert!(!evm_config.executor_factory.evm_factory().jit_support_enabled());
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_support_ignores_plain_factory() {
        let evm_config = EthEvmConfig::mainnet();

        assert!(evm_config.jit_backend().is_none());

        let evm_config = evm_config.with_jit_support();
        assert!(evm_config.jit_backend().is_none());
    }
}
