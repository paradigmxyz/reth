//! Index-addressed storage with per-slot copy-on-write for snapshots.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::ops::{Deref, DerefMut, Index, IndexMut};
use either::Either;

/// The maximum length of either arena slot table. The remaining index bit tags blinded children.
pub(super) const MAX_SLOTS: usize = 1 << 31;

/// The bit width of each directory digit and the resulting fan-out.
const RADIX_BITS: usize = 4;
const RADIX: usize = 1 << RADIX_BITS;
/// The starting bit of the highest radix digit needed for the largest index.
const TOP_SHIFT: usize = (MAX_SLOTS.ilog2() as usize - 1) / RADIX_BITS * RADIX_BITS;

/// A table of contiguous slots indexed from zero up to [`MAX_SLOTS`].
///
/// Cloning a dense table copies the values. After snapshot preparation, a clone shares the
/// base and existing overrides; replacements and appends then affect only that table. The
/// next mutable access to an exclusively owned table folds its overrides back into the base.
#[derive(Debug, Clone)]
pub(super) struct Slots<T>(SlotStorage<T>);

impl<T> Slots<T> {
    pub(super) const fn new() -> Self {
        Self(SlotStorage::Dense(Vec::new()))
    }

    /// Reserves room for `capacity` slots, up to the maximum slot count.
    pub(super) fn with_capacity(capacity: usize) -> Self {
        assert!(capacity <= MAX_SLOTS, "slot capacity exceeds the maximum slot count");
        Self(SlotStorage::Dense(Vec::with_capacity(capacity)))
    }

    /// Prepares this table for cheap cloning without making it read-only.
    pub(super) fn prepare_snapshot(&mut self) {
        if let SlotStorage::Dense(values) = &mut self.0 {
            self.0 = SlotStorage::Layered(Box::new(LayeredSlots::new(core::mem::take(values))));
        }
    }

    /// Returns whether cloning this table can share its storage.
    pub(super) const fn is_layered(&self) -> bool {
        matches!(&self.0, SlotStorage::Layered(_))
    }

    /// Returns whether another table currently holds a reference to the same base.
    pub(super) fn is_shared(&self) -> bool {
        matches!(&self.0, SlotStorage::Layered(layered) if layered.is_shared())
    }

