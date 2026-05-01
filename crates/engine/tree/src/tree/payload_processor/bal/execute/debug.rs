use alloy_eip7928::{AccountChanges, BalanceChange, CodeChange, NonceChange, SlotChanges};
use alloy_primitives::{keccak256, Address, B256, U256};

/// Maximum number of entries shown per side in a [`FieldDiffWindow`]. Bounds
/// log volume when the diverging field has many entries past the divergence
/// point.
const FIELD_WINDOW: usize = 8;

/// Compact summary of where two BALs first diverge.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct BalDivergence {
    received_accounts: usize,
    rebuilt_accounts: usize,
    first_diff: Option<AccountDivergence>,
}

#[derive(Debug, PartialEq, Eq)]
struct AccountDivergence {
    index: usize,
    received: Option<AccountSummary>,
    rebuilt: Option<AccountSummary>,
    fields_differ: AccountFieldFlags,
}

impl AccountDivergence {
    fn received_only(index: usize, account: &AccountChanges) -> Self {
        Self {
            index,
            received: Some(AccountSummary::from_account(account)),
            rebuilt: None,
            fields_differ: AccountFieldFlags::default(),
        }
    }

    fn rebuilt_only(index: usize, account: &AccountChanges) -> Self {
        Self {
            index,
            received: None,
            rebuilt: Some(AccountSummary::from_account(account)),
            fields_differ: AccountFieldFlags::default(),
        }
    }

