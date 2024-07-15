//! [EIP-4895](https://eips.ethereum.org/EIPS/eip-4895) Withdrawal types.

use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use reth_codecs::{main_codec, Compact};
use std::ops::{Deref, DerefMut};

/// Re-export from `alloy_eips`.
#[doc(inline)]
pub use alloy_eips::eip4895::Withdrawal;

/// Represents a collection of Withdrawals.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct Withdrawals(Vec<Withdrawal>);

impl Withdrawals {
    /// Create a new Withdrawals instance.
    pub fn new(withdrawals: Vec<Withdrawal>) -> Self {
        Self(withdrawals)
    }

    /// Calculate the total size, including capacity, of the Withdrawals.
    #[inline]
    pub fn total_size(&self) -> usize {
        self.capacity() * std::mem::size_of::<Withdrawal>()
    }

    /// Calculate a heuristic for the in-memory size of the [Withdrawals].
    #[inline]
    pub fn size(&self) -> usize {
        self.len() * std::mem::size_of::<Withdrawal>()
    }

    /// Get an iterator over the Withdrawals.
    pub fn iter(&self) -> std::slice::Iter<'_, Withdrawal> {
        self.0.iter()
    }

    /// Get a mutable iterator over the Withdrawals.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, Withdrawal> {
        self.0.iter_mut()
    }

    /// Convert [Self] into raw vec of withdrawals.
    pub fn into_inner(self) -> Vec<Withdrawal> {
        self.0
    }
}

impl IntoIterator for Withdrawals {
    type Item = Withdrawal;
    type IntoIter = std::vec::IntoIter<Withdrawal>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl AsRef<[Withdrawal]> for Withdrawals {
    fn as_ref(&self) -> &[Withdrawal] {
        &self.0
    }
}

impl Deref for Withdrawals {
    type Target = Vec<Withdrawal>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Withdrawals {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Vec<Withdrawal>> for Withdrawals {
    fn from(withdrawals: Vec<Withdrawal>) -> Self {
        Self(withdrawals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Address;
    use alloy_rlp::{RlpDecodable, RlpEncodable};
    use proptest::proptest;

    /// This type is kept for compatibility tests after the codec support was added to alloy-eips
    /// Withdrawal type natively
    #[main_codec]
    #[derive(Debug, Clone, PartialEq, Eq, Default, Hash, RlpEncodable, RlpDecodable)]
    struct RethWithdrawal {
        /// Monotonically increasing identifier issued by consensus layer.
        index: u64,
        /// Index of validator associated with withdrawal.
        validator_index: u64,
        /// Target address for withdrawn ether.
        address: Address,
        /// Value of the withdrawal in gwei.
        amount: u64,
    }

    impl PartialEq<Withdrawal> for RethWithdrawal {
        fn eq(&self, other: &Withdrawal) -> bool {
            self.index == other.index &&
                self.validator_index == other.validator_index &&
                self.address == other.address &&
                self.amount == other.amount
        }
    }

    // <https://github.com/paradigmxyz/reth/issues/1614>
    #[test]
    fn test_withdrawal_serde_roundtrip() {
        let input = r#"[{"index":"0x0","validatorIndex":"0x0","address":"0x0000000000000000000000000000000000001000","amount":"0x1"},{"index":"0x1","validatorIndex":"0x1","address":"0x0000000000000000000000000000000000001001","amount":"0x1"},{"index":"0x2","validatorIndex":"0x2","address":"0x0000000000000000000000000000000000001002","amount":"0x1"},{"index":"0x3","validatorIndex":"0x3","address":"0x0000000000000000000000000000000000001003","amount":"0x1"},{"index":"0x4","validatorIndex":"0x4","address":"0x0000000000000000000000000000000000001004","amount":"0x1"},{"index":"0x5","validatorIndex":"0x5","address":"0x0000000000000000000000000000000000001005","amount":"0x1"},{"index":"0x6","validatorIndex":"0x6","address":"0x0000000000000000000000000000000000001006","amount":"0x1"},{"index":"0x7","validatorIndex":"0x7","address":"0x0000000000000000000000000000000000001007","amount":"0x1"},{"index":"0x8","validatorIndex":"0x8","address":"0x0000000000000000000000000000000000001008","amount":"0x1"},{"index":"0x9","validatorIndex":"0x9","address":"0x0000000000000000000000000000000000001009","amount":"0x1"},{"index":"0xa","validatorIndex":"0xa","address":"0x000000000000000000000000000000000000100a","amount":"0x1"},{"index":"0xb","validatorIndex":"0xb","address":"0x000000000000000000000000000000000000100b","amount":"0x1"},{"index":"0xc","validatorIndex":"0xc","address":"0x000000000000000000000000000000000000100c","amount":"0x1"},{"index":"0xd","validatorIndex":"0xd","address":"0x000000000000000000000000000000000000100d","amount":"0x1"},{"index":"0xe","validatorIndex":"0xe","address":"0x000000000000000000000000000000000000100e","amount":"0x1"},{"index":"0xf","validatorIndex":"0xf","address":"0x000000000000000000000000000000000000100f","amount":"0x1"}]"#;

        let withdrawals: Vec<Withdrawal> = serde_json::from_str(input).unwrap();
        let s = serde_json::to_string(&withdrawals).unwrap();
        assert_eq!(input, s);
    }

    proptest!(
        #[test]
        fn test_roundtrip_withdrawal_compat(withdrawal: RethWithdrawal) {
            // Convert to buffer and then create alloy_access_list from buffer and
            // compare
            let mut compacted_reth_withdrawal = Vec::<u8>::new();
            let len = withdrawal.clone().to_compact(&mut compacted_reth_withdrawal);

            // decode the compacted buffer to AccessList
            let alloy_withdrawal = Withdrawal::from_compact(&compacted_reth_withdrawal, len).0;
            assert_eq!(withdrawal, alloy_withdrawal);

            let mut compacted_alloy_withdrawal = Vec::<u8>::new();
            let alloy_len = alloy_withdrawal.to_compact(&mut compacted_alloy_withdrawal);
            assert_eq!(len, alloy_len);
            assert_eq!(compacted_reth_withdrawal, compacted_alloy_withdrawal);
        }
    );
}
