//! Defines the types for blob transactions, legacy, and other EIP-2718 transactions included in a
//! response to `GetPooledTransactions`.

use super::{
    error::TransactionConversionError,
    signature::{recover_signer, with_eip155_parity},
    TxEip7702,
};
use crate::{
    BlobTransaction, BlobTransactionSidecar, Signature, Transaction, TransactionSigned,
    TransactionSignedEcRecovered, EIP4844_TX_TYPE_ID,
};
use alloy_consensus::{
    transaction::{TxEip1559, TxEip2930, TxEip4844, TxLegacy},
    SignableTransaction, TxEip4844WithSidecar,
};
use alloy_eips::eip2718::{Decodable2718, Eip2718Result, Encodable2718};
use alloy_primitives::{Address, TxHash, B256};
use alloy_rlp::{Decodable, Encodable, Error as RlpError, Header};
use bytes::Buf;
use derive_more::{AsRef, Deref};
use serde::{Deserialize, Serialize};

/// A response to `GetPooledTransactions`. This can include either a blob transaction, or a
/// non-4844 signed transaction.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PooledTransactionsElement {
    /// A legacy transaction
    Legacy {
        /// The inner transaction
        transaction: TxLegacy,
        /// The signature
        signature: Signature,
        /// The hash of the transaction
        hash: TxHash,
    },
    /// An EIP-2930 typed transaction
    Eip2930 {
        /// The inner transaction
        transaction: TxEip2930,
        /// The signature
        signature: Signature,
        /// The hash of the transaction
        hash: TxHash,
    },
    /// An EIP-1559 typed transaction
    Eip1559 {
        /// The inner transaction
        transaction: TxEip1559,
        /// The signature
        signature: Signature,
        /// The hash of the transaction
        hash: TxHash,
    },
    /// An EIP-7702 typed transaction
    Eip7702 {
        /// The inner transaction
        transaction: TxEip7702,
        /// The signature
        signature: Signature,
        /// The hash of the transaction
        hash: TxHash,
    },
    /// A blob transaction, which includes the transaction, blob data, commitments, and proofs.
    BlobTransaction(BlobTransaction),
}

impl PooledTransactionsElement {
    /// Tries to convert a [`TransactionSigned`] into a [`PooledTransactionsElement`].
    ///
    /// This function used as a helper to convert from a decoded p2p broadcast message to
    /// [`PooledTransactionsElement`]. Since [`BlobTransaction`] is disallowed to be broadcasted on
    /// p2p, return an err if `tx` is [`Transaction::Eip4844`].
    pub fn try_from_broadcast(tx: TransactionSigned) -> Result<Self, TransactionSigned> {
        match tx {
            TransactionSigned { transaction: Transaction::Legacy(tx), signature, hash } => {
                Ok(Self::Legacy { transaction: tx, signature, hash })
            }
            TransactionSigned { transaction: Transaction::Eip2930(tx), signature, hash } => {
                Ok(Self::Eip2930 { transaction: tx, signature, hash })
            }
            TransactionSigned { transaction: Transaction::Eip1559(tx), signature, hash } => {
                Ok(Self::Eip1559 { transaction: tx, signature, hash })
            }
            TransactionSigned { transaction: Transaction::Eip7702(tx), signature, hash } => {
                Ok(Self::Eip7702 { transaction: tx, signature, hash })
            }
            // Not supported because missing blob sidecar
            tx @ TransactionSigned { transaction: Transaction::Eip4844(_), .. } => Err(tx),
            #[cfg(feature = "optimism")]
            // Not supported because deposit transactions are never pooled
            tx @ TransactionSigned { transaction: Transaction::Deposit(_), .. } => Err(tx),
        }
    }

