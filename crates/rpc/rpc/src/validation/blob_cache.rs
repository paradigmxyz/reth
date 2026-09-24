//! Small cache of successfully validated builder-submission blobs.

use super::ValidationApiError;
use alloy_consensus::EnvKzgSettings;
use alloy_eips::eip7594::{BlobTransactionSidecarEip7594, CELLS_PER_EXT_BLOB};
use alloy_primitives::B256;
use alloy_rpc_types_engine::BlobsBundleV2;
use parking_lot::Mutex;
use std::collections::VecDeque;

/// Maximum number of individual blobs retained across competing submissions.
const VALIDATED_BLOB_CACHE_CAPACITY: usize = 15;

/// Reuses KZG validation only for exact V2 blob sidecar matches.
#[derive(Debug, Default)]
pub(super) struct BlobValidationCache {
    /// Every entry contains exactly one validated blob.
    entries: Mutex<VecDeque<BlobTransactionSidecarEip7594>>,
}

impl BlobValidationCache {
    /// Validates a V2 bundle, verifying only blobs absent from the cache.
    pub(super) fn validate(
        &self,
        mut bundle: BlobsBundleV2,
    ) -> Result<Vec<B256>, ValidationApiError> {
        if bundle.blobs.len() != bundle.commitments.len() ||
            bundle.blobs.len().checked_mul(CELLS_PER_EXT_BLOB) != Some(bundle.proofs.len())
        {
            return Err(ValidationApiError::InvalidBlobsBundle)
        }

        let versioned_hashes = bundle.versioned_hashes();
        let hits = {
            let entries = self.entries.lock();
            (0..bundle.blobs.len())
                .map(|index| {
                    let proofs = &bundle.proofs
                        [index * CELLS_PER_EXT_BLOB..(index + 1) * CELLS_PER_EXT_BLOB];
                    entries.iter().any(|cached| {
                        cached.commitments[0] == bundle.commitments[index] &&
                            cached.cell_proofs == proofs &&
                            cached.blobs[0] == bundle.blobs[index]
                    })
                })
                .collect::<Vec<_>>()
        };
        if hits.iter().all(|hit| *hit) {
            return Ok(versioned_hashes)
        }

        let mut missing = Vec::new();
        for hit in hits {
            let sidecar = bundle.pop_sidecar(1);
            if !hit {
                missing.push(sidecar);
            }
        }

        let sidecar = BlobsBundleV2::new(missing)
            .try_into_sidecar()
            .map_err(|_| ValidationApiError::InvalidBlobsBundle)?;
        let missing_hashes = sidecar.versioned_hashes().collect::<Vec<_>>();
        sidecar.validate(&missing_hashes, EnvKzgSettings::default().get())?;

        for index in 0..sidecar.blobs.len() {
            let proofs =
                &sidecar.cell_proofs[index * CELLS_PER_EXT_BLOB..(index + 1) * CELLS_PER_EXT_BLOB];
            self.insert(BlobTransactionSidecarEip7594::new(
                vec![sidecar.blobs[index]],
                vec![sidecar.commitments[index]],
                proofs.to_vec(),
            ));
        }

        Ok(versioned_hashes)
    }

    fn insert(&self, sidecar: BlobTransactionSidecarEip7594) {
        let mut entries = self.entries.lock();
        if entries
            .iter()
            .any(|cached| cached.commitments == sidecar.commitments && cached == &sidecar)
        {
            return;
        }
        if entries.len() == VALIDATED_BLOB_CACHE_CAPACITY {
            entries.pop_front();
        }
        entries.push_back(sidecar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Blob, BlobTransactionSidecar, Bytes48, SidecarBuilder, SimpleCoder};
    use alloy_rpc_types_engine::BlobsBundleV1;

    fn valid_v1_bundle(data: &[u8]) -> BlobsBundleV1 {
        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(data);
        BlobsBundleV1::new([builder.build::<BlobTransactionSidecar>().unwrap()])
    }

    fn valid_v2_bundle(data: &[u8]) -> BlobsBundleV2 {
        valid_v1_bundle(data).try_into_v2().unwrap()
    }

    #[test]
    fn mixed_bundle_validates_only_missing_blobs() {
        let cache = BlobValidationCache::default();
        let mut bundle = valid_v2_bundle(b"first blob");
        let second = valid_v2_bundle(b"second blob");
        cache.validate(valid_v2_bundle(b"first blob")).unwrap();

        bundle.blobs.extend(second.blobs);
        bundle.commitments.extend(second.commitments);
        bundle.proofs.extend(second.proofs);
        assert_eq!(cache.validate(bundle.clone()).unwrap(), bundle.versioned_hashes());
        assert_eq!(cache.entries.lock().len(), 2);
    }

    #[test]
    fn v2_reuses_only_exact_validated_blobs() {
        let cache = BlobValidationCache::default();
        let hashes = valid_v2_bundle(b"peer das blob").versioned_hashes();

        assert_eq!(cache.validate(valid_v2_bundle(b"peer das blob")).unwrap(), hashes);
        assert_eq!(cache.validate(valid_v2_bundle(b"peer das blob")).unwrap(), hashes);
        assert_eq!(cache.entries.lock().len(), 1);

        let mut changed_blob = valid_v2_bundle(b"peer das blob");
        changed_blob.blobs[0].0[0] ^= 1;
        assert!(cache.validate(changed_blob).is_err());

        let mut changed_proof = valid_v2_bundle(b"peer das blob");
        changed_proof.proofs[0].0[0] ^= 1;
        assert!(cache.validate(changed_proof).is_err());
        assert_eq!(cache.entries.lock().len(), 1);
    }

    #[test]
    fn validates_uncached_blob_even_when_another_hits() {
        let cache = BlobValidationCache::default();
        let mut bundle = valid_v2_bundle(b"first blob");
        let mut second = valid_v2_bundle(b"second blob");
        cache.validate(valid_v2_bundle(b"first blob")).unwrap();

        second.proofs[0].0[0] ^= 1;
        bundle.blobs.extend(second.blobs);
        bundle.commitments.extend(second.commitments);
        bundle.proofs.extend(second.proofs);
        assert!(cache.validate(bundle).is_err());
        assert_eq!(cache.entries.lock().len(), 1);
    }

    #[test]
    fn rejects_incomplete_bundles_without_changing_cache() {
        let cache = BlobValidationCache::default();
        let mut v2 = valid_v2_bundle(b"blob");
        v2.proofs.pop();
        assert!(matches!(cache.validate(v2), Err(ValidationApiError::InvalidBlobsBundle)));
        assert!(cache.entries.lock().is_empty());
    }

    #[test]
    fn retains_only_fifteen_recent_blobs() {
        let cache = BlobValidationCache::default();
        let blob = Blob::default();
        for index in 0..=VALIDATED_BLOB_CACHE_CAPACITY {
            cache.insert(BlobTransactionSidecarEip7594::new(
                vec![blob],
                vec![Bytes48::repeat_byte(index as u8)],
                vec![Bytes48::ZERO; CELLS_PER_EXT_BLOB],
            ));
        }

        let entries = cache.entries.lock();
        assert_eq!(entries.len(), VALIDATED_BLOB_CACHE_CAPACITY);
        assert_eq!(entries.front().unwrap().commitments[0], Bytes48::repeat_byte(1));
    }
}
