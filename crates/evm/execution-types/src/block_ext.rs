//! Opaque extension data carried with an executed block.

use alloc::sync::Arc;
use core::{any::Any, fmt};

/// Arbitrary data attached to an executed block, such as a pending batch of an authenticated
/// store that replaces the MPT.
///
/// A state-root job or a payload builder attaches it, and the engine carries it on the block until
/// the block is persisted, so a persistence hook can use it. Reth never inspects it.
#[derive(Clone)]
pub struct BlockExt(Arc<dyn Any + Send + Sync>);

impl BlockExt {
    /// Wraps `value` as a block extension.
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// Returns the wrapped value if it is a `T`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for BlockExt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockExt").finish_non_exhaustive()
    }
}

/// Two exts are equal when they share the same allocation.
impl PartialEq for BlockExt {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for BlockExt {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downcast_and_identity() {
        let ext = BlockExt::new(7u64);
        assert_eq!(ext.downcast_ref::<u64>(), Some(&7));
        assert_eq!(ext.downcast_ref::<u32>(), None);
        assert_eq!(ext.clone(), ext);
        assert_ne!(BlockExt::new(7u64), ext);
    }
}
