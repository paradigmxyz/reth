//! Command for debugging merkle trie calculation.
use crate::{
    args::{utils::genesis_value_parser, DatabaseArgs},
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::Parser;
use reth_db::{cursor::DbCursorRO, init_db, tables, transaction::DbTx};
use reth_primitives::{
    fs,
    stage::{StageCheckpoint, StageId},
    ChainSpec, PruneTargets,
};
use reth_provider::{ProviderFactory, StageCheckpointReader};
use reth_stages::{
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, MerkleStage,
        StorageHashingStage,
    },
    ExecInput, PipelineError, Stage,
};
use std::sync::Arc;

/// `reth merkle-debug` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
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
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    #[clap(flatten)]
    db: DatabaseArgs,

    /// The height to finish at
    #[arg(long)]
    to: u64,

    /// The depth after which we should start comparing branch nodes
    #[arg(long)]
    skip_node_depth: Option<usize>,
}

impl Command {
    /// Execute `merkle-debug` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        fs::create_dir_all(&db_path)?;

        let db = Arc::new(init_db(db_path, self.db.log_level)?);
        let factory = ProviderFactory::new(&db, self.chain.clone());
        let provider_rw = factory.provider_rw().map_err(PipelineError::Interface)?;

        let execution_checkpoint_block =
            provider_rw.get_stage_checkpoint(StageId::Execution)?.unwrap_or_default().block_number;
        assert!(execution_checkpoint_block < self.to, "Nothing to run");

        // Check if any of hashing or merkle stages aren't on the same block number as
        // Execution stage or have any intermediate progress.
        let should_reset_stages =
            [StageId::AccountHashing, StageId::StorageHashing, StageId::MerkleExecute]
                .into_iter()
                .map(|stage_id| provider_rw.get_stage_checkpoint(stage_id))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(Option::unwrap_or_default)
                .any(|checkpoint| {
                    checkpoint.block_number != execution_checkpoint_block ||
                        checkpoint.stage_checkpoint.is_some()
                });

        let factory = reth_revm::Factory::new(self.chain.clone());
        let mut execution_stage = ExecutionStage::new(
            factory,
            ExecutionStageThresholds { max_blocks: Some(1), max_changes: None },
            PruneTargets::all(),
        );

        let mut account_hashing_stage = AccountHashingStage::default();
        let mut storage_hashing_stage = StorageHashingStage::default();
        let mut merkle_stage = MerkleStage::default_execution();