    /// Converts from an EIP-4844 [`TransactionSignedEcRecovered`] to a
    /// [`PooledTransactionsElementEcRecovered`] with the given sidecar.
    ///
    /// Returns an `Err` containing the original `TransactionSigned` if the transaction is not
    /// EIP-4844.
    pub fn try_from_blob_transaction(
        tx: TransactionSigned,
        sidecar: BlobTransactionSidecar,
    ) -> Result<Self, TransactionSigned> {
        Ok(match tx {
            // If the transaction is an EIP-4844 transaction...
            TransactionSigned { transaction: Transaction::Eip4844(tx), signature, hash } => {
                // Construct a `PooledTransactionsElement::BlobTransaction` with provided sidecar.
                Self::BlobTransaction(BlobTransaction {
                    signature,
                    hash,
                    transaction: TxEip4844WithSidecar { tx, sidecar },
                })
            }
            // If the transaction is not EIP-4844, return an error with the original
            // transaction.
            _ => return Err(tx),
        })
    }

    /// Heavy operation that return signature hash over rlp encoded transaction.
    /// It is only for signature signing or signer recovery.
    pub fn signature_hash(&self) -> B256 {
        match self {
            Self::Legacy { transaction, .. } => transaction.signature_hash(),
            Self::Eip2930 { transaction, .. } => transaction.signature_hash(),
            Self::Eip1559 { transaction, .. } => transaction.signature_hash(),
            Self::Eip7702 { transaction, .. } => transaction.signature_hash(),
            Self::BlobTransaction(blob_tx) => blob_tx.transaction.signature_hash(),
        }
    }

    /// Reference to transaction hash. Used to identify transaction.
    pub const fn hash(&self) -> &TxHash {
        match self {
            Self::Legacy { hash, .. } |
            Self::Eip2930 { hash, .. } |
            Self::Eip1559 { hash, .. } |
            Self::Eip7702 { hash, .. } => hash,
            Self::BlobTransaction(tx) => &tx.hash,
        }
    }

    /// Returns the signature of the transaction.
    pub const fn signature(&self) -> &Signature {
        match self {
            Self::Legacy { signature, .. } |
            Self::Eip2930 { signature, .. } |
            Self::Eip1559 { signature, .. } |
            Self::Eip7702 { signature, .. } => signature,
            Self::BlobTransaction(blob_tx) => &blob_tx.signature,
        }
    }

    /// Returns the transaction nonce.
    pub const fn nonce(&self) -> u64 {
        match self {
            Self::Legacy { transaction, .. } => transaction.nonce,
            Self::Eip2930 { transaction, .. } => transaction.nonce,
            Self::Eip1559 { transaction, .. } => transaction.nonce,
            Self::Eip7702 { transaction, .. } => transaction.nonce,
            Self::BlobTransaction(blob_tx) => blob_tx.transaction.tx.nonce,
        }
    }

    /// Recover signer from signature and hash.
    ///
    /// Returns `None` if the transaction's signature is invalid, see also [`Self::recover_signer`].
    pub fn recover_signer(&self) -> Option<Address> {
        recover_signer(self.signature(), self.signature_hash())
    }

    /// Tries to recover signer and return [`PooledTransactionsElementEcRecovered`].
    ///
    /// Returns `Err(Self)` if the transaction's signature is invalid, see also
    /// [`Self::recover_signer`].
    pub fn try_into_ecrecovered(self) -> Result<PooledTransactionsElementEcRecovered, Self> {
        match self.recover_signer() {
            None => Err(self),
            Some(signer) => Ok(PooledTransactionsElementEcRecovered { transaction: self, signer }),
        }
    }

    /// Create [`TransactionSignedEcRecovered`] by converting this transaction into
    /// [`TransactionSigned`] and [`Address`] of the signer.
    pub fn into_ecrecovered_transaction(self, signer: Address) -> TransactionSignedEcRecovered {
        TransactionSignedEcRecovered::from_signed_transaction(self.into_transaction(), signer)
    }

    /// Returns the inner [`TransactionSigned`].
    pub fn into_transaction(self) -> TransactionSigned {
        match self {
            Self::Legacy { transaction, signature, hash } => {
                TransactionSigned { transaction: Transaction::Legacy(transaction), signature, hash }
            }
            Self::Eip2930 { transaction, signature, hash } => TransactionSigned {
                transaction: Transaction::Eip2930(transaction),
                signature,
                hash,
            },
            Self::Eip1559 { transaction, signature, hash } => TransactionSigned {
                transaction: Transaction::Eip1559(transaction),
                signature,
                hash,
            },
            Self::Eip7702 { transaction, signature, hash } => TransactionSigned {
                transaction: Transaction::Eip7702(transaction),
                signature,
                hash,
            },
            Self::BlobTransaction(blob_tx) => blob_tx.into_parts().0,
        }
    }

