use super::BscEvm;
use crate::evm::{
    api::{
        builder::BscBuilder,
        ctx::{BscContext, DefaultBsc},
    },
    precompiles::BscPrecompiles,
    spec::BscSpecId,
    transaction::BscTxEnv,
};
use reth_evm::{precompiles::PrecompilesMap, EvmEnv, EvmFactory};
use reth_revm::{Context, Database};
use revm::{
    context::{
        result::{EVMError, HaltReason},
        TxEnv,
    },
    inspector::NoOpInspector,
    Inspector,
};

/// Factory producing [`BscEvm`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct BscEvmFactory;

impl EvmFactory for BscEvmFactory {
    type Evm<DB: Database<Error: Send + Sync + 'static>, I: Inspector<BscContext<DB>>> =
        BscEvm<DB, I, Self::Precompiles>;
    type Context<DB: Database<Error: Send + Sync + 'static>> = BscContext<DB>;
    type Tx = BscTxEnv<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = BscSpecId;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database<Error: Send + Sync + 'static>>(
        &self,
        db: DB,
        input: EvmEnv<BscSpecId>,
    ) -> Self::Evm<DB, NoOpInspector> {
        BscEvm {
            inner: Context::bsc()
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .with_db(db)
                .build_bsc_with_inspector(NoOpInspector {})
                .with_precompiles(PrecompilesMap::from_static(
                    BscPrecompiles::default().precompiles(),
                )),
            inspect: false,
        }
    }

    fn create_evm_with_inspector<
        DB: Database<Error: Send + Sync + 'static>,
        I: Inspector<Self::Context<DB>>,
    >(
        &self,
        db: DB,
        input: EvmEnv<BscSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        BscEvm {
            inner: Context::bsc()
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .with_db(db)
                .build_bsc_with_inspector(inspector)
                .with_precompiles(PrecompilesMap::from_static(
                    BscPrecompiles::default().precompiles(),
                )),
            inspect: true,
        }
    }
}
