use alloc::{sync::Arc, vec::Vec};
use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use reth_storage_errors::provider::ProviderResult;

/// Store for Block Access Lists (BALs).
///
/// This abstraction intentionally does not prescribe where BALs live. Implementations may keep
/// recent BALs in memory, read canonical BALs from static files, or compose multiple tiers behind
/// a single interface.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BalStore: Send + Sync + 'static {
    /// Insert the BAL for the given block.
    fn insert(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> ProviderResult<()>;

    /// Fetch BALs for the given block hashes.
    ///
    /// The returned vector must align with `block_hashes`.
    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>>;

    /// Fetch BALs for the requested range.
    ///
    /// Implementations may stop at the first gap and return the contiguous prefix.
    fn get_by_range(&self, start: BlockNumber, count: u64) -> ProviderResult<Vec<Bytes>>;
}

/// Clone-friendly façade around a BAL store implementation.
#[derive(Clone)]
pub struct BalStoreHandle {
    inner: Arc<dyn BalStore>,
}

impl BalStoreHandle {
    /// Creates a new [`BalStoreHandle`] from the given implementation.
    pub fn new(inner: impl BalStore) -> Self {
        Self { inner: Arc::new(inner) }
    }

    /// Creates a [`BalStoreHandle`] backed by [`NoopBalStore`].
    pub fn noop() -> Self {
        Self::new(NoopBalStore)
    }

    /// Insert the BAL for the given block.
    #[inline]
    pub fn insert(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> ProviderResult<()> {
        self.inner.insert(block_hash, block_number, bal)
    }

    /// Fetch BALs for the given block hashes.
    #[inline]
    pub fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        self.inner.get_by_hashes(block_hashes)
    }

    /// Fetch BALs for the requested range.
    #[inline]
    pub fn get_by_range(&self, start: BlockNumber, count: u64) -> ProviderResult<Vec<Bytes>> {
        self.inner.get_by_range(start, count)
    }
}

impl Default for BalStoreHandle {
    fn default() -> Self {
        Self::noop()
    }
}

impl core::fmt::Debug for BalStoreHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BalStoreHandle").finish_non_exhaustive()
    }
}

/// Provider-side access to BAL storage.
#[auto_impl::auto_impl(&, Arc)]
pub trait BalProvider {
    /// Returns the configured BAL store handle.
    fn bal_store(&self) -> &BalStoreHandle;
}

/// No-op BAL store used as the default wiring target until a concrete implementation is injected.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopBalStore;

impl BalStore for NoopBalStore {
    fn insert(
        &self,
        _block_hash: BlockHash,
        _block_number: BlockNumber,
        _bal: Bytes,
    ) -> ProviderResult<()> {
        Ok(())
    }

    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        Ok(block_hashes.iter().map(|_| None).collect())
    }

    fn get_by_range(&self, _start: BlockNumber, _count: u64) -> ProviderResult<Vec<Bytes>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn noop_store_returns_empty_results() {
        let store = BalStoreHandle::default();
        let hashes = [B256::random(), B256::random()];

        let by_hash = store.get_by_hashes(&hashes).unwrap();
        let by_range = store.get_by_range(1, 10).unwrap();

        assert_eq!(by_hash, vec![None, None]);
        assert!(by_range.is_empty());
    }
}
