//! Usage: `cargo bench --package reth-chain-state --bench trie_state`

#![allow(missing_docs)]

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use alloy_primitives::{Address, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rand::{seq::SliceRandom, Rng};
use reth_chain_state::{
    ExecutedBlock, ExecutedBlockWithTrieUpdates, MemoryOverlayStateProviderRef,
};
use reth_primitives::{Account, NodePrimitives};
use reth_storage_api::{noop::NoopProvider, StorageRootProvider};
use reth_trie::{
    updates::{StorageTrieUpdates, TrieUpdates},
    BranchNodeCompact, HashedPostState, HashedStorage, Nibbles,
};

struct HashedPostStateFactory {
    num_updated_accounts: usize,
    num_removed_accounts: usize,
    hashed_address_choices: Vec<B256>,
    num_storages: usize,
    num_changes_per_storage: usize,
    storage_key_choices: Vec<B256>,
}

impl HashedPostStateFactory {
    fn erc20() -> Self {
        Self {
            num_updated_accounts: 30000,
            num_removed_accounts: 0,
            hashed_address_choices: (0..100000).map(|_| B256::random()).collect(),
            num_storages: 30000,
            num_changes_per_storage: 1,
            storage_key_choices: vec![B256::random()],
        }
    }

    fn raw_transfer() -> Self {
        Self {
            num_updated_accounts: 75000,
            num_removed_accounts: 0,
            hashed_address_choices: (0..100000).map(|_| B256::random()).collect(),
            num_storages: 75000,
            num_changes_per_storage: 0,
            storage_key_choices: Vec::new(),
        }
    }

    fn uniswap() -> Self {
        Self {
            num_updated_accounts: 6360,
            num_removed_accounts: 0,
            hashed_address_choices: (0..100000).map(|_| B256::random()).collect(),
            num_storages: 6360,
            num_changes_per_storage: 3,
            storage_key_choices: vec![B256::random(), B256::random(), B256::random()],
        }
    }

    fn generate<R: Rng>(&self, rng: &mut R) -> HashedPostState {
        HashedPostState {
            accounts: self
                .hashed_address_choices
                .choose_multiple(rng, self.num_updated_accounts + self.num_removed_accounts)
                .enumerate()
                .map(|(i, k)| (*k, (i < self.num_updated_accounts).then_some(Account::default())))
                .collect(),
            storages: self
                .hashed_address_choices
                .choose_multiple(rng, self.num_storages)
                .map(|k| {
                    (
                        *k,
                        HashedStorage {
                            wiped: false,
                            storage: self
                                .storage_key_choices
                                .choose_multiple(rng, self.num_changes_per_storage)
                                .map(|k| (*k, U256::ZERO))
                                .collect(),
                        },
                    )
                })
                .collect(),
        }
    }
}

struct TrieUpdatesFactory {
    num_updated_nodes: usize,
    num_removed_nodes: usize,
    account_nibbles_choices: Vec<Nibbles>,
    num_storage_tries: usize,
    hashed_address_choices: Vec<B256>,
    storage_nibbles_choices: Vec<Nibbles>,
    num_updated_nodes_per_storage_trie: usize,
    num_removed_nodes_per_storage_trie: usize,
}

impl TrieUpdatesFactory {
    fn erc20() -> Self {
        Self {
            num_updated_nodes: 6600,
            num_removed_nodes: 0,
            account_nibbles_choices: (0..7600)
                .map(|_| Nibbles::unpack(B256::random().as_slice()))
                .collect(),
            num_storage_tries: 70000,
            hashed_address_choices: (0..100000).map(|_| B256::random()).collect(),
            storage_nibbles_choices: vec![Nibbles::unpack(B256::random().as_slice())],
            num_updated_nodes_per_storage_trie: 1,
            num_removed_nodes_per_storage_trie: 0,
        }
    }

    fn raw_transfer() -> Self {
        Self {
            num_updated_nodes: 7500,
            num_removed_nodes: 0,
            account_nibbles_choices: (0..7600)
                .map(|_| Nibbles::unpack(B256::random().as_slice()))
                .collect(),
            num_storage_tries: 100000,
            hashed_address_choices: (0..100000).map(|_| B256::random()).collect(),
            storage_nibbles_choices: Vec::new(),
            num_updated_nodes_per_storage_trie: 0,
            num_removed_nodes_per_storage_trie: 0,
        }
    }

    fn uniswap() -> Self {
        Self {
            num_updated_nodes: 4200,
            num_removed_nodes: 0,
            account_nibbles_choices: (0..6000)
                .map(|_| Nibbles::unpack(B256::random().as_slice()))
                .collect(),
            num_storage_tries: 32000,
            hashed_address_choices: (0..60000).map(|_| B256::random()).collect(),
            storage_nibbles_choices: vec![Nibbles::unpack(B256::random().as_slice())],
            num_updated_nodes_per_storage_trie: 1,
            num_removed_nodes_per_storage_trie: 0,
        }
    }

    fn generate<R: Rng>(&self, rng: &mut R) -> TrieUpdates {
        TrieUpdates {
            changed_nodes: self
                .account_nibbles_choices
                .choose_multiple(rng, self.num_updated_nodes + self.num_removed_nodes)
                .enumerate()
                .map(|(i, k)| {
                    (
                        k.clone(),
                        (i < self.num_updated_nodes).then_some(BranchNodeCompact::default()),
                    )
                })
                .collect(),
            storage_tries: self
                .hashed_address_choices
                .choose_multiple(rng, self.num_storage_tries)
                .map(|k| {
                    (
                        *k,
                        StorageTrieUpdates {
                            is_deleted: false,
                            changed_nodes: (self
                                .storage_nibbles_choices
                                .choose_multiple(
                                    rng,
                                    self.num_updated_nodes_per_storage_trie +
                                        self.num_removed_nodes_per_storage_trie,
                                )
                                .enumerate()
                                .map(|(i, k)| {
                                    (
                                        k.clone(),
                                        (i < self.num_updated_nodes_per_storage_trie)
                                            .then_some(BranchNodeCompact::default()),
                                    )
                                })
                                .collect()),
                        },
                    )
                })
                .collect(),
        }
    }
}

fn generate_erc20_blocks<N: NodePrimitives>(len: usize) -> Vec<ExecutedBlockWithTrieUpdates<N>> {
    let mut rng = rand::thread_rng();
    let hashed_post_state_factory = HashedPostStateFactory::erc20();
    let trie_updates_factory = TrieUpdatesFactory::erc20();
    (0..len)
        .map(|_| ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::default(),
                execution_output: Arc::default(),
                hashed_state: Arc::new(hashed_post_state_factory.generate(&mut rng)),
            },
            trie: Arc::new(trie_updates_factory.generate(&mut rng)),
        })
        .collect()
}

