//! Benchmarks for `save_blocks` method including actual database insertion.
//!
//! This benchmark measures the full end-to-end performance of persisting blocks,
//! including state merging, sorting, and database writes.
//!
//! To simulate real-world conditions, the database is pre-populated with existing
//! state before benchmarking new block writes.

#![allow(missing_docs)]

use alloy_consensus::Header;
use alloy_primitives::{Address, BlockNumber, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_chain_state::{ComputedTrieData, ExecutedBlock};
use reth_chainspec::MAINNET;
use reth_db::test_utils::{create_test_rw_db, create_test_static_files_dir};
use reth_db_api::{cursor::DbCursorRW, tables, transaction::DbTxMut};
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{Account, RecoveredBlock, SealedBlock, SealedHeader, StorageEntry};
use reth_provider::{
    providers::{RocksDBBuilder, StaticFileProvider},
    DBProvider, DatabaseProviderFactory, HashingWriter, ProviderFactory, SaveBlocksMode,
    StaticFileProviderFactory, StaticFileWriter,
};
use reth_static_file_types::StaticFileSegment;
use reth_trie::{updates::TrieUpdates, HashedPostState, KeccakKeyHasher};
use revm_database::BundleState;
use revm_state::AccountInfo;
use std::sync::Arc;

type MockNodeTypes = reth_node_types::AnyNodeTypesWithEngine<
    reth_ethereum_primitives::EthPrimitives,
    reth_ethereum_engine_primitives::EthEngineTypes,
    reth_chainspec::ChainSpec,
    reth_provider::EthStorage,
    reth_ethereum_engine_primitives::EthEngineTypes,
>;

type MockNodeTypesWithDB<DB = reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>> =
    reth_node_types::NodeTypesWithDBAdapter<MockNodeTypes, Arc<DB>>;

/// Create a fresh provider factory for each benchmark iteration
fn create_provider_factory() -> ProviderFactory<MockNodeTypesWithDB> {
    let (static_dir, _) = create_test_static_files_dir();
    let db = create_test_rw_db();

    // Create RocksDB in a temp directory
    let rocksdb_dir = tempfile::tempdir().expect("failed to create temp dir");
    let rocksdb = RocksDBBuilder::new(rocksdb_dir.path())
        .with_default_tables()
        .build()
        .expect("failed to create RocksDB");

    let factory = ProviderFactory::new(
        db,
        MAINNET.clone(),
        StaticFileProvider::read_write(static_dir.keep()).expect("static file provider"),
        rocksdb,
    )
    .expect("failed to create provider factory");

    // Initialize static files with genesis block (block 0)
    // This is required before we can write blocks starting from block 1
    let genesis_header =
        Header { number: 0, gas_limit: 30_000_000, timestamp: 1_000_000, ..Default::default() };
    let genesis_hash = B256::ZERO;

    // Initialize all static file segments with genesis data
    {
        let sf_provider = factory.static_file_provider();

        // Headers segment
        let mut writer = sf_provider
            .latest_writer(StaticFileSegment::Headers)
            .expect("failed to get headers writer");
        writer
            .append_header(&genesis_header, &genesis_hash)
            .expect("failed to append genesis header");
        writer.commit().expect("failed to commit headers");

        // Transactions segment (empty for genesis)
        let mut writer = sf_provider
            .latest_writer(StaticFileSegment::Transactions)
            .expect("failed to get transactions writer");
        writer.increment_block(0).expect("failed to increment block");
        writer.commit().expect("failed to commit transactions");

        // Receipts segment (empty for genesis)
        let mut writer = sf_provider
            .latest_writer(StaticFileSegment::Receipts)
            .expect("failed to get receipts writer");
        writer.increment_block(0).expect("failed to increment block");
        writer.commit().expect("failed to commit receipts");
    }

    factory
}

/// Generate random address
fn random_address(rng: &mut StdRng) -> Address {
    Address::from_slice(&rng.random::<[u8; 20]>())
}

/// Pre-populate the database with existing accounts and storage to simulate real-world conditions.
/// Returns the addresses that were created (for potential updates in blocks).
fn populate_existing_state(
    factory: &ProviderFactory<MockNodeTypesWithDB>,
    num_accounts: usize,
    storage_slots_per_account: usize,
    seed: u64,
) -> Vec<Address> {
    let mut rng = StdRng::seed_from_u64(seed);
    let provider = factory.provider_rw().expect("failed to get provider");

    let mut addresses = Vec::with_capacity(num_accounts);

    // Insert accounts into PlainAccountState
    let accounts: Vec<_> = (0..num_accounts)
        .map(|_| {
            let address = random_address(&mut rng);
            addresses.push(address);
            let account = Account {
                nonce: rng.random::<u64>(),
                balance: U256::from(rng.random::<u64>()),
                bytecode_hash: None,
            };
            (address, account)
        })
        .collect();

    // Insert into PlainAccountState table
    {
        let tx = provider.tx_ref();
        let mut cursor = tx.cursor_write::<tables::PlainAccountState>().unwrap();
        for (address, account) in &accounts {
            cursor.upsert(*address, account).unwrap();
        }
    }

    // Insert storage for each account
    if storage_slots_per_account > 0 {
        let tx = provider.tx_ref();
        let mut cursor = tx.cursor_write::<tables::PlainStorageState>().unwrap();
        for address in &addresses {
            for _ in 0..storage_slots_per_account {
                let slot = B256::from_slice(&rng.random::<[u8; 32]>());
                let value = U256::from(rng.random::<u64>());
                cursor.upsert(*address, &StorageEntry { key: slot, value }).unwrap();
            }
        }
    }

    // Also populate hashed state for realistic trie operations
    let hashed_accounts = accounts.iter().map(|(addr, acc)| (*addr, Some(*acc)));
    provider.insert_account_for_hashing(hashed_accounts).unwrap();

    if storage_slots_per_account > 0 {
        let mut rng2 = StdRng::seed_from_u64(seed); // Reset to get same addresses
        for _ in 0..num_accounts {
            let _ = random_address(&mut rng2); // Skip to sync with storage generation
        }

        let storage_entries: Vec<_> = addresses
            .iter()
            .map(|address| {
                let slots: Vec<_> = (0..storage_slots_per_account)
                    .map(|_| {
                        let slot = B256::from_slice(&rng2.random::<[u8; 32]>());
                        let value = U256::from(rng2.random::<u64>());
                        StorageEntry { key: slot, value }
                    })
                    .collect();
                (*address, slots.into_iter())
            })
            .collect();
        provider.insert_storage_for_hashing(storage_entries).unwrap();
    }

    provider.commit().expect("failed to commit initial state");

    addresses
}

/// Generate ExecutedBlocks with realistic state changes.
/// Some accounts will be updates to existing accounts (simulating real txs).
fn generate_executed_blocks(
    num_blocks: usize,
    accounts_per_block: usize,
    storage_slots_per_account: usize,
    existing_addresses: &[Address],
    update_ratio: f64, // Ratio of updates vs new accounts (0.0-1.0)
    seed: u64,
) -> Vec<ExecutedBlock<reth_ethereum_primitives::EthPrimitives>> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut blocks = Vec::with_capacity(num_blocks);
    let mut parent_hash = B256::ZERO;

    for block_num in 1..=num_blocks {
        let block_number = block_num as BlockNumber;

        // Build bundle state with account and storage changes
        let mut bundle_builder = BundleState::builder(block_number..=block_number);

        for _ in 0..accounts_per_block {
            // Decide whether to update an existing account or create new
            let address = if !existing_addresses.is_empty() && rng.random_bool(update_ratio) {
                // Update existing account
                existing_addresses[rng.random_range(0..existing_addresses.len())]
            } else {
                // Create new account
                random_address(&mut rng)
            };

            let info = AccountInfo {
                balance: U256::from(rng.random::<u64>()),
                nonce: rng.random::<u64>(),
                code_hash: B256::ZERO,
                code: None,
                account_id: None,
            };

            bundle_builder = bundle_builder
                .state_present_account_info(address, info.clone())
                .revert_account_info(block_number, address, Some(None));

            // Add storage changes
            if storage_slots_per_account > 0 {
                let storage: alloy_primitives::map::HashMap<U256, (U256, U256)> = (0..
                    storage_slots_per_account)
                    .map(|_| {
                        let slot = U256::from_be_bytes(rng.random::<[u8; 32]>());
                        let value = U256::from(rng.random::<u64>());
                        (slot, (U256::ZERO, value))
                    })
                    .collect();
                bundle_builder = bundle_builder.state_storage(address, storage);
            }
        }

        let bundle_state = bundle_builder.build();

        // Create execution outcome - empty receipts since we're benchmarking state writes
        // (transactions would require matching the static file expectations)
        let execution_outcome = ExecutionOutcome::new(
            bundle_state,
            vec![vec![]], // Empty receipts for this block
            block_number,
            Vec::new(),
        );

        // Create block header
        let header = Header {
            number: block_number,
            parent_hash,
            gas_limit: 30_000_000,
            gas_used: 21000 * accounts_per_block as u64,
            timestamp: 1_000_000 + block_number,
            ..Default::default()
        };

        let sealed_header = SealedHeader::seal_slow(header);
        parent_hash = sealed_header.hash();

        let block = SealedBlock::from_sealed_parts(sealed_header, Default::default());
        let recovered = RecoveredBlock::new_sealed(block, vec![]);

        // Create hashed post state from bundle and sort it
        let hashed_state = HashedPostState::from_bundle_state::<KeccakKeyHasher>(
            execution_outcome.state().state(),
        );
        let hashed_state_sorted = hashed_state.into_sorted();

        // Create empty trie updates (in real use, these would be computed)
        let trie_updates_sorted = TrieUpdates::default().into_sorted();

        let trie_data = ComputedTrieData {
            hashed_state: Arc::new(hashed_state_sorted),
            trie_updates: Arc::new(trie_updates_sorted),
            anchored_trie_input: None,
        };

        let executed =
            ExecutedBlock::new(Arc::new(recovered), Arc::new(execution_outcome), trie_data);

        blocks.push(executed);
    }

    blocks
}

