//! Standalone benchmark for the BAL (EIP-7928) parallel state-root machinery.
//!
//! Runs ONLY the state-root path (prewarm BAL load -> proof workers -> sparse-trie reveal -> root),
//! stripped of execution, networking, engine API, and persistence, replaying captured blocks
//! against a read-only MDBX database. See project memory `sr-isolation-bench`.
//!
//! Reconstruct approach: feed the production `PayloadProcessor::spawn` the captured BAL + parent
//! state root with an empty tx iterator and `parallel_bal_execution = true`; the BAL prewarm path
//! regenerates the entire state-update stream from the BAL alone (execution never feeds SR in BAL
//! mode). `transaction_count` is set to the real value so `halve_workers` stays false.
//!
//! Cross-block: a single long-lived `PayloadProcessor` replays N consecutive blocks. Each block's
//! `parent_state_root` chains to the previous computed root (preserved sparse trie reuse). Prior
//! blocks' writes are made visible two ways:
//!  - proof workers: an Immediate overlay (accumulated hashed leaves + trie nodes) on the
//!    multiproof factory;
//!  - prewarm `basic_account` reads: an unhashed `BundleState` overlay (accumulated
//!    account/storage, derived from the BALs exactly as `send_bal_hashed_state` does) passed as an
//!    `ExecutedBlock` to the `StateProviderBuilder`.

// Match the node's global allocator (jemalloc) so allocation behavior is representative.
#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

use std::{collections::HashMap, path::Path, str::FromStr, sync::Arc, time::Instant};

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpecBuilder;
use reth_db::{mdbx::DatabaseArguments, open_db_read_only, ClientVersion, DatabaseEnv};
use reth_engine_tree::tree::{
    payload_processor::{ExecutionEnv, PayloadProcessor},
    precompile_cache::PrecompileCacheMap,
    StateProviderBuilder, TreeConfig,
};
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::BlockExecutionOutput;
use reth_node_ethereum::EthereumNode;
use reth_node_types::NodeTypesWithDBAdapter;
use reth_primitives_traits::{Account, Recovered};
use reth_provider::{
    providers::{
        BlockchainProvider, OverlayBuilder, OverlayStateProviderFactory, RocksDBProvider,
        StaticFileProvider,
    },
    AccountReader, ProviderFactory, StateProviderFactory,
};
use reth_trie::{updates::TrieUpdates, HashedPostState};
use reth_trie_db::ChangesetCache;
use revm::{database::BundleState, state::AccountInfo};

type NodeTypes = NodeTypesWithDBAdapter<EthereumNode, DatabaseEnv>;
type Provider = BlockchainProvider<NodeTypes>;

/// Per-block captured SR inputs.
struct BlockInput {
    parent_hash: B256,
    parent_state_root: B256,
    transaction_count: usize,
    gas_used: u64,
    bal: Bytes,
}

fn load_block(dir: &Path, number: u64) -> eyre::Result<BlockInput> {
    let meta = std::fs::read_to_string(dir.join(format!("{number:08}.meta")))?;
    let mut parent_hash = None;
    let mut parent_state_root = None;
    let mut gas_used = 0u64;
    let mut tx_count = 0usize;
    for line in meta.lines() {
        let (k, v) = line.split_once('\t').ok_or_else(|| eyre::eyre!("bad meta line: {line}"))?;
        match k {
            "parent_hash" => parent_hash = Some(B256::from_str(v.trim())?),
            "parent_state_root" => parent_state_root = Some(B256::from_str(v.trim())?),
            "gas_used" => gas_used = v.trim().parse()?,
            "tx_count" => tx_count = v.trim().parse()?,
            _ => {}
        }
    }
    let bal = Bytes::from(std::fs::read(dir.join(format!("{number:08}.bal")))?);
    Ok(BlockInput {
        parent_hash: parent_hash.ok_or_else(|| eyre::eyre!("no parent_hash"))?,
        parent_state_root: parent_state_root.ok_or_else(|| eyre::eyre!("no parent_state_root"))?,
        transaction_count: tx_count,
        gas_used,
        bal,
    })
}