    /// Returns true if the transaction is an EIP-4844 transaction.
    #[inline]
    pub const fn is_eip4844(&self) -> bool {
        matches!(self, Self::BlobTransaction(_))
    }

    /// Returns the [`TxLegacy`] variant if the transaction is a legacy transaction.
    pub const fn as_legacy(&self) -> Option<&TxLegacy> {
        match self {
            Self::Legacy { transaction, .. } => Some(transaction),
            _ => None,
        }
    }

    /// Returns the [`TxEip2930`] variant if the transaction is an EIP-2930 transaction.
    pub const fn as_eip2930(&self) -> Option<&TxEip2930> {
        match self {
            Self::Eip2930 { transaction, .. } => Some(transaction),
            _ => None,
        }
    }

    /// Returns the [`TxEip1559`] variant if the transaction is an EIP-1559 transaction.
    pub const fn as_eip1559(&self) -> Option<&TxEip1559> {
        match self {
            Self::Eip1559 { transaction, .. } => Some(transaction),
            _ => None,
        }
    }

    /// Returns the [`TxEip4844`] variant if the transaction is an EIP-4844 transaction.
    pub const fn as_eip4844(&self) -> Option<&TxEip4844> {
        match self {
            Self::BlobTransaction(tx) => Some(&tx.transaction.tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip7702`] variant if the transaction is an EIP-7702 transaction.
    pub const fn as_eip7702(&self) -> Option<&TxEip7702> {
        match self {
            Self::Eip7702 { transaction, .. } => Some(transaction),
            _ => None,
        }
    }

    /// Returns the blob gas used for all blobs of the EIP-4844 transaction if it is an EIP-4844
    /// transaction.
    ///
    /// This is the number of blobs times the
    /// [`DATA_GAS_PER_BLOB`](crate::constants::eip4844::DATA_GAS_PER_BLOB) a single blob consumes.
    pub fn blob_gas_used(&self) -> Option<u64> {
        self.as_eip4844().map(TxEip4844::blob_gas)
    }

    /// Max fee per blob gas for eip4844 transaction [`TxEip4844`].
    ///
    /// Returns `None` for non-eip4844 transactions.
    ///
    /// This is also commonly referred to as the "Blob Gas Fee Cap" (`BlobGasFeeCap`).
    pub const fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            Self::BlobTransaction(tx) => Some(tx.transaction.tx.max_fee_per_blob_gas),
            _ => None,
        }
    }

    /// Max priority fee per gas for eip1559 transaction, for legacy and eip2930 transactions this
    /// is `None`
    ///
    /// This is also commonly referred to as the "Gas Tip Cap" (`GasTipCap`).
    pub const fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Legacy { .. } | Self::Eip2930 { .. } => None,
            Self::Eip1559 { transaction, .. } => Some(transaction.max_priority_fee_per_gas),
            Self::Eip7702 { transaction, .. } => Some(transaction.max_priority_fee_per_gas),
            Self::BlobTransaction(tx) => Some(tx.transaction.tx.max_priority_fee_per_gas),
        }
    }

    /// Max fee per gas for eip1559 transaction, for legacy transactions this is `gas_price`.
    ///
    /// This is also commonly referred to as the "Gas Fee Cap" (`GasFeeCap`).
    pub const fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::Legacy { transaction, .. } => transaction.gas_price,
            Self::Eip2930 { transaction, .. } => transaction.gas_price,
            Self::Eip1559 { transaction, .. } => transaction.max_fee_per_gas,
            Self::Eip7702 { transaction, .. } => transaction.max_fee_per_gas,
            Self::BlobTransaction(tx) => tx.transaction.tx.max_fee_per_gas,
        }
    }
}

impl Encodable for PooledTransactionsElement {
    /// This encodes the transaction _with_ the signature, and an rlp header.
    ///
    /// For legacy transactions, it encodes the transaction data:
    /// `rlp(tx-data)`
    ///
    /// For EIP-2718 typed transactions, it encodes the transaction type followed by the rlp of the
    /// transaction:
    /// `rlp(tx-type || rlp(tx-data))`
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.network_encode(out);
    }

    fn length(&self) -> usize {
        let mut payload_length = self.encode_2718_len();
        if !self.is_legacy() {
            payload_length += Header { list: false, payload_length }.length();
        }

        payload_length
    }
}

impl Decodable for PooledTransactionsElement {
    /// Decodes an enveloped post EIP-4844 [`PooledTransactionsElement`].
    ///
    /// CAUTION: this expects that `buf` is `rlp(tx_type || rlp(tx-data))`
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        // From the EIP-4844 spec:
        // Blob transactions have two network representations. During transaction gossip responses
        // (`PooledTransactions`), the EIP-2718 `TransactionPayload` of the blob transaction is
        // wrapped to become:
        //
        // `rlp([tx_payload_body, blobs, commitments, proofs])`
        //
        // This means the full wire encoding is:
        // `rlp(tx_type || rlp([transaction_payload_body, blobs, commitments, proofs]))`
        //
        // First, we check whether or not the transaction is a legacy transaction.
        if buf.is_empty() {
            return Err(RlpError::InputTooShort)
        }

        // keep the original buf around for legacy decoding
        let mut original_encoding = *buf;

        // If the header is a list header, it is a legacy transaction. Otherwise, it is a typed
        // transaction
        let header = Header::decode(buf)?;

        // Check if the tx is a list
        if header.list {
            // decode as legacy transaction
            let tx = Self::fallback_decode(&mut original_encoding)?;

            // advance the buffer by however long the legacy transaction decoding advanced the
            // buffer
            *buf = original_encoding;

            Ok(tx)
        } else {
            // decode the type byte, only decode BlobTransaction if it is a 4844 transaction
            let tx_type = *buf.first().ok_or(RlpError::InputTooShort)?;
            let remaining_len = buf.len();

            // Advance the buffer past the type byte
            buf.advance(1);

            let tx = Self::typed_decode(tx_type, buf).map_err(RlpError::from)?;

            // check that the bytes consumed match the payload length
            let bytes_consumed = remaining_len - buf.len();
            if bytes_consumed != header.payload_length {
                return Err(RlpError::UnexpectedLength)
            }

            Ok(tx)
        }
    }
}

impl Encodable2718 for PooledTransactionsElement {
    fn type_flag(&self) -> Option<u8> {
        match self {
            Self::Legacy { .. } => None,
            Self::Eip2930 { .. } => Some(0x01),
            Self::Eip1559 { .. } => Some(0x02),
            Self::BlobTransaction { .. } => Some(0x03),
            Self::Eip7702 { .. } => Some(0x04),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match self {
            Self::Legacy { transaction, signature, .. } => {
                // method computes the payload len with a RLP header
                transaction.encoded_len_with_signature(&with_eip155_parity(
                    signature,
                    transaction.chain_id,
                ))
            }
            Self::Eip2930 { transaction, signature, .. } => {
                // method computes the payload len without a RLP header
                transaction.encoded_len_with_signature(signature, false)
            }
            Self::Eip1559 { transaction, signature, .. } => {
                // method computes the payload len without a RLP header
                transaction.encoded_len_with_signature(signature, false)
            }
            Self::Eip7702 { transaction, signature, .. } => {
                // method computes the payload len without a RLP header
                transaction.encoded_len_with_signature(signature, false)
            }
            Self::BlobTransaction(blob_tx) => {
                // the encoding does not use a header, so we set `with_header` to false
                blob_tx.payload_len_with_type(false)
            }
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        // The encoding of `tx-data` depends on the transaction type. Refer to these docs for more
        // information on the exact format:
        // - Legacy: TxLegacy::encode_with_signature
        // - EIP-2930: TxEip2930::encode_with_signature
        // - EIP-1559: TxEip1559::encode_with_signature
        // - EIP-4844: BlobTransaction::encode_with_type_inner
        // - EIP-7702: TxEip7702::encode_with_signature
        match self {
            Self::Legacy { transaction, signature, .. } => transaction
                .encode_with_signature_fields(
                    &with_eip155_parity(signature, transaction.chain_id),
                    out,
                ),
            Self::Eip2930 { transaction, signature, .. } => {
                transaction.encode_with_signature(signature, out, false)
            }
            Self::Eip1559 { transaction, signature, .. } => {
                transaction.encode_with_signature(signature, out, false)
            }
            Self::Eip7702 { transaction, signature, .. } => {
                transaction.encode_with_signature(signature, out, false)
            }
            Self::BlobTransaction(blob_tx) => {
                // The inner encoding is used with `with_header` set to true, making the final
                // encoding:
                // `tx_type || rlp([transaction_payload_body, blobs, commitments, proofs]))`
                blob_tx.encode_with_type_inner(out, false);
            }
        }
    }
}

impl Decodable2718 for PooledTransactionsElement {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty {
            EIP4844_TX_TYPE_ID => {
                // Recall that the blob transaction response `TransactionPayload` is encoded like
                // this: `rlp([tx_payload_body, blobs, commitments, proofs])`
                //
                // Note that `tx_payload_body` is a list:
                // `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
                //
                // This makes the full encoding:
                // `tx_type (0x03) || rlp([[chain_id, nonce, ...], blobs, commitments, proofs])`

                // Now, we decode the inner blob transaction:
                // `rlp([[chain_id, nonce, ...], blobs, commitments, proofs])`
                let blob_tx = BlobTransaction::decode_inner(buf)?;
                Ok(Self::BlobTransaction(blob_tx))
            }
            tx_type => {
                let typed_tx = TransactionSigned::typed_decode(tx_type, buf)?;

                match typed_tx.transaction {
                    Transaction::Legacy(_) => Err(RlpError::Custom(
                        "legacy transactions should not be a result of typed decoding",
                    ).into()),
                    // because we checked the tx type, we can be sure that the transaction is not a
                    // blob transaction
                    Transaction::Eip4844(_) => Err(RlpError::Custom(
                        "EIP-4844 transactions can only be decoded with transaction type 0x03",
                    ).into()),
                    Transaction::Eip2930(tx) => Ok(Self::Eip2930 {
                        transaction: tx,
                        signature: typed_tx.signature,
                        hash: typed_tx.hash,
                    }),
                    Transaction::Eip1559(tx) => Ok(Self::Eip1559 {
                        transaction: tx,
                        signature: typed_tx.signature,
                        hash: typed_tx.hash,
                    }),
                    Transaction::Eip7702(tx) => Ok(Self::Eip7702 {
                        transaction: tx,
                        signature: typed_tx.signature,
                        hash: typed_tx.hash,
                    }),
                    #[cfg(feature = "optimism")]
                    Transaction::Deposit(_) => Err(RlpError::Custom("Optimism deposit transaction cannot be decoded to PooledTransactionsElement").into())
                }
            }
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        // decode as legacy transaction
        let (transaction, hash, signature) =
            TransactionSigned::decode_rlp_legacy_transaction_tuple(buf)?;

        Ok(Self::Legacy { transaction, signature, hash })
    }
}

