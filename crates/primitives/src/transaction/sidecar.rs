#![cfg(feature = "c-kzg")]
#![cfg_attr(docsrs, doc(cfg(feature = "c-kzg")))]

#[cfg(any(test, feature = "arbitrary"))]
use crate::{
    constants::eip4844::{FIELD_ELEMENTS_PER_BLOB, MAINNET_KZG_TRUSTED_SETUP},
    kzg::{KzgCommitment, KzgProof, BYTES_PER_FIELD_ELEMENT},
};
use crate::{
    keccak256,
    kzg::{
        self, Blob, Bytes48, KzgSettings, BYTES_PER_BLOB, BYTES_PER_COMMITMENT, BYTES_PER_PROOF,
    },
    Signature, Transaction, TransactionSigned, TxEip4844, TxHash, B256, EIP4844_TX_TYPE_ID,
};
use alloy_rlp::{Decodable, Encodable, Error as RlpError, Header};
use bytes::BufMut;
#[cfg(any(test, feature = "arbitrary"))]
use proptest::{
    arbitrary::{any as proptest_any, ParamsFor},
    collection::vec as proptest_vec,
    strategy::{BoxedStrategy, Strategy},
};
use serde::{Deserialize, Serialize};

/// An error that can occur when validating a [BlobTransaction].
#[derive(Debug, thiserror::Error)]
pub enum BlobTransactionValidationError {
    /// Proof validation failed.
    #[error("invalid KZG proof")]
    InvalidProof,
    /// An error returned by [`kzg`].
    #[error("KZG error: {0:?}")]
    KZGError(#[from] kzg::Error),
    /// The inner transaction is not a blob transaction.
    #[error("unable to verify proof for non blob transaction: {0}")]
    NotBlobTransaction(u8),
    /// The versioned hash is incorrect.
    #[error("wrong versioned hash: have {have}, expected {expected}")]
    WrongVersionedHash {
        /// The versioned hash we got
        have: B256,
        /// The versioned hash we expected
        expected: B256,
    },
}

/// A response to `GetPooledTransactions` that includes blob data, their commitments, and their
/// corresponding proofs.
///
/// This is defined in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#networking) as an element
/// of a `PooledTransactions` response.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BlobTransaction {
    /// The transaction hash.
    pub hash: TxHash,
    /// The transaction payload.
    pub transaction: TxEip4844,
    /// The transaction signature.
    pub signature: Signature,
    /// The transaction's blob sidecar.
    pub sidecar: BlobTransactionSidecar,
}

impl BlobTransaction {
    /// Constructs a new [BlobTransaction] from a [TransactionSigned] and a
    /// [BlobTransactionSidecar].
    ///
    /// Returns an error if the signed transaction is not [TxEip4844]
    pub fn try_from_signed(
        tx: TransactionSigned,
        sidecar: BlobTransactionSidecar,
    ) -> Result<Self, (TransactionSigned, BlobTransactionSidecar)> {
        let TransactionSigned { transaction, signature, hash } = tx;
        match transaction {
            Transaction::Eip4844(transaction) => Ok(Self { hash, transaction, signature, sidecar }),
            transaction => {
                let tx = TransactionSigned { transaction, signature, hash };
                Err((tx, sidecar))
            }
        }
    }

    /// Verifies that the transaction's blob data, commitments, and proofs are all valid.
    ///
    /// See also [TxEip4844::validate_blob]
    pub fn validate(
        &self,
        proof_settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        self.transaction.validate_blob(&self.sidecar, proof_settings)
    }

    /// Splits the [BlobTransaction] into its [TransactionSigned] and [BlobTransactionSidecar]
    /// components.
    pub fn into_parts(self) -> (TransactionSigned, BlobTransactionSidecar) {
        let transaction = TransactionSigned {
            transaction: Transaction::Eip4844(self.transaction),
            hash: self.hash,
            signature: self.signature,
        };

        (transaction, self.sidecar)
    }