    pub(super) const fn len(&self) -> usize {
        match &self.0 {
            SlotStorage::Dense(values) => values.len(),
            SlotStorage::Layered(layered) => layered.len,
        }
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub(super) fn get(&self, index: usize) -> Option<&T> {
        match &self.0 {
            SlotStorage::Dense(values) => values.get(index),
            SlotStorage::Layered(layered) => layered.get(index),
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &T> {
        match &self.0 {
            SlotStorage::Dense(values) => Either::Left(values.iter()),
            SlotStorage::Layered(layered) => {
                Either::Right((0..layered.len).map(move |index| layered.value_at(index)))
            }
        }
    }

    /// Copies all logical slots into independent dense storage, preserving their indices.
    pub(super) fn deep_clone(&self, clone_value: impl FnMut(&T) -> T) -> Self {
        let mut values = Vec::with_capacity(self.len());
        values.extend(self.iter().map(clone_value));
        Self(SlotStorage::Dense(values))
    }

    #[cfg(test)]
    pub(super) const fn capacity(&self) -> usize {
        match &self.0 {
            SlotStorage::Dense(values) => values.capacity(),
            SlotStorage::Layered(_) => panic!("capacity is only defined for a dense table"),
        }
    }
}

impl<T: Clone> Slots<T> {
    /// Visits selected slots without copying rejected values.
    pub(super) fn for_each_mut_if(
        &mut self,
        mut predicate: impl FnMut(&T) -> bool,
        mut visit: impl FnMut(usize, &mut T),
    ) {
        match &mut self.0 {
            SlotStorage::Dense(values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    if predicate(value) {
                        visit(index, value);
                    }
                }
            }
            SlotStorage::Layered(layered) => {
                for index in 0..layered.len {
                    if predicate(&self[index]) {
                        visit(index, &mut self[index]);
                    }
                }
            }
        }
    }

    /// Appends a slot and returns its index. Panics if the slot table is full.
    pub(super) fn push(&mut self, value: T) -> usize {
        match self.storage_mut() {
            SlotStorage::Dense(values) => {
                let index = next_slot(values.len());
                values.push(value);
                index
            }
            SlotStorage::Layered(layered) => layered.push(value),
        }
    }

    /// Overwrites a slot without copying its old value out of shared storage.
    /// Panics if the index is out of bounds.
    #[track_caller]
    pub(super) fn set(&mut self, index: usize, value: T) {
        match self.storage_mut() {
            SlotStorage::Dense(values) => *dense_slot_mut(values, index) = value,
            SlotStorage::Layered(layered) => layered.set(index, value),
        }
    }

    /// Overwrites a slot and returns its previous value, cloning it if shared.
    /// Panics if the index is out of bounds.
    #[track_caller]
    pub(super) fn replace(&mut self, index: usize, value: T) -> T {
        match self.storage_mut() {
            SlotStorage::Dense(values) => core::mem::replace(dense_slot_mut(values, index), value),
            SlotStorage::Layered(layered) => layered.replace(index, value),
        }
    }

    /// Overwrites an existing slot if the predicate passes, without copying the old value.
    /// Returns `false` if the index is out of bounds or the predicate rejects the value.
    pub(super) fn set_if(
        &mut self,
        index: usize,
        value: T,
        predicate: impl FnOnce(&T) -> bool,
    ) -> bool {
        match self.storage_mut() {
            SlotStorage::Dense(values) => {
                if let Some(slot) = values.get_mut(index) &&
                    predicate(slot)
                {
                    *slot = value;
                    true
                } else {
                    false
                }
            }
            SlotStorage::Layered(layered) => layered.set_if(index, value, predicate),
        }
    }

    /// Overwrites an existing slot if the predicate passes and returns the previous value.
    /// Returns `None` if the index is out of bounds or the predicate rejects the value.
    pub(super) fn replace_if(
        &mut self,
        index: usize,
        value: T,
        predicate: impl FnOnce(&T) -> bool,
    ) -> Option<T> {
        match self.storage_mut() {
            SlotStorage::Dense(values) => {
                let slot = values.get_mut(index)?;
                predicate(slot).then(|| core::mem::replace(slot, value))
            }
            SlotStorage::Layered(layered) => {
                predicate(layered.get(index)?).then(|| layered.replace(index, value))
            }
        }
    }

    /// Returns a mutable view for compaction, or `None` if the base is still shared.
    pub(super) fn exclusive_slots(&mut self) -> Option<ExclusiveSlots<'_, T>> {
        match self.storage_mut() {
            SlotStorage::Dense(values) => Some(ExclusiveSlots(values)),
            SlotStorage::Layered(_) => None,
        }
    }

    /// Recovers dense storage when the base becomes exclusively owned, reclaiming superseded
    /// values and reusing the base allocation.
    #[inline]
    fn storage_mut(&mut self) -> &mut SlotStorage<T> {
        if let SlotStorage::Layered(layered) = &self.0 &&
            !layered.is_shared()
        {
            let storage = core::mem::replace(&mut self.0, SlotStorage::Dense(Vec::new()));
            let SlotStorage::Layered(layered) = storage else { unreachable!() };
            self.0 = SlotStorage::Dense(layered.into_dense());
        }
        &mut self.0
    }
}

impl<T> Index<usize> for Slots<T> {
    type Output = T;

    #[inline]
    #[track_caller]
    fn index(&self, index: usize) -> &T {
        let Some(value) = self.get(index) else { out_of_bounds(index, self.len()) };
        value
    }
}

impl<T: Clone> IndexMut<usize> for Slots<T> {
    #[inline]
    #[track_caller]
    fn index_mut(&mut self, index: usize) -> &mut T {
        match self.storage_mut() {
            SlotStorage::Dense(values) => dense_slot_mut(values, index),
            SlotStorage::Layered(layered) => layered.value_mut(index),
        }
    }
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// An exclusive view of the slots. Compaction can reorder and truncate, but cannot append.
pub(super) struct ExclusiveSlots<'a, T>(&'a mut Vec<T>);

impl<T> ExclusiveSlots<'_, T> {
    pub(super) fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    pub(super) const fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub(super) fn shrink_to(&mut self, min_capacity: usize) {
        self.0.shrink_to(min_capacity);
    }
}

impl<T> Deref for ExclusiveSlots<'_, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.0.as_slice()
    }
}

impl<T> DerefMut for ExclusiveSlots<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.0.as_mut_slice()
    }
}

#[derive(Debug, Clone)]
enum SlotStorage<T> {
    Dense(Vec<T>),
    Layered(Box<LayeredSlots<T>>),
}

/// The base covers `0..base.len()`. Overrides may replace those slots and must cover every
/// appended slot in `base.len()..len`, with no entries at or beyond `len`.
#[derive(Debug, Clone)]
struct LayeredSlots<T> {
    base: Arc<Vec<T>>,
    overrides: SlotOverrides<T>,
    root_shift: usize,
    len: usize,
}

