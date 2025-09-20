use super::TxPayment;
use alloy_consensus::{
    crypto::RecoveryError, transaction::SignerRecoverable, Signed, TransactionEnvelope,
};
use alloy_eips::Encodable2718;
use alloy_primitives::{Sealed, Signature, B256};
use alloy_rlp::BufMut;
use op_alloy_consensus::{OpTxEnvelope, TxDeposit};
use reth_codecs::{
    alloy::transaction::{Envelope, FromTxCompact, ToTxCompact},
    Compact,
};
use reth_ethereum::primitives::{serde_bincode_compat::RlpBincode, InMemorySize};
use reth_op::{primitives::SignedTransaction, OpTransaction};
use revm_primitives::Address;

/// Either [`OpTxEnvelope`] or [`TxPayment`].
#[derive(Debug, Clone, TransactionEnvelope)]
#[envelope(tx_type_name = TxTypeCustom)]
pub enum CustomTransaction {
    /// A regular Optimism transaction as defined by [`OpTxEnvelope`].
    #[envelope(flatten)]
    Op(OpTxEnvelope),
    /// A [`TxPayment`] tagged with type 0x7E.
    #[envelope(ty = 42)]
    Payment(Signed<TxPayment>),
}

impl RlpBincode for CustomTransaction {}

impl reth_codecs::alloy::transaction::Envelope for CustomTransaction {
    fn signature(&self) -> &Signature {
        match self {
            CustomTransaction::Op(tx) => tx.signature(),
            CustomTransaction::Payment(tx) => tx.signature(),
        }
    }

    fn tx_type(&self) -> Self::TxType {
        match self {
            CustomTransaction::Op(tx) => TxTypeCustom::Op(tx.tx_type()),
            CustomTransaction::Payment(_) => TxTypeCustom::Payment,
        }
    }
}

impl FromTxCompact for CustomTransaction {
    type TxType = TxTypeCustom;

    fn from_tx_compact(buf: &[u8], tx_type: Self::TxType, signature: Signature) -> (Self, &[u8])
    where
        Self: Sized,
    {
        match tx_type {
            TxTypeCustom::Op(tx_type) => {
                let (tx, buf) = OpTxEnvelope::from_tx_compact(buf, tx_type, signature);
                (Self::Op(tx), buf)
            }
            TxTypeCustom::Payment => {
                let (tx, buf) = TxPayment::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Payment(tx), buf)
            }
        }
    }
}

impl ToTxCompact for CustomTransaction {
    fn to_tx_compact(&self, buf: &mut (impl BufMut + AsMut<[u8]>)) {
        match self {
            CustomTransaction::Op(tx) => tx.to_tx_compact(buf),
            CustomTransaction::Payment(tx) => {
                tx.tx().to_compact(buf);
            }
        }
    }
}

impl Compact for CustomTransaction {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        // Simple implementation: just serialize tx_type + signature + tx_data
        let start = buf.as_mut().len();

        // Serialize signature
        let _sig_len = self.signature().to_compact(buf);

        // Serialize tx type
        let _tx_type_len = self.tx_type().to_compact(buf);

        // Serialize transaction data
        self.to_tx_compact(buf);

        buf.as_mut().len() - start
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        // Simple implementation: deserialize signature + tx_type + tx_data
        let (signature, buf) = Signature::from_compact(buf, 0);
        let (tx_type, buf) = TxTypeCustom::from_compact(buf, 0);
        Self::from_tx_compact(buf, tx_type, signature)
    }
}

impl OpTransaction for CustomTransaction {
    fn is_deposit(&self) -> bool {
        match self {
            CustomTransaction::Op(op) => op.is_deposit(),
            CustomTransaction::Payment(_) => false,
        }
    }

    fn as_deposit(&self) -> Option<&Sealed<TxDeposit>> {
        match self {
            CustomTransaction::Op(op) => op.as_deposit(),
            CustomTransaction::Payment(_) => None,
        }
    }
}

impl SignerRecoverable for CustomTransaction {
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        match self {
            CustomTransaction::Op(tx) => SignerRecoverable::recover_signer(tx),
            CustomTransaction::Payment(tx) => SignerRecoverable::recover_signer(tx),
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        match self {
            CustomTransaction::Op(tx) => SignerRecoverable::recover_signer_unchecked(tx),
            CustomTransaction::Payment(tx) => SignerRecoverable::recover_signer_unchecked(tx),
        }
    }
}

impl SignedTransaction for CustomTransaction {
    fn tx_hash(&self) -> &B256 {
        match self {
            CustomTransaction::Op(tx) => SignedTransaction::tx_hash(tx),
            CustomTransaction::Payment(tx) => tx.hash(),
        }
    }
}

impl InMemorySize for CustomTransaction {
    fn size(&self) -> usize {
        match self {
            CustomTransaction::Op(tx) => InMemorySize::size(tx),
            CustomTransaction::Payment(tx) => InMemorySize::size(tx),
        }
    }
}
