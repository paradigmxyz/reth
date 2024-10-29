use alloc::vec::Vec;
use alloy_primitives::{
    keccak256,
    map::{HashMap, HashSet},
    Address, BlockNumber, Bytes, StorageKey, B256, U256,
};
use reth_primitives::{Account, Bytecode};
use reth_storage_api::{
    AccountReader, BlockHashReader, StateProofProvider, StateProvider, StateRootProvider,
    StorageRootProvider,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof, StorageProof,
    TrieInput,
};

/// Mock state for testing
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct StateProviderTest {
    accounts: HashMap<Address, (HashMap<StorageKey, U256>, Account)>,
    contracts: HashMap<B256, Bytecode>,
    block_hash: HashMap<u64, B256>,
}

impl StateProviderTest {
    /// Insert account.
    pub fn insert_account(
        &mut self,
        address: Address,
        mut account: Account,
        bytecode: Option<Bytes>,
        storage: HashMap<StorageKey, U256>,
    ) {
        if let Some(bytecode) = bytecode {
            let hash = keccak256(&bytecode);
            account.bytecode_hash = Some(hash);
            self.contracts.insert(hash, Bytecode::new_raw(bytecode));
        }
        self.accounts.insert(address, (storage, account));
    }

    /// Insert a block hash.
    pub fn insert_block_hash(&mut self, block_number: u64, block_hash: B256) {
        self.block_hash.insert(block_number, block_hash);
    }
}

impl AccountReader for StateProviderTest {
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        Ok(self.accounts.get(&address).map(|(_, acc)| *acc))
    }
}

impl BlockHashReader for StateProviderTest {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        Ok(self.block_hash.get(&number).copied())
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let range = start..end;
        Ok(self
            .block_hash
            .iter()
            .filter_map(|(block, hash)| range.contains(block).then_some(*hash))
            .collect())
    }
}

impl StateRootProvider for StateProviderTest {
    fn state_root(&self, _hashed_state: HashedPostState) -> ProviderResult<B256> {
        unimplemented!("state root computation is not supported")
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        unimplemented!("state root computation is not supported")
    }

    fn state_root_with_updates(
        &self,
        _hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        unimplemented!("state root computation is not supported")
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        unimplemented!("state root computation is not supported")
    }
}

impl StorageRootProvider for StateProviderTest {
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        unimplemented!("storage root is not supported")
    }

    fn storage_proof(
        &self,
        _address: Address,
        _slot: B256,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        unimplemented!("proof generation is not supported")
    }
}

impl StateProofProvider for StateProviderTest {
    fn proof(
        &self,
        _input: TrieInput,
        _address: Address,
        _slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        unimplemented!("proof generation is not supported")
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: HashMap<B256, HashSet<B256>>,
    ) -> ProviderResult<MultiProof> {
        unimplemented!("proof generation is not supported")
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        unimplemented!("witness generation is not supported")
    }
}

impl StateProvider for StateProviderTest {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<alloy_primitives::StorageValue>> {
        Ok(self.accounts.get(&account).and_then(|(storage, _)| storage.get(&storage_key).copied()))
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        Ok(self.contracts.get(&code_hash).cloned())
    }
}
