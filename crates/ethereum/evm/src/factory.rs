use alloc::sync::Arc;
use reth_evm::{
    eth::EthEvmContext, Database, EthEvm, EvmEnv, EvmFactory, MaybeCachedPrecompileProvider,
    PrecompileCache,
};
use revm::{
    context::{
        result::{EVMError, HaltReason},
        BlockEnv, CfgEnv, TxEnv,
    },
    handler::EthPrecompiles,
    inspector::NoOpInspector,
    primitives::hardfork::SpecId,
    Context, Inspector, MainBuilder, MainContext,
};

/// Factory producing [`CachedPrecompileEthEvmFactory`].
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
#[allow(dead_code)]
pub struct MaybeCachedPrecompileEthEvmFactory {
    cache_enabled: bool,
    precompile_cache: Arc<PrecompileCache>,
}

impl MaybeCachedPrecompileEthEvmFactory {
    /// Creates a new `CachedPrecompileEthEvmFactory`.
    pub fn new(cache_enabled: bool) -> Self {
        Self { cache_enabled, precompile_cache: Default::default() }
    }

    fn precompile_provider(&self) -> MaybeCachedPrecompileProvider<EthPrecompiles> {
        if self.cache_enabled {
            MaybeCachedPrecompileProvider::new_with_cache(
                EthPrecompiles::default(),
                self.precompile_cache.clone(),
            )
        } else {
            MaybeCachedPrecompileProvider::new_without_cache(EthPrecompiles::default())
        }
    }
}

type CachedPrecompileEthEvm<DB, I> = EthEvm<DB, I, MaybeCachedPrecompileProvider<EthPrecompiles>>;

impl EvmFactory for MaybeCachedPrecompileEthEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>>> = CachedPrecompileEthEvm<DB, I>;
    type Context<DB: Database> = Context<BlockEnv, TxEnv, CfgEnv, DB>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = SpecId;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        EthEvm::new(
            Context::mainnet()
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(NoOpInspector {})
                .with_precompiles(self.precompile_provider()),
            false,
        )
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        EthEvm::new(
            Context::mainnet()
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(inspector)
                .with_precompiles(self.precompile_provider()),
            true,
        )
    }
}
