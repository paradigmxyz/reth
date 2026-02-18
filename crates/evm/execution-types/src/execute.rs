use crate::HashedStorageBundleState;
use alloy_primitives::{Address, B256, U256};
use reth_primitives_traits::{Account, Bytecode};
use revm::database::{states::BundleState, BundleAccount};

pub use alloy_evm::block::BlockExecutionResult;

/// The kind of state stored in a [`BlockExecutionOutput`].
///
/// After execution, the state always uses plain `U256` storage keys ([`BundleState`]).
/// When state is reconstructed from hashed database tables, storage keys are keccak256 hashes
/// (`B256`) and are stored in a [`HashedStorageBundleState`] to prevent mixing key formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleKind {
    /// Plain storage keys (`U256`), produced by EVM execution or plain DB reconstruction.
    Plain(BundleState),
    /// Hashed storage keys (`B256`), produced by hashed DB reconstruction.
    Hashed(HashedStorageBundleState),
}

impl BundleKind {
    /// Returns a reference to the plain [`BundleState`], or `None` if hashed.
    pub const fn as_plain(&self) -> Option<&BundleState> {
        match self {
            Self::Plain(state) => Some(state),
            Self::Hashed(_) => None,
        }
    }

    /// Returns the plain [`BundleState`], or `None` if hashed.
    pub fn into_plain(self) -> Option<BundleState> {
        match self {
            Self::Plain(state) => Some(state),
            Self::Hashed(_) => None,
        }
    }

    /// Returns a reference to the [`HashedStorageBundleState`], or `None` if plain.
    pub const fn as_hashed(&self) -> Option<&HashedStorageBundleState> {
        match self {
            Self::Plain(_) => None,
            Self::Hashed(state) => Some(state),
        }
    }

    /// Returns `true` if this is a plain bundle state.
    pub const fn is_plain(&self) -> bool {
        matches!(self, Self::Plain(_))
    }

    /// Returns `true` if this is a hashed bundle state.
    pub const fn is_hashed(&self) -> bool {
        matches!(self, Self::Hashed(_))
    }
}

impl Default for BundleKind {
    fn default() -> Self {
        Self::Plain(BundleState::default())
    }
}

impl From<BundleState> for BundleKind {
    fn from(state: BundleState) -> Self {
        Self::Plain(state)
    }
}

impl From<HashedStorageBundleState> for BundleKind {
    fn from(state: HashedStorageBundleState) -> Self {
        Self::Hashed(state)
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
    pub state: BundleKind,
}

impl<T> BlockExecutionOutput<T> {
    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        match &self.state {
            BundleKind::Plain(state) => state.bytecode(code_hash).map(Bytecode),
            BundleKind::Hashed(_) => None,
        }
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        match &self.state {
            BundleKind::Plain(state) => {
                state.account(address).map(|a| a.info.as_ref().map(Into::into))
            }
            BundleKind::Hashed(_) => None,
        }
    }

    /// Returns the state [`BundleAccount`] for the given address.
    ///
    /// Returns `None` if the state is hashed.
    pub fn account_state(&self, address: &Address) -> Option<&BundleAccount> {
        match &self.state {
            BundleKind::Plain(state) => state.account(address),
            BundleKind::Hashed(_) => None,
        }
    }

    /// Get storage if value is known.
    ///
    /// This means that depending on status we can potentially return `U256::ZERO`.
    /// Returns `None` if the state is hashed.
    pub fn storage(&self, address: &Address, storage_key: U256) -> Option<U256> {
        match &self.state {
            BundleKind::Plain(state) => {
                state.account(address).and_then(|a| a.storage_slot(storage_key))
            }
            BundleKind::Hashed(_) => None,
        }
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