fn empty_txs() -> (
    Vec<Result<Recovered<TransactionSigned>, core::convert::Infallible>>,
    fn(
        Result<Recovered<TransactionSigned>, core::convert::Infallible>,
    ) -> Result<Recovered<TransactionSigned>, core::convert::Infallible>,
) {
    (Vec::new(), std::convert::identity)
}

/// Accumulated unhashed post-state across blocks (for the prewarm `basic_account` overlay).
#[derive(Default)]
struct UnhashedState {
    accounts: HashMap<Address, Account>,
    storage: HashMap<Address, HashMap<U256, U256>>,
}

impl UnhashedState {
    /// Apply one block's BAL, mirroring `send_bal_hashed_state` (unchanged fields fall back to the
    /// accumulated value, else the persisted base read through `db`).
    fn apply_bal(&mut self, bal: &DecodedBal, db: &dyn AccountReader) {
        for ac in bal.as_bal().iter() {
            let addr = ac.address;
            let balance = ac.balance_changes.last().map(|c| c.post_balance);
            let nonce = ac.nonce_changes.last().map(|c| c.new_nonce);
            let code_hash = ac.code_changes.last().map(|c| {
                if c.new_code.is_empty() {
                    KECCAK_EMPTY
                } else {
                    keccak256(&c.new_code)
                }
            });
            if balance.is_none() &&
                nonce.is_none() &&
                code_hash.is_none() &&
                ac.storage_changes.is_empty()
            {
                continue;
            }
            let existing = self
                .accounts
                .get(&addr)
                .copied()
                .or_else(|| db.basic_account(&addr).ok().flatten());
            let account = Account {
                balance: balance
                    .unwrap_or_else(|| existing.map(|a| a.balance).unwrap_or(U256::ZERO)),
                nonce: nonce.unwrap_or_else(|| existing.map(|a| a.nonce).unwrap_or(0)),
                bytecode_hash: code_hash
                    .or_else(|| existing.and_then(|a| a.bytecode_hash).or(Some(KECCAK_EMPTY))),
            };
            self.accounts.insert(addr, account);
            for sc in &ac.storage_changes {
                if let Some(last) = sc.changes.last() {
                    self.storage.entry(addr).or_default().insert(sc.slot, last.new_value);
                }
            }
        }
    }

    /// Build a single `ExecutedBlock` carrying the accumulated state as a `BundleState` overlay.
    fn to_executed_block(&self, range_end: u64) -> ExecutedBlock {
        let mut builder = BundleState::builder(0..=range_end);
        for (addr, account) in &self.accounts {
            builder = builder.state_present_account_info(
                *addr,
                AccountInfo {
                    balance: account.balance,
                    nonce: account.nonce,
                    code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
                    ..Default::default()
                },
            );
            if let Some(slots) = self.storage.get(addr) {
                builder = builder.state_storage(
                    *addr,
                    slots.iter().map(|(k, v)| (*k, (U256::ZERO, *v))).collect(),
                );
            }
        }
        let mut eb = ExecutedBlock::<EthPrimitives>::default();
        eb.execution_output =
            Arc::new(BlockExecutionOutput { state: builder.build(), ..Default::default() });
        eb
    }
}

// Register the default (rustc/cpp) symbol demangler with the embedded Tracy client, so sampled
// stack frames show readable names instead of mangled `_ZN...` symbols. Expands to the
// `___tracy_demangle` hook that tracy-client-sys calls; must live at crate root.
tracy_client::register_demangler!();

