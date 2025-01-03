use alloy_eips::eip4844::{BlobAndProofV1, BlobTransactionSidecar};
use alloy_primitives::B256;

/// Helper function to match versioned hashes and construct `BlobAndProofV1` objects.
pub(crate) fn match_versioned_hashes(
    blob_sidecar: &BlobTransactionSidecar,
    versioned_hashes: &[B256],
) -> Vec<Option<BlobAndProofV1>> {
    let mut result = vec![None; versioned_hashes.len()];
    for (i, blob_versioned_hash) in blob_sidecar.versioned_hashes().enumerate() {
        for (j, target_versioned_hash) in versioned_hashes.iter().enumerate() {
            if blob_versioned_hash == *target_versioned_hash {
                result[j].get_or_insert_with(|| BlobAndProofV1 {
                    blob: Box::new(blob_sidecar.blobs[i]),
                    proof: blob_sidecar.proofs[i],
                });
            }
        }
    }
    result
}
