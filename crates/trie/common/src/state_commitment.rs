//! Opaque state commitment data carried with a block.

use alloc::sync::Arc;
use core::{any::Any, fmt};

/// Data a custom state-root strategy attaches to a block, such as a pending batch of an
/// authenticated store that replaces the MPT.
///
/// The engine carries it from the state-root job (or the payload builder) on the block until the
/// block is persisted, so a persistence hook can apply it. Reth never inspects it.
#[derive(Clone)]
pub struct StateCommitment(Arc<dyn Any + Send + Sync>);

impl StateCommitment {
    /// Wraps `value` as a state commitment.
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// Returns the wrapped value if it is a `T`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for StateCommitment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateCommitment").finish_non_exhaustive()
    }
}

/// Two commitments are equal when they share the same allocation.
impl PartialEq for StateCommitment {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for StateCommitment {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downcast_and_identity() {
        let commitment = StateCommitment::new(7u64);
        assert_eq!(commitment.downcast_ref::<u64>(), Some(&7));
        assert_eq!(commitment.downcast_ref::<u32>(), None);
        assert_eq!(commitment.clone(), commitment);
        assert_ne!(StateCommitment::new(7u64), commitment);
    }
}
