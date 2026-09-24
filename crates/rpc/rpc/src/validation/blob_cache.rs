//! Small cache of successfully validated builder-submission blobs.

use super::ValidationApiError;
use alloy_consensus::{Blob, Bytes48, EnvKzgSettings};
use alloy_eips::eip7594::CELLS_PER_EXT_BLOB;
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{BlobsBundleV1, BlobsBundleV2};
use parking_lot::Mutex;
use std::collections::VecDeque;

/// Maximum number of individual blobs retained across competing submissions.
const VALIDATED_BLOB_CACHE_CAPACITY: usize = 12;

/// Reuses KZG validation only for exact blob, commitment, and proof matches.
#[derive(Debug, Default)]
pub(super) struct BlobValidationCache {
    entries: Mutex<VecDeque<ValidatedBlob>>,
}

/// A validated tuple includes the proof format, so a matching commitment alone cannot skip
/// verification of different blob data or proofs.
#[derive(Debug)]
enum ValidatedBlob {
    V1 { blob: Bytes, commitment: Bytes48, proof: Bytes48 },
    V2 { blob: Bytes, commitment: Bytes48, proofs: Vec<Bytes48> },
}

impl BlobValidationCache {
    /// Validates a V1 bundle, verifying only blobs absent from the cache.
    pub(super) fn validate_v1(
        &self,
        bundle: BlobsBundleV1,
    ) -> Result<Vec<B256>, ValidationApiError> {
        if bundle.blobs.len() != bundle.commitments.len() ||
            bundle.blobs.len() != bundle.proofs.len()
        {
            return Err(ValidationApiError::InvalidBlobsBundle)
        }

        let versioned_hashes = bundle.versioned_hashes();
        let hits = {
            let mut entries = self.entries.lock();
            bundle
                .blobs
                .iter()
                .zip(&bundle.commitments)
                .zip(&bundle.proofs)
                .map(|((blob, commitment), proof)| {
                    Self::contains_v1(&mut entries, blob, commitment, proof)
                })
                .collect::<Vec<_>>()
        };

        let (misses, miss_hashes) = Self::v1_misses(bundle, hits, &versioned_hashes);

        if misses.blobs.is_empty() {
            return Ok(versioned_hashes)
        }

        let sidecar =
            misses.try_into_sidecar().map_err(|_| ValidationApiError::InvalidBlobsBundle)?;
        sidecar.validate(&miss_hashes, EnvKzgSettings::default().get())?;

        let mut entries = self.entries.lock();
        for ((blob, commitment), proof) in
            sidecar.blobs.iter().zip(&sidecar.commitments).zip(&sidecar.proofs)
        {
            if !Self::contains_v1(&mut entries, blob, commitment, proof) {
                Self::insert(
                    &mut entries,
                    ValidatedBlob::V1 {
                        blob: Bytes::copy_from_slice(blob.as_slice()),
                        commitment: *commitment,
                        proof: *proof,
                    },
                );
            }
        }

        Ok(versioned_hashes)
    }

    /// Validates a V2 bundle, verifying only blobs absent from the cache.
    pub(super) fn validate_v2(
        &self,
        bundle: BlobsBundleV2,
    ) -> Result<Vec<B256>, ValidationApiError> {
        if bundle.blobs.len() != bundle.commitments.len() ||
            bundle.blobs.len().checked_mul(CELLS_PER_EXT_BLOB) != Some(bundle.proofs.len())
        {
            return Err(ValidationApiError::InvalidBlobsBundle)
        }

        let versioned_hashes = bundle.versioned_hashes();
        let hits = {
            let mut entries = self.entries.lock();
            bundle
                .blobs
                .iter()
                .zip(&bundle.commitments)
                .zip(bundle.proofs.as_chunks::<CELLS_PER_EXT_BLOB>().0)
                .map(|((blob, commitment), proofs)| {
                    Self::contains_v2(&mut entries, blob, commitment, proofs)
                })
                .collect::<Vec<_>>()
        };

        let (misses, miss_hashes) = Self::v2_misses(bundle, hits, &versioned_hashes);

        if misses.blobs.is_empty() {
            return Ok(versioned_hashes)
        }

        let sidecar =
            misses.try_into_sidecar().map_err(|_| ValidationApiError::InvalidBlobsBundle)?;
        sidecar.validate(&miss_hashes, EnvKzgSettings::default().get())?;

        let mut entries = self.entries.lock();
        for ((blob, commitment), proofs) in sidecar
            .blobs
            .iter()
            .zip(&sidecar.commitments)
            .zip(sidecar.cell_proofs.as_chunks::<CELLS_PER_EXT_BLOB>().0)
        {
            if !Self::contains_v2(&mut entries, blob, commitment, proofs) {
                Self::insert(
                    &mut entries,
                    ValidatedBlob::V2 {
                        blob: Bytes::copy_from_slice(blob.as_slice()),
                        commitment: *commitment,
                        proofs: proofs.to_vec(),
                    },
                );
            }
        }

        Ok(versioned_hashes)
    }

