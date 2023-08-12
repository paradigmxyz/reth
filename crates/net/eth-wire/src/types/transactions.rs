//! Implements the `GetPooledTransactions` and `PooledTransactions` message types.
use bytes::Buf;
use reth_codecs::derive_arbitrary;
use reth_primitives::{
    kzg::{self, Blob, Bytes48, KzgProof, KzgSettings},
    Signature, Transaction, TransactionSigned, TransactionSignedNoHash, TxEip4844,
    EIP4844_TX_TYPE_ID, H256,
};
use reth_rlp::{
    Decodable, DecodeError, Encodable, Header, RlpDecodableWrapper, RlpEncodableWrapper,
    EMPTY_LIST_CODE,
};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg(any(test, feature = "arbitrary"))]
use proptest::{
    arbitrary::{any as proptest_any, ParamsFor},
    collection::vec as proptest_vec,
    strategy::{BoxedStrategy, Strategy},
};

#[cfg(any(test, feature = "arbitrary"))]
use reth_primitives::{
    constants::eip4844::{FIELD_ELEMENTS_PER_BLOB, KZG_TRUSTED_SETUP},
    kzg::{KzgCommitment, BYTES_PER_BLOB, BYTES_PER_FIELD_ELEMENT},
};

/// A list of transaction hashes that the peer would like transaction bodies for.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GetPooledTransactions(
    /// The transaction hashes to request transaction bodies for.
    pub Vec<H256>,
);

impl<T> From<Vec<T>> for GetPooledTransactions
where
    T: Into<H256>,
{
    fn from(hashes: Vec<T>) -> Self {
        GetPooledTransactions(hashes.into_iter().map(|h| h.into()).collect())
    }
}

/// The response to [`GetPooledTransactions`], containing the transaction bodies associated with
/// the requested hashes.
///
/// This response may not contain all bodies requested, but the bodies should be in the same order
/// as the request's hashes. Hashes may be skipped, and the client should ensure that each body
/// corresponds to a requested hash. Hashes may need to be re-requested if the bodies are not
/// included in the response.
#[derive_arbitrary(rlp, 10)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PooledTransactions(
    /// The transaction bodies, each of which should correspond to a requested hash.
    pub Vec<TransactionSigned>,
);

impl From<Vec<TransactionSigned>> for PooledTransactions {
    fn from(txs: Vec<TransactionSigned>) -> Self {
        PooledTransactions(txs)
    }
}

impl From<PooledTransactions> for Vec<TransactionSigned> {
    fn from(txs: PooledTransactions) -> Self {
        txs.0
    }
}

/// A response to [`GetPooledTransactions`]. This can include either a blob transaction, or a
/// non-4844 signed transaction.
// TODO: redo arbitrary for this encoding - the previous encoding was incorrect
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PooledTransactionResponse {
    /// A blob transaction, which includes the transaction, blob data, commitments, and proofs.
    BlobTransaction(BlobTransaction),
    /// A non-4844 signed transaction.
    Transaction(TransactionSigned),
}

impl Encodable for PooledTransactionResponse {
    /// Encodes an enveloped post EIP-4844 [PooledTransaction] response.
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

impl Decodable for PooledTransactionResponse {
    /// Decodes an enveloped post EIP-4844 [PooledTransaction] response.
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

        // If the header is a list header, it is a legacy transaction. Otherwise, it is a typed
        // transaction
        let header = Header::decode(buf)?;

        // Check if the tx is a list
        if header.list {
            // decode as legacy transaction
            let legacy_tx = TransactionSigned::decode_rlp_legacy_transaction(buf)?;
            Ok(PooledTransactionResponse::Transaction(legacy_tx))
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
                Ok(PooledTransactionResponse::BlobTransaction(blob_tx))
            } else {
                // DO NOT advance the buffer, since we want the enveloped decoding to decode it
                // again and advance the buffer on its own.
                let typed_tx = TransactionSigned::decode_enveloped_typed_transaction(buf)?;
                Ok(PooledTransactionResponse::Transaction(typed_tx))
            }
        }
    }
}

/// A response to [`GetPooledTransactions`] that includes blob data, their commitments, and their
/// corresponding proofs.
///
/// This is defined in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#networking) as an element
/// of a [PooledTransactions] response.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BlobTransaction {
    /// The transaction payload.
    pub transaction: TransactionSigned,
    /// The transaction's blob data.
    pub blobs: Vec<Blob>,
    /// The transaction's blob commitments.
    pub commitments: Vec<Bytes48>,
    /// The transaction's blob proofs.
    pub proofs: Vec<Bytes48>,
}