impl<T> LayeredSlots<T> {
    fn new(base: Vec<T>) -> Self {
        debug_assert!(base.len() <= MAX_SLOTS);
        Self {
            len: base.len(),
            base: Arc::new(base),
            overrides: SlotOverrides::Empty,
            root_shift: 0,
        }
    }

    fn is_shared(&self) -> bool {
        Arc::strong_count(&self.base) > 1
    }

    fn get(&self, index: usize) -> Option<&T> {
        (index < self.len).then(|| self.value_at(index))
    }

    fn value_at(&self, index: usize) -> &T {
        // Indices beyond the directory's current range can only refer to the base.
        if index >> self.root_shift < RADIX &&
            let Some(value) = self.overrides.get(index, self.root_shift)
        {
            return value;
        }
        self.base_value(index)
    }

    fn base_value(&self, index: usize) -> &T {
        self.base.get(index).expect("appended slot is missing its override")
    }
}

impl<T: Clone> LayeredSlots<T> {
    #[cold]
    fn into_dense(self) -> Vec<T> {
        // The base had one strong reference before the move, and no weak references are made.
        let mut values = Arc::into_inner(self.base).expect("slot base must be exclusively owned");
        self.overrides.apply(&mut values, 0, self.root_shift);
        assert_eq!(values.len(), self.len, "merged slot count differs from the logical length");
        values
    }

    fn push(&mut self, value: T) -> usize {
        let index = next_slot(self.len);
        self.write_override(index, value);
        self.len += 1;
        index
    }

    #[track_caller]
    fn set(&mut self, index: usize, value: T) {
        self.check_index(index);
        self.write_override(index, value);
    }

    #[track_caller]
    fn value_mut(&mut self, index: usize) -> &mut T {
        self.check_index(index);
        self.grow_overrides_for(index);
        let value = self.overrides.slot_mut(index, self.root_shift).get_or_insert_with(|| {
            Arc::new(self.base.get(index).expect("appended slot is missing its override").clone())
        });
        Arc::make_mut(value)
    }

    #[track_caller]
    fn replace(&mut self, index: usize, value: T) -> T {
        self.check_index(index);
        self.grow_overrides_for(index);
        self.overrides
            .slot_mut(index, self.root_shift)
            .replace(Arc::new(value))
            .map_or_else(|| self.base_value(index).clone(), Arc::unwrap_or_clone)
    }

    fn set_if(&mut self, index: usize, value: T, predicate: impl FnOnce(&T) -> bool) -> bool {
        if self.get(index).is_some_and(predicate) {
            self.write_override(index, value);
            true
        } else {
            false
        }
    }

    fn write_override(&mut self, index: usize, value: T) {
        self.grow_overrides_for(index);
        *self.overrides.slot_mut(index, self.root_shift) = Some(Arc::new(value));
    }

    /// Extends only this version's directory far enough to address an edited slot.
    fn grow_overrides_for(&mut self, index: usize) {
        if index >> self.root_shift < RADIX {
            return;
        }

        let shift = index.ilog2() as usize / RADIX_BITS * RADIX_BITS;
        debug_assert!(shift <= TOP_SHIFT);
        if matches!(self.overrides, SlotOverrides::Empty) {
            self.root_shift = shift;
            return;
        }
        while self.root_shift < shift {
            let mut children = core::array::from_fn(|_| SlotOverrides::Empty);
            children[0] = core::mem::replace(&mut self.overrides, SlotOverrides::Empty);
            self.overrides = SlotOverrides::Directory(Arc::new(children));
            self.root_shift += RADIX_BITS;
        }
    }

    #[track_caller]
    fn check_index(&self, index: usize) {
        if index >= self.len {
            out_of_bounds(index, self.len);
        }
    }
}

/// A radix tree of overrides, growing only to cover modified indices.
#[derive(Debug, Clone)]
enum SlotOverrides<T> {
    /// None of the slots in this subtree override the base.
    Empty,
    /// An internal directory; the shift is nonzero.
    Directory(Arc<[Self; RADIX]>),
    /// The final digit selects an optional override. Cloning a page shares its values.
    Page(Arc<[Option<Arc<T>>; RADIX]>),
}

impl<T> SlotOverrides<T> {
    fn get(&self, index: usize, shift: usize) -> Option<&T> {
        debug_assert!(index < MAX_SLOTS);
        let digit = (index >> shift) & (RADIX - 1);
        match self {
            Self::Empty => None,
            Self::Directory(children) => {
                debug_assert!(shift >= RADIX_BITS);
                children[digit].get(index, shift - RADIX_BITS)
            }
            Self::Page(entries) => {
                debug_assert_eq!(shift, 0);
                entries[digit].as_deref()
            }
        }
    }
}

