use alloc::vec::Vec;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{map::B256Map, B256};
use core::{convert::Infallible, marker::PhantomData};
use evm2::{
    bytecode::Bytecode,
    evm::{
        AccountChangeRef, AccountInfo, AccountInfoRef, BlockStateAccumulator, FrozenBlockState,
        StateChangeSink, StateChangeSource, StorageChangeRef,
    },
};
use reth_primitives_traits::Account;
use reth_trie_common::{
    HashedPostState, HashedPostStateSorted, HashedStorage, HashedStorageSorted, KeyHasher,
};

/// Returns the hashed post-state represented by an evm2 state-change source.
pub fn evm2_state_source_hashed_post_state<KH, S>(source: &S) -> HashedPostState
where
    KH: KeyHasher,
    S: StateChangeSource,
{
    let mut sink = HashedPostStateSink::<KH>::default();
    match source.visit(&mut sink) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    sink.into_hashed_post_state()
}

/// Freezes any evm2 state-change source into an owned block state.
pub fn evm2_block_state_from_state_source<S>(source: &S) -> FrozenBlockState
where
    S: StateChangeSource,
{
    let mut state = BlockStateAccumulator::new();
    match source.visit(&mut state) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    state.freeze()
}

/// Returns trie-ready sorted hashed post-state for an evm2 block state.
pub fn evm2_block_state_hashed_post_state_sorted<KH>(
    state: &FrozenBlockState,
) -> HashedPostStateSorted
where
    KH: KeyHasher,
{
    let mut accounts = state
        .accounts_sorted()
        .into_iter()
        .map(|(address, delta)| {
            (KH::hash_key(&address), delta.current.as_ref().map(account_info_to_reth))
        })
        .collect::<Vec<_>>();
    accounts.sort_unstable_by_key(|(address, _)| *address);

    let mut storages = B256Map::default();
    for address in state.storage_wipes_sorted() {
        storages.insert(
            KH::hash_key(&address),
            HashedStorageSorted { storage_slots: Vec::new(), wiped: true },
        );
    }

    for (key, delta) in state.storage_sorted() {
        let storage = storages
            .entry(KH::hash_key(&key.address()))
            .or_insert_with(|| HashedStorageSorted { storage_slots: Vec::new(), wiped: false });
        if storage.wiped && delta.current.is_zero() {
            continue
        }
        storage
            .storage_slots
            .push((KH::hash_key(&B256::new(key.key().to_be_bytes())), delta.current));
    }

    for storage in storages.values_mut() {
        storage.storage_slots.sort_unstable_by_key(|(slot, _)| *slot);
    }

    HashedPostStateSorted::new(accounts, storages)
}

struct HashedPostStateSink<KH> {
    state: HashedPostState,
    _key_hasher: PhantomData<KH>,
}

impl<KH> Default for HashedPostStateSink<KH> {
    fn default() -> Self {
        Self { state: HashedPostState::default(), _key_hasher: PhantomData }
    }
}

impl<KH> HashedPostStateSink<KH> {
    fn into_hashed_post_state(self) -> HashedPostState {
        self.state
    }
}

impl<KH> StateChangeSink for HashedPostStateSink<KH>
where
    KH: KeyHasher,
{
    type Error = Infallible;

    fn bytecode(&mut self, _code_hash: B256, _code: &Bytecode) -> Result<(), Self::Error> {
        Ok(())
    }

    fn account(&mut self, change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
        self.state
            .accounts
            .insert(KH::hash_key(&change.address), change.current.map(account_info_ref_to_reth));
        Ok(())
    }

    fn storage_wipe(&mut self, address: alloy_primitives::Address) -> Result<(), Self::Error> {
        self.state
            .storages
            .entry(KH::hash_key(&address))
            .or_insert_with(|| HashedStorage { wiped: true, storage: Default::default() });
        Ok(())
    }

    fn storage(&mut self, change: StorageChangeRef) -> Result<(), Self::Error> {
        let hashed_address = KH::hash_key(&change.address);
        let storage = self.state.storages.entry(hashed_address).or_default();
        if storage.wiped && change.current.is_zero() {
            return Ok(())
        }
        storage.storage.insert(KH::hash_key(&B256::new(change.key.to_be_bytes())), change.current);
        Ok(())
    }
}

fn account_info_ref_to_reth(info: AccountInfoRef<'_>) -> Account {
    account_parts_to_reth(info.nonce, info.balance, info.code_hash)
}

fn account_info_to_reth(info: &AccountInfo) -> Account {
    account_parts_to_reth(info.nonce, info.balance, info.code_hash)
}

fn account_parts_to_reth(nonce: u64, balance: alloy_primitives::U256, code_hash: B256) -> Account {
    let bytecode_hash = (!code_hash.is_zero() && code_hash != KECCAK_EMPTY).then_some(code_hash);
    Account { nonce, balance, bytecode_hash }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use evm2::evm::{AccountChangeRef, AccountInfoRef, StorageChangeRef};
    use reth_trie_common::KeccakKeyHasher;

    #[test]
    fn sorted_hashed_post_state_matches_streaming_conversion() {
        let address = Address::repeat_byte(0x01);
        let wiped_address = Address::repeat_byte(0x02);

        let mut accumulator = BlockStateAccumulator::new();
        accumulator
            .account(AccountChangeRef {
                address,
                original: None,
                current: Some(AccountInfoRef {
                    balance: U256::from(10),
                    nonce: 1,
                    code_hash: B256::ZERO,
                    code: None,
                }),
            })
            .unwrap();
        accumulator.storage_wipe(wiped_address).unwrap();
        accumulator
            .storage(StorageChangeRef {
                address,
                key: U256::from(1),
                original: U256::ZERO,
                current: U256::from(2),
            })
            .unwrap();
        accumulator
            .storage(StorageChangeRef {
                address: wiped_address,
                key: U256::from(3),
                original: U256::from(4),
                current: U256::ZERO,
            })
            .unwrap();
        accumulator
            .storage(StorageChangeRef {
                address: wiped_address,
                key: U256::from(5),
                original: U256::ZERO,
                current: U256::from(6),
            })
            .unwrap();

        let state = accumulator.freeze();
        let sorted = evm2_block_state_hashed_post_state_sorted::<KeccakKeyHasher>(&state);
        let streaming =
            evm2_state_source_hashed_post_state::<KeccakKeyHasher, _>(&state).into_sorted();

        assert_eq!(sorted, streaming);
    }
}
