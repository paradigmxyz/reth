//! Database debugging tool
use crate::common::{AccessRights, Environment, EnvironmentArgs};
use clap::Parser;
use itertools::Itertools;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_db::{mdbx::tx::Tx, static_file::iter_static_files, tables, DatabaseError};
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_db_common::{
    init::{insert_genesis_header, insert_genesis_history, insert_genesis_state},
    DbTool,
};
use reth_node_builder::NodeTypesWithEngine;
use reth_node_core::args::StageEnum;
use reth_provider::{writer::UnifiedStorageWriter, StaticFileProviderFactory};
use reth_prune::PruneSegment;
use reth_stages::StageId;
use reth_static_file_types::StaticFileSegment;

/// `reth drop-stage` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    stage: StageEnum,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `db` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
    ) -> eyre::Result<()> {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RW)?;

        let static_file_provider = provider_factory.static_file_provider();

        let tool = DbTool::new(provider_factory)?;

        let static_file_segment = match self.stage {
            StageEnum::Headers => Some(StaticFileSegment::Headers),
            StageEnum::Bodies => Some(StaticFileSegment::Transactions),
            StageEnum::Execution => Some(StaticFileSegment::Receipts),
            _ => None,
        };

        // Delete static file segment data before inserting the genesis header below
        if let Some(static_file_segment) = static_file_segment {
            let static_file_provider = tool.provider_factory.static_file_provider();
            let static_files = iter_static_files(static_file_provider.directory())?;
            if let Some(segment_static_files) = static_files.get(&static_file_segment) {
                // Delete static files from the highest to the lowest block range
                for (block_range, _) in segment_static_files
                    .iter()
                    .sorted_by_key(|(block_range, _)| block_range.start())
                    .rev()
                {
                    static_file_provider.delete_jar(static_file_segment, block_range.start())?;
                }
            }
        }

        let provider_rw = tool.provider_factory.provider_rw()?;
        let tx = provider_rw.tx_ref();

        match self.stage {
            StageEnum::Headers => {
                tx.clear::<tables::CanonicalHeaders>()?;
                tx.clear::<tables::Headers>()?;
                tx.clear::<tables::HeaderTerminalDifficulties>()?;
                tx.clear::<tables::HeaderNumbers>()?;
                reset_stage_checkpoint(tx, StageId::Headers)?;

                insert_genesis_header(&provider_rw.0, &static_file_provider, &self.env.chain)?;
            }
            StageEnum::Bodies => {
                tx.clear::<tables::BlockBodyIndices>()?;
                tx.clear::<tables::Transactions>()?;
                reset_prune_checkpoint(tx, PruneSegment::Transactions)?;

                tx.clear::<tables::TransactionBlocks>()?;
                tx.clear::<tables::BlockOmmers>()?;
                tx.clear::<tables::BlockWithdrawals>()?;
                reset_stage_checkpoint(tx, StageId::Bodies)?;

                insert_genesis_header(&provider_rw.0, &static_file_provider, &self.env.chain)?;
            }
            StageEnum::Senders => {
                tx.clear::<tables::TransactionSenders>()?;
                // Reset pruned numbers to not count them in the next rerun's stage progress
                reset_prune_checkpoint(tx, PruneSegment::SenderRecovery)?;
                reset_stage_checkpoint(tx, StageId::SenderRecovery)?;
            }
            StageEnum::Execution => {
                tx.clear::<tables::PlainAccountState>()?;
                tx.clear::<tables::PlainStorageState>()?;
                tx.clear::<tables::AccountChangeSets>()?;
                tx.clear::<tables::StorageChangeSets>()?;
                tx.clear::<tables::Bytecodes>()?;
                tx.clear::<tables::Receipts>()?;

                reset_prune_checkpoint(tx, PruneSegment::Receipts)?;
                reset_prune_checkpoint(tx, PruneSegment::ContractLogs)?;
                reset_stage_checkpoint(tx, StageId::Execution)?;

                let alloc = &self.env.chain.genesis().alloc;
                insert_genesis_state(&provider_rw.0, alloc.iter())?;
            }
            StageEnum::AccountHashing => {
                tx.clear::<tables::HashedAccounts>()?;
                reset_stage_checkpoint(tx, StageId::AccountHashing)?;
            }
            StageEnum::StorageHashing => {
                tx.clear::<tables::HashedStorages>()?;
                reset_stage_checkpoint(tx, StageId::StorageHashing)?;
            }
            StageEnum::Hashing => {
                // Clear hashed accounts
                tx.clear::<tables::HashedAccounts>()?;
                reset_stage_checkpoint(tx, StageId::AccountHashing)?;

                // Clear hashed storages
                tx.clear::<tables::HashedStorages>()?;
                reset_stage_checkpoint(tx, StageId::StorageHashing)?;
            }
            StageEnum::Merkle => {
                tx.clear::<tables::AccountsTrie>()?;
                tx.clear::<tables::StoragesTrie>()?;

                reset_stage_checkpoint(tx, StageId::MerkleExecute)?;
                reset_stage_checkpoint(tx, StageId::MerkleUnwind)?;

                tx.delete::<tables::StageCheckpointProgresses>(
                    StageId::MerkleExecute.to_string(),
                    None,
                )?;
            }
            StageEnum::AccountHistory | StageEnum::StorageHistory => {
                tx.clear::<tables::AccountsHistory>()?;
                tx.clear::<tables::StoragesHistory>()?;

                reset_stage_checkpoint(tx, StageId::IndexAccountHistory)?;
                reset_stage_checkpoint(tx, StageId::IndexStorageHistory)?;

                insert_genesis_history(&provider_rw.0, self.env.chain.genesis().alloc.iter())?;
            }
            StageEnum::TxLookup => {
                tx.clear::<tables::TransactionHashNumbers>()?;
                reset_prune_checkpoint(tx, PruneSegment::TransactionLookup)?;

                reset_stage_checkpoint(tx, StageId::TransactionLookup)?;
                insert_genesis_header(&provider_rw.0, &static_file_provider, &self.env.chain)?;
            }
        }

        tx.put::<tables::StageCheckpoints>(StageId::Finish.to_string(), Default::default())?;

        UnifiedStorageWriter::commit_unwind(provider_rw, static_file_provider)?;

        Ok(())
    }
}

fn reset_prune_checkpoint(
    tx: &Tx<reth_db::mdbx::RW>,
    prune_segment: PruneSegment,
) -> Result<(), DatabaseError> {
    if let Some(mut prune_checkpoint) = tx.get::<tables::PruneCheckpoints>(prune_segment)? {
        prune_checkpoint.block_number = None;
        prune_checkpoint.tx_number = None;
        tx.put::<tables::PruneCheckpoints>(prune_segment, prune_checkpoint)?;
    }

    Ok(())
}

fn reset_stage_checkpoint(
    tx: &Tx<reth_db::mdbx::RW>,
    stage_id: StageId,
) -> Result<(), DatabaseError> {
    tx.put::<tables::StageCheckpoints>(stage_id.to_string(), Default::default())?;

    Ok(())
}
