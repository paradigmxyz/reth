//! Reference state and generated execution outcomes, independent of trie-task output.

use alloy_primitives::{
    keccak256,
    map::{HashMap, U256Map},
    Address, B256, U256,
};
use alloy_trie::{HashBuilder, Nibbles, TrieAccount};
use evm2::evm::AccountInfo;
use rand::{rngs::StdRng, Rng};
use reth_primitives_traits::Account;
use reth_trie::{bundle_state::BundleState, HashedPostState, HashedStorage};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct ModelAccount {
    pub account: Account,
    pub storage: BTreeMap<U256, U256>,
}

pub type Model = BTreeMap<Address, ModelAccount>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Account { address: Address, account: Account },
    Storage { address: Address, slot: U256, value: U256 },
}

impl Operation {
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
                if value.is_zero() {
                    entry.storage.remove(slot);
                } else {
                    entry.storage.insert(*slot, *value);
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

pub fn generate(rng: &mut StdRng, state: &Model, updates: usize) -> Vec<Operation> {
    let mut state = state.clone();
    (0..updates)
        .map(|_| {
            let address = Address::from_word(B256::from(U256::from(rng.random_range(1..=16u64))));
            let operation = if !state.contains_key(&address) || rng.random_ratio(1, 4) {
                let mut account =
                    state.get(&address).map(|entry| entry.account.to_owned()).unwrap_or_default();
                account.nonce += 1;
                account.balance = U256::from(rng.random_range(1..=1_000_000u64));
                Operation::Account { address, account }
            } else {
                let slot = U256::from(rng.random_range(0..64u64));
                let value = if rng.random_ratio(1, 3) {
                    U256::ZERO
                } else {
                    U256::from(rng.random::<u64>())
                };
                Operation::Storage { address, slot, value }
            };
            operation.apply(&mut state);
            operation
        })
        .collect()
}

pub fn apply(
    state: &mut Model,
    number: u64,
    operations: &[Operation],
) -> (BundleState, Vec<HashedPostState>) {
    let before = state.clone();
    let mut touched = BTreeMap::<Address, BTreeSet<U256>>::new();
    let mut stream = Vec::new();
    for operation in operations {
        let (address, delta) = operation.apply(state);
        let slots = touched.entry(address).or_default();
        if let Operation::Storage { slot, .. } = operation {
            slots.insert(*slot);
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
    (bundle.build(), stream)
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
