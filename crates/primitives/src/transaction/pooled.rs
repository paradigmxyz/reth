//! Defines the types for blob transactions, legacy, and other EIP-2718 transactions included in a
//! response to `GetPooledTransactions`.
use crate::{
    Address, BlobTransaction, Bytes, Signature, Transaction, TransactionSigned,
    TransactionSignedEcRecovered, TxEip1559, TxEip2930, TxHash, TxLegacy, EIP4844_TX_TYPE_ID, H256,
};
use bytes::Buf;
use derive_more::{AsRef, Deref};
use reth_rlp::{Decodable, DecodeError, Encodable, Header, EMPTY_LIST_CODE};
use serde::{Deserialize, Serialize};

/// A response to `GetPooledTransactions`. This can include either a blob transaction, or a
/// non-4844 signed transaction.
// TODO: redo arbitrary for this encoding - the previous encoding was incorrect
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
    /// A blob transaction, which includes the transaction, blob data, commitments, and proofs.
    BlobTransaction(BlobTransaction),
}

impl PooledTransactionsElement {
    /// Tries to convert a [TransactionSigned] into a [PooledTransactionsElement].
    ///
    /// [BlobTransaction] are disallowed from being propagated, hence this returns an error if the
    /// `tx` is [Transaction::Eip4844]
    pub fn try_from_broadcast(tx: TransactionSigned) -> Result<Self, TransactionSigned> {
        if tx.is_eip4844() {
            return Err(tx)
        }
        Ok(tx.into())
    }

    /// Heavy operation that return signature hash over rlp encoded transaction.
    /// It is only for signature signing or signer recovery.
    pub fn signature_hash(&self) -> H256 {
        match self {
            Self::Legacy { transaction, .. } => transaction.signature_hash(),
            Self::Eip2930 { transaction, .. } => transaction.signature_hash(),
            Self::Eip1559 { transaction, .. } => transaction.signature_hash(),
            Self::BlobTransaction(blob_tx) => blob_tx.transaction.signature_hash(),
        }
    }

    /// Reference to transaction hash. Used to identify transaction.
    pub fn hash(&self) -> &TxHash {
        match self {
            PooledTransactionsElement::Legacy { hash, .. } => hash,
            PooledTransactionsElement::Eip2930 { hash, .. } => hash,
            PooledTransactionsElement::Eip1559 { hash, .. } => hash,
            PooledTransactionsElement::BlobTransaction(tx) => &tx.hash,
        }
    }

    /// Returns the signature of the transaction.
    pub fn signature(&self) -> &Signature {
        match self {
            Self::Legacy { signature, .. } => signature,
            Self::Eip2930 { signature, .. } => signature,
            Self::Eip1559 { signature, .. } => signature,
            Self::BlobTransaction(blob_tx) => &blob_tx.signature,
        }
    }

    /// Recover signer from signature and hash.
    ///
    /// Returns `None` if the transaction's signature is invalid, see also [Self::recover_signer].
    pub fn recover_signer(&self) -> Option<Address> {
        let signature_hash = self.signature_hash();
        self.signature().recover_signer(signature_hash)
    }

    /// Tries to recover signer and return [`PooledTransactionsElementEcRecovered`].
    ///
    /// Returns `Err(Self)` if the transaction's signature is invalid, see also
    /// [Self::recover_signer].
    pub fn try_into_ecrecovered(self) -> Result<PooledTransactionsElementEcRecovered, Self> {
        match self.recover_signer() {
            None => Err(self),
            Some(signer) => Ok(PooledTransactionsElementEcRecovered { transaction: self, signer }),
        }
    }

