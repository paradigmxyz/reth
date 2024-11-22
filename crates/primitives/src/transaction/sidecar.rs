#![cfg_attr(docsrs, doc(cfg(feature = "c-kzg")))]

use crate::{Transaction, TransactionSigned};
use alloy_consensus::{transaction::RlpEcdsaTx, Signed, TxEip4844WithSidecar};
use alloy_eips::eip4844::BlobTransactionSidecar;
use derive_more::Deref;
use serde::{Deserialize, Serialize};

/// A response to `GetPooledTransactions` that includes blob data, their commitments, and their
/// corresponding proofs.
///
/// This is defined in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#networking) as an element
/// of a `PooledTransactions` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Deref)]
pub struct BlobTransaction(pub Signed<TxEip4844WithSidecar>);

impl BlobTransaction {
    /// Constructs a new [`BlobTransaction`] from a [`TransactionSigned`] and a
    /// [`BlobTransactionSidecar`].
    ///
    /// Returns an error if the signed transaction is not [`Transaction::Eip4844`]
    pub fn try_from_signed(
        tx: TransactionSigned,
        sidecar: BlobTransactionSidecar,
    ) -> Result<Self, (TransactionSigned, BlobTransactionSidecar)> {
        let hash = tx.hash();
        let TransactionSigned { transaction, signature, .. } = tx;
        match transaction {
            Transaction::Eip4844(transaction) => Ok(Self(Signed::new_unchecked(
                TxEip4844WithSidecar { tx: transaction, sidecar },
                signature,
                hash,
            ))),
            transaction => {
                let tx = TransactionSigned::new(transaction, signature, hash);
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
    ) -> Result<(), alloy_eips::eip4844::BlobTransactionValidationError> {
        self.tx().validate_blob(proof_settings)
    }

    /// Splits the [`BlobTransaction`] into its [`TransactionSigned`] and [`BlobTransactionSidecar`]
    /// components.
    pub fn into_parts(self) -> (TransactionSigned, BlobTransactionSidecar) {
        let (transaction, signature, hash) = self.0.into_parts();
        let (transaction, sidecar) = transaction.into_parts();
        let transaction = TransactionSigned::new(transaction.into(), signature, hash);
        (transaction, sidecar)
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
            TxEip4844WithSidecar::rlp_decode_signed(data)?.into_parts();
        Ok(Self(Signed::new_unchecked(transaction, signature, hash)))
    }
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
        let sidecar = BlobTransactionSidecar::try_from_blobs(blobs).unwrap();

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
        let sidecar = BlobTransactionSidecar::try_from_blobs(blobs).unwrap();

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
        let sidecar = BlobTransactionSidecar::try_from_blobs(blobs).unwrap();

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
        let sidecar = BlobTransactionSidecar::try_from_blobs(blobs).unwrap();

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