impl BlobTransaction {
    /// Verifies that the transaction's blob data, commitments, and proofs are all valid.
    ///
    /// Takes as input the [KzgSettings], which should contain the the parameters derived from the
    /// KZG trusted setup.
    ///
    /// This ensures that the blob transaction payload has the same number of blob data elements,
    /// commitments, and proofs. Each blob data element is verified against its commitment and
    /// proof.
    ///
    /// Returns `false` if any blob KZG proof in the response fails to verify.
    pub fn validate(&self, proof_settings: &KzgSettings) -> Result<bool, kzg::Error> {
        // Verify as a batch
        KzgProof::verify_blob_kzg_proof_batch(
            self.blobs.as_slice(),
            self.commitments.as_slice(),
            self.proofs.as_slice(),
            proof_settings,
        )
    }

    /// Encodes the [BlobTransaction] fields as RLP, with a tx type. If `with_header` is `false`,
    /// the following will be encoded:
    /// `tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// If `with_header` is `true`, the following will be encoded:
    /// `rlp(tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs]))`
    ///
    /// NOTE: The header will be a byte string header, not a list header.
    pub fn encode_with_type_inner(&self, out: &mut dyn bytes::BufMut, with_header: bool) {
        // Calculate the length of:
        // `tx_type || rlp([transaction_payload_body, blobs, commitments, proofs])`
        //
        // to construct and encode the string header
        if with_header {
            Header {
                list: false,
                // add one for the tx type
                payload_length: 1 + self.payload_len(),
            }
            .encode(out);
        }

        out.put_u8(EIP4844_TX_TYPE_ID);

        // Now we encode the inner blob transaction:
        self.encode_inner(out);
    }

    /// Encodes the [BlobTransaction] fields as RLP, with the following format:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP encoding methods, and does not
    /// represent the full RLP encoding of the blob transaction.
    pub fn encode_inner(&self, out: &mut dyn bytes::BufMut) {
        // First we construct both required list headers.
        //
        // The `transaction_payload_body` length is the length of the fields, plus the length of
        // its list header.
        let tx_header = Header {
            list: true,
            payload_length: self.transaction.fields_len() +
                self.transaction.signature.payload_len(),
        };

        let tx_length = tx_header.length() + tx_header.payload_length;

        // The payload length is the length of the `tranascation_payload_body` list, plus the
        // length of the blobs, commitments, and proofs.
        let payload_length =
            tx_length + self.blobs.length() + self.commitments.length() + self.proofs.length();

        // First we use the payload len to construct the first list header
        let blob_tx_header = Header { list: true, payload_length };

        // Encode the blob tx header first
        blob_tx_header.encode(out);

        // Encode the inner tx list header, then its fields
        tx_header.encode(out);
        self.transaction.encode_fields(out);

        // Encode the blobs, commitments, and proofs
        self.blobs.encode(out);
        self.commitments.encode(out);
        self.proofs.encode(out);
    }

    /// Ouputs the length of the RLP encoding of the blob transaction, including the tx type byte,
    /// optionally including the length of a wrapping string header. If `with_header` is `false`,
    /// the length of the following will be calculated:
    /// `tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// If `with_header` is `true`, the length of the following will be calculated:
    /// `rlp(tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs]))`
    pub fn payload_len_with_type(&self, with_header: bool) -> usize {
        if with_header {
            // Construct a header and use that to calculate the total length
            let wrapped_header = Header {
                list: false,
                // add one for the tx type byte
                payload_length: 1 + self.payload_len(),
            };

            // The total length is now the length of the header plus the length of the payload
            // (which includes the tx type byte)
            wrapped_header.length() + wrapped_header.payload_length
        } else {
            // Just add the length of the tx type to the payload length
            1 + self.payload_len()
        }
    }

    /// Outputs the length of the RLP encoding of the blob transaction with the following format:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP encoding length methods, and
    /// does not represent the full RLP encoding of the blob transaction.
    pub fn payload_len(&self) -> usize {
        // The `transaction_payload_body` length is the length of the fields, plus the length of
        // its list header.
        let tx_header = Header {
            list: true,
            payload_length: self.transaction.fields_len() +
                self.transaction.signature.payload_len(),
        };

        let tx_length = tx_header.length() + tx_header.payload_length;

        // The payload length is the length of the `tranascation_payload_body` list, plus the
        // length of the blobs, commitments, and proofs.
        tx_length + self.blobs.length() + self.commitments.length() + self.proofs.length()
    }

