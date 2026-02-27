//! `reth db stage-checkpoints` command for viewing and setting stage checkpoint values.

use clap::{Args, Parser, Subcommand, ValueEnum};
use reth_db_common::DbTool;
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, StageCheckpointReader,
    StageCheckpointWriter,
};
use reth_stages::StageId;

use crate::common::AccessRights;

/// `reth db stage-checkpoints` subcommand
#[derive(Debug, Parser)]
pub struct Command {
    #[command(subcommand)]
    command: Subcommands,
}

impl Command {
    /// Returns database access rights required for the command.
    pub fn access_rights(&self) -> AccessRights {
        match &self.command {
            Subcommands::Get { .. } => AccessRights::RO,
            Subcommands::Set(_) => AccessRights::RW,
        }
    }

    /// Execute the command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        match self.command {
            Subcommands::Get { stage } => Self::get(tool, stage),
            Subcommands::Set(args) => Self::set(tool, args),
        }
    }

    fn get<N: ProviderNodeTypes>(tool: &DbTool<N>, stage: Option<StageArg>) -> eyre::Result<()> {
        let provider = tool.provider_factory.provider()?;

        match stage {
            Some(stage) => {
                let stage_id = stage.into();
                let checkpoint = provider.get_stage_checkpoint(stage_id)?;
                println!("{stage_id}: {checkpoint:?}");
            }
            None => {
                let mut checkpoints = provider.get_all_checkpoints()?;
                checkpoints.sort_by(|a, b| a.0.cmp(&b.0));
                for (stage, checkpoint) in checkpoints {
                    println!("{stage}: {checkpoint:?}");
                }
            }
        }

        Ok(())
    }

    fn set<N: ProviderNodeTypes>(tool: &DbTool<N>, args: SetArgs) -> eyre::Result<()> {
        let stage_id: StageId = args.stage.into();
        let provider_rw = tool.provider_factory.database_provider_rw()?;

        let previous = provider_rw.get_stage_checkpoint(stage_id)?;
        let mut checkpoint = previous.unwrap_or_default();
        checkpoint.block_number = args.block_number;

        if args.clear_stage_unit {
            checkpoint.stage_checkpoint = None;
        }

        provider_rw.save_stage_checkpoint(stage_id, checkpoint)?;

        provider_rw.commit()?;

        println!("Updated checkpoint for {stage_id}: {checkpoint:?}");

        Ok(())
    }
}

#[derive(Debug, Subcommand)]
enum Subcommands {
    /// Get stage checkpoint(s) from database.
    Get {
        /// Specific stage to query. If omitted, shows all stages.
        #[arg(long, value_enum)]
        stage: Option<StageArg>,
    },
    /// Set a stage checkpoint.
    Set(SetArgs),
}

/// Arguments for the `set` subcommand.
#[derive(Debug, Args)]
pub struct SetArgs {
    /// Stage to update.
    #[arg(long, value_enum)]
    stage: StageArg,

    /// Block number to set as stage checkpoint.
    #[arg(long)]
    block_number: u64,

    /// Clear stage-specific unit checkpoint payload.
    #[arg(long)]
    clear_stage_unit: bool,
}

/// CLI-friendly stage names.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum StageArg {
    Era,
    Headers,
    Bodies,
    SenderRecovery,
    Execution,
    PruneSenderRecovery,
    MerkleUnwind,
    AccountHashing,
    StorageHashing,
    MerkleExecute,
    TransactionLookup,
    IndexStorageHistory,
    IndexAccountHistory,
    Prune,
    Finish,
}

