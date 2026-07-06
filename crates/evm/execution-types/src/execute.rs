use alloc::vec::Vec;
use alloy_eips::eip7685::Requests;
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Map, U256Map},
    Address, B256, U256,
};
#[cfg(not(feature = "std"))]
use core::cell::OnceCell;
use core::ops::Deref;
use evm2::{
    bytecode::Bytecode as ExecutableBytecode,
    evm::{AccountInfo, BlockStateAccumulator, StateChangeSink, StateChangeSource, Tracked},
};
use reth_primitives_traits::{Account, Bytecode};
use reth_trie_common::{HashedPostState, KeyHasher};
#[cfg(feature = "std")]
use std::sync::OnceLock as OnceCell;

/// The result of executing a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockExecutionResult<T> {
    /// All the receipts of the transactions in the block.
    pub receipts: Vec<T>,
    /// All the EIP-7685 requests in the block.
    pub requests: Requests,
    /// The total gas used by the block.
    pub gas_used: u64,
    /// Blob gas used by the block.
    pub blob_gas_used: u64,
}

impl<T> Default for BlockExecutionResult<T> {
    fn default() -> Self {
        Self {
            receipts: Default::default(),
            requests: Default::default(),
            gas_used: 0,
            blob_gas_used: 0,
        }
    }
}

/// [`BlockExecutionResult`] combined with state.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    derive_more::AsRef,
    derive_more::AsMut,
    derive_more::Deref,
    derive_more::DerefMut,
)]
pub struct BlockExecutionOutput<T> {
    /// All the receipts of the transactions in the block.
    #[as_ref]
    #[as_mut]
    #[deref]
    #[deref_mut]
    pub result: BlockExecutionResult<T>,
    /// The changed state of the block after execution.
    pub state: IndexedBlockState,
    /// Trie-ready hashed post-state for this block, if produced during execution.
    pub hashed_state: Option<HashedPostState>,
}

impl<T> BlockExecutionOutput<T> {
    /// Creates a new block execution output and indexes the changed state for hot overlay lookups.
    pub fn new(result: BlockExecutionResult<T>, state: BlockStateAccumulator) -> Self {
        Self { result, state: state.into(), hashed_state: None }
    }

    /// Creates a new block execution output with a precomputed hashed post-state.
    pub fn new_with_hashed_state(
        result: BlockExecutionResult<T>,
        state: BlockStateAccumulator,
        hashed_state: HashedPostState,
    ) -> Self {
        Self { result, state: state.into(), hashed_state: Some(hashed_state) }
    }

    /// Attaches an optional precomputed hashed post-state to the output.
    pub fn with_hashed_state(mut self, hashed_state: Option<HashedPostState>) -> Self {
        self.hashed_state = hashed_state;
        self
    }

    /// Returns the hashed post-state for this block output.
    pub fn hash_state_slow<KH: KeyHasher>(&self) -> HashedPostState {
        self.hashed_state
            .clone()
            .unwrap_or_else(|| crate::hashed_post_state_from_state_source::<KH, _>(&self.state))
    }

    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.state.bytecode(code_hash)
    }

    /// Return original bytecode length if known.
    pub fn bytecode_len(&self, code_hash: &B256) -> Option<usize> {
        self.state.bytecode_len(code_hash)
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.account_state(address)
            .map(|account| account.current.as_ref().map(account_info_to_reth))
    }

    /// Returns the state account change for the given address.
    pub fn account_state(&self, address: &Address) -> Option<&Tracked<Option<AccountInfo>>> {
        self.state.account_state(address)
    }

    /// Get storage if value is known.
    ///
    /// This means that depending on status we can potentially return `U256::ZERO`.
    pub fn storage(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.state.storage_value(address, storage_key)
    }
}

impl<T> Default for BlockExecutionOutput<T> {
    fn default() -> Self {
        Self {
            result: BlockExecutionResult {
                receipts: Default::default(),
                requests: Default::default(),
                gas_used: 0,
                blob_gas_used: 0,
            },
            state: Default::default(),
            hashed_state: None,
        }
    }
}

/// Indexed execution state used by in-memory overlay providers.
#[derive(Debug, Clone)]
pub struct IndexedBlockState {
    inner: BlockStateAccumulator,
    index: OnceCell<BlockStateIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockStateIndex {
    accounts: AddressMap<Tracked<Option<AccountInfo>>>,
    storage_wipes: AddressSet,
    storage: AddressMap<U256Map<U256>>,
    bytecode: B256Map<ExecutableBytecode>,
}

impl IndexedBlockState {
    /// Creates a new lazily indexed execution state.
    pub const fn new(inner: BlockStateAccumulator) -> Self {
        Self { inner, index: OnceCell::new() }
    }

