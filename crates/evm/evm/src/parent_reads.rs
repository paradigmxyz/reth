//! Optional transport for provider-certified parent storage reads.
//!
//! A witness is only an opaque payload here. The provider which consumes it must check its own
//! private witness type, live parent view, read index, key and value. These hooks never authorize
//! skipping canonical state reads or dependency validation.

use alloc::sync::Arc;
use alloy_evm::Database;
use alloy_primitives::{Address, U256};
use core::{any::Any, fmt};

/// One provider-owned batch of successful reads from an immutable parent view.
#[derive(Clone)]
pub struct ParentReadBatch {
    /// Opaque data interpreted only by the issuing provider adapter.
    pub opaque: Arc<dyn Any + Send + Sync>,
    /// Checked retained-payload estimate, including the batch's exposed capacities.
    ///
    /// This is not an allocator or RSS bound. Returning an overflowing estimate must decline
    /// retention rather than weaken an existing memory budget.
    pub estimated_bytes: usize,
}

impl fmt::Debug for ParentReadBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParentReadBatch")
            .field("estimated_bytes", &self.estimated_bytes)
            .finish_non_exhaustive()
    }
}

/// Reads one parent storage value and records its absolute candidate read index.
pub type ReadParentStorage<DB> =
    fn(&mut DB, usize, Address, U256) -> Result<U256, <DB as revm::Database>::Error>;

/// Typed callbacks for recording a single worker execution.
///
/// Only storage reads which actually reach the provider use `storage`. Predictions and forwarded
/// values must not be recorded through this interface. Implementations perform the original read
/// exactly once and record only successful returns. Failed or unwound captures must be discarded.
pub struct CaptureParentReads<DB: Database> {
    /// Starts a fresh capture, discarding any previous incomplete one.
    pub begin: fn(&mut DB),
    /// Reads storage and records its absolute position in the candidate's read list.
    pub storage: ReadParentStorage<DB>,
    /// Seals the successful capture into one batch, without retaining a provider or shared cache.
    pub finish: fn(&mut DB) -> Option<ParentReadBatch>,
    /// Drops any incomplete capture without changing provider state.
    pub discard: fn(&mut DB),
}

impl<DB: Database> Copy for CaptureParentReads<DB> {}

impl<DB: Database> Clone for CaptureParentReads<DB> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<DB: Database> fmt::Debug for CaptureParentReads<DB> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CaptureParentReads").finish_non_exhaustive()
    }
}

/// Typed callbacks for offering a witness below an ordinary canonical state read.
///
/// `offer` cannot return a storage value or a validation decision. The current underlying provider
/// checks the witness if the ordinary read reaches it. Clear an unused offer after the read,
/// including when account lookup or storage access returns an error.
pub struct ValidateParentReads<DB: Database> {
    /// Offers one exact recorded read to the current provider adapter.
    pub offer: fn(&mut DB, &ParentReadBatch, usize, Address, U256, U256),
    /// Removes an unused offer.
    pub clear: fn(&mut DB),
}

impl<DB: Database> Copy for ValidateParentReads<DB> {}

impl<DB: Database> Clone for ValidateParentReads<DB> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<DB: Database> fmt::Debug for ValidateParentReads<DB> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidateParentReads").finish_non_exhaustive()
    }
}

/// Optional provider operations for either a worker or the ordered executor.
#[derive(Debug)]
pub enum ParentReadHooks<DB: Database> {
    /// Records original successful provider reads on a worker.
    Capture(CaptureParentReads<DB>),
    /// Offers private provider witnesses during ordered validation.
    Validate(ValidateParentReads<DB>),
}

impl<DB: Database> Copy for ParentReadHooks<DB> {}

impl<DB: Database> Clone for ParentReadHooks<DB> {
    fn clone(&self) -> Self {
        *self
    }
}
