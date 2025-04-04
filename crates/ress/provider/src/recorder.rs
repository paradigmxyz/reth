use alloy_primitives::{keccak256, Address, B256, U256};
use reth_revm::{
    state::{AccountInfo, Bytecode},
    Database,
};
use reth_trie::{HashedPostState, HashedStorage};

/// The state witness recorder that records all state accesses during execution.
/// It does so by implementing the [`reth_revm::Database`] and recording accesses of accounts and
/// slots.
pub(crate) struct StateWitnessRecorderDatabase<D> {
    database: D,
    state: HashedPostState,
}

impl<D> StateWitnessRecorderDatabase<D> {
    pub(crate) fn new(database: D) -> Self {
        Self { database, state: Default::default() }
    }

    pub(crate) fn into_state(self) -> HashedPostState {
        self.state
    }
}

impl<D: Database> Database for StateWitnessRecorderDatabase<D> {
    type Error = D::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let maybe_account = self.database.basic(address)?;
        let hashed_address = keccak256(address);
        self.state.accounts.insert(hashed_address, maybe_account.as_ref().map(|acc| acc.into()));
        Ok(maybe_account)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let value = self.database.storage(address, index)?;
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(B256::from(index));
        self.state
            .storages
            .entry(hashed_address)
            .or_insert_with(|| HashedStorage::new(false))
            .storage
            .insert(hashed_slot, value);
        Ok(value)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.database.block_hash(number)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.database.code_by_hash(code_hash)
    }
}
