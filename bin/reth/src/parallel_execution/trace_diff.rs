//! Command for comparing traces between regular and parallel execution.
use crate::{
    args::{utils::genesis_value_parser, DatabaseArgs, ExecutionArgs},
    dirs::{DataDirPath, MaybePlatformPath},
    runner::CliContext,
};
use clap::Parser;
use reth_db::init_db;
use reth_interfaces::RethError;
use reth_primitives::{
    fs,
    revm::env::{fill_cfg_and_block_env, fill_tx_env},
    revm_primitives::Env,
    ChainSpec, TransitionId, B256,
};
use reth_provider::{
    BlockReader, HeaderProvider, HistoricalStateProvider, ProviderError, ProviderFactory,
    TransactionVariant, TransactionsProvider,
};
use reth_revm::{
    database::StateProviderDatabase,
    db::CacheDB,
    parallel::{
        executor::ParallelExecutor,
        queue::{TransitionQueue, TransitionQueueStore},
        resolve_block_dependencies,
    },
    DatabaseRef, State, EVM,
};
use reth_revm_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use reth_rpc::eth::revm_utils::replay_transactions_until;
use reth_rpc_types::trace::geth::{DefaultFrame, GethDefaultTracingOptions};
use reth_stages::PipelineError;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

/// `reth parallel-execution generate` command
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
    /// - holesky
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

    /// Path to the block queues for parallel execution.
    #[arg(long = "parallel-queue-store")]
    queue_store: PathBuf,

    #[arg()]
    transaction_hash: B256,
}

impl Command {
    /// Execute `parallel-execution trace-diff` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        fs::create_dir_all(&db_path)?;
        let db = Arc::new(init_db(db_path, self.db.log_level)?);

        let factory = ProviderFactory::new(&db, self.chain.clone());
        let provider = factory.provider().map_err(PipelineError::Interface)?;

        let tx_id = provider
            .transaction_id(self.transaction_hash)?
            .ok_or(eyre::eyre!("transaction {} not found", self.transaction_hash))?;
        let block_number = provider
            .transaction_block(tx_id)?
            .ok_or(ProviderError::BlockNumberForTransactionIndexNotFound)?;

        let td = provider
            .header_td_by_number(block_number)?
            .ok_or(ProviderError::TotalDifficultyNotFound { block_number })?;
        let (block, senders) = provider
            .block_with_senders(block_number, TransactionVariant::WithHash)?
            .ok_or(ProviderError::BlockNotFound(block_number.into()))?
            .into_components();

        let mut target_env = Env::default();
        fill_cfg_and_block_env(
            &mut target_env.cfg,
            &mut target_env.block,
            &self.chain,
            &block.header,
            td,
        );

        assert!(!block.body.is_empty(), "block is not empty");

        let target_tx_idx = block
            .body
            .iter()
            .position(|tx| tx.hash == self.transaction_hash)
            .ok_or(eyre::eyre!("target transaction hash is missing from the body"))?;
        let target_tx = block.body.get(target_tx_idx).expect("exists");
        let target_sender = *senders.get(target_tx_idx).expect("exists");
        fill_tx_env(&mut target_env.tx, target_tx, target_sender);

        let provider = factory.provider().map_err(PipelineError::Interface)?;
        let sp = HistoricalStateProvider::new(provider.into_tx(), block_number);
        let sp_database = Box::new(StateProviderDatabase::new(&sp));

        // replay all transactions prior to the targeted transaction
        let mut db = CacheDB::new(sp_database.clone());
        replay_transactions_until(
            &mut db,
            target_env.cfg.clone(),
            target_env.block.clone(),
            block.body.clone(),
            self.transaction_hash,
        )?;
        let expected_trace = self.inspect_target(db, target_env.clone())?;

        let queue_store = Arc::new(TransitionQueueStore::new(self.queue_store.clone()));
        let mut parallel_executor = ParallelExecutor::new(
            factory.clone(),
            self.chain.clone(),
            Box::new(ctx.task_executor.clone()),
            queue_store.clone(), // unused
            sp_database,
            0,
            0,
            None,
        )?;

        // Execute batches until the target tx
        let queue = queue_store.load(1..=block_number)?.ok_or(eyre::eyre!("queue not found"))?;
        for batch in queue.batches() {
            if batch.contains(&TransitionId::transaction(block_number, target_tx_idx as u32)) {
                break
            }

            tracing::trace!(target: "reth::cli", ?batch, "Executing transaction batch");
            parallel_executor.execute_batch(&batch)?;
        }

        let parallel_trace = self.inspect_target(parallel_executor.state(), target_env)?;

        let mut position = 0;
        let mut expected_struct_log_iter = expected_trace.struct_logs.into_iter().peekable();
        let mut parallel_struct_log_iter = parallel_trace.struct_logs.into_iter().peekable();
        while expected_struct_log_iter.peek().is_some() || parallel_struct_log_iter.peek().is_some()
        {
            let expected = expected_struct_log_iter.next();
            pretty_assertions::assert_eq!(
                expected,
                parallel_struct_log_iter.next(),
                "struct log at position {position} does not match"
            );
            tracing::trace!(target: "reth::cli", struct_log = ?expected, position, "Struct log matched");
            position += 1;
        }

        assert_eq!(expected_trace.failed, parallel_trace.failed, "`failed` flag mismatch");
        assert_eq!(expected_trace.gas, parallel_trace.gas, "gas mismatch");
        assert_eq!(
            expected_trace.return_value, parallel_trace.return_value,
            "return value mismatch"
        );

        tracing::info!(target: "reth::cli", "Transaction traces match");

        Ok(())
    }

    fn inspect_target<DB: DatabaseRef<Error = RethError>>(
        &self,
        database: DB,
        env: Env,
    ) -> eyre::Result<DefaultFrame> {
        let config = GethDefaultTracingOptions::default();
        let mut inspector =
            TracingInspector::new(TracingInspectorConfig::from_geth_config(&config));

        let mut evm = EVM::with_env(env);
        evm.database(database);
        let result = evm.inspect_ref(&mut inspector)?;

        let gas_used = result.result.gas_used();
        let return_value = result.result.into_output().unwrap_or_default();
        Ok(inspector.into_geth_builder().geth_traces(gas_used, return_value, config))
    }
}
