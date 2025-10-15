use alloy_primitives::{
    map::{AddressSet, HashMap},
    Address, B256, U256,
};
use clap::Parser;
use eyre::{OptionExt, Result};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_engine_tree::tree::{
    executor::WorkloadExecutor,
    multiproof::{MultiProofConfig, MultiProofMessage, MultiProofTask},
    precompile_cache::PrecompileCacheMap,
    PayloadProcessor,
};
use reth_ethereum::{
    chainspec::{ChainSpec, EthChainSpec},
    cli::chainspec::EthereumChainSpecParser,
    evm::{
        primitives::block::StateChangeSource,
        revm::{
            db::BundleState,
            state::{AccountStatus, EvmStorageSlot},
        },
        EthEvmConfig,
    },
    node::EthereumNode,
    provider::providers::{ConsistentDbView, ReadOnlyConfig},
    storage::{BlockReader, DatabaseProviderFactory},
    trie::{
        hashed_cursor::HashedPostStateCursorFactory, DatabaseHashedCursorFactory,
        DatabaseTrieCursorFactory, HashedPostState, KeccakKeyHasher, StateRoot, TrieInput,
    },
};
use reth_node_api::TreeConfig;
use reth_node_core::args::DatadirArgs;
use reth_primitives_traits::{Account, StorageEntry};
use reth_trie_parallel::{
    proof_task::{ProofTaskCtx, ProofWorkerHandle},
    root::ParallelStateRoot,
};
use std::{
    sync::{mpsc::channel, Arc},
    time::{Duration, Instant},
};

/// Command for benchmarking different trie calculation methods.
#[derive(Debug, Parser)]
pub struct Command {
    #[command(flatten)]
    pub datadir: DatadirArgs,

    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = EthereumChainSpecParser::help_message(),
        default_value = EthereumChainSpecParser::default_value(),
        value_parser = EthereumChainSpecParser::parser(),
        global = true
    )]
    pub chain: Arc<ChainSpec>,

    #[arg(long, help = "Number of state transitions (transactions in the block)")]
    pub state_transitions: usize,
    #[arg(
        long,
        help = "Delay between state transitions",
        value_parser = humantime::parse_duration
    )]
    pub state_transition_delay: Duration,
    #[arg(long, help = "Number of changed accounts per state transition")]
    pub changed_accounts: usize,
    #[arg(long, help = "New accounts per state transition")]
    pub new_accounts: usize,
    #[arg(long, help = "Number of changed storages per account per state transition")]
    pub changed_storages: usize,
    #[arg(long, help = "New storages per account per state transition")]
    pub new_storages: usize,
    #[arg(
        long,
        default_value = "1",
        help = "Number of iterations to run for calculating statistics"
    )]
    pub iterations: usize,
}

impl Command {
    /// Execute the command
    pub async fn execute(self, _ctx: CliContext) -> Result<()> {
        let datadir = self.datadir.clone().resolve_datadir(self.chain.chain());

        let factory = EthereumNode::provider_factory_builder()
            .open_read_only(self.chain.clone(), ReadOnlyConfig::from_datadir(datadir))?;

        let provider = factory.provider()?;
        let consistent_view = ConsistentDbView::new_with_latest_tip(factory)?;

        // Structures to store timing results for each method
        let mut sequential_times = Vec::with_capacity(self.iterations);
        let mut parallel_times = Vec::with_capacity(self.iterations);
        let mut task_times = Vec::with_capacity(self.iterations);

        // Run multiple iterations
        for i in 0..self.iterations {
            println!("\nIteration {}/{}", i + 1, self.iterations);

            // Generate new state transitions for each iteration
            let (state_transitions, bundle_state) =
                self.generate_state_transitions(provider.tx_ref())?;
            println!("Accounts in bundle state: {}", bundle_state.state.len());

            let hashed_post_state =
                HashedPostState::from_bundle_state::<KeccakKeyHasher>(&bundle_state.state);
            let trie_input = TrieInput::from_state(hashed_post_state);

            // Run sequential state root calculation
            let (sequential_root, sequential_time) =
                self.calculate_sequential_state_root(provider.tx_ref(), trie_input.clone())?;
            sequential_times.push(sequential_time);

            // Run parallel state root calculation
            let (parallel_root, parallel_time) =
                self.calculate_parallel_state_root(consistent_view.clone(), trie_input.clone())?;
            parallel_times.push(parallel_time);

            // Run state root task calculation
            let (task_root, task_time) = self.calculate_state_root_task(
                consistent_view.clone(),
                trie_input.clone(),
                state_transitions,
            )?;
            task_times.push(task_time);

            // Verify all methods produce the same result
            assert_eq!(sequential_root, parallel_root, "Sequential and parallel roots differ");
            assert_eq!(sequential_root, task_root, "Sequential and task roots differ");
        }

        // Calculate and print statistics
        if self.iterations > 1 {
            println!("\n========== Statistics over {} iterations ==========", self.iterations);
            println!("\nSequential State Root:");
            self.print_statistics(&sequential_times);
            println!("\nParallel State Root:");
            self.print_statistics(&parallel_times);
            println!("\nTask-based State Root:");
            self.print_statistics(&task_times);
        }

        Ok(())
    }