impl<T: Clone> SlotOverrides<T> {
    /// Allocates missing directories and copies shared directories along the path.
    fn slot_mut(&mut self, index: usize, shift: usize) -> &mut Option<Arc<T>> {
        debug_assert!(index < MAX_SLOTS);
        let digit = (index >> shift) & (RADIX - 1);
        if matches!(self, Self::Empty) {
            *self = if shift == 0 {
                Self::Page(Arc::new(core::array::from_fn(|_| None)))
            } else {
                Self::Directory(Arc::new(core::array::from_fn(|_| Self::Empty)))
            };
        }

        match self {
            Self::Empty => unreachable!(),
            Self::Directory(children) => {
                debug_assert!(shift >= RADIX_BITS);
                Arc::make_mut(children)[digit].slot_mut(index, shift - RADIX_BITS)
            }
            Self::Page(entries) => {
                debug_assert_eq!(shift, 0);
                &mut Arc::make_mut(entries)[digit]
            }
        }
    }

    /// Merges overrides in ascending index order so appended slots can reuse the base allocation.
    fn apply(self, values: &mut Vec<T>, prefix: usize, shift: usize) {
        match self {
            Self::Empty => {}
            Self::Directory(children) => {
                assert!(shift >= RADIX_BITS, "unexpected slot override directory");
                for (digit, child) in Arc::unwrap_or_clone(children).into_iter().enumerate() {
                    child.apply(values, prefix | (digit << shift), shift - RADIX_BITS);
                }
            }
            Self::Page(page) => {
                assert_eq!(shift, 0, "unexpected slot override page");
                for (offset, entry) in Arc::unwrap_or_clone(page).into_iter().enumerate() {
                    if let Some(entry) = entry {
                        let index = prefix | offset;
                        let value = Arc::unwrap_or_clone(entry);
                        if index < values.len() {
                            values[index] = value;
                        } else {
                            assert_eq!(
                                index,
                                values.len(),
                                "appended slot overrides must be contiguous"
                            );
                            values.push(value);
                        }
                    }
                }
            }
        }
    }
}

#[inline]
fn next_slot(len: usize) -> usize {
    assert!(len < MAX_SLOTS, "slot table is full");
    len
}

#[inline]
#[track_caller]
fn dense_slot_mut<T>(values: &mut [T], index: usize) -> &mut T {
    let len = values.len();
    let Some(slot) = values.get_mut(index) else { out_of_bounds(index, len) };
    slot
}

