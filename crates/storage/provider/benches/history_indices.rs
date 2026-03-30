#![allow(missing_docs, unreachable_pub)]

use alloy_consensus::Header;
use alloy_primitives::{
    map::{FbBuildHasher, HashMap},
    Address, B256, U256,
};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use reth_chain_state::{ComputedTrieData, ExecutedBlock};
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use reth_provider::{
    test_utils::create_test_provider_factory, OriginalValuesKnown, SaveBlocksMode,
    StateWriteConfig, StateWriter, StorageSettings, StorageSettingsCache,
};
use reth_storage_api::WriteStateInput;
use revm_database::BundleState;
use revm_state::AccountInfo;
use std::sync::Arc;

const NUM_BLOCKS: u64 = 24;
const ACCOUNTS_PER_BLOCK: usize = 128;
const SLOTS_PER_ACCOUNT: u64 = 8;

fn build_bench_blocks() -> Vec<ExecutedBlock> {
    let mut blocks = Vec::with_capacity(NUM_BLOCKS as usize);
    let mut parent_hash = B256::ZERO;

    for block_num in 1..=NUM_BLOCKS {
        let mut builder = BundleState::builder(block_num..=block_num);

        for acct_idx in 0..ACCOUNTS_PER_BLOCK {
            let address = bench_address(block_num, acct_idx);
            let info = AccountInfo {
                nonce: block_num,
                balance: U256::from(block_num * 1_000 + acct_idx as u64),
                ..Default::default()
            };

            let storage: HashMap<U256, (U256, U256), FbBuildHasher<32>> = (0..SLOTS_PER_ACCOUNT)
                .map(|slot| {
                    let key = U256::from(slot + acct_idx as u64 * 1_000);
                    (key, (U256::ZERO, U256::from(block_num * 10_000 + slot)))
                })
                .collect();

            let revert_storage: Vec<(U256, U256)> = (0..SLOTS_PER_ACCOUNT)
                .map(|slot| (U256::from(slot + acct_idx as u64 * 1_000), U256::ZERO))
                .collect();

            builder = builder
                .state_present_account_info(address, info)
                .revert_account_info(block_num, address, Some(None))
                .state_storage(address, storage)
                .revert_storage(block_num, address, revert_storage);
        }

        let bundle = builder.build();
        let header = Header {
            number: block_num,
            parent_hash,
            difficulty: U256::from(1),
            ..Default::default()
        };
        let block =
            SealedBlock::<reth_ethereum_primitives::Block>::seal_parts(header, Default::default());
        parent_hash = block.hash();

        blocks.push(ExecutedBlock::new(
            Arc::new(block.try_recover().unwrap()),
            Arc::new(BlockExecutionOutput {
                result: BlockExecutionResult {
                    receipts: vec![],
                    requests: Default::default(),
                    gas_used: 0,
                    blob_gas_used: 0,
                },
                state: bundle,
            }),
            ComputedTrieData::default(),
        ));
    }

    blocks
}

fn bench_address(block_num: u64, acct_idx: usize) -> Address {
    let mut bytes = [0u8; 20];
    bytes[17] = (block_num & 0xff) as u8;
    bytes[18] = ((acct_idx >> 8) & 0xff) as u8;
    bytes[19] = (acct_idx & 0xff) as u8;
    Address::from(bytes)
}

fn bench_save_blocks_history_indices(c: &mut Criterion) {
    let blocks = build_bench_blocks();

    c.bench_function("save_blocks_history_indices", |b| {
        b.iter_batched(
            || {
                let factory = create_test_provider_factory();
                factory.set_storage_settings_cache(StorageSettings::v1());
                let blocks = blocks.clone();

                let genesis = SealedBlock::<reth_ethereum_primitives::Block>::from_sealed_parts(
                    SealedHeader::new(
                        Header { number: 0, difficulty: U256::from(1), ..Default::default() },
                        B256::ZERO,
                    ),
                    Default::default(),
                );

                let genesis_executed = ExecutedBlock::new(
                    Arc::new(genesis.try_recover().unwrap()),
                    Arc::new(BlockExecutionOutput {
                        result: BlockExecutionResult {
                            receipts: vec![],
                            requests: Default::default(),
                            gas_used: 0,
                            blob_gas_used: 0,
                        },
                        state: Default::default(),
                    }),
                    ComputedTrieData::default(),
                );

                let provider_rw = factory.provider_rw().unwrap();
                provider_rw.save_blocks(vec![genesis_executed], SaveBlocksMode::Full).unwrap();
                provider_rw.commit().unwrap();

                let provider_rw = factory.provider_rw().unwrap();
                provider_rw.save_blocks(blocks.clone(), SaveBlocksMode::BlocksOnly).unwrap();

                for block in &blocks {
                    provider_rw
                        .write_state(
                            WriteStateInput::Single {
                                outcome: block.execution_outcome(),
                                block: block.recovered_block().number,
                            },
                            OriginalValuesKnown::No,
                            StateWriteConfig {
                                write_receipts: false,
                                write_account_changesets: true,
                                write_storage_changesets: true,
                            },
                        )
                        .unwrap();
                }

                (provider_rw, blocks)
            },
            |(provider_rw, blocks)| {
                provider_rw.benchmark_history_indices(&blocks).unwrap();
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(history_indices, bench_save_blocks_history_indices);
criterion_main!(history_indices);
