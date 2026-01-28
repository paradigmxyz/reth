#[cfg(feature = "read-tx-timeouts")]
#[cfg(feature = "read-tx-timeouts")]
use crate::tx::access::WeakRoTxPtr;
use crate::{MdbxError, MdbxResult, tx::RoTxPtr};
use std::{
    sync::{Arc, Weak},
    time::Duration,
};

/// Inner storage for RO transactions with timeout support.
///
/// When a timeout is set, the Arc is handed off to a background thread that
/// will drop it after the timeout, causing the transaction to be aborted.
/// The transaction keeps a Weak reference and must upgrade it for each
/// operation.
///
/// When no timeout is set, we keep the Arc ourselves and can use it directly.
#[cfg(feature = "read-tx-timeouts")]
pub(crate) struct RoInner {
    /// If we own the Arc (no timeout), we can use it directly without upgrade.
    owner: Option<Arc<RoTxPtr>>,
    /// Weak reference for timeout case - must upgrade to use.
    #[cfg(feature = "read-tx-timeouts")]
    weak: WeakRoTxPtr,
}

impl RoInner {
    /// Create a new RoInner with no timeout (we keep the Arc).
    pub(crate) fn new_owned(arc: Arc<RoTxPtr>) -> Self {
        let weak = Arc::downgrade(&arc);
        Self {
            owner: Some(arc),

            #[cfg(feature = "read-tx-timeouts")]
            weak,
        }
    }

    /// Create a new RoInner with timeout (background thread has the Arc).
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn new_with_timeout(ptr: RoTxPtr, duration: Duration) -> Self {
        let arc = Arc::new(ptr);
        let weak = Arc::downgrade(&arc);
        std::thread::spawn(move || {
            std::thread::sleep(duration);
            // Drop the Arc, aborting the transaction.
            drop(arc);
        });
        Self { owner: None, weak }
    }

    /// Get a reference to the owner Arc, if we have it.
    pub(crate) fn owner(&self) -> Option<&Arc<RoTxPtr>> {
        self.owner.as_ref()
    }

    /// Get a reference to the weak pointer.
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn weak(&self) -> &Weak<RoTxPtr> {
        &self.weak
    }

    /// Try to upgrade the weak reference to take ownership.
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn try_upgrade(&mut self) -> MdbxResult<()> {
        if self.owner.is_some() {
            return Ok(());
        }
        if let Some(arc) = self.weak.upgrade() {
            self.owner = Some(arc);
            return Ok(());
        }
        Err(MdbxError::ReadTransactionTimeout.into())
    }
}
