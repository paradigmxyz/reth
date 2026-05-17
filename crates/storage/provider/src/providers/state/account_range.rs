use reth_storage_api::{AccountRangeEntry, AccountRangeResult};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::PrefixSet,
    trie_cursor::TrieCursorFactory,
    walker::TrieWalker,
};

/// Builds one account-range page from trie-backed state traversal.
pub(super) fn account_range<T, H>(
    trie_cursor_factory: &T,
    hashed_cursor_factory: &H,
    start: alloy_primitives::B256,
    limit: usize,
) -> ProviderResult<AccountRangeResult>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory,
{
    if limit == 0 {
        return Ok(AccountRangeResult::default())
    }

    let trie_cursor = trie_cursor_factory.account_trie_cursor()?;
    let hashed_account_cursor = hashed_cursor_factory.hashed_account_cursor()?;
    let walker =
        TrieWalker::<_>::state_trie(trie_cursor, PrefixSet::default()).with_branch_skips_disabled();
    let mut iter =
        TrieNodeIter::state_trie(walker, hashed_account_cursor).with_start_hashed_key(start);

    let mut accounts = Vec::with_capacity(limit);
    while let Some(node) = iter.try_next()? {
        if let TrieElement::Leaf(hash, account) = node {
            if accounts.len() == limit {
                return Ok(AccountRangeResult { accounts, next_key: Some(hash) })
            }
            accounts.push(AccountRangeEntry { hash, account });
        }
    }

    Ok(AccountRangeResult { accounts, next_key: None })
}

#[cfg(test)]
mod tests {
    use super::account_range;
    use alloy_primitives::{map::B256Map, B256, U256};
    use reth_primitives_traits::Account;
    use reth_storage_api::AccountRangeEntry;
    use reth_trie::{
        hashed_cursor::mock::MockHashedCursorFactory, trie_cursor::mock::MockTrieCursorFactory,
    };
    use std::collections::BTreeMap;

    fn account(nonce: u64) -> Account {
        Account { nonce, balance: U256::from(nonce), bytecode_hash: None }
    }

    fn key(byte: u8) -> B256 {
        B256::with_last_byte(byte)
    }

    fn factories() -> (MockTrieCursorFactory, MockHashedCursorFactory) {
        let accounts =
            BTreeMap::from([(key(1), account(1)), (key(3), account(3)), (key(5), account(5))]);
        (
            MockTrieCursorFactory::new(BTreeMap::default(), B256Map::default()),
            MockHashedCursorFactory::new(accounts, B256Map::default()),
        )
    }

    #[test]
    fn account_range_returns_accounts_in_hashed_order() {
        let (trie, hashed) = factories();

        let result = account_range(&trie, &hashed, B256::ZERO, 10).unwrap();

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
        let (trie, hashed) = factories();

        let result = account_range(&trie, &hashed, key(3), 1).unwrap();

        assert_eq!(result.accounts, vec![AccountRangeEntry { hash: key(3), account: account(3) }]);
        assert_eq!(result.next_key, Some(key(5)));
    }

    #[test]
    fn account_range_returns_no_next_key_on_last_page() {
        let (trie, hashed) = factories();

        let result = account_range(&trie, &hashed, key(3), 2).unwrap();

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
        let (trie, hashed) = factories();

        let result = account_range(&trie, &hashed, B256::ZERO, 0).unwrap();

        assert_eq!(result.accounts, Vec::new());
        assert_eq!(result.next_key, None);
    }
}
