use alloc::sync::Arc;
use core::marker::PhantomData;

use alloy_consensus::Header;
use reth_chainspec::ChainSpec;
#[cfg(feature = "std")]
use reth_chainspec::{EthChainSpec, EthExecutorSpec};
#[cfg(feature = "std")]
use reth_evm::precompile_cache::{CachedPrecompileProvider, PrecompileCacheMap};

#[cfg(feature = "std")]
use crate::{EthBlockExecutionCtx, EthBlockExecutor, EthEvmEnv, EthExecutor, HashedStateMode};

/// Ethereum block executor factory.
#[derive(Debug, Clone)]
pub struct EthBlockExecutorFactory<C = ChainSpec, EvmFactory = ()> {
    /// Chain specification.
    chain_spec: Arc<C>,
    /// Shared precompile cache.
    #[cfg(feature = "std")]
    precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
    _evm_factory: PhantomData<EvmFactory>,
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

    /// Returns the shared precompile cache map.
    #[cfg(feature = "std")]
    pub const fn precompile_cache_map(&self) -> &PrecompileCacheMap<evm2::SpecId> {
        &self.precompile_cache_map
    }

    /// Returns an executor for block execution over the provided database.
    #[cfg(feature = "std")]
    pub fn executor<DB>(&self, db: DB) -> EthExecutor<C, DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        EthExecutor::new(self.chain_spec.clone(), db, self.precompile_cache_map.clone())
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
        let mut version = evm2::Version::new(env.spec);
        version.chain_id = self.chain_spec.chain_id();

        evm2::Evm::<evm2::BaseEvmTypes>::new_with_execution_config(
            evm2::ExecutionConfig::for_spec_and_version(env.spec, version),
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

    /// Creates an EVM instance for prewarming.
    #[cfg(feature = "std")]
    pub fn prewarm_evm<DB>(
        &self,
        state_provider: DB,
        env: EthEvmEnv,
    ) -> evm2::Evm<evm2::BaseEvmTypes>
    where
        C: EthChainSpec<Header = Header>,
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
            evm2::evm::Db::new(reth_storage_api::EvmStateProviderDatabase::new(state_provider)),
            alloc::boxed::Box::new(CachedPrecompileProvider::new(
                evm2::Precompiles::base(env.spec),
                self.precompile_cache_map.clone(),
                env.spec,
                None,
            )),
        )
    }
}
