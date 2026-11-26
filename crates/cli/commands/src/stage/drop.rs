//! Database debugging tool
use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::Parser;
use reth_chainspec::EthChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_db::{mdbx::tx::Tx, DatabaseError};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_db_common::{
    init::{
        insert_genesis_account_history, insert_genesis_header, insert_genesis_state,
        insert_genesis_storage_history,
    },
    DbTool,
};
use reth_node_api::{HeaderTy, ReceiptTy, TxTy};
use reth_node_core::args::StageEnum;
use reth_provider::{
    DBProvider, RocksDBProviderFactory, StaticFileProviderFactory, StaticFileWriter,
    StorageSettingsCache,
};
use reth_prune::PruneSegment;
use reth_stages::StageId;
use reth_static_file_types::StaticFileSegment;
use std::sync::Arc;

/// `reth drop-stage` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    stage: StageEnum,
}

impl<C: ChainSpecParser> Command<C> {
    /// Execute `db` command
    pub async fn execute<N: CliNodeTypes>(self, runtime: reth_tasks::Runtime) -> eyre::Result<()>
    where
        C: ChainSpecParser<ChainSpec = N::ChainSpec>,
    {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RW, runtime)?;

        let tool = DbTool::new(provider_factory)?;

        let static_file_segments = match self.stage {
            StageEnum::Headers => vec![StaticFileSegment::Headers],
            StageEnum::Bodies => vec![StaticFileSegment::Transactions],
            StageEnum::Execution => vec![
                StaticFileSegment::Receipts,
                StaticFileSegment::AccountChangeSets,
                StaticFileSegment::StorageChangeSets,
            ],
            StageEnum::Senders => vec![StaticFileSegment::TransactionSenders],
            _ => vec![],
        };

        // Calling `StaticFileProviderRW::prune_*` will instruct the writer to prune rows only
        // when `StaticFileProviderRW::commit` is called. We need to do that instead of
        // deleting the jar files, otherwise if the task were to be interrupted after we
        // have deleted them, BUT before we have committed the checkpoints to the database, we'd
        // lose essential data.
        let static_file_provider = tool.provider_factory.static_file_provider();
        for segment in static_file_segments {
            if let Some(highest_block) = static_file_provider.get_highest_static_file_block(segment)
            {
                let mut writer = static_file_provider.latest_writer(segment)?;

                match segment {
                    StaticFileSegment::Headers => {
                        writer.prune_headers(highest_block)?;
                    }
                    StaticFileSegment::Transactions => {
                        let to_delete = static_file_provider
                            .get_highest_static_file_tx(segment)
                            .map(|tx_num| tx_num + 1)
                            .unwrap_or_default();
                        writer.prune_transactions(to_delete, 0)?;
                    }
                    StaticFileSegment::Receipts => {
                        let to_delete = static_file_provider
                            .get_highest_static_file_tx(segment)
                            .map(|tx_num| tx_num + 1)
                            .unwrap_or_default();
                        writer.prune_receipts(to_delete, 0)?;
                    }
                    StaticFileSegment::TransactionSenders => {
                        let to_delete = static_file_provider
                            .get_highest_static_file_tx(segment)
                            .map(|tx_num| tx_num + 1)
                            .unwrap_or_default();
                        writer.prune_transaction_senders(to_delete, 0)?;
                    }
                    StaticFileSegment::AccountChangeSets => {
                        writer.prune_account_changesets(highest_block)?;
                    }
                    StaticFileSegment::StorageChangeSets => {
                        writer.prune_storage_changesets(highest_block)?;
                    }
                }
            }
        }

        let provider_rw = tool.provider_factory.unwind_provider_rw()?;
        let tx = provider_rw.tx_ref();
        let storage_v2 = provider_rw.cached_storage_settings().storage_v2;

