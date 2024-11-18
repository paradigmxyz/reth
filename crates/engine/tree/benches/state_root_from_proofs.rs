#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use reth_engine_tree::tree::calculate_state_root_from_proofs;
use reth_provider::{providers::ConsistentDbView, test_utils::create_test_provider_factory};
use reth_trie::{
    updates::TrieUpdatesSorted, HashedPostState, HashedPostStateSorted, HashedStorage, MultiProof,
};
use revm_primitives::{
    keccak256, Account, AccountInfo, AccountStatus, Address, EvmStorage, EvmStorageSlot, HashMap,
    HashSet, B256, U256,
};

fn create_test_state(size: usize) -> (HashMap<B256, HashSet<B256>>, HashedPostState) {
    let mut state = HashedPostState::default();
    let mut targets = HashMap::default();

    for i in 0..size {
        let address = Address::random();
        let hashed_address = keccak256(address);

        // Create account
        let info = AccountInfo {
            balance: U256::from(100 + i),
            nonce: i as u64,
            code_hash: B256::random(),
            code: Default::default(),
        };

        // Create storage with multiple slots
        let mut storage = EvmStorage::default();
        let mut slots = HashSet::default();
        for j in 0..100 {
            let slot = U256::from(j);
            let value = U256::from(100 + j);
            storage.insert(slot, EvmStorageSlot::new(value));
            slots.insert(keccak256(B256::from(slot)));
        }

        let account = Account { info, storage: storage.clone(), status: AccountStatus::Loaded };

        state.accounts.insert(hashed_address, Some(account.info.into()));
        state.storages.insert(
            hashed_address,
            HashedStorage::from_iter(
                false,
                storage.into_iter().map(|(k, v)| (keccak256(B256::from(k)), v.present_value)),
            ),
        );
        targets.insert(hashed_address, slots);
    }

    (targets, state)
}

fn bench_state_root_collection(c: &mut Criterion) {
    let factory = create_test_provider_factory();
    let view = ConsistentDbView::new(factory, None);

    let mut group = c.benchmark_group("state_root_collection");
    for size in &[10, 100, 1000] {
        let (_targets, state) = create_test_state(*size);
        let multiproof = MultiProof::default();

        group.bench_with_input(format!("size_{}", size), size, |b, _| {
            b.iter(|| {
                black_box(calculate_state_root_from_proofs(
                    view.clone(),
                    &TrieUpdatesSorted::default(),
                    &HashedPostStateSorted::default(),
                    multiproof.clone(),
                    state.clone(),
                ))
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_state_root_collection);
criterion_main!(benches);
