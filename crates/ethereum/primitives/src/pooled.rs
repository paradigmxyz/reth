//! Ethereum pooled transaction representation used by Reth across supported forks.

use alloy_consensus::{
    error::ValueError,
    transaction::{
        eip8141::CachedFrameTransaction, SignerRecoverable, TxEip4844Sidecar, TxEip8141Variant,
        TxEip8141WithSidecar, TxHashRef,
    },
    EthereumTxEnvelope, InMemorySize, Signed, TransactionEnvelope, TxEip1559, TxEip2930,
    TxEip4844WithSidecar, TxEip7702, TxLegacy,
};
use alloy_eips::eip7594::{BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant};
use alloy_primitives::{Sealable, Sealed};

use crate::TransactionSigned;

/// Ethereum pooled transaction representation used by Reth across supported forks.
///
/// EIP-4844 uses [`BlobTransactionSidecarVariant`] so nodes can operate on either side of Osaka.
/// EIP-8141 activates after EIP-7594 and therefore always carries the EIP-7594 sidecar when the
/// transaction references blobs.
#[derive(Clone, Debug, TransactionEnvelope)]
#[envelope(
    alloy_consensus = alloy_consensus,
    tx_type_name = PooledTxType,
    typed = PooledTypedTransaction,
    arbitrary_cfg(feature = "arbitrary")
)]
pub enum PooledTransactionVariant {
    /// An untagged legacy transaction.
    #[envelope(ty = 0)]
    Legacy(Signed<TxLegacy>),
    /// An EIP-2930 transaction.
    #[envelope(ty = 1)]
    Eip2930(Signed<TxEip2930>),
    /// An EIP-1559 transaction.
    #[envelope(ty = 2)]
    Eip1559(Signed<TxEip1559>),
    /// An EIP-4844 transaction with a fork-appropriate blob sidecar.
    #[envelope(ty = 3)]
    Eip4844(Signed<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>),
    /// An EIP-7702 transaction.
    #[envelope(ty = 4)]
    Eip7702(Signed<TxEip7702>),
    /// An EIP-8141 transaction, optionally with its EIP-7594 sidecar.
    #[envelope(ty = 6)]
    Eip8141(Sealed<CachedFrameTransaction<BlobTransactionSidecarEip7594>>),
}

