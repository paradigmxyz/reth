use super::*;

/// Blob‑specific extension for `TransactionPool`
#[cfg(feature = "blob")]
use crate::traits::NewBlobSidecar;
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2},
    eip7594::BlobTransactionSidecarVariant,
};

/// Extra API that is available **only** when the `blob` feature is enabled.
///
/// It exposes everything a caller needs to inspect, fetch or delete
/// EIP‑4844 blob sidecars that live in the pool’s `BlobStore`.
///
/// All methods are **read‑only** except `delete_*` and `cleanup_blobs`, which
/// are maintenance helpers.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlobPoolExt: TransactionPool {
    /// Subscribe to notifications whenever a new blob sidecar is stored.
    fn blob_transaction_sidecars_listener(&self) -> Receiver<NewBlobSidecar>;

    /// Return the sidecar for `tx_hash` (if it exists in the store).
    fn get_blob(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;

    /// Fetch sidecars for several hashes; missing ones are simply skipped.
    fn get_all_blobs(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<(TxHash, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError>;

    /// Same as [`get_all_blobs`] but requires *all* blobs to be present.
    fn get_all_blobs_exact(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;

    /// Convenience helper for Engine‑API: return proofs **v1** for the
    /// requested versioned hashes, preserving order and gaps.
    fn get_blobs_for_versioned_hashes_v1(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError>;

    /// Same, but uses the stricter **v2** semantics (all‑or‑none).
    fn get_blobs_for_versioned_hashes_v2(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError>;

    /// Remove a single blob sidecar from the store.
    fn delete_blob(&self, tx: B256);

    /// Remove several sidecars at once.
    fn delete_blobs(&self, txs: Vec<B256>);

    /// Opportunistic GC: drop orphaned sidecars that no pool tx references.
    fn cleanup_blobs(&self);
}
