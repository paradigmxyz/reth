use alloc::{sync::Arc, vec::Vec};
use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use reth_storage_errors::provider::ProviderResult;

#[cfg(feature = "std")]
pub use self::subscriptions::{
    BalNotification, BalNotificationSender, BalNotificationStream, BalNotifications,
};

#[cfg(feature = "std")]
mod subscriptions {
    use super::{BlockHash, BlockNumber, Bytes};
    use core::{
        pin::Pin,
        task::{ready, Context, Poll},
    };
    use tokio::sync::broadcast;
    use tokio_stream::{
        wrappers::{errors::BroadcastStreamRecvError, BroadcastStream},
        Stream,
    };

    /// Notification emitted when a new BAL is inserted into the store.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct BalNotification {
        /// Hash of the block the BAL belongs to.
        pub block_hash: BlockHash,
        /// Number of the block the BAL belongs to.
        pub block_number: BlockNumber,
        /// Raw BAL RLP payload.
        pub bal: Bytes,
    }

    impl BalNotification {
        /// Creates a new [`BalNotification`].
        pub const fn new(block_hash: BlockHash, block_number: BlockNumber, bal: Bytes) -> Self {
            Self { block_hash, block_number, bal }
        }
    }

    /// Type alias for a receiver that receives [`BalNotification`] events.
    pub type BalNotifications = broadcast::Receiver<BalNotification>;

    /// Type alias for a sender that sends [`BalNotification`] events.
    pub type BalNotificationSender = broadcast::Sender<BalNotification>;

    /// A stream of [`BalNotification`]s.
    #[derive(Debug)]
    pub struct BalNotificationStream {
        st: BroadcastStream<BalNotification>,
    }

    impl BalNotificationStream {
        pub(crate) fn new(notifications: BalNotifications) -> Self {
            Self { st: BroadcastStream::new(notifications) }
        }
    }

    impl Stream for BalNotificationStream {
        type Item = BalNotification;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            loop {
                return match ready!(Pin::new(&mut self.st).poll_next(cx)) {
                    Some(Ok(notification)) => Poll::Ready(Some(notification)),
                    Some(Err(BroadcastStreamRecvError::Lagged(_))) => continue,
                    None => Poll::Ready(None),
                }
            }
        }
    }
}

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

    /// Fetch BAL response entries for the given block hashes, stopping after the soft limit is
    /// exceeded.
    ///
    /// Entries are returned in request order. Unavailable BALs are represented as an RLP-encoded
    /// empty list (`0xc0`). The limit is soft: the entry that exceeds the limit is included.
    fn get_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
    ) -> ProviderResult<Vec<Bytes>> {
        let mut out = Vec::new();
        self.append_by_hashes_with_limit(block_hashes, limit, &mut out)?;
        out.shrink_to_fit();
        Ok(out)
    }

    /// Extends the given vector with BAL response entries for the given hashes.
    ///
    /// This adheres to the expected behavior of [`Self::get_by_hashes_with_limit`].
    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        let mut size = 0;
        for bal in self.get_by_hashes(block_hashes)? {
            let bal = bal.unwrap_or_else(|| Bytes::from_static(&[0xc0]));
            size += bal.len();
            out.push(bal);

            if limit.exceeds(size) {
                break
            }
        }
        Ok(())
    }

    /// Fetch BALs for the requested range.
    ///
    /// Implementations may stop at the first gap and return the contiguous prefix.
    fn get_by_range(&self, start: BlockNumber, count: u64) -> ProviderResult<Vec<Bytes>>;

    /// Subscribes to BAL insert notifications.
    ///
    /// Notifications are emitted only after a BAL has been successfully inserted into the store.
    /// They do not imply canonicality.
    #[cfg(feature = "std")]
    fn subscribe_to_bal(&self) -> BalNotifications;

    /// Convenience method to get a stream of [`BalNotification`]s.
    #[cfg(feature = "std")]
    fn bal_stream(&self) -> BalNotificationStream {
        BalNotificationStream::new(self.subscribe_to_bal())
    }
}

