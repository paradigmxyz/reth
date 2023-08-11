//! Unwinding a certain block range

use crate::{
    args::{utils::genesis_value_parser, DatabaseArgs, StageEnum},
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::{Parser, Subcommand};
use futures_util::Stream;
use reth_beacon_consensus::BeaconConsensus;
use reth_db::{cursor::DbCursorRO, database::Database, open_db, tables, transaction::DbTx};
use reth_interfaces::p2p::{
    bodies::downloader::{BodyDownloader, BodyDownloaderResult},
    error::DownloadResult,
    headers::{
        downloader::{HeaderDownloader, SyncTarget},
        error::HeadersDownloaderResult,
    },
};
use reth_primitives::{BlockHashOrNumber, BlockNumber, ChainSpec, PruneModes, SealedHeader};
use reth_provider::{
    BlockExecutionWriter, ProviderFactory, StageCheckpointReader, StageCheckpointWriter,
};
use reth_stages::{
    stages::{
        AccountHashingStage, BodyStage, ExecutionStage, ExecutionStageThresholds, HeaderStage,
        HeaderSyncMode, IndexAccountHistoryStage, IndexStorageHistoryStage, MerkleStage,
        SenderRecoveryStage, StorageHashingStage, TransactionLookupStage,
        MERKLE_STAGE_DEFAULT_CLEAN_THRESHOLD,
    },
    Stage, UnwindInput,
};
use std::{
    ops::RangeInclusive,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tracing::*;

/// `reth stage unwind` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t, global = true)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser,
        global = true
    )]
    chain: Arc<ChainSpec>,

    #[arg(value_enum)]
    stage: Option<StageEnum>,

    #[clap(flatten)]
    db: DatabaseArgs,

    #[clap(subcommand)]
    command: Subcommands,
}

impl Command {
    /// Execute `db stage unwind` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        if !db_path.exists() {
            eyre::bail!("Database {db_path:?} does not exist.")
        }

        let db = open_db(db_path.as_ref(), self.db.log_level)?;

        let range = self.command.unwind_range(&db)?;

        if *range.start() == 0 {
            eyre::bail!("Cannot unwind genesis block")
        }

        let factory = ProviderFactory::new(&db, self.chain.clone());
        let mut provider_rw = factory.provider_rw()?;

        let stage = match self.stage {
            Some(stage) => stage,
            None => {
                let blocks_and_execution = provider_rw
                    .take_block_and_execution_range(&self.chain, range)
                    .map_err(|err| eyre::eyre!("Transaction error on unwind: {err:?}"))?;

                provider_rw.commit()?;
                info!(target: "reth::cli", "Unwind {:?} blocks", blocks_and_execution.len());

                return Ok(())
            }
        };

        let mut unwind_stage: Box<dyn Stage<_>> = match stage {
            StageEnum::Headers => {
                let stage = HeaderStage::new(MockHeaderDownloader, HeaderSyncMode::Continuous);
                Box::new(stage)
            }
            StageEnum::Bodies => {
                let consensus = Arc::new(BeaconConsensus::new(self.chain.clone()));
                let stage = BodyStage { downloader: MockBodyDownloader, consensus };
                Box::new(stage)
            }
            StageEnum::Senders => Box::new(SenderRecoveryStage::new(u64::MAX)),
            StageEnum::Execution => {
                let factory = reth_revm::Factory::new(self.chain.clone());
                Box::new(ExecutionStage::new(
                    factory,
                    ExecutionStageThresholds { max_blocks: None, max_changes: None },
                    MERKLE_STAGE_DEFAULT_CLEAN_THRESHOLD,
                    PruneModes::all(),
                ))
            }
            StageEnum::TxLookup => Box::new(TransactionLookupStage::new(u64::MAX)),
            StageEnum::AccountHashing => Box::new(AccountHashingStage::new(1, u64::MAX)),
            StageEnum::StorageHashing => Box::new(StorageHashingStage::new(1, u64::MAX)),
            StageEnum::Merkle => Box::new(MerkleStage::default_execution()),
            StageEnum::AccountHistory => Box::<IndexAccountHistoryStage>::default(),
            StageEnum::StorageHistory => Box::<IndexStorageHistoryStage>::default(),
            _ => return Ok(()),
        };

        let to = *range.start();
        let stage_id = unwind_stage.id();
        let mut checkpoint = provider_rw.get_stage_checkpoint(stage_id)?.unwrap_or_default();
        if checkpoint.block_number < to {
            info!(target: "reth::cli", "No need to unwind this stage, checkpoint.block_number: {:?}, unwind_to: {:?}", checkpoint.block_number, to);
            return Ok(())
        }

        while checkpoint.block_number > to {
            let input = UnwindInput { checkpoint, unwind_to: to, bad_block: None };

            let output = unwind_stage.unwind(&provider_rw, input).await;
            match output {
                Ok(unwind_output) => {
                    checkpoint = unwind_output.checkpoint;
                    provider_rw.save_stage_checkpoint(stage_id, checkpoint)?;

                    provider_rw.commit()?;
                    provider_rw = factory.provider_rw()?
                }
                Err(err) => {
                    error!(target: "reth::cli", "Unwind stage error: {:?}", err);
                    break
                }
            }
        }

        Ok(())
    }
}

