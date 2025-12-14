//! Compact implementation for [`AlloyAccountChanges`] and related types.

use crate::Compact;
use alloc::vec::Vec;
use alloy_eips::eip7928::{
    balance_change::BalanceChange as AlloyBalanceChange,
    code_change::CodeChange as AlloyCodeChange, nonce_change::NonceChange as AlloyNonceChange,
    AccountChanges as AlloyAccountChanges, SlotChanges as AlloySlotChange,
};
use alloy_primitives::{Address, Bytes,U256};
use reth_codecs_derive::add_arbitrary_tests;

/// `AccountChanges` acts as bridge which simplifies Compact implementation for `AlloyAccountChanges`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Compact)]
#[cfg_attr(
    any(test, feature = "test-utils"),
    derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize)
)]
#[reth_codecs(crate = "crate")]
#[cfg_attr(feature = "test-utils", allow(unreachable_pub), visibility::make(pub))]
#[add_arbitrary_tests(crate, compact)]
pub(crate) struct AccountChanges {
    /// The address of the account whose changes are stored.
    pub address: Address,
    /// List of slot changes for this account.
    pub storage_changes: Vec<SlotChanges>,
    /// List of storage reads for this account.
    pub storage_reads: Vec<U256>,
    /// List of balance changes for this account.
    pub balance_changes: Vec<BalanceChange>,
    /// List of nonce changes for this account.
    pub nonce_changes: Vec<NonceChange>,
    /// List of code changes for this account.
    pub code_changes: Vec<CodeChange>,
}

/// `BalanceChange` acts as bridge which simplifies Compact implementation for `AlloyBalanceChange`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Compact)]
#[cfg_attr(
    any(test, feature = "test-utils"),
    derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize)
)]
#[reth_codecs(crate = "crate")]
#[cfg_attr(feature = "test-utils", allow(unreachable_pub), visibility::make(pub))]
#[add_arbitrary_tests(crate, compact)]
pub(crate) struct BalanceChange {
    /// The index of bal that stores balance change.
    pub block_access_index: u64,
    /// The post-transaction balance of the account.
    pub post_balance: U256,
}

/// `CodeChange` acts as bridge which simplifies Compact implementation for `AlloyCodeChange`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Compact)]
#[cfg_attr(
    any(test, feature = "test-utils"),
    derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize)
)]
#[reth_codecs(crate = "crate")]
#[cfg_attr(feature = "test-utils", allow(unreachable_pub), visibility::make(pub))]
#[add_arbitrary_tests(crate, compact)]
pub(crate) struct CodeChange {
    /// The index of bal that stores this code change.
    pub block_access_index: u64,
    /// The new code of the account.
    pub new_code: Bytes,
}

/// `NonceChange` acts as bridge which simplifies Compact implementation for `AlloyNonceChange`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Compact)]
#[cfg_attr(
    any(test, feature = "test-utils"),
    derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize)
)]
#[reth_codecs(crate = "crate")]
#[cfg_attr(feature = "test-utils", allow(unreachable_pub), visibility::make(pub))]
#[add_arbitrary_tests(crate, compact)]
pub(crate) struct NonceChange {
    /// The index of bal that stores this nonce change.
    pub block_access_index: u64,
    /// The new code of the account.
    pub new_nonce: u64,
}

/// `SlotChanges` acts as bridge which simplifies Compact implementation for `AlloySlotChange`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Compact)]
#[cfg_attr(
    any(test, feature = "test-utils"),
    derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize)
)]
#[reth_codecs(crate = "crate")]
#[cfg_attr(feature = "test-utils", allow(unreachable_pub), visibility::make(pub))]
#[add_arbitrary_tests(crate, compact)]
pub(crate) struct SlotChanges {
    /// The storage slot key being modified.
    pub slot: U256,
    /// A list of write operations to this slot, ordered by transaction index.
    pub changes: Vec<StorageChange>,
}

/// `StorageChange` acts as bridge which simplifies Compact implementation for `AlloyStorageChange`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Compact)]
#[cfg_attr(
    any(test, feature = "test-utils"),
    derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize)
)]
#[reth_codecs(crate = "crate")]
#[cfg_attr(feature = "test-utils", allow(unreachable_pub), visibility::make(pub))]
#[add_arbitrary_tests(crate, compact)]
pub(crate) struct StorageChange {
    /// Index of the bal that stores the performed write.
    pub block_access_index: u64,
    /// The new value written to the storage slot.
    pub new_value: U256,
}

impl Compact for AlloyAccountChanges {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let acc_change = AccountChanges {
            address: self.address,
            storage_changes: self
                .storage_changes
                .iter()
                .map(|sc| SlotChanges {
                    slot: sc.slot,
                    changes: sc
                        .changes
                        .iter()
                        .map(|c| StorageChange {
                            block_access_index: c.block_access_index,
                            new_value: c.new_value,
                        })
                        .collect(),
                })
                .collect(),
            storage_reads: self.storage_reads.clone(),
            balance_changes: self
                .balance_changes
                .iter()
                .map(|bc| BalanceChange {
                    block_access_index: bc.block_access_index,
                    post_balance: bc.post_balance,
                })
                .collect(),
            nonce_changes: self
                .nonce_changes
                .iter()
                .map(|nc| NonceChange {
                    block_access_index: nc.block_access_index,
                    new_nonce: nc.new_nonce,
                })
                .collect(),
            code_changes: self
                .code_changes
                .iter()
                .map(|cc| CodeChange {
                    block_access_index: cc.block_access_index,
                    new_code: cc.new_code.clone(),
                })
                .collect(),
        };

        acc_change.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (account_changes, rest) = AccountChanges::from_compact(buf, len);

        let alloy_changes = Self {
            address: account_changes.address,
            storage_changes: account_changes
                .storage_changes
                .into_iter()
                .map(|sc| AlloySlotChange {
                    slot: sc.slot,
                    changes: sc
                        .changes
                        .into_iter()
                        .map(|c| alloy_eips::eip7928::storage_change::StorageChange {
                            block_access_index: c.block_access_index,
                            new_value: c.new_value,
                        })
                        .collect(),
                })
                .collect(),
            storage_reads: account_changes.storage_reads,
            balance_changes: account_changes
                .balance_changes
                .into_iter()
                .map(|bc| AlloyBalanceChange {
                    block_access_index: bc.block_access_index,
                    post_balance: bc.post_balance,
                })
                .collect(),
            nonce_changes: account_changes
                .nonce_changes
                .into_iter()
                .map(|nc| AlloyNonceChange {
                    block_access_index: nc.block_access_index,
                    new_nonce: nc.new_nonce,
                })
                .collect(),
            code_changes: account_changes
                .code_changes
                .into_iter()
                .map(|cc| AlloyCodeChange {
                    block_access_index: cc.block_access_index,
                    new_code: cc.new_code,
                })
                .collect(),
        };

        (alloy_changes, rest)
    }
}

// impl Compact for AccountChanges {
//     fn to_compact<B>(&self, buf: &mut B) -> usize
//     where
//         B: bytes::BufMut + AsMut<[u8]>,
//     {
//         Self::to_compact(self, buf)
//     }

//     fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
//         Self::from_compact(buf, len)
//     }
// }
