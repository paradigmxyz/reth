//! Defines the types for blob transactions, legacy, and other EIP-2718 transactions included in a
//! response to `GetPooledTransactions`.

#![cfg(feature = "c-kzg")]
#![cfg_attr(docsrs, doc(cfg(feature = "c-kzg")))]

use crate::{
    Address, BlobTransaction, BlobTransactionSidecar, Bytes, Signature, Transaction,
    TransactionSigned, TransactionSignedEcRecovered, TxEip1559, TxEip2930, TxHash, TxLegacy, B256,
    EIP4844_TX_TYPE_ID,
};
use alloy_rlp::{Decodable, Encodable, Error as RlpError, Header, EMPTY_LIST_CODE};
use bytes::{Buf, BytesMut};
use derive_more::{AsRef, Deref};
use reth_codecs::add_arbitrary_tests;
use serde::{Deserialize, Serialize};

/// A response to `GetPooledTransactions`. This can include either a blob transaction, or a
/// non-4844 signed transaction.
#[add_arbitrary_tests]
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
    /// An Optimism deposit transaction
    #[cfg(feature = "optimism")]
    Deposit {
        /// The inner transaction
        transaction: crate::TxDeposit,
        /// The signature
        signature: Signature,
        /// The hash of the transaction
        hash: TxHash,
    },
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

    /// Converts from an EIP-4844 [TransactionSignedEcRecovered] to a
    /// [PooledTransactionsElementEcRecovered] with the given sidecar.
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
                PooledTransactionsElement::BlobTransaction(BlobTransaction {
                    transaction: tx,
                    signature,
                    hash,
                    sidecar,
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
            Self::BlobTransaction(blob_tx) => blob_tx.transaction.signature_hash(),
            #[cfg(feature = "optimism")]
            Self::Deposit { .. } => B256::ZERO,
        }
    }

    /// Reference to transaction hash. Used to identify transaction.
    pub fn hash(&self) -> &TxHash {
        match self {
            PooledTransactionsElement::Legacy { hash, .. } |
            PooledTransactionsElement::Eip2930 { hash, .. } |
            PooledTransactionsElement::Eip1559 { hash, .. } => hash,
            PooledTransactionsElement::BlobTransaction(tx) => &tx.hash,
            #[cfg(feature = "optimism")]
            PooledTransactionsElement::Deposit { hash, .. } => hash,
        }
    }

    /// Returns the signature of the transaction.
    pub fn signature(&self) -> &Signature {
        match self {
            Self::Legacy { signature, .. } |
            Self::Eip2930 { signature, .. } |
            Self::Eip1559 { signature, .. } => signature,
            Self::BlobTransaction(blob_tx) => &blob_tx.signature,
            #[cfg(feature = "optimism")]
            Self::Deposit { .. } => {
                panic!("Deposit transactions do not have a signature! This is a bug.")
            }
        }
    }

    /// Returns the transaction nonce.
    pub fn nonce(&self) -> u64 {
        match self {
            Self::Legacy { transaction, .. } => transaction.nonce,
            Self::Eip2930 { transaction, .. } => transaction.nonce,
            Self::Eip1559 { transaction, .. } => transaction.nonce,
            Self::BlobTransaction(blob_tx) => blob_tx.transaction.nonce,
            #[cfg(feature = "optimism")]
            Self::Deposit { .. } => 0,
        }
    }

    /// Recover signer from signature and hash.
    ///
    /// Returns `None` if the transaction's signature is invalid, see also [Self::recover_signer].
    pub fn recover_signer(&self) -> Option<Address> {
        self.signature().recover_signer(self.signature_hash())
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
    /// This should be used for `eth_sendRawTransaction`, for any transaction type. Blob
    /// transactions **must** include the blob sidecar as part of the raw encoding.
    ///
    /// This method can not be used for decoding the `transactions` field of `engine_newPayload`,
    /// because EIP-4844 transactions for that method do not include the blob sidecar. The blobs
    /// are supplied in an argument separate from the payload.
    ///
    /// A raw transaction is either a legacy transaction or EIP-2718 typed transaction, with a
    /// special case for EIP-4844 transactions.
    ///
    /// For legacy transactions, the format is encoded as: `rlp(tx)`. This format will start with a
    /// RLP list header.
    ///
    /// For EIP-2718 typed transactions, the format is encoded as the type of the transaction
    /// followed by the rlp of the transaction: `type || rlp(tx)`.
    ///
    /// For EIP-4844 transactions, the format includes a blob sidecar (the blobs, commitments, and
    /// proofs) after the transaction:
    /// `type || rlp([tx_payload_body, blobs, commitments, proofs])`
    ///
    /// Where `tx_payload_body` is encoded as a RLP list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    pub fn decode_enveloped(tx: Bytes) -> alloy_rlp::Result<Self> {
        let mut data = tx.as_ref();

        if data.is_empty() {
            return Err(RlpError::InputTooShort)
        }

        // Check if the tx is a list - tx types are less than EMPTY_LIST_CODE (0xc0)
        if data[0] >= EMPTY_LIST_CODE {
            // decode as legacy transaction
            let (transaction, hash, signature) =
                TransactionSigned::decode_rlp_legacy_transaction_tuple(&mut data)?;

            Ok(Self::Legacy { transaction, signature, hash })
        } else {
            // decode the type byte, only decode BlobTransaction if it is a 4844 transaction
            let tx_type = *data.first().ok_or(RlpError::InputTooShort)?;

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
                    Transaction::Legacy(_) => Err(RlpError::Custom(
                        "legacy transactions should not be a result of EIP-2718 decoding",
                    )),
                    Transaction::Eip4844(_) => Err(RlpError::Custom(
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
                    #[cfg(feature = "optimism")]
                    Transaction::Deposit(tx) => Ok(PooledTransactionsElement::Deposit {
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
            #[cfg(feature = "optimism")]
            Self::Deposit { transaction, signature, hash } => TransactionSigned {
                transaction: Transaction::Deposit(transaction),
                signature,
                hash,
            },
        }
    }

    /// Returns the length without an RLP header - this is used for eth/68 sizes.
    pub fn length_without_header(&self) -> usize {
        match self {
            Self::Legacy { transaction, signature, .. } => {
                // method computes the payload len with a RLP header
                transaction.payload_len_with_signature(signature)
            }
            Self::Eip2930 { transaction, signature, .. } => {
                // method computes the payload len without a RLP header
                transaction.payload_len_with_signature_without_header(signature)
            }
            Self::Eip1559 { transaction, signature, .. } => {
                // method computes the payload len without a RLP header
                transaction.payload_len_with_signature_without_header(signature)
            }
            Self::BlobTransaction(blob_tx) => {
                // the encoding does not use a header, so we set `with_header` to false
                blob_tx.payload_len_with_type(false)
            }
            #[cfg(feature = "optimism")]
            Self::Deposit { transaction, .. } => transaction.payload_len_without_header(),
        }
    }

    /// Returns the enveloped encoded transactions.
    ///
    /// See also [TransactionSigned::encode_enveloped]
    pub fn envelope_encoded(&self) -> Bytes {
        let mut buf = BytesMut::new();
        self.encode_enveloped(&mut buf);
        buf.freeze().into()
    }

    /// Encodes the transaction into the "raw" format (e.g. `eth_sendRawTransaction`).
    /// This format is also referred to as "binary" encoding.
    ///
    /// For legacy transactions, it encodes the RLP of the transaction into the buffer:
    /// `rlp(tx-data)`
    /// For EIP-2718 typed it encodes the type of the transaction followed by the rlp of the
    /// transaction: `tx-type || rlp(tx-data)`
    pub fn encode_enveloped(&self, out: &mut dyn bytes::BufMut) {
        // The encoding of `tx-data` depends on the transaction type. Refer to these docs for more
        // information on the exact format:
        // - Legacy: TxLegacy::encode_with_signature
        // - EIP-2930: TxEip2930::encode_with_signature
        // - EIP-1559: TxEip1559::encode_with_signature
        // - EIP-4844: BlobTransaction::encode_with_type_inner
        match self {
            Self::Legacy { transaction, signature, .. } => {
                transaction.encode_with_signature(signature, out)
            }
            Self::Eip2930 { transaction, signature, .. } => {
                transaction.encode_with_signature(signature, out, false)
            }
            Self::Eip1559 { transaction, signature, .. } => {
                transaction.encode_with_signature(signature, out, false)
            }
            Self::BlobTransaction(blob_tx) => {
                // The inner encoding is used with `with_header` set to true, making the final
                // encoding:
                // `tx_type || rlp([transaction_payload_body, blobs, commitments, proofs]))`
                blob_tx.encode_with_type_inner(out, false);
            }
            #[cfg(feature = "optimism")]
            Self::Deposit { transaction, .. } => {
                transaction.encode(out, false);
            }
        }
    }
}

impl Encodable for PooledTransactionsElement {
    /// Encodes an enveloped post EIP-4844 [PooledTransactionsElement].
    ///
    /// For legacy transactions, this encodes the transaction as `rlp(tx-data)`.
    ///
    /// For EIP-2718 transactions, this encodes the transaction as `rlp(tx_type || rlp(tx-data)))`,
    /// ___including__ the RLP-header for the entire transaction.
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        // The encoding of `tx-data` depends on the transaction type. Refer to these docs for more
        // information on the exact format:
        // - Legacy: TxLegacy::encode_with_signature
        // - EIP-2930: TxEip2930::encode_with_signature
        // - EIP-1559: TxEip1559::encode_with_signature
        // - EIP-4844: BlobTransaction::encode_with_type_inner
        match self {
            Self::Legacy { transaction, signature, .. } => {
                transaction.encode_with_signature(signature, out)
            }
            Self::Eip2930 { transaction, signature, .. } => {
                // encodes with string header
                transaction.encode_with_signature(signature, out, true)
            }
            Self::Eip1559 { transaction, signature, .. } => {
                // encodes with string header
                transaction.encode_with_signature(signature, out, true)
            }
            Self::BlobTransaction(blob_tx) => {
                // The inner encoding is used with `with_header` set to true, making the final
                // encoding:
                // `rlp(tx_type || rlp([transaction_payload_body, blobs, commitments, proofs]))`
                blob_tx.encode_with_type_inner(out, true);
            }
            #[cfg(feature = "optimism")]
            Self::Deposit { transaction, .. } => {
                transaction.encode(out, true);
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
            #[cfg(feature = "optimism")]
            Self::Deposit { transaction, .. } => {
                // method computes the payload len with a RLP header
                transaction.payload_len()
            }
        }
    }
}

impl Decodable for PooledTransactionsElement {
    /// Decodes an enveloped post EIP-4844 [PooledTransactionsElement].
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
            let (transaction, hash, signature) =
                TransactionSigned::decode_rlp_legacy_transaction_tuple(&mut original_encoding)?;

            // advance the buffer by however long the legacy transaction decoding advanced the
            // buffer
            *buf = original_encoding;

            Ok(Self::Legacy { transaction, signature, hash })
        } else {
            // decode the type byte, only decode BlobTransaction if it is a 4844 transaction
            let tx_type = *buf.first().ok_or(RlpError::InputTooShort)?;
            let remaining_len = buf.len();

            if tx_type == EIP4844_TX_TYPE_ID {
                // Recall that the blob transaction response `TransactionPayload` is encoded like
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

                // check that the bytes consumed match the payload length
                let bytes_consumed = remaining_len - buf.len();
                if bytes_consumed != header.payload_length {
                    return Err(RlpError::UnexpectedLength)
                }

                Ok(PooledTransactionsElement::BlobTransaction(blob_tx))
            } else {
                // DO NOT advance the buffer for the type, since we want the enveloped decoding to
                // decode it again and advance the buffer on its own.
                let typed_tx = TransactionSigned::decode_enveloped_typed_transaction(buf)?;

                // check that the bytes consumed match the payload length
                let bytes_consumed = remaining_len - buf.len();
                if bytes_consumed != header.payload_length {
                    return Err(RlpError::UnexpectedLength)
                }

                // because we checked the tx type, we can be sure that the transaction is not a
                // blob transaction or legacy
                match typed_tx.transaction {
                    Transaction::Legacy(_) => Err(RlpError::Custom(
                        "legacy transactions should not be a result of EIP-2718 decoding",
                    )),
                    Transaction::Eip4844(_) => Err(RlpError::Custom(
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
                    #[cfg(feature = "optimism")]
                    Transaction::Deposit(tx) => Ok(PooledTransactionsElement::Deposit {
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
            #[cfg(feature = "optimism")]
            Transaction::Deposit(tx) => {
                PooledTransactionsElement::Deposit { transaction: tx, signature, hash }
            }
        }
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
        Ok(match PooledTransactionsElement::from(TransactionSigned::arbitrary(u)?) {
            // If the generated `PooledTransactionsElement` is a blob transaction...
            PooledTransactionsElement::BlobTransaction(mut tx) => {
                // Generate a sidecar for the blob transaction using arbitrary data.
                tx.sidecar = crate::BlobTransactionSidecar::arbitrary(u)?;
                // Return the blob transaction with the generated sidecar.
                PooledTransactionsElement::BlobTransaction(tx)
            }
            // If the generated `PooledTransactionsElement` is not a blob transaction...
            tx => tx,
        })
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for PooledTransactionsElement {
    type Parameters = ();
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::{any, Strategy};

        any::<(TransactionSigned, crate::BlobTransactionSidecar)>()
            .prop_map(move |(transaction, sidecar)| {
                // this will have an empty sidecar
                let pooled_txs_element = PooledTransactionsElement::from(transaction);

                // generate a sidecar for blob txs
                if let PooledTransactionsElement::BlobTransaction(mut tx) = pooled_txs_element {
                    tx.sidecar = sidecar;
                    PooledTransactionsElement::BlobTransaction(tx)
                } else {
                    pooled_txs_element
                }
            })
            .boxed()
    }

    type Strategy = proptest::strategy::BoxedStrategy<PooledTransactionsElement>;
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

    /// Transform back to [`TransactionSignedEcRecovered`]
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

    /// Converts from an EIP-4844 [TransactionSignedEcRecovered] to a
    /// [PooledTransactionsElementEcRecovered] with the given sidecar.
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
impl From<TransactionSignedEcRecovered> for PooledTransactionsElementEcRecovered {
    fn from(tx: TransactionSignedEcRecovered) -> Self {
        Self { transaction: tx.signed_transaction.into(), signer: tx.signer }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, hex};
    use assert_matches::assert_matches;

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

        for hex_data in input_too_short.iter() {
            let input_rlp = &mut &hex_data[..];
            let res = PooledTransactionsElement::decode(input_rlp);

            assert!(
                res.is_err(),
                "expected err after decoding rlp input: {:x?}",
                Bytes::copy_from_slice(hex_data)
            );

            // this is a legacy tx so we can attempt the same test with decode_enveloped
            let input_rlp = &mut &hex_data[..];
            let res =
                PooledTransactionsElement::decode_enveloped(Bytes::copy_from_slice(input_rlp));

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

        let res = PooledTransactionsElement::decode_enveloped(data.into()).unwrap();
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
        let res = PooledTransactionsElement::decode_enveloped(Bytes::copy_from_slice(data));
        assert_matches!(res, Ok(_tx));
    }
}
