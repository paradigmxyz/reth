use alloy_primitives::{keccak256, Address, B256, U256};
use reth_primitives_traits::{Account, Bytecode};
use revm::database::BundleState;

pub use alloy_evm::block::BlockExecutionResult;

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
    pub state: BundleState,
}

impl<T> BlockExecutionOutput<T> {
    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.state.bytecode(code_hash).map(Bytecode)
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.state.account(address).map(|a| a.info.as_ref().map(Into::into))
    }

    /// Get account by hashed address if account is known.
    /// This iterates through all accounts and checks if the hashed address matches.
    pub fn hashed_account(&self, hashed_address: &B256) -> Option<Option<Account>> {
        for (address, bundle_account) in self.state.state().iter() {
            if keccak256(address) == *hashed_address {
                return Some(bundle_account.info.as_ref().map(Into::into));
            }
        }
        None
    }

    /// Get storage if value is known.
    ///
    /// This means that depending on status we can potentially return `U256::ZERO`.
    pub fn storage(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.state.account(address).and_then(|a| a.storage_slot(storage_key))
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
