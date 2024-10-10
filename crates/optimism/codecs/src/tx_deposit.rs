#![cfg(feature = "optimism")]

//! Encoding for deposit transaction for op-reth database

use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use op_alloy_consensus::TxDeposit as AlloyTxDeposit;
use reth_codecs::Compact;
use reth_codecs_derive::add_arbitrary_tests;

/// Deposit transactions, also known as deposits are initiated on L1, and executed on L2.
///
/// This is a helper type for encoding. It allows for using derive on the type instead of manually
/// managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Asides references, should match struct [`op_alloy_consensus::TxDeposit`]
#[derive(Debug, Compact)]
pub struct TxDepositEncode<'a> {
    source_hash: &'a B256,
    from: &'a Address,
    to: &'a TxKind,
    mint: &'a Option<u128>,
    value: &'a U256,
    gas_limit: u64,
    is_system_transaction: bool,
    input: &'a Bytes,
}

/// Deposit transactions, also known as deposits are initiated on L1, and executed on L2.
///
/// This is a helper type for decoding. It allows for using derive on the type instead of manually
/// managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Make sure this struct is 1:1 with [`op_alloy_consensus::TxDeposit`]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Compact)]
#[cfg_attr(test, derive(arbitrary::Arbitrary, Default, serde::Serialize, serde::Deserialize))]
#[add_arbitrary_tests(compact)]
pub struct TxDepositDecode {
    source_hash: B256,
    from: Address,
    to: TxKind,
    mint: Option<u128>,
    value: U256,
    gas_limit: u64,
    is_system_transaction: bool,
    input: Bytes,
}

impl<'a: 'b, 'b> From<&'a AlloyTxDeposit> for TxDepositEncode<'b> {
    fn from(tx: &'a AlloyTxDeposit) -> Self {
        let AlloyTxDeposit {
            source_hash,
            from,
            to,
            mint,
            value,
            gas_limit,
            is_system_transaction,
            input,
        } = tx;

        Self {
            source_hash,
            from,
            to,
            mint,
            value,
            gas_limit: *gas_limit,
            is_system_transaction: *is_system_transaction,
            input,
        }
    }
}

impl From<TxDepositDecode> for AlloyTxDeposit {
    fn from(tx: TxDepositDecode) -> Self {
        let TxDepositDecode {
            source_hash,
            from,
            to,
            mint,
            value,
            gas_limit,
            is_system_transaction,
            input,
        } = tx;

        Self { source_hash, from, to, mint, value, gas_limit, is_system_transaction, input }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_ensure_backwards_compatibility_optimism() {
        assert_eq!(TxDepositEncode::<'_>::bitflag_encoded_bytes(), 2);
    }
}
