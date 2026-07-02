use reth_storage_api::{AccountRangeEntry, AccountRangeResult};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::hashed_cursor::{HashedCursor, HashedCursorFactory};

/// Builds one account-range page by seeking and scanning the hashed account cursor.
pub(super) fn account_range<H>(
    hashed_cursor_factory: &H,
    start: alloy_primitives::B256,
    limit: usize,
) -> ProviderResult<AccountRangeResult>
where
    H: HashedCursorFactory,
{
    if limit == 0 {
        return Ok(AccountRangeResult::default())
    }

    let mut cursor = hashed_cursor_factory.hashed_account_cursor()?;
    let mut entry = cursor.seek(start)?;

    let mut accounts = Vec::with_capacity(limit);
    while let Some((hash, account)) = entry {
        if accounts.len() == limit {
            return Ok(AccountRangeResult { accounts, next_key: Some(hash) })
        }
        accounts.push(AccountRangeEntry { hash, account });
        entry = cursor.next()?;
    }

    Ok(AccountRangeResult { accounts, next_key: None })
}

#[cfg(test)]
mod tests {
    use super::account_range;
    use alloy_primitives::{map::B256Map, B256, U256};
    use reth_primitives_traits::Account;
    use reth_storage_api::AccountRangeEntry;
    use reth_trie::hashed_cursor::mock::MockHashedCursorFactory;
    use std::collections::BTreeMap;

    fn account(nonce: u64) -> Account {
        Account { nonce, balance: U256::from(nonce), bytecode_hash: None }
    }

    fn key(byte: u8) -> B256 {
        B256::with_last_byte(byte)
    }

    fn hashed_factory() -> MockHashedCursorFactory {
        let accounts =
            BTreeMap::from([(key(1), account(1)), (key(3), account(3)), (key(5), account(5))]);
        MockHashedCursorFactory::new(accounts, B256Map::default())
    }

    #[test]
    fn account_range_returns_accounts_in_hashed_order() {
        let hashed = hashed_factory();

        let result = account_range(&hashed, B256::ZERO, 10).unwrap();

        assert_eq!(
            result.accounts,
            vec![
                AccountRangeEntry { hash: key(1), account: account(1) },
                AccountRangeEntry { hash: key(3), account: account(3) },
                AccountRangeEntry { hash: key(5), account: account(5) },
            ]
        );
    }

    #[test]
    fn account_range_uses_inclusive_start_and_next_key() {
        let hashed = hashed_factory();

        let result = account_range(&hashed, key(3), 1).unwrap();

        assert_eq!(result.accounts, vec![AccountRangeEntry { hash: key(3), account: account(3) }]);
        assert_eq!(result.next_key, Some(key(5)));
    }

    #[test]
    fn account_range_returns_no_next_key_on_last_page() {
        let hashed = hashed_factory();

        let result = account_range(&hashed, key(3), 2).unwrap();

        assert_eq!(
            result.accounts,
            vec![
                AccountRangeEntry { hash: key(3), account: account(3) },
                AccountRangeEntry { hash: key(5), account: account(5) },
            ]
        );
        assert_eq!(result.next_key, None);
    }

    #[test]
    fn account_range_returns_empty_page_for_zero_limit() {
        let hashed = hashed_factory();

        let result = account_range(&hashed, B256::ZERO, 0).unwrap();

        assert_eq!(result.accounts, Vec::new());
        assert_eq!(result.next_key, None);
    }
}
