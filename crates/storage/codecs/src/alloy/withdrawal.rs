//! Compact implementation for [`AlloyWithdrawal`]

use crate::Compact;
use alloc::vec::Vec;
use alloy_eips::eip4895::{Withdrawal as AlloyWithdrawal, Withdrawals};
use alloy_primitives::Address;
use reth_codecs_derive::add_arbitrary_tests;

/// Withdrawal acts as bridge which simplifies Compact implementation for AlloyWithdrawal.
///
/// Notice: Make sure this struct is 1:1 with `alloy_eips::eip4895::Withdrawal`
#[derive(Debug, Clone, PartialEq, Eq, Default, Compact)]
#[cfg_attr(
    any(test, feature = "test-utils"),
    derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize)
)]
#[reth_codecs(crate = "crate")]
#[cfg_attr(feature = "test-utils", allow(unreachable_pub), visibility::make(pub))]
#[add_arbitrary_tests(crate, compact)]
pub(crate) struct Withdrawal {
    /// Monotonically increasing identifier issued by consensus layer.
    index: u64,
    /// Index of validator associated with withdrawal.
    validator_index: u64,
    /// Target address for withdrawn ether.
    address: Address,
    /// Value of the withdrawal in gwei.
    amount: u64,
}

impl Compact for AlloyWithdrawal {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let withdrawal = Withdrawal {
            index: self.index,
            validator_index: self.validator_index,
            address: self.address,
            amount: self.amount,
        };
        withdrawal.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (withdrawal, _) = Withdrawal::from_compact(buf, len);
        let alloy_withdrawal = Self {
            index: withdrawal.index,
            validator_index: withdrawal.validator_index,
            address: withdrawal.address,
            amount: withdrawal.amount,
        };
        (alloy_withdrawal, buf)
    }
}

impl Compact for Withdrawals {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        self.as_ref().to_compact(buf)
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (withdrawals, new_buf) = Vec::from_compact(buf, buf.len());
        buf = new_buf;
        let alloy_withdrawals = Self::new(withdrawals);
        (alloy_withdrawals, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::proptest;
    use proptest_arbitrary_interop::arb;

    proptest! {
        #[test]
        fn roundtrip_withdrawal(withdrawal in arb::<AlloyWithdrawal>()) {
            let mut compacted_withdrawal = Vec::<u8>::new();
            let len = withdrawal.to_compact(&mut compacted_withdrawal);
            let (decoded, _) = AlloyWithdrawal::from_compact(&compacted_withdrawal, len);
            assert_eq!(withdrawal, decoded)
        }

        #[test]
        fn roundtrip_withdrawals(withdrawals in arb::<Withdrawals>()) {
            let mut compacted_withdrawals = Vec::<u8>::new();
            let len = withdrawals.to_compact(&mut compacted_withdrawals);
            let (decoded, _) = Withdrawals::from_compact(&compacted_withdrawals, len);
            assert_eq!(withdrawals, decoded);
        }
    }

    // each value in the database has an extra field named flags that encodes metadata about other
    // fields in the value, e.g. offset and length.
    //
    // this check is to ensure we do not inadvertently add too many fields to a struct which would
    // expand the flags field and break backwards compatibility
    #[test]
    fn test_ensure_backwards_compatibility() {
        assert_eq!(Withdrawal::bitflag_encoded_bytes(), 2);
    }
}