fn generate_raw_transfer_blocks<N: NodePrimitives>(
    len: usize,
) -> Vec<ExecutedBlockWithTrieUpdates<N>> {
    let mut rng = rand::thread_rng();
    let hashed_post_state_factory = HashedPostStateFactory::raw_transfer();
    let trie_updates_factory = TrieUpdatesFactory::raw_transfer();
    (0..len)
        .map(|_| ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::default(),
                execution_output: Arc::default(),
                hashed_state: Arc::new(hashed_post_state_factory.generate(&mut rng)),
            },
            trie: Arc::new(trie_updates_factory.generate(&mut rng)),
        })
        .collect()
}

fn generate_uniswap_blocks<N: NodePrimitives>(len: usize) -> Vec<ExecutedBlockWithTrieUpdates<N>> {
    let mut rng = rand::thread_rng();
    let hashed_post_state_factory = HashedPostStateFactory::uniswap();
    let trie_updates_factory = TrieUpdatesFactory::uniswap();
    (0..len)
        .map(|_| ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::default(),
                execution_output: Arc::default(),
                hashed_state: Arc::new(hashed_post_state_factory.generate(&mut rng)),
            },
            trie: Arc::new(trie_updates_factory.generate(&mut rng)),
        })
        .collect()
}

#[inline]
fn run_trie_state<N: NodePrimitives>(executed_blocks: Vec<ExecutedBlockWithTrieUpdates<N>>) {
    let provider =
        MemoryOverlayStateProviderRef::<N>::new(Box::new(NoopProvider::default()), executed_blocks);

    // Because `trie_state` is a private function, instead of calling it, we have to do this:
    provider.storage_root(Address::ZERO, HashedStorage::default()).unwrap();
}

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("ERC20", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let blocks = generate_erc20_blocks(11);
                let start = Instant::now();
                run_trie_state::<reth_primitives::EthPrimitives>(black_box(blocks));
                total += start.elapsed();
            }
            total
        })
    });
    c.bench_function("Raw Transfer", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let blocks = generate_raw_transfer_blocks(10);
                let start = Instant::now();
                run_trie_state::<reth_primitives::EthPrimitives>(black_box(blocks));
                total += start.elapsed();
            }
            total
        })
    });
    c.bench_function("Uniswap", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let blocks = generate_uniswap_blocks(3);
                let start = Instant::now();
                run_trie_state::<reth_primitives::EthPrimitives>(black_box(blocks));
                total += start.elapsed();
            }
            total
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