/// `reth stage unwind` subcommand
#[derive(Subcommand, Debug, Eq, PartialEq)]
enum Subcommands {
    /// Unwinds the database until the given block number (range is inclusive).
    #[clap(name = "to-block")]
    ToBlock { target: BlockHashOrNumber },
    /// Unwinds the given number of blocks from the database.
    #[clap(name = "num-blocks")]
    NumBlocks { amount: u64 },
}

impl Subcommands {
    /// Returns the block range to unwind.
    ///
    /// This returns an inclusive range: [target..=latest]
    fn unwind_range<DB: Database>(&self, db: DB) -> eyre::Result<RangeInclusive<u64>> {
        let tx = db.tx()?;
        let mut cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let last = cursor.last()?.ok_or_else(|| eyre::eyre!("No blocks in database"))?;

        let target = match self {
            Subcommands::ToBlock { target } => match target {
                BlockHashOrNumber::Hash(hash) => tx
                    .get::<tables::HeaderNumbers>(*hash)?
                    .ok_or_else(|| eyre::eyre!("Block hash not found in database: {hash:?}"))?,
                BlockHashOrNumber::Number(num) => *num,
            },
            Subcommands::NumBlocks { amount } => last.0.saturating_sub(*amount),
        } + 1;
        Ok(target..=last.0)
    }
}

#[derive(Debug, Clone)]
struct MockHeaderDownloader;

impl HeaderDownloader for MockHeaderDownloader {
    fn update_local_head(&mut self, _head: SealedHeader) {}

    fn update_sync_target(&mut self, _target: SyncTarget) {}

    fn set_batch_size(&mut self, _limit: usize) {}
}

impl Stream for MockHeaderDownloader {
    type Item = HeadersDownloaderResult<Vec<SealedHeader>>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}
#[derive(Debug, Clone)]
struct MockBodyDownloader;

impl BodyDownloader for MockBodyDownloader {
    fn set_download_range(&mut self, _range: RangeInclusive<BlockNumber>) -> DownloadResult<()> {
        Ok(())
    }
}

impl Stream for MockBodyDownloader {
    type Item = BodyDownloaderResult;
    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unwind() {
        let cmd = Command::parse_from(["reth", "--datadir", "dir", "to-block", "100"]);
        assert_eq!(cmd.command, Subcommands::ToBlock { target: BlockHashOrNumber::Number(100) });

        let cmd = Command::parse_from(["reth", "--datadir", "dir", "num-blocks", "100"]);
        assert_eq!(cmd.command, Subcommands::NumBlocks { amount: 100 });
    }
}