impl TryFrom<TransactionSigned> for PooledTransactionsElement {
    type Error = TransactionConversionError;

    fn try_from(tx: TransactionSigned) -> Result<Self, Self::Error> {
        Self::try_from_broadcast(tx).map_err(|_| TransactionConversionError::UnsupportedForP2P)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for PooledTransactionsElement {
    /// Generates an arbitrary `PooledTransactionsElement`.
    ///
    /// This function generates an arbitrary `PooledTransactionsElement` by creating a transaction
    /// and, if applicable, generating a sidecar for blob transactions.
    ///
    /// It handles the generation of sidecars and constructs the resulting
    /// `PooledTransactionsElement`.
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        // Attempt to create a `TransactionSigned` with arbitrary data.
        let tx_signed = TransactionSigned::arbitrary(u)?;
        // Attempt to create a `PooledTransactionsElement` with arbitrary data, handling the Result.
        match Self::try_from(tx_signed) {
            Ok(Self::BlobTransaction(mut tx)) => {
                // Successfully converted to a BlobTransaction, now generate a sidecar.
                tx.transaction.sidecar = crate::BlobTransactionSidecar::arbitrary(u)?;
                Ok(Self::BlobTransaction(tx))
            }
            Ok(tx) => Ok(tx), // Successfully converted, but not a BlobTransaction.
            Err(_) => Err(arbitrary::Error::IncorrectFormat), /* Conversion failed, return an
                                * arbitrary error. */
        }
    }
}

/// A signed pooled transaction with recovered signer.
#[derive(Debug, Clone, PartialEq, Eq, AsRef, Deref)]
pub struct PooledTransactionsElementEcRecovered {
    /// Signer of the transaction
    signer: Address,
    /// Signed transaction
    #[deref]
    #[as_ref]
    transaction: PooledTransactionsElement,
}

// === impl PooledTransactionsElementEcRecovered ===

impl PooledTransactionsElementEcRecovered {
    /// Signer of transaction recovered from signature
    pub const fn signer(&self) -> Address {
        self.signer
    }