    fn v1_misses(
        mut bundle: BlobsBundleV1,
        hits: Vec<bool>,
        versioned_hashes: &[B256],
    ) -> (BlobsBundleV1, Vec<B256>) {
        let miss_hashes = versioned_hashes
            .iter()
            .zip(&hits)
            .filter_map(|(hash, hit)| (!hit).then_some(*hash))
            .collect();

        let mut index = 0;
        bundle.blobs.retain(|_| {
            let keep = !hits[index];
            index += 1;
            keep
        });
        let mut index = 0;
        bundle.commitments.retain(|_| {
            let keep = !hits[index];
            index += 1;
            keep
        });
        let mut index = 0;
        bundle.proofs.retain(|_| {
            let keep = !hits[index];
            index += 1;
            keep
        });

        (bundle, miss_hashes)
    }

    fn v2_misses(
        mut bundle: BlobsBundleV2,
        hits: Vec<bool>,
        versioned_hashes: &[B256],
    ) -> (BlobsBundleV2, Vec<B256>) {
        let miss_hashes = versioned_hashes
            .iter()
            .zip(&hits)
            .filter_map(|(hash, hit)| (!hit).then_some(*hash))
            .collect();

        let mut index = 0;
        bundle.blobs.retain(|_| {
            let keep = !hits[index];
            index += 1;
            keep
        });
        let mut index = 0;
        bundle.commitments.retain(|_| {
            let keep = !hits[index];
            index += 1;
            keep
        });
        let mut index = 0;
        bundle.proofs.retain(|_| {
            let keep = !hits[index / CELLS_PER_EXT_BLOB];
            index += 1;
            keep
        });

        (bundle, miss_hashes)
    }

    fn contains_v1(
        entries: &mut VecDeque<ValidatedBlob>,
        blob: &Blob,
        commitment: &Bytes48,
        proof: &Bytes48,
    ) -> bool {
        let blob_bytes = blob.as_slice();
        let Some(index) = entries.iter().position(|entry| {
            matches!(entry, ValidatedBlob::V1 { blob: cached_blob, commitment: cached_commitment, proof: cached_proof }
                if cached_commitment == commitment && cached_proof == proof && cached_blob.as_ref() == blob_bytes)
        }) else {
            return false
        };
        let entry = entries.remove(index).expect("cache entry at a valid index");
        entries.push_back(entry);
        true
    }

    fn contains_v2(
        entries: &mut VecDeque<ValidatedBlob>,
        blob: &Blob,
        commitment: &Bytes48,
        proofs: &[Bytes48],
    ) -> bool {
        let blob_bytes = blob.as_slice();
        let Some(index) = entries.iter().position(|entry| {
            matches!(entry, ValidatedBlob::V2 { blob: cached_blob, commitment: cached_commitment, proofs: cached_proofs }
                if cached_commitment == commitment && cached_proofs == proofs && cached_blob.as_ref() == blob_bytes)
        }) else {
            return false
        };
        let entry = entries.remove(index).expect("cache entry at a valid index");
        entries.push_back(entry);
        true
    }

