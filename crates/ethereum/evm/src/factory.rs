use alloc::sync::Arc;
use core::{fmt::Debug, marker::PhantomData};

#[cfg(feature = "std")]
use alloy_consensus::Header;
use reth_chainspec::ChainSpec;
#[cfg(feature = "std")]
use reth_chainspec::{EthChainSpec, EthExecutorSpec};
#[cfg(feature = "std")]
use reth_evm::precompile_cache::{CachedPrecompileProvider, PrecompileCacheMap};

#[cfg(feature = "std")]
use crate::{EthBlockExecutionCtx, EthBlockExecutor, EthEvmEnv, HashedStateMode};
#[cfg(feature = "std")]
use crate::{EthPrimitives, EthTxEnv};

/// Ethereum block executor factory.
#[derive(Debug)]
pub struct EthBlockExecutorFactory<C = ChainSpec, EvmFactory = ()> {
    /// Chain specification.
    chain_spec: Arc<C>,
    /// Shared precompile cache.
    #[cfg(feature = "std")]
    precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
    _evm_factory: PhantomData<EvmFactory>,
}

impl<C, EvmFactory> Clone for EthBlockExecutorFactory<C, EvmFactory> {
    fn clone(&self) -> Self {
        Self {
            chain_spec: self.chain_spec.clone(),
            #[cfg(feature = "std")]
            precompile_cache_map: self.precompile_cache_map.clone(),
            _evm_factory: PhantomData,
        }
    }
}

impl<C> EthBlockExecutorFactory<C> {
    /// Creates a new Ethereum block executor factory.
    pub fn new(chain_spec: Arc<C>) -> Self {
        Self::new_with_evm_factory(chain_spec, ())
    }
}

impl<C, EvmFactory> EthBlockExecutorFactory<C, EvmFactory> {
    /// Creates a new Ethereum block executor factory with the given EVM factory placeholder.
    pub fn new_with_evm_factory(chain_spec: Arc<C>, _evm_factory: EvmFactory) -> Self {
        Self {
            chain_spec,
            #[cfg(feature = "std")]
            precompile_cache_map: PrecompileCacheMap::default(),
            _evm_factory: PhantomData,
        }
    }

    /// Returns the chain spec associated with this factory.
    pub const fn chain_spec(&self) -> &Arc<C> {
        &self.chain_spec
    }

    /// Creates a configured Ethereum block executor.
    #[cfg(feature = "std")]
    pub fn create_executor<'a, DB>(
        &'a self,
        evm: evm2::Evm<evm2::BaseEvmTypes>,
        ctx: EthBlockExecutionCtx<'a>,
        hashed_state_mode: HashedStateMode,
    ) -> EthBlockExecutor<'a, DB>
    where
        C: EthChainSpec<Header = Header> + EthExecutorSpec,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        EthBlockExecutor::new(
            evm,
            ctx,
            self.chain_spec.chain_id(),
            self.chain_spec.deposit_contract_address(),
            hashed_state_mode,
        )
    }

    /// Creates an EVM instance with the configured Ethereum execution environment.
    #[cfg(feature = "std")]
    pub fn evm_with_env<DB>(&self, db: DB, env: EthEvmEnv) -> evm2::Evm<evm2::BaseEvmTypes>
    where
        C: EthChainSpec<Header = Header>,
        DB: evm2::evm::DynDatabase + 'static,
    {
        evm2::Evm::<evm2::BaseEvmTypes>::new_with_execution_config(
            evm2::ExecutionConfig::for_spec_and_version(env.spec, env.version),
            env.spec,
            env.block,
            evm2::ethereum::ethereum_tx_registry(env.spec),
            db,
            alloc::boxed::Box::new(CachedPrecompileProvider::new(
                evm2::Precompiles::base(env.spec),
                self.precompile_cache_map.clone(),
                env.spec,
                None,
            )),
        )
    }
}

#[cfg(feature = "std")]
impl<C, EvmFactory> reth_evm::execute::BlockExecutorFactory
    for EthBlockExecutorFactory<C, EvmFactory>
where
    C: EthChainSpec<Header = Header> + EthExecutorSpec + Debug + Send + Sync + 'static,
    EvmFactory: Clone + Debug + Send + Sync + Unpin + 'static,
{
    type Primitives = EthPrimitives;
    type Transaction = EthTxEnv;
    type EvmEnv = EthEvmEnv;
    type ExecutionCtx<'a>
        = EthBlockExecutionCtx<'a>
    where
        Self: 'a;
    type Executor<'a, DB>
        = EthBlockExecutor<'a, DB>
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    fn create_executor<'a, DB>(
        &'a self,
        evm: evm2::Evm<evm2::BaseEvmTypes>,
        ctx: Self::ExecutionCtx<'a>,
        hashed_state_mode: HashedStateMode,
    ) -> Self::Executor<'a, DB>
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        Self::create_executor(self, evm, ctx, hashed_state_mode)
    }

    fn evm_tx<'a>(
        &self,
        tx: &'a Self::Transaction,
    ) -> &'a <evm2::BaseEvmTypes as evm2::EvmTypes>::Tx {
        tx.as_envelope()
    }
}
