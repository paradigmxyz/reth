//! LIFO recycling of slot indices across arena snapshots.

use super::slots::LayeredSlots;
use alloc::{boxed::Box, vec::Vec};
use core::mem;

/// A dense stack until snapshot preparation moves its allocation into a shared slot table.
#[derive(Debug, Clone)]
pub(super) struct FreeList<T>(FreeListStorage<T>);

#[derive(Debug, Clone)]
enum FreeListStorage<T> {
    Dense(Vec<T>),
    /// The logical stack is the prefix up to `len`, which never exceeds the backing length.
    /// A shared pop only moves the cursor; a push reuses popped slots before appending.
    Layered {
        slots: Box<LayeredSlots<T>>,
        len: usize,
    },
}

impl<T> FreeList<T> {
    pub(super) const fn new() -> Self {
        Self(FreeListStorage::Dense(Vec::new()))
    }

    pub(super) const fn len(&self) -> usize {
        match &self.0 {
            FreeListStorage::Dense(values) => values.len(),
            FreeListStorage::Layered { len, .. } => *len,
        }
    }

    /// Returns whether cloning needs no copy of the stored indices.
    pub(super) const fn is_snapshot_ready(&self) -> bool {
        match &self.0 {
            FreeListStorage::Dense(values) => values.is_empty(),
            FreeListStorage::Layered { .. } => true,
        }
    }

    /// Returns whether another list holds a reference to its backing allocation.
    pub(super) fn is_shared(&self) -> bool {
        matches!(&self.0, FreeListStorage::Layered { slots, .. } if slots.is_shared())
    }

    /// Shares the existing backing allocation without copying any indices.
    pub(super) fn prepare_snapshot(&mut self) {
        if let FreeListStorage::Dense(values) = &mut self.0 &&
            !values.is_empty()
        {
            let len = values.len();
            let slots = Box::new(LayeredSlots::new(mem::take(values)));
            self.0 = FreeListStorage::Layered { slots, len };
        }
    }

    #[cfg(test)]
    pub(super) const fn capacity(&self) -> usize {
        match &self.0 {
            FreeListStorage::Dense(values) => values.capacity(),
            FreeListStorage::Layered { .. } => {
                panic!("capacity is only defined for a dense free list")
            }
        }
    }
}

impl<T: Copy> FreeList<T> {
    /// Pushes an index onto the current version's stack.
    #[inline]
    pub(super) fn push(&mut self, value: T) {
        match self.recover_dense() {
            FreeListStorage::Dense(values) => values.push(value),
            FreeListStorage::Layered { slots, len } => {
                if *len == slots.len() {
                    slots.push(value);
                } else {
                    slots.set(*len, value);
                }
                *len += 1;
            }
        }
    }

    /// Removes the most recently pushed index, or returns `None` if it is empty.
    #[inline]
    pub(super) fn pop(&mut self) -> Option<T> {
        match self.recover_dense() {
            FreeListStorage::Dense(values) => values.pop(),
            FreeListStorage::Layered { slots, len } => {
                let last = len.checked_sub(1)?;
                let value = *slots.value_at(last);
                *len = last;
                Some(value)
            }
        }
    }

    /// Clears an exclusively owned list for compaction, reusing its dense allocation.
    pub(super) fn clear_for_rebuild(&mut self) -> FreeListRebuild<'_, T> {
        if let FreeListStorage::Layered { slots, len } = &mut self.0 {
            assert!(!slots.is_shared(), "cannot rebuild a shared free list");
            // The old indices are not needed: avoid materializing any overrides.
            *len = 0;
        }
        let FreeListStorage::Dense(values) = self.recover_dense() else {
            unreachable!("the free list was just checked to be exclusive");
        };
        values.clear();
        FreeListRebuild(values)
    }

    /// Copies only the logical stack into independent dense storage.
    pub(super) fn deep_clone(&self) -> Self {
        Self(match &self.0 {
            FreeListStorage::Dense(values) => FreeListStorage::Dense(values.clone()),
            FreeListStorage::Layered { slots, len } => {
                FreeListStorage::Dense((0..*len).map(|index| *slots.value_at(index)).collect())
            }
        })
    }

    /// Restores the dense stack once no other version owns its base.
    #[inline]
    fn recover_dense(&mut self) -> &mut FreeListStorage<T> {
        if let FreeListStorage::Layered { slots, .. } = &self.0 &&
            !slots.is_shared()
        {
            self.merge_prefix();
        }
        &mut self.0
    }

    #[cold]
    fn merge_prefix(&mut self) {
        let FreeListStorage::Layered { slots, len } = mem::take(self).0 else {
            unreachable!("the free list was just checked to be layered");
        };
        self.0 = FreeListStorage::Dense(slots.into_dense_prefix(len));
    }
}

