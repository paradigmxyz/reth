use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::B256;
use core::{convert::Infallible, marker::PhantomData};
use evm2::{
    bytecode::Bytecode,
    evm::{
        AccountChangeRef, AccountInfoRef, BlockStateAccumulator, FrozenBlockState, StateChangeSink,
        StateChangeSource, StorageChangeRef,
    },
};
use reth_primitives_traits::Account;
use reth_trie_common::{HashedPostState, HashedStorage, KeyHasher};

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
    let bytecode_hash =
        (!info.code_hash.is_zero() && info.code_hash != KECCAK_EMPTY).then_some(info.code_hash);
    Account { nonce: info.nonce, balance: info.balance, bytecode_hash }
}