    /// Encodes the [BlobTransaction] fields as RLP, with a tx type. If `with_header` is `false`,
    /// the following will be encoded:
    /// `tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// If `with_header` is `true`, the following will be encoded:
    /// `rlp(tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs]))`
    ///
    /// NOTE: The header will be a byte string header, not a list header.
    pub(crate) fn encode_with_type_inner(&self, out: &mut dyn bytes::BufMut, with_header: bool) {
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
    pub(crate) fn encode_inner(&self, out: &mut dyn bytes::BufMut) {
        // First we construct both required list headers.
        //
        // The `transaction_payload_body` length is the length of the fields, plus the length of
        // its list header.
        let tx_header = Header {
            list: true,
            payload_length: self.transaction.fields_len() + self.signature.payload_len(),
        };

        let tx_length = tx_header.length() + tx_header.payload_length;

        // The payload length is the length of the `tranascation_payload_body` list, plus the
        // length of the blobs, commitments, and proofs.
        let payload_length = tx_length + self.sidecar.fields_len();

        // First we use the payload len to construct the first list header
        let blob_tx_header = Header { list: true, payload_length };

        // Encode the blob tx header first
        blob_tx_header.encode(out);

        // Encode the inner tx list header, then its fields
        tx_header.encode(out);
        self.transaction.encode_fields(out);

        // Encode the signature
        self.signature.encode(out);

        // Encode the blobs, commitments, and proofs
        self.sidecar.encode_inner(out);
    }

    /// Ouputs the length of the RLP encoding of the blob transaction, including the tx type byte,
    /// optionally including the length of a wrapping string header. If `with_header` is `false`,
    /// the length of the following will be calculated:
    /// `tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// If `with_header` is `true`, the length of the following will be calculated:
    /// `rlp(tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs]))`
    pub(crate) fn payload_len_with_type(&self, with_header: bool) -> usize {
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
    pub(crate) fn payload_len(&self) -> usize {
        // The `transaction_payload_body` length is the length of the fields, plus the length of
        // its list header.
        let tx_header = Header {
            list: true,
            payload_length: self.transaction.fields_len() + self.signature.payload_len(),
        };

        let tx_length = tx_header.length() + tx_header.payload_length;

        // The payload length is the length of the `tranascation_payload_body` list, plus the
        // length of the blobs, commitments, and proofs.
        let payload_length = tx_length + self.sidecar.fields_len();

        // We use the calculated payload len to construct the first list header, which encompasses
        // everything in the tx - the length of the second, inner list header is part of
        // payload_length
        let blob_tx_header = Header { list: true, payload_length };

        // The final length is the length of:
        //  * the outer blob tx header +
        //  * the inner tx header +
        //  * the inner tx fields +
        //  * the signature fields +
        //  * the sidecar fields
        blob_tx_header.length() + blob_tx_header.payload_length
    }

    /// Decodes a [BlobTransaction] from RLP. This expects the encoding to be:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP decoding methods, and does not
    /// represent the full RLP decoding of the `PooledTransactionsElement` type.
    pub(crate) fn decode_inner(data: &mut &[u8]) -> alloy_rlp::Result<Self> {
        // decode the _first_ list header for the rest of the transaction
        let outer_header = Header::decode(data)?;
        if !outer_header.list {
            return Err(RlpError::Custom("PooledTransactions blob tx must be encoded as a list"))
        }

        let outer_remaining_len = data.len();

        // Now we need to decode the inner 4844 transaction and its signature:
        //
        // `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
        let inner_header = Header::decode(data)?;
        if !inner_header.list {
            return Err(RlpError::Custom(
                "PooledTransactions inner blob tx must be encoded as a list",
            ))
        }

        let inner_remaining_len = data.len();

        // inner transaction
        let transaction = TxEip4844::decode_inner(data)?;

        // signature
        let signature = Signature::decode(data)?;

        // the inner header only decodes the transaction and signature, so we check the length here
        let inner_consumed = inner_remaining_len - data.len();
        if inner_consumed != inner_header.payload_length {
            return Err(RlpError::UnexpectedLength)
        }

        // All that's left are the blobs, commitments, and proofs
        let sidecar = BlobTransactionSidecar::decode_inner(data)?;

        // # Calculating the hash
        //
        // The full encoding of the `PooledTransaction` response is:
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
        // Instead, we use `encode_with_signature`, which RLP encodes the transaction with a
        // signature for hashing without a header. We then hash the result.
        let mut buf = Vec::new();
        transaction.encode_with_signature(&signature, &mut buf, false);
        let hash = keccak256(&buf);

        // the outer header is for the entire transaction, so we check the length here
        let outer_consumed = outer_remaining_len - data.len();
        if outer_consumed != outer_header.payload_length {
            return Err(RlpError::UnexpectedLength)
        }

        Ok(Self { transaction, hash, signature, sidecar })
    }
}

/// This represents a set of blobs, and its corresponding commitments and proofs.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(C)]
pub struct BlobTransactionSidecar {
    /// The blob data.
    pub blobs: Vec<Blob>,
    /// The blob commitments.
    pub commitments: Vec<Bytes48>,
    /// The blob proofs.
    pub proofs: Vec<Bytes48>,
}

impl BlobTransactionSidecar {
    /// Creates a new [BlobTransactionSidecar] using the given blobs, commitments, and proofs.
    pub fn new(blobs: Vec<Blob>, commitments: Vec<Bytes48>, proofs: Vec<Bytes48>) -> Self {
        Self { blobs, commitments, proofs }
    }

    /// Encodes the inner [BlobTransactionSidecar] fields as RLP bytes, without a RLP header.
    ///
    /// This encodes the fields in the following order:
    /// - `blobs`
    /// - `commitments`
    /// - `proofs`
    #[inline]
    pub(crate) fn encode_inner(&self, out: &mut dyn bytes::BufMut) {
        BlobTransactionSidecarRlp::wrap_ref(self).encode(out);
    }

    /// Outputs the RLP length of the [BlobTransactionSidecar] fields, without a RLP header.
    pub fn fields_len(&self) -> usize {
        BlobTransactionSidecarRlp::wrap_ref(self).fields_len()
    }

    /// Decodes the inner [BlobTransactionSidecar] fields from RLP bytes, without a RLP header.
    ///
    /// This decodes the fields in the following order:
    /// - `blobs`
    /// - `commitments`
    /// - `proofs`
    #[inline]
    pub(crate) fn decode_inner(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(BlobTransactionSidecarRlp::decode(buf)?.unwrap())
    }

    /// Calculates a size heuristic for the in-memory size of the [BlobTransactionSidecar].
    #[inline]
    pub fn size(&self) -> usize {
        self.blobs.len() * BYTES_PER_BLOB + // blobs
        self.commitments.len() * BYTES_PER_COMMITMENT + // commitments
        self.proofs.len() * BYTES_PER_PROOF // proofs
    }
}

impl From<reth_rpc_types::BlobTransactionSidecar> for BlobTransactionSidecar {
    fn from(value: reth_rpc_types::BlobTransactionSidecar) -> Self {
        // SAFETY: Same repr and size
        unsafe { std::mem::transmute(value) }
    }
}

impl From<BlobTransactionSidecar> for reth_rpc_types::BlobTransactionSidecar {
    fn from(value: BlobTransactionSidecar) -> Self {
        // SAFETY: Same repr and size
        unsafe { std::mem::transmute(value) }
    }
}

impl Encodable for BlobTransactionSidecar {
    /// Encodes the inner [BlobTransactionSidecar] fields as RLP bytes, without a RLP header.
    fn encode(&self, out: &mut dyn BufMut) {
        self.encode_inner(out)
    }

    fn length(&self) -> usize {
        self.fields_len()
    }
}

impl Decodable for BlobTransactionSidecar {
    /// Decodes the inner [BlobTransactionSidecar] fields from RLP bytes, without a RLP header.
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::decode_inner(buf)
    }
}

// Wrapper for c-kzg rlp
#[repr(C)]
struct BlobTransactionSidecarRlp {
    blobs: Vec<[u8; BYTES_PER_BLOB]>,
    commitments: Vec<[u8; BYTES_PER_COMMITMENT]>,
    proofs: Vec<[u8; BYTES_PER_PROOF]>,
}

const _: [(); std::mem::size_of::<BlobTransactionSidecar>()] =
    [(); std::mem::size_of::<BlobTransactionSidecarRlp>()];

const _: [(); std::mem::size_of::<BlobTransactionSidecar>()] =
    [(); std::mem::size_of::<reth_rpc_types::BlobTransactionSidecar>()];

impl BlobTransactionSidecarRlp {
    fn wrap_ref(other: &BlobTransactionSidecar) -> &Self {
        // SAFETY: Same repr and size
        unsafe { &*(other as *const BlobTransactionSidecar).cast::<Self>() }
    }

