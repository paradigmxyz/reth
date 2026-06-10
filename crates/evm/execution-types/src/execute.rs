use alloc::vec::Vec;
use alloy_eips::eip7685::Requests;
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Map, U256Map},
    Address, B256, U256,
};
use core::ops::Deref;
use evm2::{
    bytecode::Bytecode as Evm2Bytecode,
    evm::{AccountInfo, BlockStateAccumulator, StateChangeSink, StateChangeSource, Tracked},
};
use reth_primitives_traits::{Account, Bytecode};

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
}

impl<T> BlockExecutionOutput<T> {
    /// Creates a new block execution output and indexes the changed state for hot overlay lookups.
    pub fn new(result: BlockExecutionResult<T>, state: BlockStateAccumulator) -> Self {
        Self { result, state: state.into() }
    }

    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.state.bytecode(code_hash)
    }

    /// Return analyzed evm2 bytecode if known.
    pub fn evm2_bytecode(&self, code_hash: &B256) -> Option<Evm2Bytecode> {
        self.state.evm2_bytecode(code_hash)
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
        }
    }
}

/// Indexed evm2 block state used by in-memory overlay providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedBlockState {
    inner: BlockStateAccumulator,
    accounts: AddressMap<Tracked<Option<AccountInfo>>>,
    storage_wipes: AddressSet,
    storage: AddressMap<U256Map<U256>>,
    bytecode: B256Map<Evm2Bytecode>,
}

impl IndexedBlockState {
    /// Returns the underlying evm2 block state.
    pub const fn inner(&self) -> &BlockStateAccumulator {
        &self.inner
    }

    /// Consumes this wrapper and returns the underlying evm2 block state.
    pub fn into_inner(self) -> BlockStateAccumulator {
        self.inner
    }

    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.bytecode.get(code_hash).map(|bytecode| Bytecode::new_raw(bytecode.original_bytes()))
    }

    /// Return analyzed evm2 bytecode if known.
    pub fn evm2_bytecode(&self, code_hash: &B256) -> Option<Evm2Bytecode> {
        self.bytecode.get(code_hash).cloned()
    }

    /// Return original bytecode length if known.
    pub fn bytecode_len(&self, code_hash: &B256) -> Option<usize> {
        self.bytecode.get(code_hash).map(|bytecode| bytecode.original_bytes().len())
    }

    /// Returns the state account change for the given address.
    pub fn account_state(&self, address: &Address) -> Option<&Tracked<Option<AccountInfo>>> {
        self.accounts.get(address)
    }

    /// Get storage if value is known.
    ///
    /// Wiped storage shadows older state with zero unless the block wrote a later value.
    pub fn storage_value(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.storage
            .get(address)
            .and_then(|storage| storage.get(&storage_key).copied())
            .or_else(|| self.storage_wipes.contains(address).then_some(U256::ZERO))
    }
}

impl From<BlockStateAccumulator> for IndexedBlockState {
    fn from(inner: BlockStateAccumulator) -> Self {
        let accounts =
            inner.accounts().map(|(address, account)| (address, account.clone())).collect();
        let storage_wipes = inner.storage_wipes().collect();
        let mut storage: AddressMap<U256Map<U256>> = AddressMap::default();
        for (key, delta) in inner.storage() {
            storage.entry(key.address()).or_default().insert(key.key(), delta.current);
        }
        let bytecode =
            inner.code().map(|(code_hash, bytecode)| (*code_hash, bytecode.clone())).collect();

        Self { inner, accounts, storage_wipes, storage, bytecode }
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
