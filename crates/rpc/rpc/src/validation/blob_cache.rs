//! Small cache of successfully validated builder-submission blobs.

use super::ValidationApiError;
use alloy_consensus::{Blob, Bytes48, EnvKzgSettings};
use alloy_eips::eip7594::CELLS_PER_EXT_BLOB;
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::BlobsBundleV2;
use itertools::izip;
use parking_lot::Mutex;
use std::collections::VecDeque;

/// Maximum number of individual blobs retained across competing submissions.
const VALIDATED_BLOB_CACHE_CAPACITY: usize = 12;

/// Reuses KZG validation only for exact blob, commitment, and cell-proof matches.
#[derive(Debug, Default)]
pub(super) struct BlobValidationCache {
    entries: Mutex<VecDeque<ValidatedBlob>>,
}

impl BlobValidationCache {
    /// Validates a V2 bundle, verifying only blobs absent from the cache.
    pub(super) fn validate(&self, bundle: BlobsBundleV2) -> Result<Vec<B256>, ValidationApiError> {
        let mut sidecar =
            bundle.try_into_sidecar().map_err(|_| ValidationApiError::InvalidBlobsBundle)?;
        let versioned_hashes = sidecar.versioned_hashes().collect();
        let hits = {
            let entries = self.entries.lock();
            izip!(
                &sidecar.blobs,
                &sidecar.commitments,
                sidecar.cell_proofs.as_chunks::<CELLS_PER_EXT_BLOB>().0
            )
            .map(|(blob, commitment, proofs)| {
                entries.iter().any(|entry| entry.matches(blob, commitment, proofs))
            })
            .collect::<Vec<_>>()
        };

        Self::retain_misses(&mut sidecar.blobs, &hits, 1);
        Self::retain_misses(&mut sidecar.commitments, &hits, 1);
        Self::retain_misses(&mut sidecar.cell_proofs, &hits, CELLS_PER_EXT_BLOB);

        if !sidecar.blobs.is_empty() {
            let miss_hashes = sidecar.versioned_hashes().collect::<Vec<_>>();
            sidecar.validate(&miss_hashes, EnvKzgSettings::default().get())?;

            let mut entries = self.entries.lock();
            for (blob, commitment, proofs) in izip!(
                &sidecar.blobs,
                &sidecar.commitments,
                sidecar.cell_proofs.as_chunks::<CELLS_PER_EXT_BLOB>().0
            ) {
                Self::insert(&mut entries, blob, commitment, proofs);
            }
        }

        Ok(versioned_hashes)
    }

    fn retain_misses<T>(items: &mut Vec<T>, hits: &[bool], per_blob: usize) {
        let mut keep = hits.iter().flat_map(|hit| std::iter::repeat_n(!hit, per_blob));
        items.retain(|_| keep.next().expect("validated sidecar lengths match"));
    }

    fn insert(
        entries: &mut VecDeque<ValidatedBlob>,
        blob: &Blob,
        commitment: &Bytes48,
        cell_proofs: &[Bytes48],
    ) {
        if entries.iter().any(|entry| entry.matches(blob, commitment, cell_proofs)) {
            return;
        }
        if entries.len() == VALIDATED_BLOB_CACHE_CAPACITY {
            entries.pop_front();
        }
        entries.push_back(ValidatedBlob {
            blob: Bytes::copy_from_slice(blob.as_slice()),
            commitment: *commitment,
            cell_proofs: cell_proofs.to_vec(),
        });
    }
}

/// A validated V2 blob with its commitment and cell proofs.
#[derive(Debug)]
struct ValidatedBlob {
    blob: Bytes,
    commitment: Bytes48,
    cell_proofs: Vec<Bytes48>,
}

impl ValidatedBlob {
    fn matches(&self, blob: &Blob, commitment: &Bytes48, cell_proofs: &[Bytes48]) -> bool {
        self.commitment == *commitment &&
            self.cell_proofs == cell_proofs &&
            self.blob.as_ref() == blob.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{BlobTransactionSidecar, SidecarBuilder, SimpleCoder};
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
    fn retains_only_twelve_recent_blobs() {
        let mut entries = VecDeque::new();
        let blob = Blob::default();
        for index in 0..=VALIDATED_BLOB_CACHE_CAPACITY {
            BlobValidationCache::insert(
                &mut entries,
                &blob,
                &Bytes48::repeat_byte(index as u8),
                &[Bytes48::ZERO; CELLS_PER_EXT_BLOB],
            );
        }

        assert_eq!(entries.len(), VALIDATED_BLOB_CACHE_CAPACITY);
        assert_eq!(entries.front().unwrap().commitment, Bytes48::repeat_byte(1));
    }
}