    fn unwrap(self) -> BlobTransactionSidecar {
        // SAFETY: Same repr and size
        unsafe { std::mem::transmute(self) }
    }

    fn encode(&self, out: &mut dyn bytes::BufMut) {
        // Encode the blobs, commitments, and proofs
        self.blobs.encode(out);
        self.commitments.encode(out);
        self.proofs.encode(out);
    }

    fn fields_len(&self) -> usize {
        self.blobs.length() + self.commitments.length() + self.proofs.length()
    }

    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self {
            blobs: Decodable::decode(buf)?,
            commitments: Decodable::decode(buf)?,
            proofs: Decodable::decode(buf)?,
        })
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for BlobTransactionSidecar {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let mut arr = [0u8; BYTES_PER_BLOB];
        let blobs: Vec<Blob> = (0..u.int_in_range(1..=16)?)
            .map(|_| {
                arr = arbitrary::Arbitrary::arbitrary(u).unwrap();

                // Ensure that each blob is canonical by ensuring each field element contained in
                // the blob is < BLS_MODULUS
                for i in 0..(FIELD_ELEMENTS_PER_BLOB as usize) {
                    arr[i * BYTES_PER_FIELD_ELEMENT] = 0;
                }

                Blob::from(arr)
            })
            .collect();

        Ok(generate_blob_sidecar(blobs))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for BlobTransactionSidecar {
    type Parameters = ParamsFor<String>;
    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        proptest_vec(proptest_vec(proptest_any::<u8>(), BYTES_PER_BLOB), 1..=5)
            .prop_map(move |blobs| {
                let blobs = blobs
                    .into_iter()
                    .map(|mut blob| {
                        let mut arr = [0u8; BYTES_PER_BLOB];

                        // Ensure that each blob is canonical by ensuring each field element
                        // contained in the blob is < BLS_MODULUS
                        for i in 0..(FIELD_ELEMENTS_PER_BLOB as usize) {
                            blob[i * BYTES_PER_FIELD_ELEMENT] = 0;
                        }

                        arr.copy_from_slice(blob.as_slice());
                        arr.into()
                    })
                    .collect();

                generate_blob_sidecar(blobs)
            })
            .boxed()
    }

    type Strategy = BoxedStrategy<BlobTransactionSidecar>;
}

/// Generates a [`BlobTransactionSidecar`] structure containing blobs, commitments, and proofs.
#[cfg(any(test, feature = "arbitrary"))]
pub fn generate_blob_sidecar(blobs: Vec<Blob>) -> BlobTransactionSidecar {
    let kzg_settings = MAINNET_KZG_TRUSTED_SETUP.clone();

    let commitments: Vec<Bytes48> = blobs
        .iter()
        .map(|blob| KzgCommitment::blob_to_kzg_commitment(&blob.clone(), &kzg_settings).unwrap())
        .map(|commitment| commitment.to_bytes())
        .collect();

    let proofs: Vec<Bytes48> = blobs
        .iter()
        .zip(commitments.iter())
        .map(|(blob, commitment)| {
            KzgProof::compute_blob_kzg_proof(blob, commitment, &kzg_settings).unwrap()
        })
        .map(|proof| proof.to_bytes())
        .collect();

    BlobTransactionSidecar { blobs, commitments, proofs }
}

#[cfg(test)]
mod tests {
    use crate::{
        hex,
        kzg::{Blob, Bytes48},
        transaction::sidecar::generate_blob_sidecar,
        BlobTransactionSidecar,
    };
    use std::{fs, path::PathBuf};

