use clap::Parser;
use reth_db_api::transaction::DbTxMut;
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, ProviderFactory,
};
use reth_prune_types::PruneSegment;

/// The arguments for the `reth db drop-prune-checkpoint` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The prune segment to drop the checkpoint for.
    #[arg(value_enum)]
    pub segment: PruneSegment,
}

impl Command {
    /// Execute `db drop-prune-checkpoint` command
    pub fn execute<N: ProviderNodeTypes>(
        self,
        provider_factory: ProviderFactory<N>,
    ) -> eyre::Result<()> {
        let provider_rw = provider_factory.database_provider_rw()?;

        // Delete the prune checkpoint for the specified segment
        provider_rw.tx_ref().delete::<reth_db_api::tables::PruneCheckpoints>(self.segment, None)?;

        provider_rw.commit()?;

        Ok(())
    }
}
