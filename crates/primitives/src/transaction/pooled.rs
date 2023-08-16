//! Includes the
use crate::{BlobTransaction, Bytes, TransactionSigned, EIP4844_TX_TYPE_ID};
use bytes::Buf;
use reth_rlp::{Decodable, DecodeError, Encodable, Header, EMPTY_LIST_CODE};
use serde::{Deserialize, Serialize};

/// A response to `GetPooledTransactions`. This can include either a blob transaction, or a
/// non-4844 signed transaction.
// TODO: redo arbitrary for this encoding - the previous encoding was incorrect
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PooledTransactionsElement {
    /// A blob transaction, which includes the transaction, blob data, commitments, and proofs.
    BlobTransaction(BlobTransaction),
    /// A non-4844 signed transaction.
    Transaction(TransactionSigned),
}

impl PooledTransactionsElement {
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
            Ok(Self::Transaction(TransactionSigned::decode_rlp_legacy_transaction(&mut data)?))
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
                Ok(PooledTransactionsElement::Transaction(typed_tx))
            }
        }
    }

    /// Returns the inner [TransactionSigned].
    pub fn into_transaction(self) -> TransactionSigned {
        match self {
            Self::Transaction(tx) => tx,
            Self::BlobTransaction(blob_tx) => blob_tx.transaction,
        }
    }
}

impl Encodable for PooledTransactionsElement {
    /// Encodes an enveloped post EIP-4844 [PooledTransactionsElement].
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        match self {
            Self::Transaction(tx) => tx.encode(out),
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
            Self::Transaction(tx) => tx.length(),
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
            let legacy_tx =
                TransactionSigned::decode_rlp_legacy_transaction(&mut original_encoding)?;

            // advance the buffer based on how far `decode_rlp_legacy_transaction` advanced the
            // buffer
            *buf = original_encoding;

            Ok(PooledTransactionsElement::Transaction(legacy_tx))
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
                Ok(PooledTransactionsElement::Transaction(typed_tx))
            }
        }
    }
}

impl From<TransactionSigned> for PooledTransactionsElement {
    /// Converts from a [TransactionSigned] to a [PooledTransactionsElement].
    ///
    /// NOTE: This will always return a [PooledTransactionsElement::Transaction] variant.
    fn from(tx: TransactionSigned) -> Self {
        Self::Transaction(tx)
    }
}