impl PooledTransactionVariant {
    /// Returns the transaction hash.
    pub fn tx_hash(&self) -> &alloy_primitives::TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash(),
            Self::Eip2930(tx) => tx.tx_hash(),
            Self::Eip1559(tx) => tx.tx_hash(),
            Self::Eip4844(tx) => tx.tx_hash(),
            Self::Eip7702(tx) => tx.tx_hash(),
            Self::Eip8141(tx) => tx.hash_ref(),
        }
    }

    /// Returns the transaction hash.
    pub fn hash(&self) -> &alloy_primitives::TxHash {
        self.tx_hash()
    }

    /// Returns the EIP-4844 transaction, if this is one.
    pub const fn as_eip4844(
        &self,
    ) -> Option<&Signed<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>> {
        match self {
            Self::Eip4844(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the EIP-8141 transaction, if this is one.
    pub const fn as_eip8141(
        &self,
    ) -> Option<&Sealed<CachedFrameTransaction<BlobTransactionSidecarEip7594>>> {
        match self {
            Self::Eip8141(tx) => Some(tx),
            _ => None,
        }
    }
}

impl From<TxEip8141WithSidecar<BlobTransactionSidecarEip7594>> for PooledTransactionVariant {
    fn from(value: TxEip8141WithSidecar<BlobTransactionSidecarEip7594>) -> Self {
        Self::Eip8141(CachedFrameTransaction::from(value).seal_slow())
    }
}

impl From<EthereumTxEnvelope<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>>
    for PooledTransactionVariant
{
    fn from(
        value: EthereumTxEnvelope<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>,
    ) -> Self {
        match value {
            EthereumTxEnvelope::Legacy(tx) => Self::Legacy(tx),
            EthereumTxEnvelope::Eip2930(tx) => Self::Eip2930(tx),
            EthereumTxEnvelope::Eip1559(tx) => Self::Eip1559(tx),
            EthereumTxEnvelope::Eip4844(tx) => Self::Eip4844(tx),
            EthereumTxEnvelope::Eip7702(tx) => Self::Eip7702(tx),
            EthereumTxEnvelope::Eip8141(tx) => {
                let (tx, hash) = tx.into_parts();
                Self::Eip8141(Sealed::new_unchecked(tx.into(), hash))
            }
        }
    }
}

impl TryFrom<TransactionSigned> for PooledTransactionVariant {
    type Error = ValueError<TransactionSigned>;

    fn try_from(value: TransactionSigned) -> Result<Self, Self::Error> {
        match value {
            TransactionSigned::Legacy(tx) => Ok(Self::Legacy(tx)),
            TransactionSigned::Eip2930(tx) => Ok(Self::Eip2930(tx)),
            TransactionSigned::Eip1559(tx) => Ok(Self::Eip1559(tx)),
            TransactionSigned::Eip4844(tx) => Err(ValueError::new_static(
                TransactionSigned::Eip4844(tx),
                "pooled transaction requires a blob sidecar",
            )),
            TransactionSigned::Eip7702(tx) => Ok(Self::Eip7702(tx)),
            TransactionSigned::Eip8141(tx) if !tx.blob_versioned_hashes.is_empty() => {
                Err(ValueError::new_static(
                    TransactionSigned::Eip8141(tx),
                    "pooled frame transaction requires a blob sidecar",
                ))
            }
            TransactionSigned::Eip8141(tx) => {
                let (tx, hash) = tx.into_parts();
                Ok(Self::Eip8141(Sealed::new_unchecked(tx.into(), hash)))
            }
        }
    }
}

impl From<PooledTransactionVariant> for TransactionSigned {
    fn from(value: PooledTransactionVariant) -> Self {
        match value {
            PooledTransactionVariant::Legacy(tx) => Self::Legacy(tx),
            PooledTransactionVariant::Eip2930(tx) => Self::Eip2930(tx),
            PooledTransactionVariant::Eip1559(tx) => Self::Eip1559(tx),
            PooledTransactionVariant::Eip4844(tx) => {
                let (tx, signature, hash) = tx.into_parts();
                let (tx, _) = tx.into_parts();
                Self::Eip4844(Signed::new_unchecked(tx, signature, hash))
            }
            PooledTransactionVariant::Eip7702(tx) => Self::Eip7702(tx),
            PooledTransactionVariant::Eip8141(tx) => {
                let (tx, hash) = tx.into_parts();
                let tx = match tx.into_inner() {
                    TxEip8141Variant::TxEip8141(tx) => tx,
                    TxEip8141Variant::TxEip8141WithSidecar(tx) => tx.into_parts().0,
                };
                Self::Eip8141(Sealed::new_unchecked(tx, hash))
            }
        }
    }
}

impl TxHashRef for PooledTransactionVariant {
    fn tx_hash(&self) -> &alloy_primitives::TxHash {
        self.tx_hash()
    }
}

impl SignerRecoverable for PooledTransactionVariant {
    fn recover_signer(
        &self,
    ) -> Result<alloy_primitives::Address, alloy_consensus::crypto::RecoveryError> {
        match self {
            Self::Legacy(tx) => SignerRecoverable::recover_signer(tx),
            Self::Eip2930(tx) => SignerRecoverable::recover_signer(tx),
            Self::Eip1559(tx) => SignerRecoverable::recover_signer(tx),
            Self::Eip4844(tx) => SignerRecoverable::recover_signer(tx),
            Self::Eip7702(tx) => SignerRecoverable::recover_signer(tx),
            Self::Eip8141(tx) => Ok(tx.tx().sender),
        }
    }

    fn recover_signer_unchecked(
        &self,
    ) -> Result<alloy_primitives::Address, alloy_consensus::crypto::RecoveryError> {
        match self {
            Self::Legacy(tx) => SignerRecoverable::recover_signer_unchecked(tx),
            Self::Eip2930(tx) => SignerRecoverable::recover_signer_unchecked(tx),
            Self::Eip1559(tx) => SignerRecoverable::recover_signer_unchecked(tx),
            Self::Eip4844(tx) => SignerRecoverable::recover_signer_unchecked(tx),
            Self::Eip7702(tx) => SignerRecoverable::recover_signer_unchecked(tx),
            Self::Eip8141(tx) => Ok(tx.tx().sender),
        }
    }
}

impl InMemorySize for PooledTransactionVariant {
    fn size(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.size(),
            Self::Eip2930(tx) => tx.size(),
            Self::Eip1559(tx) => tx.size(),
            Self::Eip4844(tx) => tx.size(),
            Self::Eip7702(tx) => tx.size(),
            Self::Eip8141(tx) => {
                tx.tx().size() +
                    tx.sidecar().map_or(0, TxEip4844Sidecar::size) +
                    core::mem::size_of::<alloy_primitives::B256>()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::TxEip8141WithSidecar, TxEip8141};
    use alloy_eips::{
        eip2718::{Decodable2718, Encodable2718},
        eip4844::{Blob, Bytes48},
        eip7594::{BlobTransactionSidecarEip7594, CELLS_PER_EXT_BLOB},
    };
    use alloy_primitives::{Address, Sealable, B256};

    #[test]
    fn eip8141_consensus_pooled_roundtrip() {
        let tx = TxEip8141 { sender: Address::repeat_byte(0x41), ..Default::default() };
        let consensus = TransactionSigned::Eip8141(tx.seal_slow());
        let pooled = PooledTransactionVariant::try_from(consensus.clone()).unwrap();

        assert!(pooled.as_eip8141().unwrap().sidecar().is_none());
        assert_eq!(TransactionSigned::from(pooled), consensus);
    }

    #[test]
    fn eip8141_sidecar_network_roundtrip() {
        let mut versioned_hash = B256::ZERO;
        versioned_hash[0] = 0x01;
        let tx = TxEip8141 {
            sender: Address::repeat_byte(0x41),
            blob_versioned_hashes: vec![versioned_hash],
            ..Default::default()
        };
        let sidecar = BlobTransactionSidecarEip7594::new(
            vec![Blob::default()],
            vec![Bytes48::default()],
            vec![Bytes48::default(); CELLS_PER_EXT_BLOB],
        );
        let pooled: PooledTransactionVariant = TxEip8141WithSidecar::new(tx, sidecar).into();

        let encoded = pooled.encoded_2718();
        let decoded = PooledTransactionVariant::decode_2718(&mut encoded.as_ref()).unwrap();

        assert_eq!(decoded, pooled);
        assert!(decoded.as_eip8141().unwrap().sidecar().is_some());
    }
}
