//! BAL divergence diagnostics.

use alloy_eip7928::AccountChanges;
use alloy_primitives::Address;

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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{AccountChanges, BalanceChange};
    use alloy_primitives::{Address, U256};

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
