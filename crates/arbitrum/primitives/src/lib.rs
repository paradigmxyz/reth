#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use alloc::vec::Vec;
use alloy_consensus::Receipt as AlloyReceipt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArbReceipt {
    Legacy(AlloyReceipt),
    Eip1559(AlloyReceipt),
    Eip2930(AlloyReceipt),
    Eip7702(AlloyReceipt),
    Deposit(ArbDepositReceipt),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ArbDepositReceipt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArbTransactionSigned {
    pub ty: ArbTxType,
}

impl ArbTransactionSigned {
    pub const fn tx_type(&self) -> ArbTxType {
        self.ty
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArbTxType {
    Deposit,
    Unsigned,
    Contract,
    Retry,
    SubmitRetryable,
    Internal,
    Legacy,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Eip658Value, Receipt};
    use alloy_primitives::Log;

    #[test]
    fn arb_receipt_variants_hold_alloy_receipt() {
        let r = Receipt { status: Eip658Value::Eip658(true), cumulative_gas_used: 1, logs: Vec::<Log>::new() };
        let e = ArbReceipt::Legacy(r.clone());
        match e {
            ArbReceipt::Legacy(rr) => {
                assert!(matches!(rr.status, Eip658Value::Eip658(true)));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn arb_deposit_receipt_variant_exists() {
        let d = ArbDepositReceipt::default();
        let e = ArbReceipt::Deposit(d);
        match e {
            ArbReceipt::Deposit(_) => {}
            _ => panic!("expected deposit variant"),
        }
    }
}
#[cfg(test)]
mod tx_type_from_envelope_tests {
    use super::*;
    use arb_alloy_consensus::ArbTxEnvelope;
    use arb_alloy_consensus::tx::{ArbDepositTx, ArbUnsignedTx, ArbContractTx, ArbRetryTx, ArbSubmitRetryableTx, ArbInternalTx};
    use alloy_primitives::{address, b256, U256};

    #[test]
    fn from_envelope_maps_all_variants() {
        let dep = ArbTxEnvelope::Deposit(ArbDepositTx {
            chain_id: U256::from(42161u64),
            l1_request_id: b256!("1111111111111111111111111111111111111111111111111111111111111111"),
            from: address!("0000000000000000000000000000000000000001"),
            to: address!("0000000000000000000000000000000000000002"),
            value: U256::from(1u64),
        });
        let uns = ArbTxEnvelope::Unsigned(ArbUnsignedTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000003"),
            nonce: 1,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: None,
            value: U256::ZERO,
            data: Vec::new(),
        });
        let con = ArbTxEnvelope::Contract(ArbContractTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000004"),
            nonce: 2,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: address!("0000000000000000000000000000000000000005"),
            value: U256::ZERO,
            data: Vec::new(),
        });
        let rty = ArbTxEnvelope::Retry(ArbRetryTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000006"),
            nonce: 3,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: address!("0000000000000000000000000000000000000007"),
            value: U256::ZERO,
            data: Vec::new(),
        });
        let sub = ArbTxEnvelope::SubmitRetryable(ArbSubmitRetryableTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000008"),
            nonce: 4,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: address!("0000000000000000000000000000000000000009"),
            value: U256::ZERO,
            data: Vec::new(),
        });
        let intx = ArbTxEnvelope::Internal(ArbInternalTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000010"),
            nonce: 5,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: address!("0000000000000000000000000000000000000011"),
            value: U256::ZERO,
            data: Vec::new(),
        });
        let leg = ArbTxEnvelope::Legacy(alloy_consensus::TxLegacy { chain_id: None, nonce: 0, gas_price: U256::ZERO, gas_limit: 21000, to: None, value: U256::ZERO, input: Vec::new(), ..Default::default() });

        let cases = vec![
            (dep, ArbTxType::Deposit),
            (uns, ArbTxType::Unsigned),
            (con, ArbTxType::Contract),
            (rty, ArbTxType::Retry),
            (sub, ArbTxType::SubmitRetryable),
            (intx, ArbTxType::Internal),
            (leg, ArbTxType::Legacy),
        ];
        for (env, expect) in cases {
            let got = ArbTxType::from(&env);
            assert_eq!(got, expect);
        }
    }
}
