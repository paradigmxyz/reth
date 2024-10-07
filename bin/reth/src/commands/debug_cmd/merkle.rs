//! Command for debugging merkle trie calculation.
use crate::{args::NetworkArgs, utils::get_single_header};
use backon::{ConstantBuilder, Retryable};
use clap::Parser;
use reth_beacon_consensus::EthBeaconConsensus;
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_cli_runner::CliContext;
use reth_cli_util::get_secret_key;
use reth_config::Config;
use reth_consensus::Consensus;
use reth_db::tables;
use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
use reth_evm::execute::{BatchExecutor, BlockExecutorProvider};
use reth_network::{BlockDownloaderProvider, NetworkHandle};
use reth_network_api::NetworkInfo;
use reth_network_p2p::full_block::FullBlockClient;
use reth_node_api::{NodeTypesWithDB, NodeTypesWithEngine};
use reth_node_ethereum::EthExecutorProvider;
use reth_primitives::BlockHashOrNumber;
use reth_provider::{
    writer::UnifiedStorageWriter, BlockNumReader, BlockWriter, ChainSpecProvider,
    DatabaseProviderFactory, HeaderProvider, LatestStateProviderRef, OriginalValuesKnown,
    ProviderError, ProviderFactory, StateWriter, StaticFileProviderFactory,
};
use reth_revm::database::StateProviderDatabase;
use reth_stages::{
    stages::{AccountHashingStage, MerkleStage, StorageHashingStage},
    ExecInput, Stage, StageCheckpoint,
};
use reth_tasks::TaskExecutor;
use std::{path::PathBuf, sync::Arc};
use tracing::*;

/// `reth debug merkle` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[command(flatten)]
    network: NetworkArgs,

    /// The number of retries per request
    #[arg(long, default_value = "5")]
    retries: usize,

    /// The height to finish at
    #[arg(long)]
    to: u64,

    /// The depth after which we should start comparing branch nodes
    #[arg(long)]
    skip_node_depth: Option<usize>,
}

impl<C: ChainSpecParser<ChainSpec = ChainSpec>> Command<C> {
    async fn build_network<N: NodeTypesWithDB<ChainSpec = C::ChainSpec>>(
        &self,
        config: &Config,
        task_executor: TaskExecutor,
        provider_factory: ProviderFactory<N>,
        network_secret_path: PathBuf,
        default_peers_path: PathBuf,
    ) -> eyre::Result<NetworkHandle> {
        let secret_key = get_secret_key(&network_secret_path)?;
        let network = self
            .network
            .network_config(config, provider_factory.chain_spec(), secret_key, default_peers_path)
            .with_task_executor(Box::new(task_executor))
            .build(provider_factory)
            .start_network()
            .await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        Ok(network)
    }