    /// Calculate and print statistics for a set of timing measurements
    fn print_statistics(&self, times: &[Duration]) {
        let mut sorted_times = times.to_vec();
        sorted_times.sort();

        // Calculate mean
        let sum: Duration = sorted_times.iter().sum();
        let mean = sum / sorted_times.len() as u32;

        // Calculate percentiles
        let p50 = self.calculate_percentile(&sorted_times, 50.0);
        let p99 = self.calculate_percentile(&sorted_times, 99.0);

        println!("  Mean:  {:?}", mean);
        println!("  P50:   {:?}", p50);
        println!("  P99:   {:?}", p99);
    }

    /// Calculate a specific percentile from sorted durations
    fn calculate_percentile(&self, sorted: &[Duration], percentile: f64) -> Duration {
        let index = ((percentile / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[index]
    }

    fn generate_state_transitions(
        &self,
        tx: &impl DbTx,
    ) -> Result<(Vec<BundleState>, BundleState)> {
        let mut accounts_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
        let mut storages_cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;

        let state_transitions = (0..self.state_transitions)
            .map(|_| {
                let mut state = Vec::new();
                let mut seen_accounts = AddressSet::default();
                let mut reverts = Vec::new();

                for _ in 0..self.changed_accounts {
                    let mut entry = loop {
                        let address = accounts_cursor
                            .seek(Address::random())?
                            .ok_or_eyre("account not found")?
                            .0;
                        if seen_accounts.contains(&address) {
                            continue;
                        }

                        let walker = storages_cursor.walk_dup(Some(address), None)?;
                        let storages = walker.collect::<Vec<_>>();
                        if storages.len() >= self.changed_storages {
                            seen_accounts.insert(address);

                            let account = tx
                                .get::<tables::PlainAccountState>(address)?
                                .ok_or_eyre("account not found")?;

                            break (
                                address,
                                Some(account.into()),
                                Some(account.into()),
                                storages
                                    .into_iter()
                                    .take(self.changed_storages)
                                    .map(|result| {
                                        let StorageEntry { key, value } = result?.1;

                                        Result::Ok((key.into(), (value, U256::random())))
                                    })
                                    .collect::<Result<HashMap<_, _>>>()?,
                            );
                        }
                    };

                    for _ in 0..self.new_storages {
                        entry.3.insert(U256::random(), (U256::ZERO, U256::random()));
                    }

                    state.push(entry.clone());
                    reverts.push((entry.0, Some(entry.1), vec![]));
                }

                for _ in 0..self.new_accounts {
                    let address = Address::random();
                    let account =
                        Account { nonce: 1, balance: U256::random(), bytecode_hash: None };
                    state.push((address, None, Some(account.into()), HashMap::default()));
                }

                Result::Ok(BundleState::new(state, vec![reverts], vec![]))
            })
            .collect::<Result<Vec<_>>>()?;

        let bundle_state =
            state_transitions.iter().cloned().fold(BundleState::default(), |mut acc, state| {
                acc.extend(state);
                acc
            });

        Ok((state_transitions, bundle_state))
    }

    /// Calculate state root using sequential method
    fn calculate_sequential_state_root(
        &self,
        tx: &impl DbTx,
        trie_input: TrieInput,
    ) -> Result<(B256, Duration)> {
        let hashed_post_state_sorted = trie_input.state.into_sorted();
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(tx);
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(tx),
            &hashed_post_state_sorted,
        );

        let start = Instant::now();
        std::thread::sleep(
            self.state_transition_delay.saturating_mul(self.state_transitions as u32),
        );
        let state_root = StateRoot::new(trie_cursor_factory, hashed_cursor_factory)
            .with_prefix_sets(trie_input.prefix_sets.freeze())
            .root()?;
        let elapsed = start.elapsed();
        println!("  Sequential: state root = {state_root}, time = {elapsed:?}");
        Ok((state_root, elapsed))
    }

    /// Calculate state root using parallel method
    fn calculate_parallel_state_root<
        P: DatabaseProviderFactory<Provider: BlockReader> + Clone + 'static,
    >(
        &self,
        consistent_view: ConsistentDbView<P>,
        trie_input: TrieInput,
    ) -> Result<(B256, Duration)> {
        let start = Instant::now();
        std::thread::sleep(
            self.state_transition_delay.saturating_mul(self.state_transitions as u32),
        );
        let parallel_state_root =
            ParallelStateRoot::new(consistent_view, trie_input).incremental_root()?;
        let elapsed = start.elapsed();
        println!("  Parallel:   state root = {parallel_state_root}, time = {elapsed:?}");
        Ok((parallel_state_root, elapsed))
    }

    /// Calculate state root using task-based method with multiproof and sparse trie
    fn calculate_state_root_task<
        P: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + 'static,
    >(
        &self,
        consistent_view: ConsistentDbView<P>,
        trie_input: TrieInput,
        state_transitions: Vec<BundleState>,
    ) -> Result<(B256, Duration)> {
        let tree_config = TreeConfig::default();
        let payload_processor = PayloadProcessor::new(
            WorkloadExecutor::default(),
            EthEvmConfig::new(self.chain.clone()),
            &tree_config,
            PrecompileCacheMap::default(),
        );

        let (to_sparse_trie, sparse_trie_rx) = channel();
        // spawn multiproof task, save the trie input
        let (_, state_root_config) = MultiProofConfig::from_input(trie_input);

        // Create and spawn the storage proof task
        let task_ctx = ProofTaskCtx::new(
            state_root_config.nodes_sorted.clone(),
            state_root_config.state_sorted.clone(),
            Default::default(),
        );
        let storage_worker_count = tree_config.storage_worker_count();
        let account_worker_count = tree_config.account_worker_count();
        let max_proof_task_concurrency = tree_config.max_proof_task_concurrency() as usize;
        let proof_handle = ProofWorkerHandle::new(
            payload_processor.executor().handle().clone(),
            consistent_view,
            task_ctx,
            storage_worker_count,
            account_worker_count,
        )?;

        // We set it to half of the proof task concurrency, because often for each multiproof we
        // spawn one Tokio task for the account proof, and one Tokio task for the storage proof.
        let max_multi_proof_task_concurrency = max_proof_task_concurrency / 2;
        let multi_proof_task = MultiProofTask::new(
            state_root_config,
            payload_processor.executor().clone(),
            proof_handle.clone(),
            to_sparse_trie,
            max_multi_proof_task_concurrency,
            tree_config
                .multiproof_chunking_enabled()
                .then_some(tree_config.multiproof_chunk_size()),
        );

        // wire the multiproof task to the prewarm task
        let to_multi_proof = multi_proof_task.state_root_message_sender();

        // spawn multi-proof task
        payload_processor.executor().spawn_blocking(move || {
            multi_proof_task.run();
        });

        // wire the sparse trie to the state root response receiver
        let (state_root_tx, state_root_rx) = channel();

        // Spawn the sparse trie task using any stored trie and parallel trie configuration.
        payload_processor.spawn_sparse_trie_task(sparse_trie_rx, proof_handle, state_root_tx);

        let start = Instant::now();

        for (i, transition) in state_transitions.into_iter().enumerate() {
            to_multi_proof
                .send(MultiProofMessage::StateUpdate(
                    StateChangeSource::Transaction(i),
                    transition
                        .state
                        .into_iter()
                        .map(|(address, account)| {
                            let status = if account.info.is_none() {
                                AccountStatus::Created
                            } else {
                                AccountStatus::Touched
                            };
                            (
                                address,
                                // NOTE: there should be an easier way to do that -.-
                                reth_ethereum::evm::revm::state::Account {
                                    info: account.info.unwrap_or_default(),
                                    transaction_id: i,
                                    storage: account
                                        .storage
                                        .into_iter()
                                        .map(|(key, storage)| {
                                            (
                                                key,
                                                EvmStorageSlot {
                                                    original_value: storage
                                                        .previous_or_original_value,
                                                    present_value: storage.present_value,
                                                    transaction_id: i,
                                                    is_cold: true,
                                                },
                                            )
                                        })
                                        .collect(),
                                    status,
                                },
                            )
                        })
                        .collect(),
                ))
                .unwrap();
            std::thread::sleep(self.state_transition_delay);
        }
        to_multi_proof.send(MultiProofMessage::FinishedStateUpdates).unwrap();

        let state_root_task_root = state_root_rx.recv().unwrap().unwrap().state_root;
        let elapsed = start.elapsed();
        println!("  Task-based: state root = {state_root_task_root}, time = {elapsed:?}");

        Ok((state_root_task_root, elapsed))
    }
}