    /// Transform back to [`PooledTransactionsElement`]
    pub fn into_transaction(self) -> PooledTransactionsElement {
        self.transaction
    }

    /// Transform back to [`TransactionSignedEcRecovered`]
    pub fn into_ecrecovered_transaction(self) -> TransactionSignedEcRecovered {
        let (tx, signer) = self.into_components();
        tx.into_ecrecovered_transaction(signer)
    }

    /// Dissolve Self to its component
    pub fn into_components(self) -> (PooledTransactionsElement, Address) {
        (self.transaction, self.signer)
    }

    /// Create [`TransactionSignedEcRecovered`] from [`PooledTransactionsElement`] and [`Address`]
    /// of the signer.
    pub const fn from_signed_transaction(
        transaction: PooledTransactionsElement,
        signer: Address,
    ) -> Self {
        Self { transaction, signer }
    }

    /// Converts from an EIP-4844 [`TransactionSignedEcRecovered`] to a
    /// [`PooledTransactionsElementEcRecovered`] with the given sidecar.
    ///
    /// Returns the transaction is not an EIP-4844 transaction.
    pub fn try_from_blob_transaction(
        tx: TransactionSignedEcRecovered,
        sidecar: BlobTransactionSidecar,
    ) -> Result<Self, TransactionSignedEcRecovered> {
        let TransactionSignedEcRecovered { signer, signed_transaction } = tx;
        let transaction =
            PooledTransactionsElement::try_from_blob_transaction(signed_transaction, sidecar)
                .map_err(|tx| TransactionSignedEcRecovered { signer, signed_transaction: tx })?;
        Ok(Self { transaction, signer })
    }
}

/// Converts a `TransactionSignedEcRecovered` into a `PooledTransactionsElementEcRecovered`.
impl TryFrom<TransactionSignedEcRecovered> for PooledTransactionsElementEcRecovered {
    type Error = TransactionConversionError;

