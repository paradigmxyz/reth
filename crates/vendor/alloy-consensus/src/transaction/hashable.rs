use alloy_eips::Typed2718;
use alloy_primitives::{Signature, TxHash};

use crate::transaction::RlpEcdsaEncodableTx;

/// Generic trait to get a transaction hash from any signature type
pub trait TxHashable<S>: Typed2718 {
    /// Calculate the transaction hash for the given signature and type.
    fn tx_hash_with_type(&self, signature: &S, ty: u8) -> TxHash;

    /// Calculate the transaction hash for the given signature.
    fn tx_hash(&self, signature: &S) -> TxHash {
        self.tx_hash_with_type(signature, self.ty())
    }
}

impl<T> TxHashable<Signature> for T
where
    T: RlpEcdsaEncodableTx,
{
    /// Calculate the transaction hash for the given signature and type.
    fn tx_hash_with_type(&self, signature: &Signature, ty: u8) -> TxHash {
        RlpEcdsaEncodableTx::tx_hash_with_type(self, signature, ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Signed, TxLegacy};
    use alloy_primitives::U256;

    #[test]
    fn hashable_signed() {
        let tx = TxLegacy::default();
        let signature = Signature::new(U256::from(1u64), U256::from(1u64), false);
        let signed = Signed::new_unhashed(tx, signature);
        let _hash = signed.hash();
    }
}
