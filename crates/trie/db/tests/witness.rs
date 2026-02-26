#![allow(missing_docs)]

use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::{
    keccak256,
    map::{HashMap, HashSet},
    Address, Bytes, B256, U256,
};
use alloy_rlp::EMPTY_STRING_CODE;
use reth_db::{cursor::DbCursorRW, tables};
use reth_db_api::transaction::DbTxMut;
use reth_primitives_traits::{Account, StorageEntry};
use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
use reth_storage_api::StorageSettingsCache;
use reth_trie::{
    proof::Proof, witness::TrieWitness, HashedPostState, HashedStorage, MultiProofTargets,
    StateRoot,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseProof, DatabaseStateRoot, DatabaseTrieCursorFactory,
    DatabaseTrieWitness,
};

type DbStateRoot<'a, TX, A> =
    StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;
type DbProof<'a, TX, A> =
    Proof<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;
type DbTrieWitness<'a, TX, A> =
    TrieWitness<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;

#[test]
fn includes_empty_node_preimage() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let address = Address::random();
    let hashed_address = keccak256(address);
    let hashed_slot = B256::random();

    reth_trie_db::with_adapter!(provider, |A| {
        // witness includes empty state trie root node
        assert_eq!(
            DbTrieWitness::<_, A>::from_tx(provider.tx_ref())
                .compute(HashedPostState {
                    accounts: HashMap::from_iter([(hashed_address, Some(Account::default()))]),
                    storages: HashMap::default(),
                })
                .unwrap(),
            HashMap::from_iter([(EMPTY_ROOT_HASH, Bytes::from([EMPTY_STRING_CODE]))])
        );

        // Insert account into database
        provider.insert_account_for_hashing([(address, Some(Account::default()))]).unwrap();

        let state_root = DbStateRoot::<_, A>::from_tx(provider.tx_ref()).root().unwrap();
        let proof = <DbProof<'_, _, A> as DatabaseProof>::from_tx(provider.tx_ref());
        let multiproof = proof
            .multiproof(MultiProofTargets::from_iter([(
                hashed_address,
                HashSet::from_iter([hashed_slot]),
            )]))
            .unwrap();

        let witness = DbTrieWitness::<_, A>::from_tx(provider.tx_ref())
            .compute(HashedPostState {
                accounts: HashMap::from_iter([(hashed_address, Some(Account::default()))]),
                storages: HashMap::from_iter([(
                    hashed_address,
                    HashedStorage::from_iter(false, [(hashed_slot, U256::from(1))]),
                )]),
            })
            .unwrap();
        assert!(witness.contains_key(&state_root));
        for node in multiproof.account_subtree.values() {
            assert_eq!(witness.get(&keccak256(node)), Some(node));
        }
        // witness includes empty state trie root node
        assert_eq!(witness.get(&EMPTY_ROOT_HASH), Some(&Bytes::from([EMPTY_STRING_CODE])));
    });
}

#[test]
fn includes_nodes_for_destroyed_storage_nodes() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let address = Address::random();
    let hashed_address = keccak256(address);
    let slot = B256::random();
    let hashed_slot = keccak256(slot);

    // Insert account and slot into database
    provider.insert_account_for_hashing([(address, Some(Account::default()))]).unwrap();
    provider
        .insert_storage_for_hashing([(address, [StorageEntry { key: slot, value: U256::from(1) }])])
        .unwrap();

    reth_trie_db::with_adapter!(provider, |A| {
        let state_root = DbStateRoot::<_, A>::from_tx(provider.tx_ref()).root().unwrap();
        let proof = <DbProof<'_, _, A> as DatabaseProof>::from_tx(provider.tx_ref());
        let multiproof = proof
            .multiproof(MultiProofTargets::from_iter([(
                hashed_address,
                HashSet::from_iter([hashed_slot]),
            )]))
            .unwrap();

        let witness = DbTrieWitness::<_, A>::from_tx(provider.tx_ref())
            .compute(HashedPostState {
                accounts: HashMap::from_iter([(hashed_address, Some(Account::default()))]),
                storages: HashMap::from_iter([(
                    hashed_address,
                    HashedStorage::from_iter(true, []),
                )]), // destroyed
            })
            .unwrap();
        assert!(witness.contains_key(&state_root));
        for node in multiproof.account_subtree.values() {
            assert_eq!(witness.get(&keccak256(node)), Some(node));
        }
        for node in multiproof.storages.values().flat_map(|storage| storage.subtree.values()) {
            assert_eq!(witness.get(&keccak256(node)), Some(node));
        }
    });
}

#[test]
fn correctly_decodes_branch_node_values() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let address = Address::random();
    let hashed_address = keccak256(address);
    let hashed_slot1 = B256::repeat_byte(1);
    let hashed_slot2 = B256::repeat_byte(2);

    // Insert account and slots into database
    provider.insert_account_for_hashing([(address, Some(Account::default()))]).unwrap();
    let mut hashed_storage_cursor =
        provider.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
    hashed_storage_cursor
        .upsert(hashed_address, &StorageEntry { key: hashed_slot1, value: U256::from(1) })
        .unwrap();
    hashed_storage_cursor
        .upsert(hashed_address, &StorageEntry { key: hashed_slot2, value: U256::from(1) })
        .unwrap();

    reth_trie_db::with_adapter!(provider, |A| {
        let state_root = DbStateRoot::<_, A>::from_tx(provider.tx_ref()).root().unwrap();
        let proof = <DbProof<'_, _, A> as DatabaseProof>::from_tx(provider.tx_ref());
        let multiproof = proof
            .multiproof(MultiProofTargets::from_iter([(
                hashed_address,
                HashSet::from_iter([hashed_slot1, hashed_slot2]),
            )]))
            .unwrap();

        let witness = DbTrieWitness::<_, A>::from_tx(provider.tx_ref())
            .compute(HashedPostState {
                accounts: HashMap::from_iter([(hashed_address, Some(Account::default()))]),
                storages: HashMap::from_iter([(
                    hashed_address,
                    HashedStorage::from_iter(
                        false,
                        [hashed_slot1, hashed_slot2]
                            .map(|hashed_slot| (hashed_slot, U256::from(2))),
                    ),
                )]),
            })
            .unwrap();
        assert!(witness.contains_key(&state_root));
        for node in multiproof.account_subtree.values() {
            assert_eq!(witness.get(&keccak256(node)), Some(node));
        }
        for node in multiproof.storages.values().flat_map(|storage| storage.subtree.values()) {
            assert_eq!(witness.get(&keccak256(node)), Some(node));
        }
    });
}
