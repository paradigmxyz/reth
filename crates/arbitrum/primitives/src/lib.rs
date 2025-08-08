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
