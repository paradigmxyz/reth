//! Command for generating execution DAGs.
use crate::{
    args::{utils::genesis_value_parser, DatabaseArgs},
    dirs::{DataDirPath, MaybePlatformPath},
    runner::CliContext,
};
use clap::Parser;
use itertools::Itertools;
use reth_db::init_db;
use reth_interfaces::RethError;
use reth_primitives::{
    fs, stage::StageId, Address, Block, BlockNumber, ChainSpec, KECCAK_EMPTY, U256,
};
use reth_provider::{
    BlockReader, HeaderProvider, HistoricalStateProvider, ProviderError, ProviderFactory,
    StageCheckpointReader, TransactionVariant,
};
use reth_revm::{
    database::StateProviderDatabase,
    db::{
        states::{bundle_state::BundleRetention, CacheAccount},
        AccountRevert, AccountStatus, State,
    },
    parallel::{
        executor::{DatabaseRefBox, ParallelExecutor},
        queue::{BlockQueue, BlockQueueStore},
        resolve_block_dependencies,
    },
};
use reth_stages::PipelineError;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
};
use tracing::*;

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

    /// The start block of the range.
    #[arg(long)]
    from: u64,

    /// The end block of the range.
    #[arg(long)]
    to: u64,

    // TODO: write files at interval
    /// The block interval for sync and unwind.
    /// Defaults to `1000`.
    #[arg(long, default_value = "1000")]
    interval: u64,

    /// Flag indicating whether results should be validated.
    #[arg(long)]
    validate: bool,

    /// Path for writing the output file.
    #[arg(long)]
    out: PathBuf,
}