    fn try_from(tx: TransactionSignedEcRecovered) -> Result<Self, Self::Error> {
        match PooledTransactionsElement::try_from(tx.signed_transaction) {
            Ok(pooled_transaction) => {
                Ok(Self { transaction: pooled_transaction, signer: tx.signer })
            }
            Err(_) => Err(TransactionConversionError::UnsupportedForP2P),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, hex};
    use assert_matches::assert_matches;
    use bytes::Bytes;

    #[test]
    fn invalid_legacy_pooled_decoding_input_too_short() {
        let input_too_short = [
            // this should fail because the payload length is longer than expected
            &hex!("d90b0280808bc5cd028083c5cdfd9e407c56565656")[..],
            // these should fail decoding
            //
            // The `c1` at the beginning is a list header, and the rest is a valid legacy
            // transaction, BUT the payload length of the list header is 1, and the payload is
            // obviously longer than one byte.
            &hex!("c10b02808083c5cd028883c5cdfd9e407c56565656"),
            &hex!("c10b0280808bc5cd028083c5cdfd9e407c56565656"),
            // this one is 19 bytes, and the buf is long enough, but the transaction will not
            // consume that many bytes.
            &hex!("d40b02808083c5cdeb8783c5acfd9e407c5656565656"),
            &hex!("d30102808083c5cd02887dc5cdfd9e64fd9e407c56"),
        ];

        for hex_data in &input_too_short {
            let input_rlp = &mut &hex_data[..];
            let res = PooledTransactionsElement::decode(input_rlp);

            assert!(
                res.is_err(),
                "expected err after decoding rlp input: {:x?}",
                Bytes::copy_from_slice(hex_data)
            );

            // this is a legacy tx so we can attempt the same test with decode_enveloped
            let input_rlp = &mut &hex_data[..];
            let res = PooledTransactionsElement::decode_2718(input_rlp);

            assert!(
                res.is_err(),
                "expected err after decoding enveloped rlp input: {:x?}",
                Bytes::copy_from_slice(hex_data)
            );
        }
    }

    // <https://holesky.etherscan.io/tx/0x7f60faf8a410a80d95f7ffda301d5ab983545913d3d789615df3346579f6c849>
    #[test]
    fn decode_eip1559_enveloped() {
        let data = hex!("02f903d382426882ba09832dc6c0848674742682ed9694714b6a4ea9b94a8a7d9fd362ed72630688c8898c80b90364492d24749189822d8512430d3f3ff7a2ede675ac08265c08e2c56ff6fdaa66dae1cdbe4a5d1d7809f3e99272d067364e597542ac0c369d69e22a6399c3e9bee5da4b07e3f3fdc34c32c3d88aa2268785f3e3f8086df0934b10ef92cfffc2e7f3d90f5e83302e31382e302d64657600000000000000000000000000000000000000000000569e75fc77c1a856f6daaf9e69d8a9566ca34aa47f9133711ce065a571af0cfd000000000000000000000000e1e210594771824dad216568b91c9cb4ceed361c00000000000000000000000000000000000000000000000000000000000546e00000000000000000000000000000000000000000000000000000000000e4e1c00000000000000000000000000000000000000000000000000000000065d6750c00000000000000000000000000000000000000000000000000000000000f288000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002cf600000000000000000000000000000000000000000000000000000000000000640000000000000000000000000000000000000000000000000000000000000000f1628e56fa6d8c50e5b984a58c0df14de31c7b857ce7ba499945b99252976a93d06dcda6776fc42167fbe71cb59f978f5ef5b12577a90b132d14d9c6efa528076f0161d7bf03643cfc5490ec5084f4a041db7f06c50bd97efa08907ba79ddcac8b890f24d12d8db31abbaaf18985d54f400449ee0559a4452afe53de5853ce090000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000028000000000000000000000000000000000000000000000000000000000000003e800000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000064ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00000000000000000000000000000000000000000000000000000000c080a01428023fc54a27544abc421d5d017b9a7c5936ad501cbdecd0d9d12d04c1a033a0753104bbf1c87634d6ff3f0ffa0982710612306003eb022363b57994bdef445a"
);

        let res = PooledTransactionsElement::decode_2718(&mut &data[..]).unwrap();
        assert_eq!(
            res.into_transaction().to(),
            Some(address!("714b6a4ea9b94a8a7d9fd362ed72630688c8898c"))
        );
    }

    #[test]
    fn legacy_valid_pooled_decoding() {
        // d3 <- payload length, d3 - c0 = 0x13 = 19
        // 0b <- nonce
        // 02 <- gas_price
        // 80 <- gas_limit
        // 80 <- to (Create)
        // 83 c5cdeb <- value
        // 87 83c5acfd9e407c <- input
        // 56 <- v (eip155, so modified with a chain id)
        // 56 <- r
        // 56 <- s
        let data = &hex!("d30b02808083c5cdeb8783c5acfd9e407c565656")[..];

        let input_rlp = &mut &data[..];
        let res = PooledTransactionsElement::decode(input_rlp);
        assert_matches!(res, Ok(_tx));
        assert!(input_rlp.is_empty());

        // this is a legacy tx so we can attempt the same test with
        // decode_rlp_legacy_transaction_tuple
        let input_rlp = &mut &data[..];
        let res = TransactionSigned::decode_rlp_legacy_transaction_tuple(input_rlp);
        assert_matches!(res, Ok(_tx));
        assert!(input_rlp.is_empty());

        // we can also decode_enveloped
        let res = PooledTransactionsElement::decode_2718(&mut &data[..]);
        assert_matches!(res, Ok(_tx));
    }
}