    #[test]
    fn test_blob_transaction_sidecar_generation() {
        // Read the contents of the JSON file into a string.
        let json_content = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/transaction/blob_data/blob1.json"),
        )
        .expect("Failed to read the blob data file");

        // Parse the JSON contents into a serde_json::Value
        let json_value: serde_json::Value =
            serde_json::from_str(&json_content).expect("Failed to deserialize JSON");

        // Extract blob data from JSON and convert it to Blob
        let blobs: Vec<Blob> = vec![Blob::from_hex(
            json_value.get("data").unwrap().as_str().expect("Data is not a valid string"),
        )
        .unwrap()];

        // Generate a BlobTransactionSidecar from the blobs
        let sidecar = generate_blob_sidecar(blobs.clone());

        // Assert commitment equality
        assert_eq!(
            sidecar.commitments,
            vec![
                Bytes48::from_hex(json_value.get("commitment").unwrap().as_str().unwrap()).unwrap()
            ]
        );
    }

    #[test]
    fn test_blob_transaction_sidecar_size() {
        // Vector to store blob data from each file
        let mut blobs: Vec<Blob> = Vec::new();

        // Iterate over each file in the folder
        for entry in fs::read_dir(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/transaction/blob_data/"),
        )
        .expect("Failed to read blob_data folder")
        {
            let entry = entry.expect("Failed to read directory entry");
            let file_path = entry.path();

            // Ensure the entry is a file and not a directory
            if !file_path.is_file() || file_path.extension().unwrap_or_default() != "json" {
                continue
            }

            // Read the contents of the JSON file into a string.
            let json_content =
                fs::read_to_string(file_path).expect("Failed to read the blob data file");

            // Parse the JSON contents into a serde_json::Value
            let json_value: serde_json::Value =
                serde_json::from_str(&json_content).expect("Failed to deserialize JSON");

            // Extract blob data from JSON and convert it to Blob
            if let Some(data) = json_value.get("data") {
                if let Some(data_str) = data.as_str() {
                    if let Ok(blob) = Blob::from_hex(data_str) {
                        blobs.push(blob);
                    }
                }
            }
        }

        // Generate a BlobTransactionSidecar from the blobs
        let sidecar = generate_blob_sidecar(blobs.clone());

        // Assert sidecar size
        assert_eq!(sidecar.size(), 524672);
    }

