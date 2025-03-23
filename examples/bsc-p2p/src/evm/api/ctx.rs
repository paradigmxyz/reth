use revm::{
    context::{BlockEnv, CfgEnv, TxEnv},
    database_interface::EmptyDB,
    Context, Journal, MainContext,
};

use crate::evm::{spec::BscSpecId, transaction::BscTransaction};

/// Type alias for the default context type of the OpEvm.
pub type BscContext<DB> =
    Context<BlockEnv, BscTransaction<TxEnv>, CfgEnv<BscSpecId>, DB, Journal<DB>>;

/// Trait that allows for a default context to be created.
pub trait DefaultBsc {
    /// Create a default context.
    fn bsc() -> BscContext<EmptyDB>;
}

impl DefaultBsc for BscContext<EmptyDB> {
    fn bsc() -> Self {
        Context::mainnet()
            .with_tx(BscTransaction::default())
            .with_cfg(CfgEnv::new_with_spec(BscSpecId::LATEST))
    }
}
