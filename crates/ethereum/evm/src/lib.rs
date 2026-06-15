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

#[cfg(feature = "std")]
use alloc::vec::Vec;
use alloc::{borrow::Cow, sync::Arc};
use alloy_consensus::{transaction::Recovered, Header};
use alloy_eips::eip4895::Withdrawal;
#[cfg(feature = "std")]
use alloy_eips::Decodable2718;
use alloy_primitives::{Bytes, B256};
#[cfg(feature = "std")]
use alloy_rpc_types_engine::ExecutionData;
use core::{convert::Infallible, fmt::Debug, marker::PhantomData};
use reth_chainspec::{ChainSpec, EthChainSpec, EthExecutorSpec, MAINNET};
use reth_ethereum_forks::{EthereumHardforks, Hardforks};
use reth_ethereum_primitives::{Block, EthPrimitives, TransactionSigned};
#[cfg(feature = "std")]
use reth_evm::{
    evm2_precompile_cache::{CachedPrecompileProvider, PrecompileCacheMap},
    ConfigureEngineEvm, ConfigureEvm2BlockExecutor, ConfigureEvm2Engine, ConfigureEvm2Prewarm,
    ExecutableTxIterator,
};
use reth_evm::{ConfigureEvm, EvmEnvFor, NextBlockEnvAttributes};
#[cfg(feature = "std")]
use reth_primitives_traits::SignedTransaction;
#[cfg(feature = "std")]
use reth_primitives_traits::{BlockBody, RecoveredBlock};
use reth_primitives_traits::{SealedBlock, SealedHeader};
#[cfg(feature = "std")]
use reth_storage_errors::any::AnyError;
#[cfg(feature = "std")]
use reth_storage_errors::provider::ProviderError;

/// Legacy Ethereum EVM type placeholder.
pub type EthEvm<DB = (), I = (), P = ()> = PhantomData<(DB, I, P)>;

/// Configured Ethereum EVM environment for the evm2 path.
#[derive(Debug, Clone, Default)]
pub struct EthEvmEnv {
    /// Active evm2 spec.
    pub spec: evm2::SpecId,
    /// evm2 block environment.
    pub block: evm2::env::BlockEnv,
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

/// Helper type with backwards compatible methods to obtain Ethereum executor providers.
#[doc(hidden)]
pub mod execute {
    use crate::EthEvmConfig;

    #[deprecated(note = "Use `EthEvmConfig` instead")]
    pub type EthExecutorProvider = EthEvmConfig;
}

mod build;
pub use build::EthBlockAssembler;

mod evm2_convert;
pub use evm2_convert::{
    evm2_block_env, evm2_block_env_with_blob_params, evm2_payload_block_env, evm2_recovered_tx,
    evm2_recovered_tx_ref, evm2_spec, evm2_spec_by_timestamp_and_block_number, Evm2RecoveredTx,
    Evm2TxEnv,
};

mod evm2_executor;
pub use evm2_executor::{
    execute_evm2_block, execute_evm2_block_with_context,
    execute_evm2_block_with_context_and_precompiles, execute_evm2_block_with_withdrawals,
    Evm2BlockExecutionContext, Evm2BlockSystemCalls, Evm2ExecutionError, Evm2HashedStateMode,
};
#[cfg(feature = "std")]
pub use evm2_executor::{
    execute_evm2_block_with_borrowed_state_provider_context,
    execute_evm2_block_with_state_provider, execute_evm2_block_with_state_provider_and_withdrawals,
    execute_evm2_block_with_state_provider_context,
    execute_evm2_block_with_state_provider_context_and_hook,
    execute_evm2_block_with_state_provider_context_precompiles_and_hook,
    execute_evm2_block_with_state_provider_context_precompiles_and_hook_envelopes,
    execute_evm2_block_with_state_provider_context_precompiles_and_hooks_envelopes,
    execute_evm2_block_with_state_provider_context_precompiles_and_hooks_envelopes_with_hashed_state_mode,
};

mod receipt;
pub use receipt::RethReceiptBuilder;

#[cfg(feature = "test-utils")]
mod test_utils;
#[cfg(feature = "test-utils")]
pub use test_utils::*;

/// Ethereum-related EVM configuration.
#[derive(Debug, Clone)]
pub struct EthEvmConfig<C = ChainSpec, EvmFactory = ()> {
    /// Ethereum block assembler placeholder.
    pub block_assembler: EthBlockAssembler<C>,
    /// Chain specification.
    pub chain_spec: Arc<C>,
    _evm_factory: PhantomData<EvmFactory>,
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
    /// Creates a new Ethereum EVM configuration with the given EVM factory placeholder.
    pub fn new_with_evm_factory(chain_spec: Arc<ChainSpec>, _evm_factory: EvmFactory) -> Self {
        Self {
            block_assembler: EthBlockAssembler::new(chain_spec.clone()),
            chain_spec,
            _evm_factory: PhantomData,
        }
    }

