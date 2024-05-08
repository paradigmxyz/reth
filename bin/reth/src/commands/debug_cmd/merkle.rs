//! Command for debugging merkle trie calculation.

use crate::{
    args::{
        get_secret_key,
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs, NetworkArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    macros::block_executor,
    utils::get_single_header,
};
use backon::{ConstantBuilder, Retryable};
use clap::Parser;
use reth_beacon_consensus::EthBeaconConsensus;
use reth_cli_runner::CliContext;
use reth_config::Config;
use reth_consensus::Consensus;
use reth_db::{cursor::DbCursorRO, init_db, tables, transaction::DbTx, DatabaseEnv};
use reth_evm::execute::{BatchBlockExecutionOutput, BatchExecutor, BlockExecutorProvider};
use reth_interfaces::p2p::full_block::FullBlockClient;
use reth_network::NetworkHandle;
use reth_network_api::NetworkInfo;
use reth_primitives::{fs, stage::StageCheckpoint, BlockHashOrNumber, ChainSpec, PruneModes};
use reth_provider::{
    BlockNumReader, BlockWriter, BundleStateWithReceipts, HeaderProvider, LatestStateProviderRef,
    OriginalValuesKnown, ProviderError, ProviderFactory, StateWriter,
};
use reth_revm::database::StateProviderDatabase;
use reth_stages::{
    stages::{AccountHashingStage, MerkleStage, StorageHashingStage},
    ExecInput, Stage,
};
use reth_tasks::TaskExecutor;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tracing::*;

/// `reth debug merkle` command
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
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    #[command(flatten)]
    db: DatabaseArgs,

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

impl Command {
    async fn build_network(
        &self,
        config: &Config,
        task_executor: TaskExecutor,
        db: Arc<DatabaseEnv>,
        network_secret_path: PathBuf,
        default_peers_path: PathBuf,
    ) -> eyre::Result<NetworkHandle> {
        let secret_key = get_secret_key(&network_secret_path)?;
        let network = self
            .network
            .network_config(config, self.chain.clone(), secret_key, default_peers_path)
            .with_task_executor(Box::new(task_executor))
            .listener_addr(SocketAddr::new(self.network.addr, self.network.port))
            .discovery_addr(SocketAddr::new(
                self.network.discovery.addr,
                self.network.discovery.port,
            ))
            .build(ProviderFactory::new(
                db,
                self.chain.clone(),
                self.datadir.unwrap_or_chain_default(self.chain.chain).static_files(),
            )?)
            .start_network()
            .await?;
        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        Ok(network)
    }

    /// Execute `merkle-debug` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        let config = Config::default();

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db();
        fs::create_dir_all(&db_path)?;

        // initialize the database
        let db = Arc::new(init_db(db_path, self.db.database_args())?);
        let factory = ProviderFactory::new(&db, self.chain.clone(), data_dir.static_files())?;
        let provider_rw = factory.provider_rw()?;

        // Configure and build network
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret());
        let network = self
            .build_network(
                &config,
                ctx.task_executor.clone(),
                db.clone(),
                network_secret_path,
                data_dir.known_peers(),
            )
            .await?;

        let executor_provider = block_executor!(self.chain.clone());

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
        .retry(&backoff)
        .notify(|err, _| warn!(target: "reth::cli", "Error requesting header: {err}. Retrying..."))
        .await?;
        info!(target: "reth::cli", target_block_number=self.to, "Finished downloading tip of block range");

        // build the full block client
        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(Arc::clone(&self.chain)));
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

            provider_rw.insert_block(sealed_block.clone(), None)?;

            td += sealed_block.difficulty;
            let mut executor = executor_provider.batch_executor(
                StateProviderDatabase::new(LatestStateProviderRef::new(
                    provider_rw.tx_ref(),
                    provider_rw.static_file_provider().clone(),
                )),
                PruneModes::none(),
            );
            executor.execute_one((&sealed_block.clone().unseal(), td).into())?;
            let BatchBlockExecutionOutput { bundle, receipts, first_block } = executor.finalize();
            BundleStateWithReceipts::new(bundle, receipts, first_block).write_to_storage(
                provider_rw.tx_ref(),
                None,
                OriginalValuesKnown::Yes,
            )?;

            let checkpoint = Some(StageCheckpoint::new(block_number - 1));

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

            // Stoarge trie
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
