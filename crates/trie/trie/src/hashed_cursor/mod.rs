use alloy_primitives::{keccak256, Address, B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::HashedPostState;
use revm::database::BundleAccount;

/// Implementation of hashed state cursor traits for the post state.
mod post_state;
pub use post_state::*;

/// Implementation of noop hashed state cursor.
pub mod noop;

/// Mock trie cursor implementations.
#[cfg(any(test, feature = "test-utils"))]
pub mod mock;

/// Metrics tracking hashed cursor implementations.
pub mod metrics;
#[cfg(feature = "metrics")]
pub use metrics::HashedCursorMetrics;
pub use metrics::{HashedCursorMetricsCache, InstrumentedHashedCursor};

/// The factory trait for creating cursors over the hashed state.
#[auto_impl::auto_impl(&)]
pub trait HashedCursorFactory {
    /// The hashed account cursor type.
    type AccountCursor<'a>: HashedCursor<Value = Account>
    where
        Self: 'a;
    /// The hashed storage cursor type.
    type StorageCursor<'a>: HashedStorageCursor<Value = U256>
    where
        Self: 'a;

    /// Returns a cursor for iterating over all hashed accounts in the state.
    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError>;

    /// Returns a cursor for iterating over all hashed storage entries in the state.
    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError>;
}

/// The cursor for iterating over hashed entries.
#[auto_impl::auto_impl(&mut)]
pub trait HashedCursor {
    /// Value returned by the cursor.
    type Value: std::fmt::Debug;

    /// Seek an entry greater than or equal to the given key and position the cursor there.
    /// Returns the first entry with the key greater than or equal to the sought key.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError>;

    /// Move the cursor to the next entry and return it.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError>;

    /// Reset the cursor to its initial state.
    ///
    /// # Important
    ///
    /// After calling this method, the subsequent operation MUST be a [`HashedCursor::seek`] call.
    fn reset(&mut self);
}

/// The cursor for iterating over hashed storage entries.
#[auto_impl::auto_impl(&mut)]
pub trait HashedStorageCursor: HashedCursor {
    /// Returns `true` if there are no entries for a given key.
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError>;

    /// Set the hashed address for the storage cursor.
    ///
    /// # Important
    ///
    /// After calling this method, the subsequent operation MUST be a [`HashedCursor::seek`] call.
    fn set_hashed_address(&mut self, hashed_address: B256);
}

/// Upper bound on the number of account entries reserved up front per page.
///
/// `limit` reaches this primitive straight from callers such as `debug_accountRange`, whose page
/// size is client-supplied. Sizing the initial allocation directly from an unbounded `limit` would
/// let a single request reserve (and, on failure, abort the process on) an arbitrarily large buffer
/// before a single row is read. The reservation is therefore capped; the returned page is still
/// bounded only by `limit`, with the vector growing on demand for legitimately large pages.
const MAX_ACCOUNT_RANGE_PREALLOC: usize = 1024;

/// Accounts in a range and the first hashed key of the next page, if any.
pub type HashedAccountRange = (Vec<(B256, Account)>, Option<B256>);

/// Builds one account-range page by seeking and scanning the hashed account cursor.
///
/// Returns up to `limit` accounts whose hashed keys are greater than or equal to `start`, in
/// hashed-key order, together with the first key of the next page when more accounts remain.
pub fn account_range(
    cursor_factory: &impl HashedCursorFactory,
    start: B256,
    limit: usize,
) -> Result<HashedAccountRange, DatabaseError> {
    if limit == 0 {
        return Ok((Vec::new(), None))
    }

    let mut cursor = cursor_factory.hashed_account_cursor()?;
    let mut entry = cursor.seek(start)?;

    let mut accounts = Vec::with_capacity(limit.min(MAX_ACCOUNT_RANGE_PREALLOC));
    while let Some((hash, account)) = entry {
        if accounts.len() == limit {
            return Ok((accounts, Some(hash)))
        }
        accounts.push((hash, account));
        entry = cursor.next()?;
    }

    Ok((accounts, None))
}

/// Materializes storage deletions for destroyed accounts as explicit zero-valued slot updates.
///
/// Accounts absent from the bundle pre-state are skipped because they cannot have parent storage.
/// Final bundle values take precedence so that destroy-then-recreate transitions retain storage
/// written by the recreated account.
pub fn zero_destroyed_account_storage<'a>(
    cursor_factory: &impl HashedCursorFactory,
    accounts: impl IntoIterator<Item = (&'a Address, &'a BundleAccount)>,
    hashed_state: &mut HashedPostState,
) -> Result<(), DatabaseError> {
    let mut destroyed_accounts = accounts
        .into_iter()
        .filter(|(_, account)| account.was_destroyed() && account.original_info.is_some())
        .map(|(address, _)| keccak256(address));
    let Some(mut hashed_address) = destroyed_accounts.next() else { return Ok(()) };
    let mut cursor = cursor_factory.hashed_storage_cursor(hashed_address)?;

    loop {
        if let Some((hashed_slot, _)) = cursor.seek(B256::ZERO)? {
            let storage = &mut hashed_state.storages.entry(hashed_address).or_default().storage;
            storage.entry(hashed_slot).or_insert(U256::ZERO);
            while let Some((hashed_slot, _)) = cursor.next()? {
                storage.entry(hashed_slot).or_insert(U256::ZERO);
            }
        }

        let Some(next_hashed_address) = destroyed_accounts.next() else { break };
        hashed_address = next_hashed_address;
        cursor.set_hashed_address(hashed_address);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::database::AccountStatus;

    #[test]
    fn zero_destroyed_storage_skips_new_accounts() {
        let address = Address::with_last_byte(1);
        let account = BundleAccount::new(None, None, Default::default(), AccountStatus::Destroyed);
        let mut hashed_state = HashedPostState::default();

        zero_destroyed_account_storage(
            &mock::MockHashedCursorFactory::default(),
            [(&address, &account)],
            &mut hashed_state,
        )
        .unwrap();

        assert!(hashed_state.storages.is_empty());
    }
}