    /// Returns the chain spec associated with this configuration.
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }
}

impl<ChainSpec, EvmFactory> EthEvmConfig<ChainSpec, EvmFactory>
where
    ChainSpec: EthChainSpec<Header = Header> + EthereumHardforks,
{
    /// Returns the evm2 spec id for the provided block header.
    pub fn evm2_spec_for_header(&self, header: &Header) -> evm2::SpecId {
        evm2_spec(self.chain_spec.as_ref(), header)
    }

    /// Returns the evm2 block environment for the provided block header.
    pub fn evm2_block_env_for_header(&self, header: &Header) -> evm2::env::BlockEnv {
        evm2_block_env_with_blob_params(
            header,
            self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
        )
    }

    /// Returns the evm2 spec id for the provided engine payload.
    #[cfg(feature = "std")]
    pub fn evm2_spec_for_payload(&self, payload: &ExecutionData) -> evm2::SpecId {
        evm2_spec_by_timestamp_and_block_number(
            self.chain_spec.as_ref(),
            payload.payload.timestamp(),
            payload.payload.block_number(),
        )
    }

    /// Returns the evm2 block environment for the provided engine payload.
    #[cfg(feature = "std")]
    pub fn evm2_block_env_for_payload(&self, payload: &ExecutionData) -> evm2::env::BlockEnv {
        evm2_payload_block_env(
            payload,
            self.chain_spec.as_ref().blob_params_at_timestamp(payload.payload.timestamp()),
        )
    }
}

impl<ChainSpec, EvmF> ConfigureEvm for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    EvmF: Clone + Debug + Send + Sync + Unpin + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type Spec = evm2::SpecId;
    type EvmEnv = EthEvmEnv;
    type TxEnv = Evm2TxEnv;
    type ExecutionCtx<'a>
        = EthBlockExecutionCtx<'a>
    where
        Self: 'a;

    fn evm_env(&self, header: &Header) -> Result<EvmEnvFor<Self>, Self::Error> {
        Ok(EthEvmEnv {
            spec: evm2_spec(self.chain_spec.as_ref(), header),
            block: evm2_block_env_with_blob_params(
                header,
                self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
            ),
        })
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        let base_fee = self
            .chain_spec
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

        Ok(EthEvmEnv {
            spec: evm2_spec(self.chain_spec.as_ref(), &header),
            block: evm2_block_env_with_blob_params(
                &header,
                self.chain_spec.as_ref().blob_params_at_timestamp(attributes.timestamp),
            ),
        })
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
}

#[cfg(feature = "std")]
impl<ChainSpec, EvmF> ConfigureEngineEvm<ExecutionData> for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    EvmF: Clone + Debug + Send + Sync + Unpin + 'static,
{
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        Ok(EthEvmEnv {
            spec: evm2_spec_by_timestamp_and_block_number(
                self.chain_spec.as_ref(),
                payload.payload.timestamp(),
                payload.payload.block_number(),
            ),
            block: evm2_payload_block_env(
                payload,
                self.chain_spec.as_ref().blob_params_at_timestamp(payload.payload.timestamp()),
            ),
        })
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
            Ok::<_, AnyError>(Evm2RecoveredTx::new(tx.with_signer(signer)))
        };

        Ok((txs, convert))
    }
}

