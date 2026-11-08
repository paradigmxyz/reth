//! Standalone crate for ethereum-specific Reth primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// Feature-only dep: activated by `reth-codec` feature for downstream consumers.
#[cfg(feature = "reth-codec")]
use reth_codecs as _;

mod receipt;
pub use receipt::*;

use alloy_consensus::TxEip4844;
pub use alloy_consensus::{transaction::PooledTransaction, TxType};

/// Typed Transaction type without a signature
pub type Transaction = alloy_consensus::EthereumTypedTransaction<TxEip4844>;

/// Signed transaction.
pub type TransactionSigned = alloy_consensus::EthereumTxEnvelope<TxEip4844>;

/// Pooled Ethereum transactions with sidecars on either side of Osaka.
pub type PooledTransactionVariant =
    alloy_consensus::transaction::PooledTransactionWithSidecarVariant;

/// Type alias for the ethereum block
pub type Block = alloy_consensus::Block<TransactionSigned>;

/// Type alias for the ethereum blockbody
pub type BlockBody = alloy_consensus::BlockBody<TransactionSigned>;

/// Helper struct that specifies the ethereum
/// [`NodePrimitives`](reth_primitives_traits::NodePrimitives) types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct EthPrimitives;

impl reth_primitives_traits::NodePrimitives for EthPrimitives {
    type Block = crate::Block;
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = crate::BlockBody;
    type SignedTx = crate::TransactionSigned;
    type Receipt = crate::Receipt;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{
        transaction::{TxEip8141WithSidecar, TxHashRef},
        TxEip8141,
    };
    use alloy_eips::{
        eip2718::{Decodable2718, Encodable2718},
        eip4844::{Blob, Bytes48},
        eip7594::{BlobTransactionSidecarEip7594, CELLS_PER_EXT_BLOB},
    };
    use alloy_primitives::{Address, Sealable, B256, U256};

    #[test]
    fn eip8141_consensus_pooled_roundtrip() {
        let tx = TxEip8141 {
            sender: Address::repeat_byte(0x41),
            nonce: 7,
            nonce_keys: Some(vec![U256::from(1), U256::from(3)]),
            ..Default::default()
        };
        let consensus = TransactionSigned::Eip8141(tx.clone().seal_slow());
        let pooled = PooledTransactionVariant::try_from(consensus.clone()).unwrap();

        assert!(pooled.as_eip8141().unwrap().sidecar().is_none());
        assert_eq!(pooled.as_eip8141().unwrap().tx(), &tx);
        assert_eq!(pooled.tx_hash(), consensus.tx_hash());
    }

    #[test]
    fn eip8141_sidecar_network_roundtrip() {
        let mut versioned_hash = B256::ZERO;
        versioned_hash[0] = 0x01;
        let tx = TxEip8141 {
            sender: Address::repeat_byte(0x41),
            nonce: 9,
            nonce_keys: Some(vec![U256::from(42)]),
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