        for block in execution_checkpoint_block + 1..=self.to {
            tracing::trace!(target: "reth::cli", block, "Executing block");
            let progress =
                if (!should_reset_stages || block > execution_checkpoint_block + 1) && block > 0 {
                    Some(block - 1)
                } else {
                    None
                };

            execution_stage
                .execute(
                    &provider_rw,
                    ExecInput {
                        target: Some(block),
                        checkpoint: block.checked_sub(1).map(StageCheckpoint::new),
                    },
                )
                .await?;

            let mut account_hashing_done = false;
            while !account_hashing_done {
                let output = account_hashing_stage
                    .execute(
                        &provider_rw,
                        ExecInput {
                            target: Some(block),
                            checkpoint: progress.map(StageCheckpoint::new),
                        },
                    )
                    .await?;
                account_hashing_done = output.done;
            }

            let mut storage_hashing_done = false;
            while !storage_hashing_done {
                let output = storage_hashing_stage
                    .execute(
                        &provider_rw,
                        ExecInput {
                            target: Some(block),
                            checkpoint: progress.map(StageCheckpoint::new),
                        },
                    )
                    .await?;
                storage_hashing_done = output.done;
            }

            let incremental_result = merkle_stage
                .execute(
                    &provider_rw,
                    ExecInput {
                        target: Some(block),
                        checkpoint: progress.map(StageCheckpoint::new),
                    },
                )
                .await;

            if incremental_result.is_err() {
                tracing::warn!(target: "reth::cli", block, "Incremental calculation failed, retrying from scratch");
                let incremental_account_trie = provider_rw
                    .tx_ref()
                    .cursor_read::<tables::AccountsTrie>()?
                    .walk_range(..)?
                    .collect::<Result<Vec<_>, _>>()?;
                let incremental_storage_trie = provider_rw
                    .tx_ref()
                    .cursor_dup_read::<tables::StoragesTrie>()?
                    .walk_range(..)?
                    .collect::<Result<Vec<_>, _>>()?;

                let clean_input = ExecInput { target: Some(block), checkpoint: None };
                loop {
                    let clean_result = merkle_stage.execute(&provider_rw, clean_input).await;
                    assert!(clean_result.is_ok(), "Clean state root calculation failed");
                    if clean_result.unwrap().done {
                        break
                    }
                }

                let clean_account_trie = provider_rw
                    .tx_ref()
                    .cursor_read::<tables::AccountsTrie>()?
                    .walk_range(..)?
                    .collect::<Result<Vec<_>, _>>()?;
                let clean_storage_trie = provider_rw
                    .tx_ref()
                    .cursor_dup_read::<tables::StoragesTrie>()?
                    .walk_range(..)?
                    .collect::<Result<Vec<_>, _>>()?;

                tracing::info!(target: "reth::cli", block, "Comparing incremental trie vs clean trie");

                // Account trie
                let mut incremental_account_mismatched = Vec::new();
                let mut clean_account_mismatched = Vec::new();
                let mut incremental_account_trie_iter =
                    incremental_account_trie.into_iter().peekable();
                let mut clean_account_trie_iter = clean_account_trie.into_iter().peekable();
                while incremental_account_trie_iter.peek().is_some() ||
                    clean_account_trie_iter.peek().is_some()
                {
                    match (incremental_account_trie_iter.next(), clean_account_trie_iter.next()) {
                        (Some(incremental), Some(clean)) => {
                            pretty_assertions::assert_eq!(
                                incremental.0,
                                clean.0,
                                "Nibbles don't match"
                            );
                            if incremental.1 != clean.1 &&
                                clean.0.inner.len() > self.skip_node_depth.unwrap_or_default()
                            {
                                incremental_account_mismatched.push(incremental);
                                clean_account_mismatched.push(clean);
                            }
                        }
                        (Some(incremental), None) => {
                            tracing::warn!(target: "reth::cli", next = ?incremental, "Incremental account trie has more entries");
                        }
                        (None, Some(clean)) => {
                            tracing::warn!(target: "reth::cli", next = ?clean, "Clean account trie has more entries");
                        }
                        (None, None) => {
                            tracing::info!(target: "reth::cli", "Exhausted all account trie entries");
                        }
                    }
                }

                // Stoarge trie
                let mut first_mismatched_storage = None;
                let mut incremental_storage_trie_iter =
                    incremental_storage_trie.into_iter().peekable();
                let mut clean_storage_trie_iter = clean_storage_trie.into_iter().peekable();
                while incremental_storage_trie_iter.peek().is_some() ||
                    clean_storage_trie_iter.peek().is_some()
                {
                    match (incremental_storage_trie_iter.next(), clean_storage_trie_iter.next()) {
                        (Some(incremental), Some(clean)) => {
                            if incremental != clean &&
                                clean.1.nibbles.inner.len() >
                                    self.skip_node_depth.unwrap_or_default()
                            {
                                first_mismatched_storage = Some((incremental, clean));
                                break
                            }
                        }
                        (Some(incremental), None) => {
                            tracing::warn!(target: "reth::cli", next = ?incremental, "Incremental storage trie has more entries");
                        }
                        (None, Some(clean)) => {
                            tracing::warn!(target: "reth::cli", next = ?clean, "Clean storage trie has more entries")
                        }
                        (None, None) => {
                            tracing::info!(target: "reth::cli", "Exhausted all storage trie entries.")
                        }
                    }
                }

                pretty_assertions::assert_eq!(
                    (
                        incremental_account_mismatched,
                        first_mismatched_storage.as_ref().map(|(incremental, _)| incremental)
                    ),
                    (
                        clean_account_mismatched,
                        first_mismatched_storage.as_ref().map(|(_, clean)| clean)
                    ),
                    "Mismatched trie nodes"
                );
            }
        }

        Ok(())
    }
}