    /// Execute `merkle-debug` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
        ctx: CliContext,
    ) -> eyre::Result<()> {
        let Environment { provider_factory, config, data_dir } =
            self.env.init::<N>(AccessRights::RW)?;

        let provider_rw = provider_factory.database_provider_rw()?;

        // Configure and build network
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret());
        let network = self
            .build_network(
                &config,
                ctx.task_executor.clone(),
                provider_factory.clone(),
                network_secret_path,
                data_dir.known_peers(),
            )
            .await?;

        let executor_provider = EthExecutorProvider::ethereum(provider_factory.chain_spec());

        // Initialize the fetch client
        info!(target: "reth::cli", target_block_number=self.to, "Downloading tip of block range");
        let fetch_client = network.fetch_client().await?;

        // fetch the header at `self.to`
        let retries = self.retries.max(1);
        let backoff = ConstantBuilder::default().with_max_times(retries);
        let client = fetch_client.clone();
        let to_header = (move || {
            get_single_header(client.clone(), BlockHashOrNumber::Number(self.to))
        })
        .retry(backoff)
        .notify(|err, _| warn!(target: "reth::cli", "Error requesting header: {err}. Retrying..."))
        .await?;
        info!(target: "reth::cli", target_block_number=self.to, "Finished downloading tip of block range");

        // build the full block client
        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));
        let block_range_client = FullBlockClient::new(fetch_client, consensus);

        // get best block number
        let best_block_number = provider_rw.best_block_number()?;
        assert!(best_block_number < self.to, "Nothing to run");

        // get the block range from the network
        let block_range = best_block_number + 1..=self.to;
        info!(target: "reth::cli", ?block_range, "Downloading range of blocks");
        let blocks = block_range_client
            .get_full_block_range(to_header.hash_slow(), self.to - best_block_number)
            .await;

        let mut td = provider_rw
            .header_td_by_number(best_block_number)?
            .ok_or(ProviderError::TotalDifficultyNotFound(best_block_number))?;

        let mut account_hashing_stage = AccountHashingStage::default();
        let mut storage_hashing_stage = StorageHashingStage::default();
        let mut merkle_stage = MerkleStage::default_execution();

        for block in blocks.into_iter().rev() {
            let block_number = block.number;
            let sealed_block = block
                .try_seal_with_senders()
                .map_err(|block| eyre::eyre!("Error sealing block with senders: {block:?}"))?;
            trace!(target: "reth::cli", block_number, "Executing block");

            provider_rw.insert_block(sealed_block.clone())?;

            td += sealed_block.difficulty;
            let mut executor = executor_provider.batch_executor(StateProviderDatabase::new(
                LatestStateProviderRef::new(
                    provider_rw.tx_ref(),
                    provider_rw.static_file_provider().clone(),
                ),
            ));
            executor.execute_and_verify_one((&sealed_block.clone().unseal(), td).into())?;
            let execution_outcome = executor.finalize();

            let mut storage_writer = UnifiedStorageWriter::from_database(&provider_rw);
            storage_writer.write_to_storage(execution_outcome, OriginalValuesKnown::Yes)?;

            let checkpoint = Some(StageCheckpoint::new(
                block_number
                    .checked_sub(1)
                    .ok_or_else(|| eyre::eyre!("GenesisBlockHasNoParent"))?,
            ));

            let mut account_hashing_done = false;
            while !account_hashing_done {
                let output = account_hashing_stage
                    .execute(&provider_rw, ExecInput { target: Some(block_number), checkpoint })?;
                account_hashing_done = output.done;
            }

            let mut storage_hashing_done = false;
            while !storage_hashing_done {
                let output = storage_hashing_stage
                    .execute(&provider_rw, ExecInput { target: Some(block_number), checkpoint })?;
                storage_hashing_done = output.done;
            }

            let incremental_result = merkle_stage
                .execute(&provider_rw, ExecInput { target: Some(block_number), checkpoint });

            if incremental_result.is_ok() {
                debug!(target: "reth::cli", block_number, "Successfully computed incremental root");
                continue
            }

            warn!(target: "reth::cli", block_number, "Incremental calculation failed, retrying from scratch");
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

            let clean_input = ExecInput { target: Some(sealed_block.number), checkpoint: None };
            loop {
                let clean_result = merkle_stage.execute(&provider_rw, clean_input);
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

            info!(target: "reth::cli", block_number, "Comparing incremental trie vs clean trie");

            // Account trie
            let mut incremental_account_mismatched = Vec::new();
            let mut clean_account_mismatched = Vec::new();
            let mut incremental_account_trie_iter = incremental_account_trie.into_iter().peekable();
            let mut clean_account_trie_iter = clean_account_trie.into_iter().peekable();
            while incremental_account_trie_iter.peek().is_some() ||
                clean_account_trie_iter.peek().is_some()
            {
                match (incremental_account_trie_iter.next(), clean_account_trie_iter.next()) {
                    (Some(incremental), Some(clean)) => {
                        similar_asserts::assert_eq!(incremental.0, clean.0, "Nibbles don't match");
                        if incremental.1 != clean.1 &&
                            clean.0 .0.len() > self.skip_node_depth.unwrap_or_default()
                        {
                            incremental_account_mismatched.push(incremental);
                            clean_account_mismatched.push(clean);
                        }
                    }
                    (Some(incremental), None) => {
                        warn!(target: "reth::cli", next = ?incremental, "Incremental account trie has more entries");
                    }
                    (None, Some(clean)) => {
                        warn!(target: "reth::cli", next = ?clean, "Clean account trie has more entries");
                    }
                    (None, None) => {
                        info!(target: "reth::cli", "Exhausted all account trie entries");
                    }
                }
            }

            // Storage trie
            let mut first_mismatched_storage = None;
            let mut incremental_storage_trie_iter = incremental_storage_trie.into_iter().peekable();
            let mut clean_storage_trie_iter = clean_storage_trie.into_iter().peekable();
            while incremental_storage_trie_iter.peek().is_some() ||
                clean_storage_trie_iter.peek().is_some()
            {
                match (incremental_storage_trie_iter.next(), clean_storage_trie_iter.next()) {
                    (Some(incremental), Some(clean)) => {
                        if incremental != clean &&
                            clean.1.nibbles.len() > self.skip_node_depth.unwrap_or_default()
                        {
                            first_mismatched_storage = Some((incremental, clean));
                            break
                        }
                    }
                    (Some(incremental), None) => {
                        warn!(target: "reth::cli", next = ?incremental, "Incremental storage trie has more entries");
                    }
                    (None, Some(clean)) => {
                        warn!(target: "reth::cli", next = ?clean, "Clean storage trie has more entries")
                    }
                    (None, None) => {
                        info!(target: "reth::cli", "Exhausted all storage trie entries.")
                    }
                }
            }

            similar_asserts::assert_eq!(
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

        info!(target: "reth::cli", ?block_range, "Successfully validated incremental roots");

        Ok(())
    }
}