#[cfg(feature = "std")]
impl<ChainSpec, EvmF> ConfigureEvm2Engine<ExecutionData> for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec:
        EthExecutorSpec + EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
    EvmF: Clone + Debug + Send + Sync + Unpin + 'static,
{
    fn evm2_chain_id(&self) -> u64 {
        self.chain_spec.chain_id()
    }

    fn evm2_deposit_contract_address(&self) -> Option<alloy_primitives::Address> {
        self.chain_spec.deposit_contract_address()
    }

    fn evm2_spec_for_header(&self, header: &Header) -> Result<evm2::SpecId, Self::Error> {
        Ok(evm2_spec(self.chain_spec.as_ref(), header))
    }

    fn evm2_block_env_for_header(
        &self,
        header: &Header,
    ) -> Result<evm2::env::BlockEnv, Self::Error> {
        Ok(evm2_block_env_with_blob_params(
            header,
            self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
        ))
    }

    fn evm2_spec_for_payload(&self, payload: &ExecutionData) -> Result<evm2::SpecId, Self::Error> {
        Ok(evm2_spec_by_timestamp_and_block_number(
            self.chain_spec.as_ref(),
            payload.payload.timestamp(),
            payload.payload.block_number(),
        ))
    }

    fn evm2_block_env_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<evm2::env::BlockEnv, Self::Error> {
        Ok(evm2_payload_block_env(
            payload,
            self.chain_spec.as_ref().blob_params_at_timestamp(payload.payload.timestamp()),
        ))
    }

    fn evm2_recovered_txs_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<
        Vec<Recovered<TransactionSigned>>,
        alloc::boxed::Box<dyn core::error::Error + Send + Sync>,
    > {
        payload
            .payload
            .transactions()
            .iter()
            .map(|tx| {
                let tx =
                    TransactionSigned::decode_2718_exact(tx.as_ref()).map_err(AnyError::new)?;
                let signer = tx.try_recover().map_err(AnyError::new)?;
                Ok(tx.with_signer(signer))
            })
            .collect::<Result<Vec<_>, AnyError>>()
            .map_err(|err| {
                alloc::boxed::Box::new(err)
                    as alloc::boxed::Box<dyn core::error::Error + Send + Sync>
            })
    }
}

#[cfg(feature = "std")]
impl<ChainSpec, EvmF> ConfigureEvm2Prewarm for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec:
        EthExecutorSpec + EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
    EvmF: Clone + Debug + Send + Sync + Unpin + 'static,
{
    type PrewarmEvm<DB>
        = evm2::Evm<evm2::BaseEvmTypes>
    where
        DB: reth_storage_api::StateProvider + Send + 'static;

    fn evm2_prewarm_evm<DB>(&self, state_provider: DB, env: EthEvmEnv) -> Self::PrewarmEvm<DB>
    where
        DB: reth_storage_api::StateProvider + Send + 'static,
    {
        let spec = env.spec;
        self.evm2_prewarm_evm_with_precompiles(
            state_provider,
            env,
            alloc::boxed::Box::new(evm2::Precompiles::base(spec)),
        )
    }

    fn evm2_prewarm_spec(&self, env: &EthEvmEnv) -> evm2::SpecId {
        env.spec
    }

    fn evm2_prewarm_evm_with_precompiles<DB>(
        &self,
        state_provider: DB,
        env: EthEvmEnv,
        precompiles: alloc::boxed::Box<
            dyn evm2::precompile::PrecompileProvider<evm2::BaseEvmTypes>,
        >,
    ) -> Self::PrewarmEvm<DB>
    where
        DB: reth_storage_api::StateProvider + Send + 'static,
    {
        let mut version = evm2::Version::new(env.spec);
        version.chain_id = self.chain_spec.chain_id();
        version.features.remove(evm2::EvmFeatures::NONCE_CHECK);
        version.features.remove(evm2::EvmFeatures::BALANCE_CHECK);

        evm2::Evm::<evm2::BaseEvmTypes>::new_with_execution_config(
            evm2::ExecutionConfig::for_spec_and_version(env.spec, version),
            env.spec,
            env.block,
            evm2::ethereum::ethereum_tx_registry(env.spec),
            evm2::evm::Db::new(reth_storage_api::Evm2StateProviderDatabase::new(state_provider)),
            precompiles,
        )
    }

    fn evm2_prewarm_tx<DB, S>(
        &self,
        evm: &mut Self::PrewarmEvm<DB>,
        tx: Evm2TxEnv,
        sink: &mut S,
    ) -> Result<evm2::TxResult, alloc::boxed::Box<dyn core::error::Error + Send + Sync>>
    where
        DB: reth_storage_api::StateProvider + Send + 'static,
        S: evm2::evm::StateChangeSink<Error = Infallible>,
    {
        enum PrewarmResolution {
            Outcome(evm2::TxResult),
            DatabaseError(evm2::evm::DbErrorCode),
            HandlerError(evm2::registry::HandlerError),
        }

        let resolution = match evm.transact(tx.as_envelope()) {
            Ok(executed) => {
                if let Some(code) = executed.result().db_error_code {
                    let _ = executed.discard();
                    PrewarmResolution::DatabaseError(code)
                } else {
                    let Ok(result) = executed.discard_with(sink);
                    PrewarmResolution::Outcome(result)
                }
            }
            Err(err) => PrewarmResolution::HandlerError(err),
        };

        match resolution {
            PrewarmResolution::Outcome(outcome) => Ok(outcome),
            PrewarmResolution::DatabaseError(code) => {
                Err(alloc::boxed::Box::new(evm2_prewarm_db_error::<DB>(evm, code)))
            }
            PrewarmResolution::HandlerError(err) => {
                Err(alloc::boxed::Box::new(Evm2ExecutionError::<ProviderError>::Handler(err)))
            }
        }
    }
}