impl From<StageArg> for StageId {
    fn from(arg: StageArg) -> Self {
        match arg {
            StageArg::Era => Self::Era,
            StageArg::Headers => Self::Headers,
            StageArg::Bodies => Self::Bodies,
            StageArg::SenderRecovery => Self::SenderRecovery,
            StageArg::Execution => Self::Execution,
            StageArg::PruneSenderRecovery => Self::PruneSenderRecovery,
            StageArg::MerkleUnwind => Self::MerkleUnwind,
            StageArg::AccountHashing => Self::AccountHashing,
            StageArg::StorageHashing => Self::StorageHashing,
            StageArg::MerkleExecute => Self::MerkleExecute,
            StageArg::TransactionLookup => Self::TransactionLookup,
            StageArg::IndexStorageHistory => Self::IndexStorageHistory,
            StageArg::IndexAccountHistory => Self::IndexAccountHistory,
            StageArg::Prune => Self::Prune,
            StageArg::Finish => Self::Finish,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use reth_provider::{
        test_utils::create_test_provider_factory, DBProvider, DatabaseProviderFactory,
        StageCheckpointReader, StageCheckpointWriter,
    };
    use reth_stages::StageCheckpoint;

    #[test]
    fn parse_set_args() {
        let command = Command::parse_from([
            "stage-checkpoints",
            "set",
            "--stage",
            "headers",
            "--block-number",
            "123",
        ]);

        assert!(matches!(
            command.command,
            Subcommands::Set(SetArgs {
                stage: StageArg::Headers,
                block_number: 123,
                clear_stage_unit: false,
            })
        ));
    }

    #[test]
    fn set_overwrites_block_number() {
        let provider_factory = create_test_provider_factory();
        let tool = DbTool::new(provider_factory.clone()).expect("db tool");

        {
            let provider_rw = provider_factory.database_provider_rw().expect("rw provider");
            provider_rw
                .save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(10))
                .expect("save checkpoint");
            provider_rw.commit().expect("commit initial checkpoint");
        }

        let command = Command {
            command: Subcommands::Set(SetArgs {
                stage: StageArg::Headers,
                block_number: 42,
                clear_stage_unit: false,
            }),
        };

        command.execute(&tool).expect("execute command");

        let provider = provider_factory.provider().expect("provider");
        let checkpoint = provider
            .get_stage_checkpoint(StageId::Headers)
            .expect("get stage checkpoint")
            .expect("missing stage checkpoint");

        assert_eq!(checkpoint.block_number, 42);
    }

    #[test]
    fn set_preserves_stage_unit_checkpoint_unless_cleared() {
        let provider_factory = create_test_provider_factory();
        let tool = DbTool::new(provider_factory.clone()).expect("db tool");

        {
            let provider_rw = provider_factory.database_provider_rw().expect("rw provider");
            let checkpoint = StageCheckpoint::new(10).with_block_range(&StageId::Execution, 5, 10);
            provider_rw
                .save_stage_checkpoint(StageId::Execution, checkpoint)
                .expect("save checkpoint");
            provider_rw.commit().expect("commit initial checkpoint");
        }

        Command {
            command: Subcommands::Set(SetArgs {
                stage: StageArg::Execution,
                block_number: 11,
                clear_stage_unit: false,
            }),
        }
        .execute(&tool)
        .expect("execute command");

        let provider = provider_factory.provider().expect("provider");
        let checkpoint = provider
            .get_stage_checkpoint(StageId::Execution)
            .expect("get stage checkpoint")
            .expect("missing stage checkpoint");
        assert!(checkpoint.stage_checkpoint.is_some());

        Command {
            command: Subcommands::Set(SetArgs {
                stage: StageArg::Execution,
                block_number: 12,
                clear_stage_unit: true,
            }),
        }
        .execute(&tool)
        .expect("execute command");

        let checkpoint = provider_factory
            .provider()
            .expect("provider")
            .get_stage_checkpoint(StageId::Execution)
            .expect("get stage checkpoint")
            .expect("missing stage checkpoint");
        assert!(checkpoint.stage_checkpoint.is_none());
    }

    #[test]
    fn set_preserves_checkpoint_progress() {
        let provider_factory = create_test_provider_factory();
        let tool = DbTool::new(provider_factory.clone()).expect("db tool");

        {
            let provider_rw = provider_factory.database_provider_rw().expect("rw provider");
            provider_rw
                .save_stage_checkpoint(StageId::MerkleExecute, StageCheckpoint::new(10))
                .expect("save checkpoint");
            provider_rw
                .save_stage_checkpoint_progress(StageId::MerkleExecute, vec![1, 2, 3])
                .expect("save progress");
            provider_rw.commit().expect("commit initial checkpoint");
        }

        Command {
            command: Subcommands::Set(SetArgs {
                stage: StageArg::MerkleExecute,
                block_number: 20,
                clear_stage_unit: false,
            }),
        }
        .execute(&tool)
        .expect("execute command");

        let provider = provider_factory.provider().expect("provider");
        let progress = provider
            .get_stage_checkpoint_progress(StageId::MerkleExecute)
            .expect("get stage checkpoint progress");

        assert_eq!(progress, Some(vec![1, 2, 3]));
    }
}