        match self.stage {
            StageEnum::Headers => {
                tx.clear::<tables::CanonicalHeaders>()?;
                tx.clear::<tables::Headers<HeaderTy<N>>>()?;
                tx.clear::<tables::HeaderNumbers>()?;
                reset_stage_checkpoint(tx, StageId::Headers)?;

                insert_genesis_header(&provider_rw, &self.env.chain)?;
            }
            StageEnum::Bodies => {
                tx.clear::<tables::BlockBodyIndices>()?;
                tx.clear::<tables::Transactions<TxTy<N>>>()?;

                tx.clear::<tables::TransactionBlocks>()?;
                tx.clear::<tables::BlockOmmers<HeaderTy<N>>>()?;
                tx.clear::<tables::BlockWithdrawals>()?;
                reset_stage_checkpoint(tx, StageId::Bodies)?;

                insert_genesis_header(&provider_rw, &self.env.chain)?;
            }
            StageEnum::Senders => {
                tx.clear::<tables::TransactionSenders>()?;
                // Reset pruned numbers to not count them in the next rerun's stage progress
                reset_prune_checkpoint(tx, PruneSegment::SenderRecovery)?;
                reset_stage_checkpoint(tx, StageId::SenderRecovery)?;
            }
            StageEnum::Execution => {
                if provider_rw.cached_storage_settings().use_hashed_state() {
                    tx.clear::<tables::HashedAccounts>()?;
                    tx.clear::<tables::HashedStorages>()?;
                    reset_stage_checkpoint(tx, StageId::AccountHashing)?;
                    reset_stage_checkpoint(tx, StageId::StorageHashing)?;
                } else {
                    tx.clear::<tables::PlainAccountState>()?;
                    tx.clear::<tables::PlainStorageState>()?;
                }
                tx.clear::<tables::AccountChangeSets>()?;
                tx.clear::<tables::StorageChangeSets>()?;
                tx.clear::<tables::Bytecodes>()?;
                tx.clear::<tables::Receipts<ReceiptTy<N>>>()?;

                reset_prune_checkpoint(tx, PruneSegment::Receipts)?;
                reset_prune_checkpoint(tx, PruneSegment::ContractLogs)?;
                reset_stage_checkpoint(tx, StageId::Execution)?;

                let alloc = &self.env.chain.genesis().alloc;
                insert_genesis_state(&provider_rw, alloc.iter())?;
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
            StageEnum::AccountHistory => {
                if !storage_v2 {
                    tx.clear::<tables::AccountsHistory>()?;
                    insert_genesis_account_history(
                        &provider_rw,
                        self.env.chain.genesis().alloc.iter(),
                    )?;
                }

                reset_stage_checkpoint(tx, StageId::IndexAccountHistory)?;
            }
            StageEnum::StorageHistory => {
                if !storage_v2 {
                    tx.clear::<tables::StoragesHistory>()?;
                    insert_genesis_storage_history(
                        &provider_rw,
                        self.env.chain.genesis().alloc.iter(),
                    )?;
                }

                reset_stage_checkpoint(tx, StageId::IndexStorageHistory)?;
            }
            StageEnum::TxLookup => {
                if !storage_v2 {
                    tx.clear::<tables::TransactionHashNumbers>()?;
                }

                reset_prune_checkpoint(tx, PruneSegment::TransactionLookup)?;

                reset_stage_checkpoint(tx, StageId::TransactionLookup)?;
                insert_genesis_header(&provider_rw, &self.env.chain)?;
            }
        }

        tx.put::<tables::StageCheckpoints>(StageId::Finish.to_string(), Default::default())?;

        provider_rw.commit()?;

        // `RocksDB` writes are not part of the MDBX transaction. Clear the `RocksDB` index only
        // after the checkpoint reset commits: if the command stops before the index is rebuilt,
        // the startup consistency check sees checkpoint 0 and clears the stale index. Write the
        // genesis history back after the clear: if the changeset tip is still 0, the startup check
        // returns before it reseeds genesis, and live persistence does not rebuild the index.
        if storage_v2 {
            let rocksdb = tool.provider_factory.rocksdb_provider();
            match self.stage {
                StageEnum::AccountHistory => {
                    rocksdb.clear::<tables::AccountsHistory>()?;
                    let provider_rw = tool.provider_factory.unwind_provider_rw()?;
                    insert_genesis_account_history(
                        &provider_rw,
                        self.env.chain.genesis().alloc.iter(),
                    )?;
                    provider_rw.commit()?;
                }
                StageEnum::StorageHistory => {
                    rocksdb.clear::<tables::StoragesHistory>()?;
                    let provider_rw = tool.provider_factory.unwind_provider_rw()?;
                    insert_genesis_storage_history(
                        &provider_rw,
                        self.env.chain.genesis().alloc.iter(),
                    )?;
                    provider_rw.commit()?;
                }
                StageEnum::TxLookup => rocksdb.clear::<tables::TransactionHashNumbers>()?,
                _ => {}
            }
        }

        Ok(())
    }
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
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