#[cfg(feature = "std")]
fn evm2_prewarm_db_error<DB>(
    evm: &mut evm2::Evm<evm2::BaseEvmTypes>,
    code: evm2::evm::DbErrorCode,
) -> Evm2ExecutionError<ProviderError>
where
    DB: reth_storage_api::StateProvider + Send + 'static,
{
    evm.database_as_mut::<evm2::evm::Db<reth_storage_api::Evm2StateProviderDatabase<DB>>>()
        .and_then(evm2::evm::Db::take_result)
        .map(Evm2ExecutionError::Database)
        .unwrap_or(Evm2ExecutionError::MissingDatabaseError(code))
}

#[cfg(feature = "std")]
impl<ChainSpec, EvmF> ConfigureEvm2BlockExecutor for EthEvmConfig<ChainSpec, EvmF>
where
    ChainSpec:
        EthExecutorSpec + EthChainSpec<Header = Header> + EthereumHardforks + Hardforks + 'static,
    EvmF: Clone + Debug + Send + Sync + Unpin + 'static,
{
    type Primitives = EthPrimitives;

    fn execute_evm2_block_with_state_provider<DB>(
        &self,
        state_provider: DB,
        block: &RecoveredBlock<Block>,
    ) -> Result<
        reth_execution_types::BlockExecutionOutput<reth_ethereum_primitives::Receipt>,
        alloc::boxed::Box<dyn core::error::Error + Send + Sync>,
    >
    where
        DB: reth_storage_api::StateProvider + Send + 'static,
    {
        let header = block.header();
        let transactions = block
            .senders_iter()
            .zip(block.body().transactions())
            .map(|(signer, tx)| Recovered::new_unchecked(tx.clone(), *signer))
            .collect::<Vec<_>>();
        let context = Evm2BlockExecutionContext {
            chain_id: self.chain_spec.chain_id(),
            system_calls: Some(Evm2BlockSystemCalls {
                parent_hash: header.parent_hash,
                parent_beacon_block_root: header.parent_beacon_block_root,
            }),
            ommers: Some(&block.body().ommers),
            withdrawals: block.body().withdrawals().map(|withdrawals| withdrawals.as_slice()),
            deposit_contract_address: self.chain_spec.deposit_contract_address(),
        };

        execute_evm2_block_with_state_provider_context(
            evm2_spec(self.chain_spec.as_ref(), header),
            evm2_block_env_with_blob_params(
                header,
                self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
            ),
            state_provider,
            header.number,
            transactions,
            context,
        )
        .map_err(|err| {
            alloc::boxed::Box::new(err) as alloc::boxed::Box<dyn core::error::Error + Send + Sync>
        })
    }

    fn execute_evm2_block_with_state_provider_ref(
        &self,
        state_provider: &dyn reth_storage_api::StateProvider,
        block: &RecoveredBlock<Block>,
    ) -> Result<
        reth_execution_types::BlockExecutionOutput<reth_ethereum_primitives::Receipt>,
        alloc::boxed::Box<dyn core::error::Error + Send + Sync>,
    > {
        let header = block.header();
        let transactions = block
            .senders_iter()
            .zip(block.body().transactions())
            .map(|(signer, tx)| Recovered::new_unchecked(tx.clone(), *signer))
            .collect::<Vec<_>>();
        let context = Evm2BlockExecutionContext {
            chain_id: self.chain_spec.chain_id(),
            system_calls: Some(Evm2BlockSystemCalls {
                parent_hash: header.parent_hash,
                parent_beacon_block_root: header.parent_beacon_block_root,
            }),
            ommers: Some(&block.body().ommers),
            withdrawals: block.body().withdrawals().map(|withdrawals| withdrawals.as_slice()),
            deposit_contract_address: self.chain_spec.deposit_contract_address(),
        };

        execute_evm2_block_with_borrowed_state_provider_context(
            evm2_spec(self.chain_spec.as_ref(), header),
            evm2_block_env_with_blob_params(
                header,
                self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
            ),
            state_provider,
            header.number,
            transactions,
            context,
        )
        .map_err(|err| {
            alloc::boxed::Box::new(err) as alloc::boxed::Box<dyn core::error::Error + Send + Sync>
        })
    }

    fn execute_evm2_block_with_database<DB>(
        &self,
        database: DB,
        block: &RecoveredBlock<Block>,
    ) -> Result<
        reth_execution_types::BlockExecutionOutput<reth_ethereum_primitives::Receipt>,
        alloc::boxed::Box<dyn core::error::Error + Send + Sync>,
    >
    where
        DB: evm2::evm::Database + 'static,
        DB::Error: Send + Sync,
    {
        let header = block.header();
        let transactions = block
            .senders_iter()
            .zip(block.body().transactions())
            .map(|(signer, tx)| Recovered::new_unchecked(tx.clone(), *signer))
            .collect::<Vec<_>>();
        let context = Evm2BlockExecutionContext {
            chain_id: self.chain_spec.chain_id(),
            system_calls: Some(Evm2BlockSystemCalls {
                parent_hash: header.parent_hash,
                parent_beacon_block_root: header.parent_beacon_block_root,
            }),
            ommers: Some(&block.body().ommers),
            withdrawals: block.body().withdrawals().map(|withdrawals| withdrawals.as_slice()),
            deposit_contract_address: self.chain_spec.deposit_contract_address(),
        };

        execute_evm2_block_with_context(
            evm2_spec(self.chain_spec.as_ref(), header),
            evm2_block_env_with_blob_params(
                header,
                self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
            ),
            database,
            header.number,
            transactions,
            context,
        )
        .map_err(|err| {
            alloc::boxed::Box::new(err) as alloc::boxed::Box<dyn core::error::Error + Send + Sync>
        })
    }

    fn execute_evm2_block_with_database_and_precompile_cache<DB>(
        &self,
        database: DB,
        block: &RecoveredBlock<Block>,
        precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
    ) -> Result<
        reth_execution_types::BlockExecutionOutput<reth_ethereum_primitives::Receipt>,
        alloc::boxed::Box<dyn core::error::Error + Send + Sync>,
    >
    where
        DB: evm2::evm::Database + 'static,
        DB::Error: Send + Sync,
    {
        let header = block.header();
        let transactions = block
            .senders_iter()
            .zip(block.body().transactions())
            .map(|(signer, tx)| Recovered::new_unchecked(tx.clone(), *signer))
            .collect::<Vec<_>>();
        let context = Evm2BlockExecutionContext {
            chain_id: self.chain_spec.chain_id(),
            system_calls: Some(Evm2BlockSystemCalls {
                parent_hash: header.parent_hash,
                parent_beacon_block_root: header.parent_beacon_block_root,
            }),
            ommers: Some(&block.body().ommers),
            withdrawals: block.body().withdrawals().map(|withdrawals| withdrawals.as_slice()),
            deposit_contract_address: self.chain_spec.deposit_contract_address(),
        };
        let spec_id = evm2_spec(self.chain_spec.as_ref(), header);

        execute_evm2_block_with_context_and_precompiles(
            spec_id,
            evm2_block_env_with_blob_params(
                header,
                self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
            ),
            database,
            header.number,
            transactions,
            context,
            alloc::boxed::Box::new(CachedPrecompileProvider::new(
                evm2::Precompiles::base(spec_id),
                precompile_cache_map,
                spec_id,
                None,
            )),
        )
        .map_err(|err| {
            alloc::boxed::Box::new(err) as alloc::boxed::Box<dyn core::error::Error + Send + Sync>
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_genesis::Genesis;
    use alloy_primitives::U256;
    use reth_chainspec::{Chain, ChainSpec};

    #[test]
    fn test_evm2_block_env_for_header_uses_chain_blob_params() {
        let chain_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .cancun_activated()
            .build();
        let blob_params = chain_spec.blob_params_at_timestamp(1).expect("cancun blob params");
        let excess_blob_gas = 1_000_000;
        let header =
            Header { timestamp: 1, excess_blob_gas: Some(excess_blob_gas), ..Default::default() };

        let env = EthEvmConfig::new(Arc::new(chain_spec)).evm2_block_env_for_header(&header);

        assert_eq!(env.blob_basefee, U256::from(blob_params.calc_blob_fee(excess_blob_gas)));
    }
}
