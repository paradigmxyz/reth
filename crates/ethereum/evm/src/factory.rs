use reth_engine_tree::tree::{precompile_cache::PrecompileCache, CachedPrecompileProvider};
use reth_evm::{eth::EthEvmContext, Database, EthEvm, EvmEnv, EvmFactory};
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
use std::sync::Arc;

/// Factory producing [`CachedPrecompileEthEvm`].
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
#[allow(dead_code)]
pub struct CachedPrecompileEthEvmFactory {
    precompile_cache: Arc<PrecompileCache>,
}

type CachedPrecompileEthEvm<DB, I> = EthEvm<DB, I, CachedPrecompileProvider<EthPrecompiles>>;

impl EvmFactory for CachedPrecompileEthEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>>> = CachedPrecompileEthEvm<DB, I>;
    type Context<DB: Database> = Context<BlockEnv, TxEnv, CfgEnv, DB>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = SpecId;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        let precompile_cache = self.precompile_cache.clone();
        EthEvm::new(
            Context::mainnet()
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(NoOpInspector {})
                .with_precompiles(CachedPrecompileProvider::new(
                    EthPrecompiles::default(),
                    precompile_cache,
                )),
            false,
        )
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let precompile_cache = self.precompile_cache.clone();
        EthEvm::new(
            Context::mainnet()
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(inspector)
                .with_precompiles(CachedPrecompileProvider::new(
                    EthPrecompiles::default(),
                    precompile_cache,
                )),
            true,
        )
    }
}
