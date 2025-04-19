use super::BscEvmInner;
use crate::evm::{spec::BscSpecId, transaction::BscTxTr};
use revm::{
    context::{Cfg, JournalOutput},
    context_interface::{Block, JournalTr},
    handler::instructions::EthInstructions,
    interpreter::interpreter::EthInterpreter,
    Context, Database,
};

/// Trait that allows for bsc BscEvm to be built.
pub trait BscBuilder: Sized {
    /// Type of the context.
    type Context;

    /// Build the bsc with an inspector.
    fn build_bsc_with_inspector<INSP>(
        self,
        inspector: INSP,
    ) -> BscEvmInner<Self::Context, INSP, EthInstructions<EthInterpreter, Self::Context>>;
}

impl<BLOCK, TX, CFG, DB, JOURNAL> BscBuilder for Context<BLOCK, TX, CFG, DB, JOURNAL>
where
    BLOCK: Block,
    TX: BscTxTr,
    CFG: Cfg<Spec = BscSpecId>,
    DB: Database,
    JOURNAL: JournalTr<Database = DB, FinalOutput = JournalOutput>,
{
    type Context = Self;

    fn build_bsc_with_inspector<INSP>(
        self,
        inspector: INSP,
    ) -> BscEvmInner<Self::Context, INSP, EthInstructions<EthInterpreter, Self::Context>> {
        BscEvmInner::new(self, inspector)
    }
}
