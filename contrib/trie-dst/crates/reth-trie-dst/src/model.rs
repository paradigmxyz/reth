//! Reference state and generated execution outcomes, independent of trie-task output.

use alloy_primitives::{
    keccak256,
    map::{HashMap, U256Map},
    Address, B256, KECCAK256_EMPTY, U256,
};
use alloy_trie::{HashBuilder, Nibbles, TrieAccount};
use rand::{rngs::StdRng, Rng};
use reth_primitives_traits::Account;
use reth_trie::{HashedPostState, HashedStorage};
use revm::{database::BundleState, state::AccountInfo};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct ModelAccount {
    pub account: Account,
    pub storage: BTreeMap<U256, U256>,
    hashed_storage: BTreeMap<B256, Vec<u8>>,
    pub touched_slots: BTreeSet<U256>,
}

pub type Model = BTreeMap<Address, ModelAccount>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Update {
    Account { address: Address, account: Account },
    Storage { address: Address, slot: U256, value: U256 },
}

impl Update {
    fn apply(&self, state: &mut Model) -> (Address, HashedPostState) {
        let mut delta = HashedPostState::default();
        let address = match self {
            Self::Account { address, account } => {
                state.entry(*address).or_default().account = account.to_owned();
                delta.accounts.insert(keccak256(address), Some(account.to_owned()));
                *address
            }
            Self::Storage { address, slot, value } => {
                let entry = state.get_mut(address).expect("storage account exists");
                entry.touched_slots.insert(*slot);
                let hashed_slot = keccak256(B256::from(*slot));
                if value.is_zero() {
                    entry.storage.remove(slot);
                    entry.hashed_storage.remove(&hashed_slot);
                } else {
                    entry.storage.insert(*slot, *value);
                    entry.hashed_storage.insert(hashed_slot, alloy_rlp::encode(value));
                }
                delta.storages.insert(
                    keccak256(address),
                    HashedStorage {
                        storage: HashMap::from_iter([(keccak256(B256::from(*slot)), *value)]),
                    },
                );
                *address
            }
        };
        (address, delta)
    }
}

pub fn random_address(rng: &mut impl Rng) -> Address {
    Address::from_word(B256::from(U256::from(rng.random::<u32>())))
}

pub fn random_slot(rng: &mut impl Rng) -> U256 {
    U256::from(rng.random::<u32>())
}

pub fn generate(rng: &mut StdRng, state: &Model, updates: usize) -> Vec<Update> {
    let mut state = state.clone();
    let mut addresses = state.keys().copied().collect::<Vec<_>>();
    (0..updates)
        .map(|_| {
            // Bound the active account set while sampling new keys across all 32 bits.
            let address =
                if !addresses.is_empty() && (addresses.len() >= 16 || rng.random_ratio(3, 4)) {
                    addresses[rng.random_range(0..addresses.len())]
                } else {
                    random_address(rng)
                };
            let operation = if !state.contains_key(&address) || rng.random_ratio(1, 4) {
                let mut account =
                    state.get(&address).map(|entry| entry.account.to_owned()).unwrap_or_default();
                account.nonce += 1;
                account.balance = U256::from(rng.random_range(1..=1_000_000u64));
                Update::Account { address, account }
            } else {
                let known_slots = &state[&address].touched_slots;
                let slot = if !known_slots.is_empty() && rng.random_bool(0.5) {
                    *known_slots.iter().nth(rng.random_range(0..known_slots.len())).unwrap()
                } else {
                    random_slot(rng)
                };
                let value = if rng.random_ratio(1, 3) {
                    U256::ZERO
                } else {
                    U256::from(rng.random::<u64>())
                };
                Update::Storage { address, slot, value }
            };
            if !state.contains_key(&address) {
                addresses.push(address);
            }
            operation.apply(&mut state);
            operation
        })
        .collect()
}

pub fn apply(
    state: &mut Model,
    number: u64,
    operations: &[Update],
) -> (BundleState, Vec<HashedPostState>) {
    let mut before = BTreeMap::<Address, (Option<AccountInfo>, BTreeMap<U256, U256>)>::new();
    for operation in operations {
        let (address, slot) = match operation {
            Update::Account { address, .. } => (*address, None),
            Update::Storage { address, slot, .. } => (*address, Some(*slot)),
        };
        let entry = before.entry(address).or_insert_with(|| {
            let info = state.get(&address).map(|entry| AccountInfo {
                balance: entry.account.balance,
                nonce: entry.account.nonce,
                code_hash: entry.account.bytecode_hash.unwrap_or(KECCAK256_EMPTY),
                ..Default::default()
            });
            (info, BTreeMap::new())
        });
        if let Some(slot) = slot {
            entry.1.entry(slot).or_insert_with(|| {
                state
                    .get(&address)
                    .and_then(|account| account.storage.get(&slot))
                    .copied()
                    .unwrap_or_default()
            });
        }
    }
    let mut stream = Vec::new();
    for operation in operations {
        let (_, delta) = operation.apply(state);
        stream.push(delta);
    }

    let mut bundle = BundleState::builder(number..=number);
    for (address, (old_info, old_storage)) in before {
        let current = &state[&address];
        let info = AccountInfo {
            balance: current.account.balance,
            nonce: current.account.nonce,
            code_hash: current.account.bytecode_hash.unwrap_or(KECCAK256_EMPTY),
            ..Default::default()
        };
        bundle = bundle.state_present_account_info(address, info);
        if let Some(info) = &old_info {
            bundle = bundle.state_original_account_info(address, info.clone());
        }
        bundle = bundle.revert_account_info(number, address, Some(old_info));
        let storage = old_storage
            .into_iter()
            .map(|(slot, original)| {
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
    (bundle.build(), stream)
}

pub fn storage_root(account: &ModelAccount) -> B256 {
    let mut builder = HashBuilder::default();
    for (key, value) in &account.hashed_storage {
        builder.add_leaf(Nibbles::unpack(*key), value);
    }
    builder.root()
}

pub fn root(state: &Model) -> B256 {
    let leaves = state
        .iter()
        .map(|(address, entry)| {
            let account: TrieAccount =
                entry.account.to_owned().into_trie_account(storage_root(entry));
            (keccak256(address), alloy_rlp::encode(account))
        })
        .collect::<BTreeMap<_, _>>();
    let mut builder = HashBuilder::default();
    for (key, value) in leaves {
        builder.add_leaf(Nibbles::unpack(key), &value);
    }
    builder.root()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_preserves_account_code_hashes() {
        let address = Address::from([1; 20]);
        let old_hash = B256::from([2; 32]);
        let new_hash = B256::from([3; 32]);
        let mut state = Model::default();
        apply(
            &mut state,
            1,
            &[Update::Account {
                address,
                account: Account { bytecode_hash: Some(old_hash), ..Default::default() },
            }],
        );
        let (bundle, _) = apply(
            &mut state,
            2,
            &[Update::Account {
                address,
                account: Account { bytecode_hash: Some(new_hash), ..Default::default() },
            }],
        );
        let account = &bundle.state[&address];
        assert_eq!(account.original_info.as_ref().unwrap().code_hash, old_hash);
        assert_eq!(account.info.as_ref().unwrap().code_hash, new_hash);
    }
}