    /// Decodes the "raw" format of transaction (e.g. `eth_sendRawTransaction`).
    ///
    /// The raw transaction is either a legacy transaction or EIP-2718 typed transaction
    /// For legacy transactions, the format is encoded as: `rlp(tx)`
    /// For EIP-2718 typed transaction, the format is encoded as the type of the transaction
    /// followed by the rlp of the transaction: `type` + `rlp(tx)`
    ///
    /// For encoded EIP-4844 transactions, the blob sidecar _must_ be included.
    pub fn decode_enveloped(tx: Bytes) -> Result<Self, DecodeError> {
        let mut data = tx.as_ref();

        if data.is_empty() {
            return Err(DecodeError::InputTooShort)
        }

        // Check if the tx is a list - tx types are less than EMPTY_LIST_CODE (0xc0)
        if data[0] >= EMPTY_LIST_CODE {
            // decode as legacy transaction
            let (transaction, hash, signature) =
                TransactionSigned::decode_rlp_legacy_transaction_tuple(&mut data)?;

            Ok(Self::Legacy { transaction, signature, hash })
        } else {
            // decode the type byte, only decode BlobTransaction if it is a 4844 transaction
            let tx_type = *data.first().ok_or(DecodeError::InputTooShort)?;

            if tx_type == EIP4844_TX_TYPE_ID {
                // Recall that the blob transaction response `TranactionPayload` is encoded like
                // this: `rlp([tx_payload_body, blobs, commitments, proofs])`
                //
                // Note that `tx_payload_body` is a list:
                // `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
                //
                // This makes the full encoding:
                // `tx_type (0x03) || rlp([[chain_id, nonce, ...], blobs, commitments, proofs])`
                //
                // First, we advance the buffer past the type byte
                data.advance(1);

                // Now, we decode the inner blob transaction:
                // `rlp([[chain_id, nonce, ...], blobs, commitments, proofs])`
                let blob_tx = BlobTransaction::decode_inner(&mut data)?;
                Ok(PooledTransactionsElement::BlobTransaction(blob_tx))
            } else {
                // DO NOT advance the buffer for the type, since we want the enveloped decoding to
                // decode it again and advance the buffer on its own.
                let typed_tx = TransactionSigned::decode_enveloped_typed_transaction(&mut data)?;

                // because we checked the tx type, we can be sure that the transaction is not a
                // blob transaction or legacy
                match typed_tx.transaction {
                    Transaction::Legacy(_) => Err(DecodeError::Custom(
                        "legacy transactions should not be a result of EIP-2718 decoding",
                    )),
                    Transaction::Eip4844(_) => Err(DecodeError::Custom(
                        "EIP-4844 transactions can only be decoded with transaction type 0x03",
                    )),
                    Transaction::Eip2930(tx) => Ok(PooledTransactionsElement::Eip2930 {
                        transaction: tx,
                        signature: typed_tx.signature,
                        hash: typed_tx.hash,
                    }),
                    Transaction::Eip1559(tx) => Ok(PooledTransactionsElement::Eip1559 {
                        transaction: tx,
                        signature: typed_tx.signature,
                        hash: typed_tx.hash,
                    }),
                }
            }
        }
    }

    /// Create [`TransactionSignedEcRecovered`] by converting this transaction into
    /// [`TransactionSigned`] and [`Address`] of the signer.
    pub fn into_ecrecovered_transaction(self, signer: Address) -> TransactionSignedEcRecovered {
        TransactionSignedEcRecovered::from_signed_transaction(self.into_transaction(), signer)
    }

    /// Returns the inner [TransactionSigned].
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
            Self::BlobTransaction(blob_tx) => blob_tx.into_parts().0,
        }
    }
}

impl Encodable for PooledTransactionsElement {
    /// Encodes an enveloped post EIP-4844 [PooledTransactionsElement].
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        match self {
            Self::Legacy { transaction, signature, .. } => {
                transaction.encode_with_signature(signature, out)
            }
            Self::Eip2930 { transaction, signature, .. } => {
                // encodes with header
                transaction.encode_with_signature(signature, out, true)
            }
            Self::Eip1559 { transaction, signature, .. } => {
                // encodes with header
                transaction.encode_with_signature(signature, out, true)
            }
            Self::BlobTransaction(blob_tx) => {
                // The inner encoding is used with `with_header` set to true, making the final
                // encoding:
                // `rlp(tx_type || rlp([transaction_payload_body, blobs, commitments, proofs]))`
                blob_tx.encode_with_type_inner(out, true);
            }
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::Legacy { transaction, signature, .. } => {
                // method computes the payload len with a RLP header
                transaction.payload_len_with_signature(signature)
            }
            Self::Eip2930 { transaction, signature, .. } => {
                // method computes the payload len with a RLP header
                transaction.payload_len_with_signature(signature)
            }
            Self::Eip1559 { transaction, signature, .. } => {
                // method computes the payload len with a RLP header
                transaction.payload_len_with_signature(signature)
            }
            Self::BlobTransaction(blob_tx) => {
                // the encoding uses a header, so we set `with_header` to true
                blob_tx.payload_len_with_type(true)
            }
        }
    }
}