#[cold]
#[track_caller]
fn out_of_bounds(index: usize, len: usize) -> ! {
    panic!("slot index {index} out of bounds for length {len}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[test]
    fn snapshots_match_vec_across_mutation_sequences() {
        let mut slots = Slots::new();
        let mut expected = Vec::new();
        let mut versions = Vec::new();
        // A deterministic sequence of appends, replacements, and in-place mutations across
        // nested snapshots, checked against independent vectors.
        let mut state = 1u64;
        for step in 0usize..1024 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let op = (state >> 32) as usize;
            if expected.is_empty() || op.is_multiple_of(3) {
                assert_eq!(slots.push(step), expected.len());
                expected.push(step);
            } else {
                let index = op % expected.len();
                if op % 3 == 1 {
                    assert_eq!(slots.replace(index, step), expected[index]);
                    expected[index] = step;
                } else {
                    slots[index] += step;
                    expected[index] += step;
                }
            }
            if step.is_multiple_of(37) {
                slots.prepare_snapshot();
                versions.push((slots.clone(), expected.clone()));
            }
            if step.is_multiple_of(17) {
                assert!(slots.iter().copied().eq(expected.iter().copied()));
            }
        }
        for (version, expected) in versions {
            assert!(version.iter().copied().eq(expected));
        }
        assert_eq!(slots.push(1024), expected.len());
        expected.push(1024);
        assert!(slots.iter().copied().eq(expected));
        assert!(!slots.is_layered());
    }

    #[test]
    fn override_directory_grows_without_aliasing_base_slots() {
        let mut original = Slots::new();
        let high = RADIX * RADIX;
        for value in 0..=high {
            original.push(value);
        }
        original.prepare_snapshot();
        let mut snapshot = original.clone();

        snapshot.set(0, high + 1);
        let SlotStorage::Layered(layered) = &snapshot.0 else { unreachable!() };
        assert_eq!(layered.root_shift, 0);
        assert_eq!(snapshot[high], high);

        let mut nested = snapshot.clone();
        nested.set(high, high + 2);
        let SlotStorage::Layered(layered) = &nested.0 else { unreachable!() };
        assert_eq!(layered.root_shift, 2 * RADIX_BITS);
        assert_eq!(nested[0], high + 1);
        assert_eq!(nested[high], high + 2);
        assert_eq!(snapshot[high], high);
        assert_eq!(original[0], 0);

        drop(snapshot);
        drop(original);
        nested.push(high + 3);
        assert_eq!(nested[0], high + 1);
        assert_eq!(nested[high], high + 2);
        assert_eq!(nested[high + 1], high + 3);
        assert!(!nested.is_layered());
    }

    #[test]
    fn nested_snapshots_share_untouched_slots_across_radix_boundaries() {
        let mut original = Slots::new();
        for value in 0..RADIX {
            original.push(value);
        }
        original.prepare_snapshot();

        let mut snapshot = original.clone();
        snapshot[0] = 100;
        snapshot.push(101);
        snapshot.push(102);
        let mut nested = snapshot.clone();

        nested[0] = 103;
        nested[RADIX] = 104;
        assert!(core::ptr::eq(&raw const original[1], &raw const nested[1]));
        assert!(core::ptr::eq(&raw const snapshot[RADIX + 1], &raw const nested[RADIX + 1]));
        assert_eq!(original.get(RADIX), None);
        assert_eq!(original[0], 0);
        assert_eq!(snapshot[0], 100);
        assert_eq!(snapshot[RADIX], 101);

        drop(original);
        drop(snapshot);
        assert!(nested.is_layered());
        nested.push(105);
        assert!(!nested.is_layered());
        assert_eq!(nested[0], 103);
        assert_eq!(nested[RADIX], 104);
        assert_eq!(nested[RADIX + 1], 102);
        assert_eq!(nested[RADIX + 2], 105);
    }

    #[test]
    fn guarded_set_does_not_clone_shared_values() {
        struct NoClone(u8);

        impl Clone for NoClone {
            fn clone(&self) -> Self {
                panic!("set_if must not clone the previous value");
            }
        }

        let mut original = Slots::new();
        assert_eq!(original.push(NoClone(0)), 0);
        original.prepare_snapshot();
        let mut snapshot = original.clone();

        assert!(snapshot.exclusive_slots().is_none());
        assert!(!snapshot.set_if(1, NoClone(1), |_| true));
        assert!(!snapshot.set_if(0, NoClone(1), |_| false));
        assert!(snapshot.set_if(0, NoClone(2), |old| old.0 == 0));
        assert_eq!(snapshot[0].0, 2);
        assert_eq!(original[0].0, 0);

        drop(original);
        assert_eq!(snapshot.exclusive_slots().unwrap()[0].0, 2);
    }

    #[test]
    fn slot_limit_addresses_the_last_radix_index() {
        let last = next_slot(MAX_SLOTS - 1);
        let mut overrides = SlotOverrides::Empty;
        *overrides.slot_mut(last, TOP_SHIFT) = Some(Arc::new(1));
        *overrides.slot_mut(0, TOP_SHIFT) = Some(Arc::new(2));
        assert_eq!(overrides.get(last, TOP_SHIFT), Some(&1));
        assert_eq!(overrides.get(0, TOP_SHIFT), Some(&2));
    }

    #[test]
    #[should_panic(expected = "slot table is full")]
    fn append_index_rejects_a_full_table() {
        next_slot(MAX_SLOTS);
    }

    #[test]
    #[should_panic(expected = "slot capacity exceeds the maximum slot count")]
    fn capacity_cannot_exceed_the_slot_limit() {
        Slots::<()>::with_capacity(MAX_SLOTS + 1);
    }

    #[test]
    fn out_of_bounds_panics_match_between_representations() {
        let mut dense = Slots::new();
        dense.push(0);
        let mut layered = dense.clone();
        layered.prepare_snapshot();
        let peer = layered.clone();

        for slots in [&mut dense, &mut layered] {
            let read_error = catch_unwind(AssertUnwindSafe(|| slots[1])).unwrap_err();
            assert_eq!(
                read_error.downcast_ref::<String>().map(String::as_str),
                Some("slot index 1 out of bounds for length 1")
            );
            let write_error = catch_unwind(AssertUnwindSafe(|| slots[1] = 1)).unwrap_err();
            assert_eq!(
                write_error.downcast_ref::<String>().map(String::as_str),
                Some("slot index 1 out of bounds for length 1")
            );
            assert_eq!(slots[0], 0);
        }

        assert_eq!(peer[0], 0);
    }
}