    fn insert(entries: &mut VecDeque<ValidatedBlob>, entry: ValidatedBlob) {
        if entries.len() == VALIDATED_BLOB_CACHE_CAPACITY {
            entries.pop_front();
        }
        entries.push_back(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{BlobTransactionSidecar, SidecarBuilder, SimpleCoder};

    fn valid_v1_bundle(data: &[u8]) -> BlobsBundleV1 {
        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(data);
        BlobsBundleV1::new([builder.build::<BlobTransactionSidecar>().unwrap()])
    }

    fn valid_v2_bundle(data: &[u8]) -> BlobsBundleV2 {
        valid_v1_bundle(data).try_into_v2().unwrap()
    }

    #[test]
    fn v1_reuses_only_exact_validated_blobs() {
        let cache = BlobValidationCache::default();
        let hashes = valid_v1_bundle(b"first blob").versioned_hashes();

        assert_eq!(cache.validate_v1(valid_v1_bundle(b"first blob")).unwrap(), hashes);
        assert_eq!(cache.validate_v1(valid_v1_bundle(b"first blob")).unwrap(), hashes);
        assert_eq!(cache.entries.lock().len(), 1);

        let mut changed_blob = valid_v1_bundle(b"first blob");
        changed_blob.blobs[0].0[0] ^= 1;
        assert!(cache.validate_v1(changed_blob).is_err());

        let mut changed_proof = valid_v1_bundle(b"first blob");
        changed_proof.proofs[0].0[0] ^= 1;
        assert!(cache.validate_v1(changed_proof).is_err());
        assert_eq!(cache.entries.lock().len(), 1);
    }

    #[test]
    fn v1_validates_misses_even_when_other_blobs_hit() {
        let cache = BlobValidationCache::default();
        let mut first = valid_v1_bundle(b"first blob");
        let mut second = valid_v1_bundle(b"second blob");
        cache.validate_v1(valid_v1_bundle(b"first blob")).unwrap();

        second.proofs[0].0[0] ^= 1;
        first.commitments.extend(second.commitments);
        first.proofs.extend(second.proofs);
        first.blobs.extend(second.blobs);
        assert!(cache.validate_v1(first).is_err());
        assert_eq!(cache.entries.lock().len(), 1);
    }

    #[test]
    fn v2_reuses_only_exact_validated_blobs() {
        let cache = BlobValidationCache::default();
        let hashes = valid_v2_bundle(b"peer das blob").versioned_hashes();

        assert_eq!(cache.validate_v2(valid_v2_bundle(b"peer das blob")).unwrap(), hashes);
        assert_eq!(cache.validate_v2(valid_v2_bundle(b"peer das blob")).unwrap(), hashes);
        assert_eq!(cache.entries.lock().len(), 1);

        let mut changed_blob = valid_v2_bundle(b"peer das blob");
        changed_blob.blobs[0].0[0] ^= 1;
        assert!(cache.validate_v2(changed_blob).is_err());

        let mut changed_proof = valid_v2_bundle(b"peer das blob");
        changed_proof.proofs[0].0[0] ^= 1;
        assert!(cache.validate_v2(changed_proof).is_err());
        assert_eq!(cache.entries.lock().len(), 1);
    }

    #[test]
    fn v1_and_v2_proofs_do_not_share_entries() {
        let cache = BlobValidationCache::default();
        let v1 = valid_v1_bundle(b"same blob");
        let v2 = valid_v2_bundle(b"same blob");

        cache.validate_v1(v1).unwrap();
        cache.validate_v2(v2).unwrap();
        assert_eq!(cache.entries.lock().len(), 2);
    }

    #[test]
    fn rejects_incomplete_bundles_without_changing_cache() {
        let cache = BlobValidationCache::default();
        let mut v1 = valid_v1_bundle(b"blob");
        v1.proofs.clear();
        assert!(matches!(cache.validate_v1(v1), Err(ValidationApiError::InvalidBlobsBundle)));

        let mut v2 = valid_v2_bundle(b"blob");
        v2.proofs.pop();
        assert!(matches!(cache.validate_v2(v2), Err(ValidationApiError::InvalidBlobsBundle)));
        assert!(cache.entries.lock().is_empty());
    }

    #[test]
    fn retains_only_twelve_recent_blobs() {
        let mut entries = VecDeque::new();
        for index in 0..=VALIDATED_BLOB_CACHE_CAPACITY {
            BlobValidationCache::insert(
                &mut entries,
                ValidatedBlob::V1 {
                    blob: Bytes::new(),
                    commitment: Bytes48::repeat_byte(index as u8),
                    proof: Bytes48::ZERO,
                },
            );
        }

        assert_eq!(entries.len(), VALIDATED_BLOB_CACHE_CAPACITY);
        assert!(
            matches!(entries.front(), Some(ValidatedBlob::V1 { commitment, .. }) if *commitment == Bytes48::repeat_byte(1))
        );
    }
}
