//! `reth db prune-checkpoints` command for viewing and setting prune checkpoint values.

use clap::{Args, Parser, Subcommand, ValueEnum};
use reth_db_common::DbTool;
use reth_provider::{providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory};
use reth_prune_types::{PruneCheckpoint, PruneMode, PruneSegment};
use reth_storage_api::{PruneCheckpointReader, PruneCheckpointWriter};

use crate::common::AccessRights;

/// `reth db prune-checkpoints` subcommand
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
}

#[derive(Debug, Subcommand)]
enum Subcommands {
    /// Get prune checkpoint(s) from database.
    ///
    /// Shows the current prune progress for each segment, including the highest
    /// pruned block/tx number and the active prune mode.
    Get {
        /// Specific segment to query. If omitted, shows all segments.
        #[arg(long, value_enum)]
        segment: Option<SegmentArg>,
    },
    /// Set a prune checkpoint for a segment.
    ///
    /// WARNING: Manually setting checkpoints can cause data inconsistencies.
    /// Only use this if you know what you're doing (e.g., recovering from a
    /// corrupted checkpoint or forcing a re-prune from a specific block).
    Set(SetArgs),
}

/// Arguments for the `set` subcommand
#[derive(Debug, Args)]
pub struct SetArgs {
    /// The prune segment to update
    #[arg(long, value_enum)]
    segment: SegmentArg,

    /// Highest pruned block number
    #[arg(long)]
    block_number: Option<u64>,

    /// Highest pruned transaction number
    #[arg(long)]
    tx_number: Option<u64>,

    /// Prune mode to write: full, distance, or before
    #[arg(long, value_enum)]
    mode: PruneModeArg,

    /// Value for distance or before mode (required unless mode is full)
    #[arg(long, required_if_eq_any([("mode", "distance"), ("mode", "before")]))]
    mode_value: Option<u64>,
}

/// CLI-friendly prune segment names (excludes deprecated variants)
#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum SegmentArg {
    SenderRecovery,
    TransactionLookup,
    Receipts,
    ContractLogs,
    AccountHistory,
    StorageHistory,
    Bodies,
}

impl From<SegmentArg> for PruneSegment {
    fn from(arg: SegmentArg) -> Self {
        match arg {
            SegmentArg::SenderRecovery => Self::SenderRecovery,
            SegmentArg::TransactionLookup => Self::TransactionLookup,
            SegmentArg::Receipts => Self::Receipts,
            SegmentArg::ContractLogs => Self::ContractLogs,
            SegmentArg::AccountHistory => Self::AccountHistory,
            SegmentArg::StorageHistory => Self::StorageHistory,
            SegmentArg::Bodies => Self::Bodies,
        }
    }
}

/// CLI-friendly prune mode
#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum PruneModeArg {
    /// Prune all blocks
    Full,
    /// Keep the last N blocks (requires --mode-value)
    Distance,
    /// Prune blocks before a specific block number (requires --mode-value)
    Before,
}

impl Command {
    /// Execute the command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        match self.command {
            Subcommands::Get { segment } => Self::get(tool, segment),
            Subcommands::Set(args) => Self::set(tool, args),
        }
    }

    fn get<N: ProviderNodeTypes>(
        tool: &DbTool<N>,
        segment: Option<SegmentArg>,
    ) -> eyre::Result<()> {
        let provider = tool.provider_factory.provider()?;

        match segment {
            Some(seg) => {
                let segment: PruneSegment = seg.into();
                match provider.get_prune_checkpoint(segment)? {
                    Some(checkpoint) => print_checkpoint(segment, &checkpoint),
                    None => println!("No checkpoint found for {segment}"),
                }
            }
            None => {
                let mut checkpoints = provider.get_prune_checkpoints()?;
                checkpoints.sort_by_key(|(seg, _)| *seg);
                if checkpoints.is_empty() {
                    println!("No prune checkpoints found.");
                } else {
                    println!(
                        "{:<25} {:>15} {:>15} {:>20}",
                        "Segment", "Block Number", "Tx Number", "Prune Mode"
                    );
                    println!("{}", "-".repeat(80));
                    for (segment, checkpoint) in &checkpoints {
                        println!(
                            "{:<25} {:>15} {:>15} {:>20}",
                            segment.to_string(),
                            fmt_opt(checkpoint.block_number),
                            fmt_opt(checkpoint.tx_number),
                            fmt_mode(&checkpoint.prune_mode),
                        );
                    }
                }
            }
        }

        Ok(())
    }

    fn set<N: ProviderNodeTypes>(tool: &DbTool<N>, args: SetArgs) -> eyre::Result<()> {
        eyre::ensure!(
            args.block_number.is_some() || args.tx_number.is_some(),
            "at least one of --block-number or --tx-number must be provided"
        );

        let prune_mode = match args.mode {
            PruneModeArg::Full => PruneMode::Full,
            PruneModeArg::Distance => PruneMode::Distance(
                args.mode_value
                    .ok_or_else(|| eyre::eyre!("--mode-value is required for distance mode"))?,
            ),
            PruneModeArg::Before => PruneMode::Before(
                args.mode_value
                    .ok_or_else(|| eyre::eyre!("--mode-value is required for before mode"))?,
            ),
        };

        let segment: PruneSegment = args.segment.into();
        let checkpoint = PruneCheckpoint {
            block_number: args.block_number,
            tx_number: args.tx_number,
            prune_mode,
        };

        let provider_rw = tool.provider_factory.database_provider_rw()?;

        // Show previous value if any
        if let Some(prev) = provider_rw.get_prune_checkpoint(segment)? {
            println!("Previous checkpoint for {segment}:");
            print_checkpoint(segment, &prev);
            println!();
        }

        provider_rw.save_prune_checkpoint(segment, checkpoint)?;
        provider_rw.commit()?;

        println!("Updated checkpoint for {segment}:");
        print_checkpoint(segment, &checkpoint);

        Ok(())
    }
}

fn print_checkpoint(segment: PruneSegment, checkpoint: &PruneCheckpoint) {
    println!("  Segment:      {segment}");
    println!("  Block Number: {}", fmt_opt(checkpoint.block_number));
    println!("  Tx Number:    {}", fmt_opt(checkpoint.tx_number));
    println!("  Prune Mode:   {}", fmt_mode(&checkpoint.prune_mode));
}

fn fmt_opt(val: Option<u64>) -> String {
    val.map_or("-".to_string(), |n| n.to_string())
}

fn fmt_mode(mode: &PruneMode) -> String {
    match mode {
        PruneMode::Full => "Full".to_string(),
        PruneMode::Distance(d) => format!("Distance({d})"),
        PruneMode::Before(b) => format!("Before({b})"),
    }
}