/// The limit to enforce for [`BalStore::get_by_hashes_with_limit`].
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GetBlockAccessListLimit {
    /// No limit, return all BALs.
    None,
    /// Enforce a size limit on the returned BALs, for example 2MB.
    ResponseSizeSoftLimit(usize),
}

impl GetBlockAccessListLimit {
    /// Returns true if the given size exceeds the limit.
    #[inline]
    pub const fn exceeds(&self, size: usize) -> bool {
        match self {
            Self::None => false,
            Self::ResponseSizeSoftLimit(limit) => size > *limit,
        }
    }
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

    /// Fetch BAL response entries for the given block hashes, stopping after the soft limit is
    /// exceeded.
    #[inline]
    pub fn get_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
    ) -> ProviderResult<Vec<Bytes>> {
        self.inner.get_by_hashes_with_limit(block_hashes, limit)
    }

    /// Extends the given vector with BAL response entries for the given hashes.
    #[inline]
    pub fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        self.inner.append_by_hashes_with_limit(block_hashes, limit, out)
    }

    /// Fetch BALs for the requested range.
    #[inline]
    pub fn get_by_range(&self, start: BlockNumber, count: u64) -> ProviderResult<Vec<Bytes>> {
        self.inner.get_by_range(start, count)
    }

    /// Subscribes to BAL insert notifications.
    #[cfg(feature = "std")]
    #[inline]
    pub fn subscribe_to_bal(&self) -> BalNotifications {
        self.inner.subscribe_to_bal()
    }

    /// Convenience method to get a stream of [`BalNotification`]s.
    #[cfg(feature = "std")]
    #[inline]
    pub fn bal_stream(&self) -> BalNotificationStream {
        self.inner.bal_stream()
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

    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        let mut size = 0;
        for _ in block_hashes {
            let bal = Bytes::from_static(&[0xc0]);
            size += bal.len();
            out.push(bal);

            if limit.exceeds(size) {
                break
            }
        }
        Ok(())
    }

    fn get_by_range(&self, _start: BlockNumber, _count: u64) -> ProviderResult<Vec<Bytes>> {
        Ok(Vec::new())
    }

    #[cfg(feature = "std")]
    fn subscribe_to_bal(&self) -> BalNotifications {
        tokio::sync::broadcast::channel(1).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    #[cfg(feature = "std")]
    use tokio::sync::broadcast::error::TryRecvError;

    #[test]
    fn noop_store_returns_empty_results() {
        let store = BalStoreHandle::default();
        let hashes = [B256::random(), B256::random()];

        let by_hash = store.get_by_hashes(&hashes).unwrap();
        let by_range = store.get_by_range(1, 10).unwrap();

        assert_eq!(by_hash, vec![None, None]);
        assert!(by_range.is_empty());
    }

    #[test]
    fn noop_store_limited_lookup_returns_prefix() {
        let store = BalStoreHandle::default();
        let hashes = [B256::random(), B256::random(), B256::random()];

        let limited = store
            .get_by_hashes_with_limit(&hashes, GetBlockAccessListLimit::ResponseSizeSoftLimit(1))
            .unwrap();

        assert_eq!(limited, vec![Bytes::from_static(&[0xc0]), Bytes::from_static(&[0xc0])]);
    }

    #[test]
    fn block_access_list_limit() {
        let limit_none = GetBlockAccessListLimit::None;
        assert!(!limit_none.exceeds(usize::MAX));

        let size_limit_2mb = GetBlockAccessListLimit::ResponseSizeSoftLimit(2 * 1024 * 1024);
        assert!(!size_limit_2mb.exceeds(1024 * 1024));
        assert!(!size_limit_2mb.exceeds(2 * 1024 * 1024));
        assert!(size_limit_2mb.exceeds(3 * 1024 * 1024));
    }

    #[cfg(feature = "std")]
    #[test]
    fn noop_store_subscription_is_empty() {
        let store = BalStoreHandle::default();
        let mut notifications = store.subscribe_to_bal();

        assert!(matches!(notifications.try_recv(), Err(TryRecvError::Closed)));
    }
}
