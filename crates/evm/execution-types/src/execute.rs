use alloy_primitives::{Address, B256, U256};
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
    pub bundle: BundleState,
}

impl<T> BlockExecutionOutput<T> {
    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.bundle.bytecode(code_hash).map(Bytecode)
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.bundle.account(address).map(|a| a.info.as_ref().map(Into::into))
    }

    /// Get storage if value is known.
    ///
    /// This means that depending on status we can potentially return `U256::ZERO`.
    pub fn storage(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.bundle.account(address).and_then(|a| a.storage_slot(storage_key))
    }
}