impl Decodable for PooledTransactionsElement {
    /// Decodes an enveloped post EIP-4844 [PooledTransactionsElement].
    ///
    /// CAUTION: this expects that `buf` is `[id, rlp(tx)]`
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
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
            return Err(DecodeError::InputTooShort)
        }

        // keep this around for buffer advancement post-legacy decoding
        let mut original_encoding = *buf;

        // If the header is a list header, it is a legacy transaction. Otherwise, it is a typed
        // transaction
        let header = Header::decode(buf)?;

        // Check if the tx is a list
        if header.list {
            // decode as legacy transaction
            let (transaction, hash, signature) =
                TransactionSigned::decode_rlp_legacy_transaction_tuple(&mut original_encoding)?;

            // advance the buffer based on how far `decode_rlp_legacy_transaction` advanced the
            // buffer
            *buf = original_encoding;

            Ok(Self::Legacy { transaction, signature, hash })
        } else {
            // decode the type byte, only decode BlobTransaction if it is a 4844 transaction
            let tx_type = *buf.first().ok_or(DecodeError::InputTooShort)?;

            if tx_type == EIP4844_TX_TYPE_ID {
                // Recall that the blob transaction response `TranactionPayload` is encoded like
                // this: `rlp([tx_payload_body, blobs, commitments, proofs])`
                //
                // Note that `tx_payload_body` is a list:
                // `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
                //
                // This makes the full encoding:
                // `tx_type (0x03) || rlp([[chain_id, nonce, ...], blobs, commitments, proofs])`
                //
                // First, we advance the buffer past the type byte
                buf.advance(1);

                // Now, we decode the inner blob transaction:
                // `rlp([[chain_id, nonce, ...], blobs, commitments, proofs])`
                let blob_tx = BlobTransaction::decode_inner(buf)?;
                Ok(PooledTransactionsElement::BlobTransaction(blob_tx))
            } else {
                // DO NOT advance the buffer for the type, since we want the enveloped decoding to
                // decode it again and advance the buffer on its own.
                let typed_tx = TransactionSigned::decode_enveloped_typed_transaction(buf)?;

                // because we checked the tx type, we can be sure that the transaction is not a
                // blob transaction or legacy
                match typed_tx.transaction {
                    Transaction::Legacy(_) => Err(DecodeError::Custom(
                        "legacy transactions should not be a result of EIP-2718 decoding",
                    )),
                    Transaction::Eip4844(_) => Err(DecodeError::Custom(
                        "EIP-4844 transactions can only be decoded with transaction type 0x03",
                    )),
                    Transaction::Eip2930(tx) => Ok(PooledTransactionsElement::Eip2930 {
                        transaction: tx,
                        signature: typed_tx.signature,
                        hash: typed_tx.hash,
                    }),
                    Transaction::Eip1559(tx) => Ok(PooledTransactionsElement::Eip1559 {
                        transaction: tx,
                        signature: typed_tx.signature,
                        hash: typed_tx.hash,
                    }),
                }
            }
        }
    }
}

impl From<TransactionSigned> for PooledTransactionsElement {
    /// Converts from a [TransactionSigned] to a [PooledTransactionsElement].
    ///
    /// NOTE: For EIP-4844 transactions, this will return an empty sidecar.
    fn from(tx: TransactionSigned) -> Self {
        let TransactionSigned { transaction, signature, hash } = tx;
        match transaction {
            Transaction::Legacy(tx) => {
                PooledTransactionsElement::Legacy { transaction: tx, signature, hash }
            }
            Transaction::Eip2930(tx) => {
                PooledTransactionsElement::Eip2930 { transaction: tx, signature, hash }
            }
            Transaction::Eip1559(tx) => {
                PooledTransactionsElement::Eip1559 { transaction: tx, signature, hash }
            }
            Transaction::Eip4844(tx) => {
                PooledTransactionsElement::BlobTransaction(BlobTransaction {
                    transaction: tx,
                    signature,
                    hash,
                    // This is empty - just for the conversion!
                    sidecar: Default::default(),
                })
            }
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
    pub fn signer(&self) -> Address {
        self.signer
    }

    /// Transform back to [`PooledTransactionsElement`]
    pub fn into_transaction(self) -> PooledTransactionsElement {
        self.transaction
    }

    /// Transform back to [`PooledTransactionsElement`]
    pub fn into_ecrecovered_transaction(self) -> TransactionSignedEcRecovered {
        let (tx, signer) = self.into_components();
        tx.into_ecrecovered_transaction(signer)
    }

    /// Desolve Self to its component
    pub fn into_components(self) -> (PooledTransactionsElement, Address) {
        (self.transaction, self.signer)
    }

    /// Create [`TransactionSignedEcRecovered`] from [`PooledTransactionsElement`] and [`Address`]
    /// of the signer.
    pub fn from_signed_transaction(
        transaction: PooledTransactionsElement,
        signer: Address,
    ) -> Self {
        Self { transaction, signer }
    }
}