fn bench_save_blocks(c: &mut Criterion) {
    let mut group = c.benchmark_group("save_blocks");
    // Increase sample size for more stable results
    group.sample_size(10);

    // Real-world Ethereum Mainnet statistics (per block):
    // - ~1,401 storage slots touched (583 writes + 818 reads)
    // - ~3.2 storage slots per transaction
    // - ~2.2 unique accounts per transaction
    // - 85% of state access is storage operations
    //
    // For typical block with ~200 txs:
    // - ~440 unique accounts (200 × 2.2)
    // - ~640 storage slots (200 × 3.2)
    // - ~1.5 storage slots per account
    //
    // Pre-populated state (simulating synced node):
    // Mainnet has ~250M accounts, but we scale down for benchmark speed

    struct BenchConfig {
        name: &'static str,
        // Pre-populated state
        existing_accounts: usize,
        existing_storage_per_account: usize,
        // Block parameters
        num_blocks: usize,
        accounts_per_block: usize,
        storage_per_account: usize,
        update_ratio: f64,
    }

    let configs = [
        // Single block persistence
        BenchConfig {
            name: "1_block",
            existing_accounts: 100_000,
            existing_storage_per_account: 10,
            num_blocks: 1,
            accounts_per_block: 440, // ~200 txs × 2.2 accounts
            storage_per_account: 2,
            update_ratio: 0.85,
        },
        // Engine tree typical persistence (3 blocks)
        BenchConfig {
            name: "3_blocks",
            existing_accounts: 100_000,
            existing_storage_per_account: 10,
            num_blocks: 3,
            accounts_per_block: 440,
            storage_per_account: 2,
            update_ratio: 0.85,
        },
        // Quick test - small batch
        BenchConfig {
            name: "10_blocks",
            existing_accounts: 100_000,
            existing_storage_per_account: 10,
            num_blocks: 10,
            accounts_per_block: 440,
            storage_per_account: 2,
            update_ratio: 0.85,
        },
        // Typical persistence batch (~50 blocks)
        BenchConfig {
            name: "50_blocks",
            existing_accounts: 100_000,
            existing_storage_per_account: 10,
            num_blocks: 50,
            accounts_per_block: 440,
            storage_per_account: 2,
            update_ratio: 0.85,
        },
        // Heavy batch (~100 blocks)
        BenchConfig {
            name: "100_blocks",
            existing_accounts: 200_000,
            existing_storage_per_account: 10,
            num_blocks: 100,
            accounts_per_block: 440,
            storage_per_account: 2,
            update_ratio: 0.85,
        },
    ];

    for config in configs {
        let total_state_changes = config.num_blocks * config.accounts_per_block;
        group.throughput(Throughput::Elements(total_state_changes as u64));

        group.bench_with_input(
            BenchmarkId::new("full_pipeline", config.name),
            &config,
            |b, cfg| {
                b.iter_batched(
                    || {
                        // Setup: create provider, populate existing state, generate blocks
                        let factory = create_provider_factory();
                        let existing = populate_existing_state(
                            &factory,
                            cfg.existing_accounts,
                            cfg.existing_storage_per_account,
                            42,
                        );
                        let blocks = generate_executed_blocks(
                            cfg.num_blocks,
                            cfg.accounts_per_block,
                            cfg.storage_per_account,
                            &existing,
                            cfg.update_ratio,
                            12345,
                        );
                        (factory, blocks)
                    },
                    |(factory, blocks)| {
                        // Benchmark: full save_blocks including DB writes
                        let provider = factory.database_provider_rw().unwrap();
                        provider
                            .save_blocks(black_box(blocks), SaveBlocksMode::Full)
                            .expect("save_blocks failed");
                        provider.commit().expect("commit failed");
                    },
                    criterion::BatchSize::PerIteration,
                );
            },
        );

        // Benchmark without commit to isolate write preparation from fsync
        group.bench_with_input(BenchmarkId::new("no_commit", config.name), &config, |b, cfg| {
            b.iter_batched(
                || {
                    let factory = create_provider_factory();
                    let existing = populate_existing_state(
                        &factory,
                        cfg.existing_accounts,
                        cfg.existing_storage_per_account,
                        42,
                    );
                    let blocks = generate_executed_blocks(
                        cfg.num_blocks,
                        cfg.accounts_per_block,
                        cfg.storage_per_account,
                        &existing,
                        cfg.update_ratio,
                        12345,
                    );
                    (factory, blocks)
                },
                |(factory, blocks)| {
                    let provider = factory.database_provider_rw().unwrap();
                    provider
                        .save_blocks(black_box(blocks), SaveBlocksMode::Full)
                        .expect("save_blocks failed");
                    // Don't commit - isolate write preparation time
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_save_blocks);
criterion_main!(benches);