fn main() -> eyre::Result<()> {
    // Configure the implicit global rayon pool up front, exactly as the node does in its launch
    // context. The sparse-trie reveal uses the global pool via par_iter/rayon::join; without this
    // the pool is built lazily on first use from inside the `sparse-trie` coordinator thread, and
    // rayon's unnamed default workers inherit that parent's `comm` — so every worker shows up as
    // `sparse-trie` in profilers. Naming them `rayon-NN` matches the node and keeps the one
    // coordinator distinct from the pool workers in Tracy/samply.
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(std::thread::available_parallelism().map_or(1, |n| n.get()))
        .thread_name(|i| format!("rayon-{i:02}"))
        .build_global();

    let datadir = std::env::var("RETH_DATADIR").unwrap_or_else(|_| "/schelk/reth".to_string());
    let datadir = Path::new(&datadir);
    let capture =
        std::env::var("RETH_BAL_DUMP_DIR").unwrap_or_else(|_| "/tmp/bal_capture".to_string());
    let capture = Path::new(&capture);
    let first: u64 =
        std::env::var("BENCH_FIRST_BLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(25115966);
    let count: u64 = std::env::var("BENCH_BLOCKS").ok().and_then(|v| v.parse().ok()).unwrap_or(5);

    // Optional tracing layers (one or both):
    //  - Chrome/Perfetto: RETH_CHROME_TRACE=/path/trace.json (+ RETH_CHROME_FILTER). The flush
    //    guard is held to end of main so it flushes on clean exit.
    //  - samply markers: RETH_LOG_SAMPLY=1 (+ RETH_SAMPLY_FILTER) — equivalent to reth's
    //    `--log.samply`; spans show as markers in the Firefox Profiler alongside the sampled stacks
    //    (run under samply).
    use tracing_subscriber::{prelude::*, EnvFilter, Layer, Registry};
    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::new();
    let chrome_guard = match std::env::var("RETH_CHROME_TRACE") {
        Ok(path) => {
            let directives = std::env::var("RETH_CHROME_FILTER").unwrap_or_else(|_| {
                "info,engine::tree::payload_processor=trace,reth_trie_parallel::proof_task=trace".to_string()
            });
            let (layer, guard) =
                tracing_chrome::ChromeLayerBuilder::new().file(path).include_args(true).build();
            layers.push(layer.with_filter(EnvFilter::new(directives)).boxed());
            Some(guard)
        }
        Err(_) => None,
    };
    if std::env::var_os("RETH_LOG_SAMPLY").is_some() {
        let directives =
            std::env::var("RETH_SAMPLY_FILTER").unwrap_or_else(|_| "debug".to_string());
        let layer = tracing_samply::SamplyLayer::new()
            .map_err(|e| eyre::eyre!("failed to create samply layer: {e}"))?;
        layers.push(layer.with_filter(EnvFilter::new(directives)).boxed());
    }
    // Tracy: RETH_LOG_TRACY=1 (+ RETH_TRACY_FILTER) — embeds the Tracy client; a Tracy profiler
    // (GUI on :8086 or headless tracy-capture) connects to record. Equivalent to reth's
    // --log.tracy.
    if std::env::var_os("RETH_LOG_TRACY").is_some() {
        let directives = std::env::var("RETH_TRACY_FILTER").unwrap_or_else(|_| {
            "info,engine::tree::payload_processor=trace,reth_trie_parallel=trace,reth_trie_sparse=trace".to_string()
        });
        layers.push(
            tracing_tracy::TracyLayer::default().with_filter(EnvFilter::new(directives)).boxed(),
        );
    }
    // Console logs driven by RUST_LOG (e.g. RUST_LOG=trie::arena=debug). Writes to stderr so it
    // doesn't clash with the per-block result lines on stdout.
    if std::env::var_os("RUST_LOG").is_some() {
        layers.push(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(EnvFilter::from_default_env())
                .boxed(),
        );
    }
    if !layers.is_empty() {
        Registry::default().with(layers).init();
    }
    let _chrome_guard = chrome_guard;

    let runtime = reth_tasks::RuntimeBuilder::new(reth_tasks::RuntimeConfig::default()).build()?;
    let chain_spec = Arc::new(ChainSpecBuilder::mainnet().build());

    let db =
        open_db_read_only(&datadir.join("db"), DatabaseArguments::new(ClientVersion::default()))?;
    let rocksdb_path = datadir.join("rocksdb");
    let rocksdb_read_only = rocksdb_path.join("CURRENT").exists();
    if !rocksdb_read_only {
        eprintln!("creating missing RocksDB sidecar at {}", rocksdb_path.display());
    }
    let factory = ProviderFactory::<NodeTypes>::new(
        db,
        chain_spec.clone(),
        StaticFileProvider::read_only(datadir.join("static_files"))?,
        RocksDBProvider::builder(&rocksdb_path)
            .with_default_tables()
            .with_read_only(rocksdb_read_only)
            .build()?,
        runtime.clone(),
    )?;
    let provider: Provider = BlockchainProvider::new(factory)?;

    let base_hash = load_block(capture, first)?.parent_hash;
    // Persisted-base state reader used for fallback fields when an account is first touched.
    let db_state = provider.state_by_block_hash(base_hash)?;

    let mut pp = PayloadProcessor::<EthEvmConfig>::new(
        runtime,
        EthEvmConfig::new(chain_spec),
        &TreeConfig::default(),
        PrecompileCacheMap::default(),
    );
    let config = TreeConfig::default();

    // Proof-worker overlay (hashed) + prewarm overlay (unhashed), both accumulated across blocks.
    let mut acc_hashed = HashedPostState::default();
    let mut acc_trie = TrieUpdates::default();
    let mut unhashed = UnhashedState::default();
    let mut prev_root: Option<B256> = None;
    let mut all_ok = true;

    for number in first..first + count {
        let block = load_block(capture, number)?;
        let expected = load_block(capture, number + 1)?.parent_state_root;
        let parent_state_root = prev_root.unwrap_or(block.parent_state_root);

        let decoded_bal = Arc::new(DecodedBal::from_rlp_bytes(block.bal.clone())?);
        let env = ExecutionEnv::<EthEvmConfig> {
            evm_env: Default::default(),
            hash: B256::ZERO,
            parent_hash: base_hash,
            parent_state_root,
            transaction_count: block.transaction_count,
            gas_used: block.gas_used,
            withdrawals: None,
            decoded_bal: Some(decoded_bal.clone()),
        };

        // prewarm reads see prior blocks via an unhashed BundleState overlay
        let overlay_blocks =
            (!unhashed.accounts.is_empty()).then(|| vec![unhashed.to_executed_block(number)]);
        let provider_builder =
            StateProviderBuilder::new(provider.clone(), base_hash, overlay_blocks);

        // proof workers see prior blocks via an Immediate trie+hashed overlay
        let overlay_builder =
            OverlayBuilder::<EthPrimitives>::new(base_hash, ChangesetCache::new())
                .with_immediate_overlay(
                    Arc::new(acc_trie.clone().into_sorted()),
                    Arc::new(acc_hashed.clone().into_sorted()),
                );
        // This base has no pin_snapshot; OverlayStateProviderFactory implements
        // DatabaseProviderROFactory directly.
        let overlay_factory = OverlayStateProviderFactory::new(provider.clone(), overlay_builder);

        let start = Instant::now();
        let mut handle =
            pp.spawn(env, empty_txs(), provider_builder, overlay_factory, &config, true);
        let hashed_rx = handle.take_hashed_state_rx();
        let outcome = handle.state_root().map_err(|e| eyre::eyre!("SR task failed: {e:?}"))?;
        let elapsed = start.elapsed();

        let ok = outcome.state_root == expected;
        all_ok &= ok;
        println!(
            "block {number} | computed {:?} | {} | {:?}",
            outcome.state_root,
            if ok { "ROOT OK ✓" } else { "MISMATCH ✗" },
            elapsed,
        );

        // accumulate for the next block's overlays
        if let Some(rx) = hashed_rx {
            if let Ok(block_hashed) = rx.recv() {
                acc_hashed.extend(block_hashed);
            }
        }
        acc_trie.extend((*outcome.trie_updates).clone());
        unhashed.apply_bal(&decoded_bal, &*db_state);
        prev_root = Some(outcome.state_root);
    }

    if all_ok {
        println!("PARITY: ALL {count} ROOTS OK ✓");
        Ok(())
    } else {
        Err(eyre::eyre!("one or more roots mismatched"))
    }
}
