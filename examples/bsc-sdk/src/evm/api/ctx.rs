use crate::evm::{spec::BscSpecId, transaction::BscTxEnv};
use revm::{
    context::{BlockEnv, CfgEnv, TxEnv},
    database_interface::EmptyDB,
    Context, Journal, MainContext,
};

/// Type alias for the default context type of the BscEvm.
pub type BscContext<DB> = Context<BlockEnv, BscTxEnv<TxEnv>, CfgEnv<BscSpecId>, DB, Journal<DB>>;

/// Trait that allows for a default context to be created.
pub trait DefaultBsc {
    /// Create a default context.
    fn bsc() -> BscContext<EmptyDB>;
}

impl DefaultBsc for BscContext<EmptyDB> {
    fn bsc() -> Self {
        Context::mainnet()
            .with_tx(BscTxEnv::default())
            .with_cfg(CfgEnv::new_with_spec(BscSpecId::LATEST))
    }
}