    #[test]
    fn test_blob_transaction_sidecar_rlp_encode() {
        // Read the contents of the JSON file into a string.
        let json_content = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/transaction/blob_data/blob1.json"),
        )
        .expect("Failed to read the blob data file");

        // Parse the JSON contents into a serde_json::Value
        let json_value: serde_json::Value =
            serde_json::from_str(&json_content).expect("Failed to deserialize JSON");

        // Extract blob data from JSON and convert it to Blob
        let blobs: Vec<Blob> = vec![Blob::from_hex(
            json_value.get("data").unwrap().as_str().expect("Data is not a valid string"),
        )
        .unwrap()];

        // Generate a BlobTransactionSidecar from the blobs
        let sidecar = generate_blob_sidecar(blobs.clone());

        // Create a vector to store the encoded RLP
        let mut encoded_rlp = Vec::new();

        // Encode the inner data of the BlobTransactionSidecar into RLP
        sidecar.encode_inner(&mut encoded_rlp);

        // Assert the equality between the expected RLP from the JSON and the encoded RLP
        assert_eq!(json_value.get("rlp").unwrap().as_str().unwrap(), hex::encode(&encoded_rlp));
    }

    #[test]
    fn test_blob_transaction_sidecar_rlp_decode() {
        // Read the contents of the JSON file into a string.
        let json_content = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/transaction/blob_data/blob1.json"),
        )
        .expect("Failed to read the blob data file");

        // Parse the JSON contents into a serde_json::Value
        let json_value: serde_json::Value =
            serde_json::from_str(&json_content).expect("Failed to deserialize JSON");

        // Extract blob data from JSON and convert it to Blob
        let blobs: Vec<Blob> = vec![Blob::from_hex(
            json_value.get("data").unwrap().as_str().expect("Data is not a valid string"),
        )
        .unwrap()];

        // Generate a BlobTransactionSidecar from the blobs
        let sidecar = generate_blob_sidecar(blobs.clone());

        // Create a vector to store the encoded RLP
        let mut encoded_rlp = Vec::new();

        // Encode the inner data of the BlobTransactionSidecar into RLP
        sidecar.encode_inner(&mut encoded_rlp);

        // Decode the RLP-encoded data back into a BlobTransactionSidecar
        let decoded_sidecar =
            BlobTransactionSidecar::decode_inner(&mut encoded_rlp.as_slice()).unwrap();

        // Assert the equality between the original BlobTransactionSidecar and the decoded one
        assert_eq!(sidecar, decoded_sidecar);
    }
}
