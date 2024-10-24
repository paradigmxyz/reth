#![cfg_attr(docsrs, doc(cfg(feature = "c-kzg")))]

use crate::{Signature, Transaction, TransactionSigned};
use alloy_consensus::{constants::EIP4844_TX_TYPE_ID, TxEip4844WithSidecar};
use alloy_primitives::TxHash;
use alloy_rlp::Header;
use serde::{Deserialize, Serialize};

#[doc(inline)]
pub use alloy_eips::eip4844::BlobTransactionSidecar;

#[cfg(feature = "c-kzg")]
pub use alloy_eips::eip4844::BlobTransactionValidationError;

/// A response to `GetPooledTransactions` that includes blob data, their commitments, and their
/// corresponding proofs.
///
/// This is defined in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#networking) as an element
/// of a `PooledTransactions` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobTransaction {
    /// The transaction hash.
    pub hash: TxHash,
    /// The transaction signature.
    pub signature: Signature,
    /// The transaction payload with the sidecar.
    #[serde(flatten)]
    pub transaction: TxEip4844WithSidecar,
}

impl BlobTransaction {
    /// Constructs a new [`BlobTransaction`] from a [`TransactionSigned`] and a
    /// [`BlobTransactionSidecar`].
    ///
    /// Returns an error if the signed transaction is not [`Transaction::Eip4844`]
    pub fn try_from_signed(
        tx: TransactionSigned,
        sidecar: BlobTransactionSidecar,
    ) -> Result<Self, (TransactionSigned, BlobTransactionSidecar)> {
        let TransactionSigned { transaction, signature, hash } = tx;
        match transaction {
            Transaction::Eip4844(transaction) => Ok(Self {
                hash,
                transaction: TxEip4844WithSidecar { tx: transaction, sidecar },
                signature,
            }),
            transaction => {
                let tx = TransactionSigned { transaction, signature, hash };
                Err((tx, sidecar))
            }
        }
    }

    /// Verifies that the transaction's blob data, commitments, and proofs are all valid.
    ///
    /// See also [`alloy_consensus::TxEip4844::validate_blob`]
    #[cfg(feature = "c-kzg")]
    pub fn validate(
        &self,
        proof_settings: &c_kzg::KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        self.transaction.validate_blob(proof_settings)
    }

    /// Splits the [`BlobTransaction`] into its [`TransactionSigned`] and [`BlobTransactionSidecar`]
    /// components.
    pub fn into_parts(self) -> (TransactionSigned, BlobTransactionSidecar) {
        let transaction = TransactionSigned {
            transaction: Transaction::Eip4844(self.transaction.tx),
            hash: self.hash,
            signature: self.signature,
        };

        (transaction, self.transaction.sidecar)
    }

    /// Encodes the [`BlobTransaction`] fields as RLP, with a tx type. If `with_header` is `false`,
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

    /// Encodes the [`BlobTransaction`] fields as RLP, with the following format:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP encoding methods, and does not
    /// represent the full RLP encoding of the blob transaction.
    pub(crate) fn encode_inner(&self, out: &mut dyn bytes::BufMut) {
        self.transaction.encode_with_signature_fields(&self.signature, out);
    }

    /// Outputs the length of the RLP encoding of the blob transaction, including the tx type byte,
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
            payload_length: self.transaction.tx.fields_len() + self.signature.rlp_vrs_len(),
        };

        let tx_length = tx_header.length() + tx_header.payload_length;

        // The payload length is the length of the `tranascation_payload_body` list, plus the
        // length of the blobs, commitments, and proofs.
        let payload_length = tx_length + self.transaction.sidecar.rlp_encoded_fields_length();

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

    /// Decodes a [`BlobTransaction`] from RLP. This expects the encoding to be:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP decoding methods, and does not
    /// represent the full RLP decoding of the `PooledTransactionsElement` type.
    pub(crate) fn decode_inner(data: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let (transaction, signature, hash) =
            TxEip4844WithSidecar::decode_signed_fields(data)?.into_parts();

        Ok(Self { transaction, hash, signature })
    }
}

/// Generates a [`BlobTransactionSidecar`] structure containing blobs, commitments, and proofs.
#[cfg(all(feature = "c-kzg", any(test, feature = "arbitrary")))]
pub fn generate_blob_sidecar(blobs: Vec<c_kzg::Blob>) -> BlobTransactionSidecar {
    use alloc::vec::Vec;
    use alloy_eips::eip4844::env_settings::EnvKzgSettings;
    use c_kzg::{KzgCommitment, KzgProof};

    let kzg_settings = EnvKzgSettings::Default;

    let commitments: Vec<c_kzg::Bytes48> = blobs
        .iter()
        .map(|blob| {
            KzgCommitment::blob_to_kzg_commitment(&blob.clone(), kzg_settings.get()).unwrap()
        })
        .map(|commitment| commitment.to_bytes())
        .collect();

    let proofs: Vec<c_kzg::Bytes48> = blobs
        .iter()
        .zip(commitments.iter())
        .map(|(blob, commitment)| {
            KzgProof::compute_blob_kzg_proof(blob, commitment, kzg_settings.get()).unwrap()
        })
        .map(|proof| proof.to_bytes())
        .collect();

    BlobTransactionSidecar::from_kzg(blobs, commitments, proofs)
}

#[cfg(all(test, feature = "c-kzg"))]
mod tests {
    use super::*;
    use crate::{kzg::Blob, PooledTransactionsElement};
    use alloc::vec::Vec;
    use alloy_eips::{
        eip2718::{Decodable2718, Encodable2718},
        eip4844::Bytes48,
    };
    use alloy_primitives::hex;
    use std::{fs, path::PathBuf, str::FromStr};

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
        let sidecar = generate_blob_sidecar(blobs);

        // Assert commitment equality
        assert_eq!(
            sidecar.commitments,
            vec![
                Bytes48::from_str(json_value.get("commitment").unwrap().as_str().unwrap()).unwrap()
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
        let sidecar = generate_blob_sidecar(blobs);

        // Create a vector to store the encoded RLP
        let mut encoded_rlp = Vec::new();

        // Encode the inner data of the BlobTransactionSidecar into RLP
        sidecar.rlp_encode_fields(&mut encoded_rlp);

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
        let sidecar = generate_blob_sidecar(blobs);

        // Create a vector to store the encoded RLP
        let mut encoded_rlp = Vec::new();

        // Encode the inner data of the BlobTransactionSidecar into RLP
        sidecar.rlp_encode_fields(&mut encoded_rlp);

        // Decode the RLP-encoded data back into a BlobTransactionSidecar
        let decoded_sidecar =
            BlobTransactionSidecar::rlp_decode_fields(&mut encoded_rlp.as_slice()).unwrap();

        // Assert the equality between the original BlobTransactionSidecar and the decoded one
        assert_eq!(sidecar, decoded_sidecar);
    }

    #[test]
    fn decode_encode_raw_4844_rlp() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/4844rlp");
        let dir = fs::read_dir(path).expect("Unable to read folder");
        for entry in dir {
            let entry = entry.unwrap();
            let content = fs::read_to_string(entry.path()).unwrap();
            let raw = hex::decode(content.trim()).unwrap();
            let tx = PooledTransactionsElement::decode_2718(&mut raw.as_ref())
                .map_err(|err| {
                    panic!("Failed to decode transaction: {:?} {:?}", err, entry.path());
                })
                .unwrap();
            // We want to test only EIP-4844 transactions
            assert!(tx.is_eip4844());
            let encoded = tx.encoded_2718();
            assert_eq!(encoded.as_slice(), &raw[..], "{:?}", entry.path());
        }
    }
}