impl<T> Default for FreeList<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// An exclusive view that appends recycled indices without checking for shared storage.
pub(super) struct FreeListRebuild<'a, T>(&'a mut Vec<T>);

impl<T> FreeListRebuild<'_, T> {
    pub(super) fn push(&mut self, value: T) {
        self.0.push(value);
    }

    pub(super) const fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) const fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub(super) fn shrink_to(&mut self, min_capacity: usize) {
        self.0.shrink_to(min_capacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_push_and_pop_keep_untouched_indices_shared() {
        let mut original = FreeList(FreeListStorage::Dense((0..64).collect()));
        original.prepare_snapshot();
        let mut appended = original.clone();
        let mut reused = original.clone();

        appended.push(64);
        assert_eq!(reused.pop(), Some(63));
        reused.push(100);

        let FreeListStorage::Layered { slots: base, len: 64 } = &original.0 else {
            panic!("expected base")
        };
        let FreeListStorage::Layered { slots: append, len: 65 } = &appended.0 else {
            panic!("expected appended slot");
        };
        let FreeListStorage::Layered { slots: reuse, len: 64 } = &reused.0 else {
            panic!("expected replacement slot");
        };
        assert!(core::ptr::eq(base.value_at(62), append.value_at(62)));
        assert!(core::ptr::eq(base.value_at(62), reuse.value_at(62)));
        assert_eq!(base.value_at(63), &63);
        assert_eq!(append.value_at(64), &64);
        assert_eq!(reuse.value_at(63), &100);

        assert_eq!(appended.pop(), Some(64));
        assert_eq!(appended.pop(), Some(63));
        assert_eq!(reused.pop(), Some(100));
        assert_eq!(original.pop(), Some(63));
        drop(original);
        drop(reused);
        assert_eq!(appended.pop(), Some(62));
        assert!(
            matches!(appended.0, FreeListStorage::Dense(values) if values.iter().copied().eq(0..62))
        );
    }

    #[test]
    fn recovery_keeps_live_overrides_and_excludes_popped_slots() {
        let mut original = FreeList(FreeListStorage::Dense((0..15).collect()));
        original.prepare_snapshot();
        let mut fork = original.clone();
        fork.push(15);
        fork.push(16);
        fork.push(17);
        assert_eq!(fork.pop(), Some(17));
        assert_eq!(fork.pop(), Some(16));

        let mut copy = fork.deep_clone();
        assert_eq!(copy.pop(), Some(15));
        assert_eq!(copy.pop(), Some(14));
        drop(original);
        fork.push(30);
        assert!(
            matches!(fork.0, FreeListStorage::Dense(values) if values.iter().copied().eq((0..16).chain([30])))
        );
    }

    #[test]
    fn rebuilding_reuses_the_exclusive_base_allocation() {
        let mut values = Vec::with_capacity(64);
        values.extend(0..16);
        let ptr = values.as_ptr();
        let mut original = FreeList(FreeListStorage::Dense(values));
        original.prepare_snapshot();
        let mut fork = original.clone();
        fork.push(16);
        drop(original);

        let mut values = fork.clear_for_rebuild();
        assert_eq!(values.len(), 0);
        assert_eq!(values.0.as_ptr(), ptr);
        values.push(17);
        assert_eq!(fork.pop(), Some(17));
        assert_eq!(fork.pop(), None);
    }
}
