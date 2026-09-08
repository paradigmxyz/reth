//! Schedule provider snapshots, persistence, and eviction with Commonware. Database operations
//! finish synchronously between scheduling points; MDBX, `RocksDB`, and static files remain real.
//! This exercises application ordering, not simulated disk I/O or power-loss recovery.

use crate::{
    providers::{BlockchainProvider, ConsistentProvider},
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    BlockHashReader, BlockWriter, CanonChainTracker, DBProvider, DatabaseProviderFactory,
    ReceiptProvider, SaveBlocksInput, StageCheckpointReader, StateWriteConfig, StateWriter,
    StorageSettings, StorageSettingsCache, TransactionsProvider,
};
use alloy_consensus::{Header, TxLegacy, TxType};
use alloy_primitives::{Address, Signature, TxKind, B256, U256};
use commonware_runtime::{deterministic, reschedule, Runner, Spawner, Supervisor};
use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
use reth_ethereum_primitives::{
    calculate_receipt_root_no_memo, Block, BlockBody, EthPrimitives, Receipt, Transaction,
    TransactionSigned,
};
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult, ExecutionOutcome};
use reth_primitives_traits::{proofs::calculate_transaction_root, Block as _, RecoveredBlock};
use reth_stages_types::StageId;
use revm::database::OriginalValuesKnown;
use std::{
    ops::Range,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

const TIP: u64 = 6;

struct Campaign {
    provider: BlockchainProvider<MockNodeTypesWithDB>,
    blocks: Vec<ExecutedBlock<EthPrimitives>>,
    transactions: Vec<TransactionSigned>,
    receipts: Vec<Receipt>,
    tx_starts: Vec<usize>,
    persisted: AtomicU64,
    trace: Mutex<Vec<Event>>,
}

impl Campaign {
    fn new() -> Self {
        let mut blocks = Vec::new();
        let mut transactions = Vec::new();
        let mut receipts = Vec::new();
        let mut tx_starts = Vec::new();
        let mut parent_hash = B256::ZERO;

        // Fixed bodies and receipts make replay independent of entropy in block generators.
        for number in 0..=TIP {
            tx_starts.push(transactions.len());
            let tx_count = if number == 0 { 0 } else { number % 3 + 1 };
            let block_transactions = (0..tx_count)
                .map(|index| {
                    TransactionSigned::new_unhashed(
                        Transaction::Legacy(TxLegacy {
                            nonce: number * 3 + index,
                            gas_limit: 21_000,
                            gas_price: 1,
                            to: TxKind::Call(Address::repeat_byte(1)),
                            value: U256::from(number),
                            ..Default::default()
                        }),
                        Signature::new(U256::from(1), U256::from(1), false),
                    )
                })
                .collect::<Vec<_>>();
            let block_receipts = (0..tx_count)
                .map(|index| Receipt {
                    tx_type: TxType::Legacy,
                    success: (number + index) % 2 == 0,
                    cumulative_gas_used: (index + 1) * 21_000,
                    logs: Vec::new(),
                })
                .collect::<Vec<_>>();
            let block = Block {
                header: Header {
                    number,
                    parent_hash,
                    timestamp: number,
                    gas_limit: 30_000_000,
                    gas_used: tx_count * 21_000,
                    transactions_root: calculate_transaction_root(&block_transactions),
                    receipts_root: calculate_receipt_root_no_memo(&block_receipts),
                    ..Default::default()
                },
                body: BlockBody { transactions: block_transactions.clone(), ..Default::default() },
            }
            .seal_slow();
            parent_hash = block.hash();
            transactions.extend(block_transactions);
            receipts.extend(block_receipts.iter().cloned());
            blocks.push(ExecutedBlock {
                recovered_block: Arc::new(RecoveredBlock::new_sealed(
                    block,
                    vec![Address::repeat_byte(1); tx_count as usize],
                )),
                execution_output: Arc::new(BlockExecutionOutput {
                    result: BlockExecutionResult {
                        receipts: block_receipts,
                        gas_used: tx_count * 21_000,
                        ..Default::default()
                    },
                    state: Default::default(),
                }),
                ..Default::default()
            });
        }
        tx_starts.push(transactions.len());

        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let writer = factory.database_provider_rw().unwrap();
        writer.insert_block(blocks[0].recovered_block()).unwrap();
        // Even an empty genesis must advance receipt and changeset segments through block zero.
        let mut genesis = ExecutionOutcome { receipts: vec![Vec::new()], ..Default::default() };
        genesis.bundle.reverts.push(Vec::new());
        writer.write_state(&genesis, OriginalValuesKnown::No, StateWriteConfig::default()).unwrap();
        writer.commit().unwrap();
        let provider = BlockchainProvider::new(factory).unwrap();
        provider
            .canonical_in_memory_state
            .update_chain(NewCanonicalChain::Commit { new: blocks[1..].to_vec() });
        provider.set_canonical_head(blocks[TIP as usize].recovered_block().clone_sealed_header());

        Self {
            provider,
            blocks,
            transactions,
            receipts,
            tx_starts,
            persisted: AtomicU64::new(0),
            trace: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, event: Event) {
        self.trace.lock().unwrap().push(event);
    }

    fn persist(&self, tip: u64) {
        let previous = self.persisted.load(Ordering::Relaxed);
        let writer = self.provider.database_provider_rw().unwrap();
        writer
            .save_blocks(&SaveBlocksInput::new(
                self.blocks[previous as usize + 1..=tip as usize].to_vec(),
                previous,
                previous,
                tip,
                tip,
            ))
            .unwrap();
        writer.commit().unwrap();
        self.persisted.store(tip, Ordering::Relaxed);
        self.record(Event::Persist(tip));
    }

    fn evict(&self, tip: u64) {
        self.provider
            .canonical_in_memory_state
            .remove_persisted_blocks(self.blocks[tip as usize].recovered_block().num_hash());
        self.record(Event::Evict(tip));
    }

    fn snapshot(&self, reader: usize) -> ConsistentProvider<MockNodeTypesWithDB> {
        self.record(Event::Open(reader));
        let snapshot = self.provider.consistent_provider().unwrap();
        self.record(Event::Pinned(reader));
        snapshot
    }

    fn read(
        &self,
        reader: usize,
        snapshot: &ConsistentProvider<MockNodeTypesWithDB>,
        range: Range<u64>,
    ) {
        let hashes = snapshot.canonical_hashes_range(range.start, range.end).unwrap();
        assert_eq!(
            hashes,
            self.blocks[range.start as usize..range.end as usize]
                .iter()
                .map(|block| block.recovered_block().hash())
                .collect::<Vec<_>>(),
            "reader {reader}: block continuity for {range:?}"
        );
        let tx_range = self.tx_starts[range.start as usize]..self.tx_starts[range.end as usize];
        let transactions =
            snapshot.transactions_by_tx_range(tx_range.start as u64..tx_range.end as u64).unwrap();
        let receipts =
            snapshot.receipts_by_tx_range(tx_range.start as u64..tx_range.end as u64).unwrap();
        assert_eq!(
            transactions,
            self.transactions[tx_range.clone()],
            "reader {reader}: transactions"
        );
        assert_eq!(receipts, self.receipts[tx_range], "reader {reader}: receipts");
        self.record(Event::Read { reader, range, hashes, transactions, receipts });
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Open(usize),
    Pinned(usize),
    Persist(u64),
    Evict(u64),
    Read {
        reader: usize,
        range: Range<u64>,
        hashes: Vec<B256>,
        transactions: Vec<TransactionSigned>,
        receipts: Vec<Receipt>,
    },
}

fn run(seed: u64) -> (String, Vec<Event>) {
    let config =
        deterministic::Config::default().with_seed(seed).with_timeout(Some(Duration::from_secs(5)));
    deterministic::Runner::new(config).start(|context| async move {
        let campaign = Arc::new(Campaign::new());

        // Persist after opening the MDBX snapshot but before returning it to the consistent
        // provider. Capturing memory second would lose these blocks from both halves of the view.
        let hook_campaign = campaign.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let hook_fired = fired.clone();
        campaign.provider.database.db_ref().set_post_transaction_hook(Box::new(move || {
            if !hook_fired.swap(true, Ordering::Relaxed) {
                hook_campaign.persist(2);
                hook_campaign.evict(2);
            }
        }));
        let pinned = campaign.snapshot(0);
        campaign.provider.database.db_ref().set_post_transaction_hook(Box::new(|| {}));
        assert!(fired.load(Ordering::Relaxed));
        campaign.read(0, &pinned, 0..TIP + 1);

        let writer_campaign = campaign.clone();
        let writer = context.child("writer").spawn(move |_| async move {
            let mut tip = 2;
            while tip < TIP {
                tip = (tip + 1 + seed % 2).min(TIP);
                writer_campaign.persist(tip);
                reschedule().await;
                writer_campaign.evict(tip);
                reschedule().await;
            }
        });

        let mut readers = Vec::new();
        for reader in 1..=2 {
            let reader_campaign = campaign.clone();
            readers.push(context.child("reader").spawn(move |_| async move {
                reschedule().await;
                let snapshot = reader_campaign.snapshot(reader);
                for round in 0..4 {
                    reschedule().await;
                    let start = ((reader + round) as u64) % TIP;
                    reader_campaign.read(reader, &snapshot, start..TIP + 1);
                }
                reader_campaign.read(reader, &snapshot, 0..TIP + 1);
            }));
        }

        for _ in 0..4 {
            reschedule().await;
            campaign.read(0, &pinned, 0..TIP + 1);
        }
        writer.await.unwrap();
        for reader in readers {
            reader.await.unwrap();
        }

        // The original snapshot must still work after the entire chain has left memory.
        assert!(campaign.provider.canonical_in_memory_state.head_state().is_none());
        campaign.read(0, &pinned, 0..TIP + 1);
        let fresh = campaign.snapshot(3);
        campaign.read(3, &fresh, 0..TIP + 1);
        assert_eq!(
            campaign
                .provider
                .database_provider_ro()
                .unwrap()
                .get_stage_checkpoint(StageId::Finish)
                .unwrap()
                .unwrap()
                .block_number,
            TIP
        );

        let trace = std::mem::take(&mut *campaign.trace.lock().unwrap());
        (context.auditor().state(), trace)
    })
}

#[test]
fn deterministic_snapshots_across_persistence() {
    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
    };
    let sweep = seeds.len() > 1;
    let mut schedules = std::collections::BTreeSet::new();
    for seed in seeds {
        eprintln!("snapshot persistence simulation: seed={seed}");
        let first = run(seed);
        assert_eq!(first, run(seed), "replay diverged for seed {seed}");
        schedules.insert(
            first
                .1
                .iter()
                .map(|event| match event {
                    Event::Open(reader) => (0, *reader as u64),
                    Event::Pinned(reader) => (1, *reader as u64),
                    Event::Persist(tip) => (2, *tip),
                    Event::Evict(tip) => (3, *tip),
                    Event::Read { reader, .. } => (4, *reader as u64),
                })
                .collect::<Vec<_>>(),
        );
    }
    if sweep {
        assert!(schedules.len() > 1, "seed sweep must exercise different schedules");
    }
}