    fn address_mismatch(index: usize, received: &AccountChanges, rebuilt: &AccountChanges) -> Self {
        Self {
            index,
            received: Some(AccountSummary::from_account(received)),
            rebuilt: Some(AccountSummary::from_account(rebuilt)),
            fields_differ: AccountFieldFlags::default(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AccountSummary {
    address: Address,
    storage_changes: usize,
    storage_reads: usize,
    balance_changes: usize,
    nonce_changes: usize,
    code_changes: usize,
}

impl AccountSummary {
    const fn from_account(account: &AccountChanges) -> Self {
        Self {
            address: account.address,
            storage_changes: account.storage_changes.len(),
            storage_reads: account.storage_reads.len(),
            balance_changes: account.balance_changes.len(),
            nonce_changes: account.nonce_changes.len(),
            code_changes: account.code_changes.len(),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AccountFieldFlags {
    storage_changes: bool,
    storage_reads: bool,
    balance_changes: bool,
    nonce_changes: bool,
    code_changes: bool,
}

impl AccountFieldFlags {
    fn new(received: &AccountChanges, rebuilt: &AccountChanges) -> Self {
        Self {
            storage_changes: received.storage_changes != rebuilt.storage_changes,
            storage_reads: received.storage_reads != rebuilt.storage_reads,
            balance_changes: received.balance_changes != rebuilt.balance_changes,
            nonce_changes: received.nonce_changes != rebuilt.nonce_changes,
            code_changes: received.code_changes != rebuilt.code_changes,
        }
    }

    const fn is_divergent(&self) -> bool {
        self.storage_changes ||
            self.storage_reads ||
            self.balance_changes ||
            self.nonce_changes ||
            self.code_changes
    }
}

pub(super) fn first_bal_divergence(
    received: &[AccountChanges],
    rebuilt: &[AccountChanges],
) -> BalDivergence {
    use core::cmp::Ordering;

    let mut change_index = 0;
    let first_diff = loop {
        match (received.get(change_index), rebuilt.get(change_index)) {
            (Some(received_account), Some(rebuilt_account)) => {
                match received_account.address.cmp(&rebuilt_account.address) {
                    Ordering::Less | Ordering::Greater => {
                        break Some(AccountDivergence::address_mismatch(
                            change_index,
                            received_account,
                            rebuilt_account,
                        ));
                    }
                    Ordering::Equal => {
                        let fields_differ =
                            AccountFieldFlags::new(received_account, rebuilt_account);
                        if fields_differ.is_divergent() {
                            break Some(AccountDivergence {
                                index: change_index,
                                received: Some(AccountSummary::from_account(received_account)),
                                rebuilt: Some(AccountSummary::from_account(rebuilt_account)),
                                fields_differ,
                            });
                        }
                        change_index += 1;
                    }
                }
            }
            (Some(account), None) => {
                break Some(AccountDivergence::received_only(change_index, account));
            }
            (None, Some(account)) => {
                break Some(AccountDivergence::rebuilt_only(change_index, account));
            }
            (None, None) => break None,
        }
    };

    BalDivergence { received_accounts: received.len(), rebuilt_accounts: rebuilt.len(), first_diff }
}

impl BalDivergence {
    /// Return a deep-diff for the first divergent account when both sides
    /// have an entry at that index with matching addresses (i.e. the
    /// divergence is in field contents, not in account membership). When
    /// `first_diff` is `None`, the addresses on each side differ, or one
    /// side is missing the account, returns `None` — those cases are
    /// already covered by the summary-level event.
    pub(super) fn deep_diff(
        &self,
        received: &[AccountChanges],
        rebuilt: &[AccountChanges],
    ) -> Option<AccountDeepDiff> {
        let div = self.first_diff.as_ref()?;
        let received_account = received.get(div.index)?;
        let rebuilt_account = rebuilt.get(div.index)?;
        if received_account.address != rebuilt_account.address {
            return None;
        }
        Some(first_account_deep_diff(received_account, rebuilt_account, &div.fields_differ))
    }
}

/// First N entries on each side of a single field starting from the position
/// where the two sides first disagree. Capped at [`FIELD_WINDOW`] per side.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct FieldDiffWindow<T> {
    matching_prefix_len: usize,
    received: Vec<T>,
    rebuilt: Vec<T>,
}

/// Trace-level deep-diff for a single account, focused on the first set
/// flag in [`AccountFieldFlags`] (priority order: `storage_changes` →
/// `storage_reads` → `balance_changes` → `nonce_changes` → `code_changes`).
/// At most one of the option fields is `Some`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct AccountDeepDiff {
    address: Address,
    storage_changes: Option<FieldDiffWindow<SlotChanges>>,
    storage_reads: Option<FieldDiffWindow<U256>>,
    balance_changes: Option<FieldDiffWindow<BalanceChange>>,
    nonce_changes: Option<FieldDiffWindow<NonceChange>>,
    code_changes: Option<FieldDiffWindow<CodeChangeSummary>>,
}

/// Compact replacement for [`CodeChange`] that hashes the bytecode instead of
/// embedding it. Bytecode payloads are arbitrarily large and never useful in
/// log output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodeChangeSummary {
    block_access_index: u64,
    code_hash: B256,
}

impl From<&CodeChange> for CodeChangeSummary {
    fn from(change: &CodeChange) -> Self {
        Self {
            block_access_index: change.block_access_index,
            code_hash: keccak256(&change.new_code),
        }
    }
}

fn field_diff_window<T: PartialEq + Clone>(received: &[T], rebuilt: &[T]) -> FieldDiffWindow<T> {
    let matching_prefix_len =
        received.iter().zip(rebuilt.iter()).take_while(|(a, b)| a == b).count();
    let received_window =
        received[matching_prefix_len..].iter().take(FIELD_WINDOW).cloned().collect();
    let rebuilt_window =
        rebuilt[matching_prefix_len..].iter().take(FIELD_WINDOW).cloned().collect();
    FieldDiffWindow { matching_prefix_len, received: received_window, rebuilt: rebuilt_window }
}

fn first_account_deep_diff(
    received: &AccountChanges,
    rebuilt: &AccountChanges,
    flags: &AccountFieldFlags,
) -> AccountDeepDiff {
    let mut deep = AccountDeepDiff {
        address: received.address,
        storage_changes: None,
        storage_reads: None,
        balance_changes: None,
        nonce_changes: None,
        code_changes: None,
    };
    if flags.storage_changes {
        deep.storage_changes =
            Some(field_diff_window(&received.storage_changes, &rebuilt.storage_changes));
    } else if flags.storage_reads {
        deep.storage_reads =
            Some(field_diff_window(&received.storage_reads, &rebuilt.storage_reads));
    } else if flags.balance_changes {
        deep.balance_changes =
            Some(field_diff_window(&received.balance_changes, &rebuilt.balance_changes));
    } else if flags.nonce_changes {
        deep.nonce_changes =
            Some(field_diff_window(&received.nonce_changes, &rebuilt.nonce_changes));
    } else if flags.code_changes {
        let received_summaries: Vec<_> =
            received.code_changes.iter().map(CodeChangeSummary::from).collect();
        let rebuilt_summaries: Vec<_> =
            rebuilt.code_changes.iter().map(CodeChangeSummary::from).collect();
        deep.code_changes = Some(field_diff_window(&received_summaries, &rebuilt_summaries));
    }
    deep
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{
        AccountChanges, BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_primitives::{Address, Bytes, U256};

    fn diagnostic_addr(byte: u8) -> Address {
        let mut address = [0u8; 20];
        address[19] = byte;
        Address::from(address)
    }

    fn diagnostic_account(address: Address, balance: u64) -> AccountChanges {
        AccountChanges {
            address,
            balance_changes: vec![BalanceChange::new(1, U256::from(balance))],
            ..Default::default()
        }
    }

    fn first_diff(received: &[AccountChanges], rebuilt: &[AccountChanges]) -> AccountDivergence {
        first_bal_divergence(received, rebuilt).first_diff.expect("expected BAL divergence")
    }

    fn assert_summary_address(summary: &Option<AccountSummary>, address: Address) {
        match summary {
            Some(summary) => assert_eq!(summary.address, address),
            None => panic!("expected account summary"),
        }
    }

    #[test]
    fn none_for_equal_bals() {
        let account = diagnostic_account(diagnostic_addr(1), 1);
        let rebuilt = vec![account.clone()];

        let div = first_bal_divergence(core::slice::from_ref(&account), &rebuilt);

        assert_eq!(
            div,
            BalDivergence { received_accounts: 1, rebuilt_accounts: 1, first_diff: None }
        );
    }

    #[test]
    fn reports_account_only_in_received() {
        let received = vec![diagnostic_account(diagnostic_addr(1), 1)];

        let diff = first_diff(&received, &[]);

        assert_eq!(diff.index, 0);
        assert_eq!(diff.received, Some(AccountSummary::from_account(&received[0])));
        assert_eq!(diff.rebuilt, None);
    }

    #[test]
    fn reports_tail_account_only_in_received_after_matching_prefix() {
        let shared = diagnostic_account(diagnostic_addr(1), 1);
        let received = vec![shared.clone(), diagnostic_account(diagnostic_addr(2), 2)];
        let rebuilt = vec![shared];

        let diff = first_diff(&received, &rebuilt);

        assert_eq!(diff.index, 1);
        assert_summary_address(&diff.received, diagnostic_addr(2));
        assert_eq!(diff.rebuilt, None);
    }

    #[test]
    fn reports_account_only_in_rebuilt() {
        let rebuilt = vec![diagnostic_account(diagnostic_addr(1), 1)];

        let diff = first_diff(&[], &rebuilt);

        assert_eq!(diff.index, 0);
        assert_eq!(diff.received, None);
        assert_eq!(diff.rebuilt, Some(AccountSummary::from_account(&rebuilt[0])));
    }

    #[test]
    fn reports_tail_account_only_in_rebuilt_after_matching_prefix() {
        let shared = diagnostic_account(diagnostic_addr(1), 1);
        let received = vec![shared.clone()];
        let rebuilt = vec![shared, diagnostic_account(diagnostic_addr(2), 2)];

        let diff = first_diff(&received, &rebuilt);

        assert_eq!(diff.index, 1);
        assert_eq!(diff.received, None);
        assert_summary_address(&diff.rebuilt, diagnostic_addr(2));
    }

    #[test]
    fn reports_changed_balance_field() {
        let received = vec![diagnostic_account(diagnostic_addr(1), 1)];
        let rebuilt = vec![diagnostic_account(diagnostic_addr(1), 2)];

        let diff = first_diff(&received, &rebuilt);

        assert_eq!(diff.index, 0);
        assert_eq!(
            diff.fields_differ,
            AccountFieldFlags { balance_changes: true, ..Default::default() }
        );
        assert!(diff.received.is_some());
        assert!(diff.rebuilt.is_some());
    }

    #[test]
    fn reports_both_addresses_for_address_mismatch() {
        let received = vec![
            diagnostic_account(diagnostic_addr(1), 1),
            diagnostic_account(diagnostic_addr(3), 1),
        ];
        let rebuilt = vec![
            diagnostic_account(diagnostic_addr(2), 1),
            diagnostic_account(diagnostic_addr(3), 2),
        ];

        let diff = first_diff(&received, &rebuilt);

        assert_eq!(diff.index, 0);
        assert_summary_address(&diff.received, diagnostic_addr(1));
        assert_summary_address(&diff.rebuilt, diagnostic_addr(2));
        assert_eq!(diff.fields_differ, AccountFieldFlags::default());
    }

    fn balance_change(idx: u64, balance: u64) -> BalanceChange {
        BalanceChange::new(idx, U256::from(balance))
    }

    #[test]
    fn field_window_empty_when_equal() {
        let received = vec![balance_change(0, 1), balance_change(1, 2)];
        let rebuilt = received.clone();

        let window = field_diff_window(&received, &rebuilt);

        assert_eq!(window.matching_prefix_len, 2);
        assert!(window.received.is_empty());
        assert!(window.rebuilt.is_empty());
    }

    #[test]
    fn field_window_diverges_at_index_zero() {
        let received = vec![balance_change(0, 1), balance_change(1, 2)];
        let rebuilt = vec![balance_change(0, 9), balance_change(1, 2)];

        let window = field_diff_window(&received, &rebuilt);

        assert_eq!(window.matching_prefix_len, 0);
        assert_eq!(window.received, received);
        assert_eq!(window.rebuilt, rebuilt);
    }

    #[test]
    fn field_window_diverges_mid_vec() {
        let received = vec![balance_change(0, 1), balance_change(1, 2), balance_change(2, 3)];
        let rebuilt = vec![balance_change(0, 1), balance_change(1, 9), balance_change(2, 3)];

        let window = field_diff_window(&received, &rebuilt);

        assert_eq!(window.matching_prefix_len, 1);
        assert_eq!(window.received, vec![balance_change(1, 2), balance_change(2, 3)]);
        assert_eq!(window.rebuilt, vec![balance_change(1, 9), balance_change(2, 3)]);
    }

    #[test]
    fn field_window_off_by_one_duplicate() {
        let received = vec![balance_change(0, 1), balance_change(1, 2), balance_change(2, 3)];
        let rebuilt = vec![
            balance_change(0, 1),
            balance_change(1, 2),
            balance_change(1, 2),
            balance_change(2, 3),
        ];

        let window = field_diff_window(&received, &rebuilt);

        assert_eq!(window.matching_prefix_len, 2);
        assert_eq!(window.received, vec![balance_change(2, 3)]);
        assert_eq!(window.rebuilt, vec![balance_change(1, 2), balance_change(2, 3)]);
    }

    #[test]
    fn field_window_caps_at_field_window_size() {
        let received: Vec<_> =
            (0..(FIELD_WINDOW as u64 + 4)).map(|i| balance_change(i, 1000 + i)).collect();
        let rebuilt: Vec<_> =
            (0..(FIELD_WINDOW as u64 + 4)).map(|i| balance_change(i, 2000 + i)).collect();

        let window = field_diff_window(&received, &rebuilt);

        assert_eq!(window.matching_prefix_len, 0);
        assert_eq!(window.received.len(), FIELD_WINDOW);
        assert_eq!(window.rebuilt.len(), FIELD_WINDOW);
    }

    #[test]
    fn deep_diff_returns_none_when_no_divergence() {
        let account = diagnostic_account(diagnostic_addr(1), 1);
        let received = vec![account.clone()];
        let rebuilt = vec![account];

        let div = first_bal_divergence(&received, &rebuilt);

        assert!(div.deep_diff(&received, &rebuilt).is_none());
    }

    #[test]
    fn deep_diff_returns_none_for_address_mismatch() {
        let received = vec![diagnostic_account(diagnostic_addr(1), 1)];
        let rebuilt = vec![diagnostic_account(diagnostic_addr(2), 1)];

        let div = first_bal_divergence(&received, &rebuilt);

        assert!(div.deep_diff(&received, &rebuilt).is_none());
    }

    #[test]
    fn deep_diff_picks_first_set_flag_in_priority_order() {
        let address = diagnostic_addr(1);
        let received = vec![AccountChanges {
            address,
            storage_changes: vec![SlotChanges::new(
                U256::from(1),
                vec![StorageChange::new(0, U256::from(10))],
            )],
            balance_changes: vec![BalanceChange::new(0, U256::from(1))],
            nonce_changes: vec![NonceChange::new(0, 1)],
            ..Default::default()
        }];
        let rebuilt = vec![AccountChanges {
            address,
            storage_changes: vec![SlotChanges::new(
                U256::from(1),
                vec![StorageChange::new(0, U256::from(20))],
            )],
            balance_changes: vec![BalanceChange::new(0, U256::from(2))],
            nonce_changes: vec![NonceChange::new(0, 2)],
            ..Default::default()
        }];

        let div = first_bal_divergence(&received, &rebuilt);
        let deep = div.deep_diff(&received, &rebuilt).expect("expected deep diff");

        assert_eq!(deep.address, address);
        assert!(deep.storage_changes.is_some());
        assert!(deep.storage_reads.is_none());
        assert!(deep.balance_changes.is_none());
        assert!(deep.nonce_changes.is_none());
        assert!(deep.code_changes.is_none());
    }

    #[test]
    fn deep_diff_for_code_changes_uses_code_hash() {
        let address = diagnostic_addr(1);
        let code_a = Bytes::from(vec![0x60, 0x01]);
        let code_b = Bytes::from(vec![0x60, 0x02]);
        let received = vec![AccountChanges {
            address,
            code_changes: vec![CodeChange::new(0, code_a.clone())],
            ..Default::default()
        }];
        let rebuilt = vec![AccountChanges {
            address,
            code_changes: vec![CodeChange::new(0, code_b.clone())],
            ..Default::default()
        }];

        let div = first_bal_divergence(&received, &rebuilt);
        let deep = div.deep_diff(&received, &rebuilt).expect("expected deep diff");
        let window = deep.code_changes.expect("expected code_changes window");

        assert_eq!(window.matching_prefix_len, 0);
        assert_eq!(window.received.len(), 1);
        assert_eq!(window.rebuilt.len(), 1);
        assert_eq!(window.received[0].code_hash, keccak256(&code_a));
        assert_eq!(window.rebuilt[0].code_hash, keccak256(&code_b));
        assert_eq!(window.received[0].block_access_index, 0);
    }

    #[test]
    fn stops_at_first_mismatch_after_matching_prefix() {
        let shared = diagnostic_account(diagnostic_addr(1), 1);
        let received = vec![
            shared.clone(),
            diagnostic_account(diagnostic_addr(2), 2),
            diagnostic_account(diagnostic_addr(4), 4),
        ];
        let rebuilt = vec![
            shared,
            diagnostic_account(diagnostic_addr(3), 3),
            diagnostic_account(diagnostic_addr(4), 5),
        ];

        let diff = first_diff(&received, &rebuilt);

        assert_eq!(diff.index, 1);
        assert_summary_address(&diff.received, diagnostic_addr(2));
        assert_summary_address(&diff.rebuilt, diagnostic_addr(3));
        assert_eq!(diff.fields_differ, AccountFieldFlags::default());
    }
}
