//! Reference state and generated execution outcomes, independent of trie-task output.

use alloy_primitives::{
    keccak256,
    map::{HashMap, U256Map},
    Address, B256, U256,
};
use alloy_trie::{HashBuilder, Nibbles, TrieAccount};
use rand::{rngs::StdRng, Rng};
use reth_primitives_traits::Account;
use reth_trie::{HashedPostState, HashedStorage};
use revm::{database::BundleState, state::AccountInfo};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct ModelAccount {
    pub account: Account,
    pub storage: BTreeMap<U256, U256>,
}

pub type Model = BTreeMap<Address, ModelAccount>;

pub fn generate(
    rng: &mut StdRng,
    state: &mut Model,
    number: u64,
    updates: usize,
) -> (BundleState, Vec<HashedPostState>, Vec<Value>) {
    let before = state.clone();
    let mut touched = BTreeMap::<Address, BTreeSet<U256>>::new();
    let mut stream = Vec::new();
    let mut operations = Vec::new();
    // Empty blocks also have to complete and preserve the trie.
    for _ in 0..if number.is_multiple_of(11) { 0 } else { updates } {
        let address = Address::from_word(B256::from(U256::from(rng.random_range(1..=16u64))));
        let new_account = !state.contains_key(&address);
        let entry = state.entry(address).or_default();
        let mut delta = HashedPostState::default();
        let hashed_address = keccak256(address);
        let slots = touched.entry(address).or_default();
        if new_account || rng.random_ratio(1, 4) {
            entry.account.nonce += 1;
            entry.account.balance = U256::from(rng.random_range(1..=1_000_000u64));
            delta.accounts.insert(hashed_address, Some(entry.account));
            operations.push(json!({"address": address, "account": entry.account}));
        } else {
            let slot = U256::from(rng.random_range(0..64u64));
            let value =
                if rng.random_ratio(1, 3) { U256::ZERO } else { U256::from(rng.random::<u64>()) };
            if value.is_zero() {
                entry.storage.remove(&slot);
            } else {
                entry.storage.insert(slot, value);
            }
            slots.insert(slot);
            delta.storages.insert(
                hashed_address,
                HashedStorage {
                    storage: HashMap::from_iter([(keccak256(B256::from(slot)), value)]),
                },
            );
            operations.push(json!({"address": address, "slot": slot, "value": value}));
        }
        stream.push(delta);
    }

    let mut bundle = BundleState::builder(number..=number);
    for (address, slots) in touched {
        let old = before.get(&address);
        let current = &state[&address];
        let info = AccountInfo {
            balance: current.account.balance,
            nonce: current.account.nonce,
            ..Default::default()
        };
        bundle = bundle.state_present_account_info(address, info);
        let old_info = old.map(|entry| AccountInfo {
            balance: entry.account.balance,
            nonce: entry.account.nonce,
            ..Default::default()
        });
        if let Some(info) = &old_info {
            bundle = bundle.state_original_account_info(address, info.clone());
        }
        bundle = bundle.revert_account_info(number, address, Some(old_info));
        let storage = slots
            .into_iter()
            .map(|slot| {
                let original =
                    old.and_then(|entry| entry.storage.get(&slot)).copied().unwrap_or_default();
                let present = current.storage.get(&slot).copied().unwrap_or_default();
                (slot, (original, present))
            })
            .collect::<U256Map<_>>();
        bundle = bundle
            .revert_storage(
                number,
                address,
                storage.iter().map(|(slot, (original, _))| (*slot, *original)).collect(),
            )
            .state_storage(address, storage);
    }
    (bundle.build(), stream, operations)
}

pub fn storage_root(account: &ModelAccount) -> B256 {
    let leaves = account
        .storage
        .iter()
        .map(|(slot, value)| (keccak256(B256::from(*slot)), alloy_rlp::encode(value)))
        .collect::<BTreeMap<_, _>>();
    let mut builder = HashBuilder::default();
    for (key, value) in leaves {
        builder.add_leaf(Nibbles::unpack(key), &value);
    }
    builder.root()
}

pub fn root(state: &Model) -> B256 {
    let leaves = state
        .iter()
        .map(|(address, entry)| {
            let account: TrieAccount = entry.account.into_trie_account(storage_root(entry));
            (keccak256(address), alloy_rlp::encode(account))
        })
        .collect::<BTreeMap<_, _>>();
    let mut builder = HashBuilder::default();
    for (key, value) in leaves {
        builder.add_leaf(Nibbles::unpack(key), &value);
    }
    builder.root()
}
