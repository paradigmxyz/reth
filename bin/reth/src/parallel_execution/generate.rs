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
use reth_primitives::{fs, stage::StageId, Address, BlockNumber, ChainSpec, KECCAK_EMPTY};
use reth_provider::{
    BlockRangeExecutor, BlockReader, HeaderProvider, HistoricalStateProviderRef, ProviderError,
    ProviderFactory, StageCheckpointReader, TransactionVariant,
};
use reth_revm::{
    database::StateProviderDatabase,
    db::{
        states::{bundle_state::BundleRetention, CacheAccount},
        AccountRevert, State,
    },
    parallel::{
        executor::{DatabaseRefBox, ParallelExecutor},
        queue::{TransitionQueue, TransitionQueueStore},
        resolve_block_dependencies,
    },
};
use reth_stages::PipelineError;
use std::{
    collections::{BTreeMap, HashMap},
    ops::RangeInclusive,
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

    /// The block interval for transitions.
    /// Defaults to `100 000`.
    #[arg(long, default_value = "100000")]
    interval: u64,

    /// Maximum batch size for the queue.
    #[arg(long, default_value = "10000")]
    max_batch_size: usize,

    /// Flag indicating whether results should be validated.
    #[arg(long)]
    validate: bool,

    /// Flag indicating whether we should skip account status checks
    #[arg(long, requires = "validate")]
    skip_account_status_validation: bool,

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

        let mut account_status_mismatches = 0;
        let mut start_block = self.from;
        let transition_store = TransitionQueueStore::new(self.out.clone());
        while start_block <= self.to {
            let end_block = self.to.min(start_block + self.interval);
            let range = start_block..=end_block;
            let mut block_rw_sets = HashMap::default();

            let provider = factory.provider().map_err(PipelineError::Interface)?;
            let sp = HistoricalStateProviderRef::new(provider.tx_ref(), start_block);
            let sp_database = Box::new(StateProviderDatabase(&sp)); // TODO:
            let mut state = State::builder()
                .with_database_boxed(sp_database.clone())
                .with_bundle_update()
                .without_state_clear()
                .build();

            for block_number in range.clone() {
                debug!(target: "reth::cli", block_number, "Executing block");

                let td = provider
                    .header_td_by_number(block_number)?
                    .ok_or(ProviderError::TotalDifficultyNotFound { block_number })?;
                let (block, senders) = provider
                    .block_with_senders(block_number, TransactionVariant::WithHash)?
                    .ok_or(ProviderError::BlockNotFound(block_number.into()))?
                    .into_components();

                let block_rw_set =
                    resolve_block_dependencies(&mut state, &self.chain, &block, &senders, td)?;
                state.merge_transitions(BundleRetention::Reverts);

                for (tx_idx, rw_set) in block_rw_set.transactions.iter().enumerate() {
                    let hash = block.body.get(tx_idx).expect("exists").hash;
                    tracing::trace!(
                        target: "reth::cli",
                        block_number, tx_idx, %hash, ?rw_set,
                        "Generated transaction rw set"
                    );
                }

                block_rw_sets.insert(block.number, block_rw_set);
            }

            tracing::debug!(target: "reth::cli", ?range, "Resolving range dependencies");
            let queue = TransitionQueue::resolve(range.clone(), block_rw_sets, self.max_batch_size);
            transition_store.save(queue.clone())?;

            if self.validate {
                tracing::debug!(target: "reth::cli", ?range, ?queue, "Validating parallel execution");
                account_status_mismatches +=
                    self.validate(&provider, sp_database, range.clone(), state)?;
                tracing::debug!(target: "reth::cli", ?range, ?queue, "Successfully validated parallel execution");
            }

            start_block = end_block + 1;
        }

        if self.validate && account_status_mismatches > 0 {
            tracing::warn!(target: "reth::cli", count = account_status_mismatches, "Account status mismatches were observed");
        }

        Ok(())
    }

    /// Returns the number of times the account status was mismatched.
    fn validate<Provider: BlockReader>(
        &self,
        provider: Provider,
        database: DatabaseRefBox<'_, RethError>,
        range: RangeInclusive<BlockNumber>,
        mut expected: State<Box<dyn reth_revm::Database<Error = RethError> + Send + '_>>,
    ) -> eyre::Result<u64> {
        let mut account_status_mismatches = 0;

        let mut parallel_executor = ParallelExecutor::new(
            provider,
            self.chain.clone(),
            Arc::new(TransitionQueueStore::new(self.out.clone())),
            database,
            None,
        )?;
        parallel_executor.execute_range(range.clone(), true)?;
        tracing::debug!(target: "reth::cli", ?range, "Successfully executed in parallel");

        let parallel_state = parallel_executor.state();

        // PATCH: remove unchanged storage entries.
        let mut removed = 0;
        for (_, account) in &mut expected.bundle_state.state {
            account.storage.retain(|_, value| {
                if value.is_changed() {
                    true
                } else {
                    removed += 1;
                    false
                }
            });
        }
        expected.bundle_state.state_size -= removed;

        let mut parallel_cache_accounts = parallel_state
            .read()
            .cache
            .accounts
            .read()
            .clone()
            .into_iter()
            .sorted_by_key(|(address, _)| *address)
            .peekable();
        let mut expected_cache_accounts =
            expected.cache.accounts.into_iter().sorted_by_key(|(address, _)| *address).peekable();
        while parallel_cache_accounts.peek().is_some() || expected_cache_accounts.peek().is_some() {
            let (parallel_address, parallel_account) = parallel_cache_accounts
                .next()
                .map(|(address, account)| (address, CacheAccount::from(account)))
                .unzip();
            let (expected_address, expected_account) = expected_cache_accounts.next().unzip();

            pretty_assertions::assert_eq!(
                parallel_address,
                expected_address,
                "Cache account address mismatch"
            );
            // Account status
            let parallel_account_status = parallel_account.as_ref().map(|acc| acc.status);
            let expected_account_status = expected_account.as_ref().map(|acc| acc.status);
            if self.skip_account_status_validation {
                if parallel_account_status != expected_account_status {
                    // Account status mismatch is a soft error.
                    // Most importantly, the transitions must match.
                    account_status_mismatches += 1;
                    tracing::warn!(
                        target: "reth::cli",
                        ?range,
                        address = ?expected_address,
                        ?parallel_account_status,
                        ?expected_account_status,
                        "Cache account status mismatch"
                    );
                }
                pretty_assertions::assert_eq!(
                    parallel_account.map(|acc| acc.account),
                    expected_account.map(|acc| acc.account),
                    "Cache account mismatch {expected_address:?}"
                );
            } else {
                pretty_assertions::assert_eq!(
                    parallel_account,
                    expected_account,
                    "Cache account mismatch {expected_address:?}"
                );
            }
        }

        pretty_assertions::assert_eq!(
            BTreeMap::from_iter(
                parallel_state
                    .read()
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

        let parallel_bundle_state = parallel_state.read().bundle_state.clone();

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
            BTreeMap::from_iter(
                parallel_bundle_state
                    .contracts
                    .into_iter()
                    .filter(|(code_hash, _)| *code_hash != KECCAK_EMPTY)
            ),
            BTreeMap::from_iter(
                expected
                    .bundle_state
                    .contracts
                    .into_iter()
                    .filter(|(code_hash, _)| *code_hash != KECCAK_EMPTY)
            ),
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

        Ok(account_status_mismatches)
    }
}