impl Command {
    /// Execute `parallel-execution generate` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        if self.from > self.to {
            eyre::bail!("Invalid block range provided")
        }

        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        fs::create_dir_all(&db_path)?;
        let db = Arc::new(init_db(db_path, self.db.log_level)?);

        let factory = ProviderFactory::new(&db, self.chain.clone());
        let provider = factory.provider().map_err(PipelineError::Interface)?;

        let latest_block_number =
            provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
        if latest_block_number.unwrap_or_default() < self.to {
            eyre::bail!("Block range end is higher than the node block height")
        }

        let mut queues = BTreeMap::<BlockNumber, BlockQueue>::default();
        for block_number in self.from..=self.to {
            debug!(target: "reth::cli", block_number, "Executing transactions");

            let td = provider
                .header_td_by_number(block_number)?
                .ok_or(ProviderError::TotalDifficultyNotFound { block_number })?;
            let (block, senders) = provider
                .block_with_senders(block_number, TransactionVariant::WithHash)?
                .ok_or(ProviderError::BlockNotFound(block_number.into()))?
                .into_components();

            if block.body.is_empty() {
                continue
            }

            let provider = factory.provider().map_err(PipelineError::Interface)?;
            let sp = HistoricalStateProvider::new(provider.into_tx(), block_number);
            let sp_database = Box::new(StateProviderDatabase(&sp));

            let (rw_sets, mut state) =
                resolve_block_dependencies(&self.chain, sp_database.clone(), &block, &senders, td)?;

            for (tx_idx, rw_set) in rw_sets.iter().enumerate() {
                let hash = block.body.get(tx_idx).expect("exists").hash;
                tracing::trace!(
                    target: "reth::cli",
                    block_number, tx_idx, %hash, ?rw_set,
                    "Generated transaction rw set"
                );
            }

            let queue = BlockQueue::resolve(&rw_sets);

            if self.validate {
                tracing::debug!(target: "reth::cli", block_number = block.number, ?queue, "Validating parallel execution");

                state.merge_transitions(BundleRetention::Reverts);

                // Patch expected state by removing unchanged storage entries.
                let mut removed = 0;
                for (_, account) in &mut state.bundle_state.state {
                    account.storage.retain(|_, value| {
                        if value.is_changed() {
                            true
                        } else {
                            removed += 1;
                            false
                        }
                    });
                }
                state.bundle_state.state_size -= removed;

                self.validate(sp_database, &block, td, senders, queue.clone(), state).await?;

                tracing::debug!(target: "reth::cli", block_number = block.number, ?queue, "Successfully validated parallel execution");
            }

            if queue.len() != block.body.len() {
                queues.insert(block_number, queue);
            }
        }

        let out_path = self.out.join(format!("parallel-{}-{}.json", self.from, self.to));
        tracing::info!(target: "reth::cli", dest = %out_path.display(), blocks = queues.len(), "Writing execution hints");
        std::fs::write(out_path, serde_json::to_string_pretty(&queues)?)?;

        Ok(())
    }

    async fn validate(
        &self,
        database: DatabaseRefBox<'_, RethError>,
        block: &Block,
        td: U256,
        senders: Vec<Address>,
        queue: BlockQueue,
        expected: State<Box<dyn reth_revm::Database<Error = RethError> + Send + '_>>,
    ) -> eyre::Result<()> {
        let mut parallel_executor = ParallelExecutor::new(
            self.chain.clone(),
            BlockQueueStore::new(HashMap::from_iter([(block.number, queue.clone())])),
            database,
            None,
        )?;
        parallel_executor.execute(&block, td, Some(senders)).await?;
        tracing::debug!(target: "reth::cli", block_number = block.number, ?queue, "Successfully executed in parallel");

        let mut parallel_state = parallel_executor.state.write().unwrap();
        parallel_state.merge_transitions(BundleRetention::Reverts);

        let mut parallel_cache_accounts = parallel_state
            .cache
            .accounts
            .clone()
            .into_iter()
            .sorted_by_key(|(address, _)| *address)
            .peekable();
        let mut expected_cache_accounts =
            expected.cache.accounts.into_iter().sorted_by_key(|(address, _)| *address).peekable();
        while parallel_cache_accounts.peek().is_some() || expected_cache_accounts.peek().is_some() {
            let parallel = parallel_cache_accounts
                .next()
                .map(|(address, account)| (address, CacheAccount::from(account)));
            let mut expected = expected_cache_accounts.next();

            // Due to how transitions are applied, the parallel executor will not produce a
            // destroyed account if it was created in the same block. If it wasn't
            // created in the same block, this will be caught when asserting transitions.
            if parallel
                .as_ref()
                .map_or(false, |(_, acc)| acc.status == AccountStatus::LoadedNotExisting) &&
                expected
                    .as_ref()
                    .map_or(false, |(_, acc)| acc.status == AccountStatus::Destroyed)
            {
                expected = expected.map(|(address, account)| {
                    (
                        address,
                        CacheAccount {
                            account: account.account,
                            status: AccountStatus::LoadedNotExisting,
                        },
                    )
                });
            }

            pretty_assertions::assert_eq!(parallel, expected, "Cache account mismatch");
        }

        pretty_assertions::assert_eq!(
            BTreeMap::from_iter(
                parallel_state
                    .cache
                    .contracts
                    .clone()
                    .into_iter()
                    .filter(|(code_hash, _)| *code_hash != KECCAK_EMPTY)
            ),
            BTreeMap::from_iter(
                expected
                    .cache
                    .contracts
                    .into_iter()
                    .filter(|(code_hash, _)| *code_hash != KECCAK_EMPTY)
            ),
            "Cache contracts mismatch"
        );

        let parallel_bundle_state = parallel_state.bundle_state.clone();

        // Assert reverts
        let parallel_reverts = BTreeMap::<Address, AccountRevert>::from_iter(
            parallel_bundle_state.reverts.clone().pop().unwrap(),
        );
        let regular_reverts = BTreeMap::<Address, AccountRevert>::from_iter(
            expected.bundle_state.reverts.clone().pop().unwrap(),
        );
        pretty_assertions::assert_eq!(parallel_reverts, regular_reverts, "Bundle reverts mismatch");

        // Assert state & contracts
        pretty_assertions::assert_eq!(
            BTreeMap::from_iter(parallel_bundle_state.state.clone()),
            BTreeMap::from_iter(expected.bundle_state.state),
            "Bundle state mismatch"
        );
        pretty_assertions::assert_eq!(
            BTreeMap::from_iter(parallel_bundle_state.contracts),
            BTreeMap::from_iter(expected.bundle_state.contracts),
            "Bundle contracts mismatch"
        );

        // Assert sizes
        assert_eq!(
            parallel_bundle_state.state_size, expected.bundle_state.state_size,
            "State size mismatch"
        );
        assert_eq!(
            parallel_bundle_state.reverts_size, expected.bundle_state.reverts_size,
            "Reverts size mismatch"
        );

        Ok(())
    }
}
