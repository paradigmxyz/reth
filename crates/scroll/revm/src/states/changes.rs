use crate::states::ScrollAccountInfo;
use reth_scroll_primitives::ScrollPostExecutionContext;
use revm::{
    db::states::{PlainStateReverts, PlainStorageChangeset, PlainStorageRevert, StateChangeset},
    primitives::{Address, Bytecode, B256},
};

/// Code copy equivalent of the [`StateChangeset`] to accommodate for the [`ScrollAccountInfo`].
#[derive(Debug)]
pub struct ScrollStateChangeset {
    /// Vector of **not** sorted accounts information.
    pub accounts: Vec<(Address, Option<ScrollAccountInfo>)>,
    /// Vector of **not** sorted storage.
    pub storage: Vec<PlainStorageChangeset>,
    /// Vector of contracts by bytecode hash. **not** sorted.
    pub contracts: Vec<(B256, Bytecode)>,
}

impl From<(StateChangeset, &ScrollPostExecutionContext)> for ScrollStateChangeset {
    fn from((changeset, context): (StateChangeset, &ScrollPostExecutionContext)) -> Self {
        Self {
            accounts: changeset
                .accounts
                .into_iter()
                .map(|(add, acc)| (add, acc.map(|a| (a, context).into())))
                .collect(),
            storage: changeset.storage,
            contracts: changeset.contracts,
        }
    }
}

/// Code copy of the [`PlainStateReverts`] to accommodate for [`ScrollAccountInfo`].
#[derive(Clone, Debug, Default)]
pub struct ScrollPlainStateReverts {
    /// Vector of account with removed contracts bytecode
    ///
    /// Note: If [`ScrollAccountInfo`] is None means that account needs to be removed.
    pub accounts: Vec<Vec<(Address, Option<ScrollAccountInfo>)>>,
    /// Vector of storage with its address.
    pub storage: Vec<Vec<PlainStorageRevert>>,
}

impl From<(PlainStateReverts, &ScrollPostExecutionContext)> for ScrollPlainStateReverts {
    fn from((reverts, context): (PlainStateReverts, &ScrollPostExecutionContext)) -> Self {
        Self {
            accounts: reverts
                .accounts
                .into_iter()
                .map(|accounts| {
                    accounts
                        .into_iter()
                        .map(|(add, acc)| (add, acc.map(|a| (a, context).into())))
                        .collect()
                })
                .collect(),
            storage: reverts.storage,
        }
    }
}

impl ScrollPlainStateReverts {
    /// Constructs new [`ScrollPlainStateReverts`] with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { accounts: Vec::with_capacity(capacity), storage: Vec::with_capacity(capacity) }
    }
}
