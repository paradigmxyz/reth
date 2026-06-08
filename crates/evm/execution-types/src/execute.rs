use alloc::vec::Vec;
use alloy_eips::eip7685::Requests;
use alloy_primitives::{Address, B256, U256};
use evm2::evm::{AccountInfo, BlockAccountDelta, FrozenBlockState};
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
    pub state: FrozenBlockState,
}

impl<T> BlockExecutionOutput<T> {
    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.state
            .code()
            .find(|(hash, _)| *hash == code_hash)
            .map(|(_, bytecode)| Bytecode::new_raw(bytecode.original_bytes()))
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.account_state(address)
            .map(|account| account.current.as_ref().map(account_info_to_reth))
    }

    /// Returns the state account change for the given address.
    pub fn account_state(&self, address: &Address) -> Option<&BlockAccountDelta> {
        self.state
            .accounts()
            .find_map(|(changed, account)| (changed == *address).then_some(account))
    }

    /// Get storage if value is known.
    ///
    /// This means that depending on status we can potentially return `U256::ZERO`.
    pub fn storage(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.state.storage().find_map(|(key, storage)| {
            (key.address() == *address && key.key() == storage_key).then_some(storage.current)
        })
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

fn account_info_to_reth(info: &AccountInfo) -> Account {
    Account {
        balance: info.balance,
        nonce: info.nonce,
        bytecode_hash: (!info.code_hash.is_zero()).then_some(info.code_hash),
    }
}