    /// Returns the underlying execution state.
    pub const fn inner(&self) -> &BlockStateAccumulator {
        &self.inner
    }

    /// Consumes this wrapper and returns the underlying execution state.
    pub fn into_inner(self) -> BlockStateAccumulator {
        self.inner
    }

    fn index(&self) -> &BlockStateIndex {
        self.index.get_or_init(|| BlockStateIndex::from_state(&self.inner))
    }

    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.index().bytecode.get(code_hash).cloned().map(Into::into)
    }

    /// Return original bytecode length if known.
    pub fn bytecode_len(&self, code_hash: &B256) -> Option<usize> {
        self.index().bytecode.get(code_hash).map(|bytecode| bytecode.original_bytes().len())
    }

    /// Returns the state account change for the given address.
    pub fn account_state(&self, address: &Address) -> Option<&Tracked<Option<AccountInfo>>> {
        self.index().accounts.get(address)
    }

    /// Get storage if value is known.
    ///
    /// Wiped storage shadows older state with zero unless the block wrote a later value.
    pub fn storage_value(&self, address: &Address, storage_key: U256) -> Option<U256> {
        let index = self.index();
        index
            .storage
            .get(address)
            .and_then(|storage| storage.get(&storage_key).copied())
            .or_else(|| index.storage_wipes.contains(address).then_some(U256::ZERO))
    }
}

impl From<BlockStateAccumulator> for IndexedBlockState {
    fn from(inner: BlockStateAccumulator) -> Self {
        Self::new(inner)
    }
}

impl PartialEq for IndexedBlockState {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for IndexedBlockState {}

impl BlockStateIndex {
    fn from_state(inner: &BlockStateAccumulator) -> Self {
        let accounts =
            inner.accounts().map(|(address, account)| (address, account.clone())).collect();
        let storage_wipes = inner.storage_wipes().collect();
        let mut storage: AddressMap<U256Map<U256>> = AddressMap::default();
        for (key, delta) in inner.storage() {
            storage.entry(key.address()).or_default().insert(key.key(), delta.current);
        }
        let bytecode =
            inner.code().map(|(code_hash, bytecode)| (*code_hash, bytecode.clone())).collect();

        Self { accounts, storage_wipes, storage, bytecode }
    }
}

impl Default for IndexedBlockState {
    fn default() -> Self {
        BlockStateAccumulator::default().into()
    }
}

impl Deref for IndexedBlockState {
    type Target = BlockStateAccumulator;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl StateChangeSource for IndexedBlockState {
    fn visit<S: StateChangeSink>(&self, sink: &mut S) -> Result<(), S::Error> {
        self.inner.visit(sink)
    }
}

fn account_info_to_reth(info: &AccountInfo) -> Account {
    Account {
        balance: info.balance,
        nonce: info.nonce,
        bytecode_hash: (!info.code_hash.is_zero()).then_some(info.code_hash),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use evm2::evm::{AccountChangeRef, AccountInfoRef, StorageChange};

    #[test]
    fn indexed_block_state_builds_lookup_index_lazily() {
        let address = Address::repeat_byte(0x42);
        let wiped_address = Address::repeat_byte(0x43);
        let code_hash = B256::repeat_byte(0x24);
        let bytecode = ExecutableBytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        let mut state = BlockStateAccumulator::new();

        state.bytecode(code_hash, &bytecode).unwrap();
        state
            .account(AccountChangeRef {
                address,
                original: None,
                current: Some(AccountInfoRef {
                    balance: U256::from(7),
                    nonce: 3,
                    code_hash,
                    code: Some(&bytecode),
                }),
            })
            .unwrap();
        state.storage_wipe(wiped_address).unwrap();
        StateChangeSink::storage(
            &mut state,
            StorageChange {
                address,
                key: U256::from(9),
                original: U256::ZERO,
                current: U256::from(10),
            },
        )
        .unwrap();

        let indexed = IndexedBlockState::new(state);
        let account = indexed.account_state(&address).unwrap().current.as_ref().unwrap();

        assert_eq!(account.balance, U256::from(7));
        assert_eq!(indexed.storage_value(&wiped_address, U256::from(8)), Some(U256::ZERO));
        assert_eq!(indexed.storage_value(&address, U256::from(9)), Some(U256::from(10)));
        assert_eq!(indexed.bytecode_len(&code_hash), Some(2));
        assert_eq!(
            indexed.bytecode(&code_hash).unwrap().original_bytes(),
            bytecode.original_bytes()
        );
    }
}