    /// Decodes a [BlobTransaction] from RLP. This expects the encoding to be:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP decoding methods, and does not
    /// represent the full RLP decoding of the [PooledTransaction] type.
    pub fn decode_inner(data: &mut &[u8]) -> Result<Self, DecodeError> {
        // decode the _first_ list header for the rest of the transaction
        let header = Header::decode(data)?;
        if !header.list {
            return Err(DecodeError::Custom("PooledTransactions blob tx must be encoded as a list"))
        }

        // Now we need to decode the inner 4844 transaction and its signature:
        //
        // `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
        let header = Header::decode(data)?;
        if !header.list {
            return Err(DecodeError::Custom(
                "PooledTransactions inner blob tx must be encoded as a list",
            ))
        }

        // inner transaction
        let transaction = Transaction::Eip4844(TxEip4844::decode_inner(data)?);

        // signature
        let signature = Signature::decode(data)?;

        // construct the tx now that we've decoded the fields in order
        let tx_no_hash = TransactionSignedNoHash { transaction, signature };

        // All that's left are the blobs, commitments, and proofs
        let blobs = <Vec<Blob> as Decodable>::decode(data)?;
        let commitments = <Vec<Bytes48> as Decodable>::decode(data)?;
        let proofs = <Vec<Bytes48> as Decodable>::decode(data)?;

        // # Calculating the hash
        //
        // The full encoding of the [PooledTransaction] response is:
        // `tx_type (0x03) || rlp([tx_payload_body, blobs, commitments, proofs])`
        //
        // The transaction hash however, is:
        // `keccak256(tx_type (0x03) || rlp(tx_payload_body))`
        //
        // Note that this is `tx_payload_body`, not `[tx_payload_body]`, which would be
        // `[[chain_id, nonce, max_priority_fee_per_gas, ...]]`, i.e. a list within a list.
        //
        // Because the pooled transaction encoding is different than the hash encoding for
        // EIP-4844 transactions, we do not use the original buffer to calculate the hash.
        //
        // Instead, we use [TransactionSignedNoHash] which will encode the transaction internally.
        let signed_tx = tx_no_hash.with_hash();

        Ok(Self { transaction: signed_tx, blobs, commitments, proofs })
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for BlobTransaction {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let mut arr = [0u8; BYTES_PER_BLOB];
        let blobs: Vec<Blob> = (0..u.int_in_range(1..=16)?)
            .map(|_| {
                arr = arbitrary::Arbitrary::arbitrary(u).unwrap();

                // Ensure that the blob is canonical by ensuring that
                // each field element contained in the blob is < BLS_MODULUS
                for i in 0..(FIELD_ELEMENTS_PER_BLOB as usize) {
                    arr[i * BYTES_PER_FIELD_ELEMENT] = 0;
                }
                Blob::from(arr)
            })
            .collect();

        Ok(generate_blob_transaction(blobs, TransactionSigned::arbitrary(u)?))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for BlobTransaction {
    type Parameters = ParamsFor<String>;
    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        (
            proptest_vec(proptest_vec(proptest_any::<u8>(), BYTES_PER_BLOB), 1..=5),
            proptest_any::<TransactionSigned>(),
        )
            .prop_map(move |(blobs, tx)| {
                let blobs = blobs
                    .into_iter()
                    .map(|mut blob| {
                        let mut arr = [0u8; BYTES_PER_BLOB];

                        // Ensure that the blob is canonical by ensuring that
                        // each field element contained in the blob is < BLS_MODULUS
                        for i in 0..(FIELD_ELEMENTS_PER_BLOB as usize) {
                            blob[i * BYTES_PER_FIELD_ELEMENT] = 0;
                        }

                        arr.copy_from_slice(blob.as_slice());
                        arr.into()
                    })
                    .collect();

                generate_blob_transaction(blobs, tx)
            })
            .boxed()
    }

    type Strategy = BoxedStrategy<BlobTransaction>;
}

#[cfg(any(test, feature = "arbitrary"))]
fn generate_blob_transaction(blobs: Vec<Blob>, transaction: TransactionSigned) -> BlobTransaction {
    let kzg_settings = KZG_TRUSTED_SETUP.clone();

    let commitments: Vec<Bytes48> = blobs
        .iter()
        .map(|blob| KzgCommitment::blob_to_kzg_commitment(blob.clone(), &kzg_settings).unwrap())
        .map(|commitment| commitment.to_bytes())
        .collect();

    let proofs: Vec<Bytes48> = blobs
        .iter()
        .zip(commitments.iter())
        .map(|(blob, commitment)| {
            KzgProof::compute_blob_kzg_proof(blob.clone(), *commitment, &kzg_settings).unwrap()
        })
        .map(|proof| proof.to_bytes())
        .collect();

    BlobTransaction { transaction, blobs, commitments, proofs }
}
#[cfg(test)]
mod test {
    use crate::{message::RequestPair, GetPooledTransactions, PooledTransactions};
    use hex_literal::hex;
    use reth_primitives::{
        hex, Signature, Transaction, TransactionKind, TransactionSigned, TxEip1559, TxLegacy, U256,
    };
    use reth_rlp::{Decodable, Encodable};
    use std::str::FromStr;

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_get_pooled_transactions() {
        let expected = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let mut data = vec![];
        let request = RequestPair::<GetPooledTransactions> {
            request_id: 1111,
            message: GetPooledTransactions(vec![
                hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
            ]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_get_pooled_transactions() {
        let data = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let request = RequestPair::<GetPooledTransactions>::decode(&mut &data[..]).unwrap();
        assert_eq!(
            request,
            RequestPair::<GetPooledTransactions> {
                request_id: 1111,
                message: GetPooledTransactions(vec![
                    hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                    hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
                ])
            }
        );
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_pooled_transactions() {
        let expected = hex!("f8d7820457f8d2f867088504a817c8088302e2489435353535353535353535353535353535353535358202008025a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10f867098504a817c809830334509435353535353535353535353535353535353535358202d98025a052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afba052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb");
        let mut data = vec![];
        let request = RequestPair::<PooledTransactions> {
            request_id: 1111,
            message: vec![
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(1),
                        nonce: 0x8u64,
                        gas_price: 0x4a817c808,
                        gas_limit: 0x2e248u64,
                        to: TransactionKind::Call(
                            hex!("3535353535353535353535353535353535353535").into(),
                        ),
                        value: 0x200u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(1),
                        nonce: 0x09u64,
                        gas_price: 0x4a817c809,
                        gas_limit: 0x33450u64,
                        to: TransactionKind::Call(
                            hex!("3535353535353535353535353535353535353535").into(),
                        ),
                        value: 0x2d9u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb",
                        )
                        .unwrap(),
                    },
                ),
            ]
            .into(),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_pooled_transactions() {
        let data = hex!("f8d7820457f8d2f867088504a817c8088302e2489435353535353535353535353535353535353535358202008025a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10f867098504a817c809830334509435353535353535353535353535353535353535358202d98025a052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afba052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb");
        let expected = RequestPair::<PooledTransactions> {
            request_id: 1111,
            message: vec![
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(1),
                        nonce: 0x8u64,
                        gas_price: 0x4a817c808,
                        gas_limit: 0x2e248u64,
                        to: TransactionKind::Call(
                            hex!("3535353535353535353535353535353535353535").into(),
                        ),
                        value: 0x200u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(1),
                        nonce: 0x09u64,
                        gas_price: 0x4a817c809,
                        gas_limit: 0x33450u64,
                        to: TransactionKind::Call(
                            hex!("3535353535353535353535353535353535353535").into(),
                        ),
                        value: 0x2d9u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb",
                        )
                        .unwrap(),
                    },
                ),
            ]
            .into(),
        };

        let request = RequestPair::<PooledTransactions>::decode(&mut &data[..]).unwrap();
        assert_eq!(request, expected);
    }

    #[test]
    fn decode_pooled_transactions_network() {
        let data = hex!("f9022980f90225f8650f84832156008287fb94cf7f9e66af820a19257a2108375b180b0ec491678204d2802ca035b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981a0612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860b87502f872041a8459682f008459682f0d8252089461815774383099e24810ab832a5b2a5425c154d58829a2241af62c000080c001a059e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafda0016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469f86b0384773594008398968094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba0ce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071a03ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88f86b01843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac3960468702769bb01b2a00802ba0e24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0aa05406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631daf86b02843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba00eb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5aea03a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18");
        let decoded_transactions =
            RequestPair::<PooledTransactions>::decode(&mut &data[..]).unwrap();

        let expected_transactions = RequestPair::<PooledTransactions> {
            request_id: 0,
            message: vec![
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(4),
                        nonce: 15u64,
                        gas_price: 2200000000,
                        gas_limit: 34811u64,
                        to: TransactionKind::Call(
                            hex!("cf7f9e66af820a19257a2108375b180b0ec49167").into(),
                        ),
                        value: 1234u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: true,
                        r: U256::from_str(
                            "0x35b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Eip1559(TxEip1559 {
                        chain_id: 4,
                        nonce: 26u64,
                        max_priority_fee_per_gas: 1500000000,
                        max_fee_per_gas: 1500000013,
                        gas_limit: 21000u64,
                        to: TransactionKind::Call(
                            hex!("61815774383099e24810ab832a5b2a5425c154d5").into(),
                        ),
                        value: 3000000000000000000u64.into(),
                        input: Default::default(),
                        access_list: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: true,
                        r: U256::from_str(
                            "0x59e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafd",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(4),
                        nonce: 3u64,
                        gas_price: 2000000000,
                        gas_limit: 10000000u64,
                        to: TransactionKind::Call(
                            hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046").into(),
                        ),
                        value: 1000000000000000u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0xce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x3ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(4),
                        nonce: 1u64,
                        gas_price: 1000000000,
                        gas_limit: 100000u64,
                        to: TransactionKind::Call(
                            hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046").into(),
                        ),
                        value: 693361000000000u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0xe24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0a",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x5406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631da",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(4),
                        nonce: 2u64,
                        gas_price: 1000000000,
                        gas_limit: 100000u64,
                        to: TransactionKind::Call(
                            hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046").into(),
                        ),
                        value: 1000000000000000u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0xeb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5ae",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x3a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18",
                        )
                        .unwrap(),
                    },
                ),
            ]
            .into(),
        };

        // checking tx by tx for easier debugging if there are any regressions
        for (decoded, expected) in
            decoded_transactions.message.0.iter().zip(expected_transactions.message.0.iter())
        {
            assert_eq!(decoded, expected);
        }

        assert_eq!(decoded_transactions, expected_transactions);
    }

    #[test]
    fn encode_pooled_transactions_network() {
        let expected = hex!("f9022980f90225f8650f84832156008287fb94cf7f9e66af820a19257a2108375b180b0ec491678204d2802ca035b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981a0612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860b87502f872041a8459682f008459682f0d8252089461815774383099e24810ab832a5b2a5425c154d58829a2241af62c000080c001a059e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafda0016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469f86b0384773594008398968094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba0ce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071a03ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88f86b01843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac3960468702769bb01b2a00802ba0e24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0aa05406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631daf86b02843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba00eb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5aea03a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18");

        let transactions = RequestPair::<PooledTransactions> {
            request_id: 0,
            message: vec![
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(4),
                        nonce: 15u64,
                        gas_price: 2200000000,
                        gas_limit: 34811u64,
                        to: TransactionKind::Call(
                            hex!("cf7f9e66af820a19257a2108375b180b0ec49167").into(),
                        ),
                        value: 1234u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: true,
                        r: U256::from_str(
                            "0x35b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Eip1559(TxEip1559 {
                        chain_id: 4,
                        nonce: 26u64,
                        max_priority_fee_per_gas: 1500000000,
                        max_fee_per_gas: 1500000013,
                        gas_limit: 21000u64,
                        to: TransactionKind::Call(
                            hex!("61815774383099e24810ab832a5b2a5425c154d5").into(),
                        ),
                        value: 3000000000000000000u64.into(),
                        input: Default::default(),
                        access_list: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: true,
                        r: U256::from_str(
                            "0x59e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafd",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(4),
                        nonce: 3u64,
                        gas_price: 2000000000,
                        gas_limit: 10000000u64,
                        to: TransactionKind::Call(
                            hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046").into(),
                        ),
                        value: 1000000000000000u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0xce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x3ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(4),
                        nonce: 1u64,
                        gas_price: 1000000000,
                        gas_limit: 100000u64,
                        to: TransactionKind::Call(
                            hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046").into(),
                        ),
                        value: 693361000000000u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0xe24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0a",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x5406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631da",
                        )
                        .unwrap(),
                    },
                ),
                TransactionSigned::from_transaction_and_signature(
                    Transaction::Legacy(TxLegacy {
                        chain_id: Some(4),
                        nonce: 2u64,
                        gas_price: 1000000000,
                        gas_limit: 100000u64,
                        to: TransactionKind::Call(
                            hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046").into(),
                        ),
                        value: 1000000000000000u64.into(),
                        input: Default::default(),
                    }),
                    Signature {
                        odd_y_parity: false,
                        r: U256::from_str(
                            "0xeb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5ae",
                        )
                        .unwrap(),
                        s: U256::from_str(
                            "0x3a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18",
                        )
                        .unwrap(),
                    },
                ),
            ]
            .into(),
        };

        let mut encoded = vec![];
        transactions.encode(&mut encoded);
        assert_eq!(encoded.len(), transactions.length());
        let encoded_str = hex::encode(encoded);
        let expected_str = hex::encode(expected);
        assert_eq!(encoded_str.len(), expected_str.len());
        assert_eq!(encoded_str, expected_str);
    }
}
