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

    /// Returns whether a valid slot would require cloning its value on mutable access.
    pub(super) fn mutation_copies(&self, index: usize) -> bool {
        match &self.0 {
            SlotStorage::Dense(_) => false,
            SlotStorage::Layered(layered) => {
                layered.is_shared() &&
                    (index >> layered.root_shift >= RADIX ||
                        !layered.overrides.owns_value(index, layered.root_shift))
            }
        }
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

    /// Vacates a slot and returns its previous value, cloning it if shared.
    /// Panics if the index is out of bounds.
    #[track_caller]
    pub(super) fn take_slot(&mut self, index: usize) -> T
    where
        T: VacantSlot,
    {
        match self.storage_mut() {
            SlotStorage::Dense(values) => {
                core::mem::replace(dense_slot_mut(values, index), T::VACANT)
            }
            SlotStorage::Layered(layered) => {
                layered.check_index(index);
                layered.take(index)
            }
        }
    }

    /// Vacates an existing slot if the predicate passes, without copying the old value.
    /// Returns `false` if the index is out of bounds or the predicate rejects the value.
    pub(super) fn vacate_if(&mut self, index: usize, predicate: impl FnOnce(&T) -> bool) -> bool
    where
        T: VacantSlot,
    {
        match self.storage_mut() {
            SlotStorage::Dense(values) => {
                if let Some(slot) = values.get_mut(index) &&
                    predicate(slot)
                {
                    *slot = T::VACANT;
                    true
                } else {
                    false
                }
            }
            SlotStorage::Layered(layered) => {
                if layered.get(index).is_some_and(predicate) {
                    drop(layered.vacate(index));
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Vacates an existing slot if the predicate passes and returns the previous value.
    /// Returns `None` if the index is out of bounds or the predicate rejects the value.
    pub(super) fn take_if(&mut self, index: usize, predicate: impl FnOnce(&T) -> bool) -> Option<T>
    where
        T: VacantSlot,
    {
        match self.storage_mut() {
            SlotStorage::Dense(values) => {
                let slot = values.get_mut(index)?;
                predicate(slot).then(|| core::mem::replace(slot, T::VACANT))
            }
            SlotStorage::Layered(layered) => {
                predicate(layered.get(index)?).then(|| layered.take(index))
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

    /// Drops superseded base values if no other version owns the base, without changing indices.
    #[inline]
    pub(super) fn reclaim_snapshot_overrides(&mut self) {
        if let SlotStorage::Layered(layered) = &self.0 &&
            !matches!(layered.overrides, SlotOverrides::Empty)
        {
            self.storage_mut();
        }
    }

    /// Recovers dense storage when the base becomes exclusively owned, reclaiming superseded
    /// values and reusing the base allocation.
    #[inline]
    fn storage_mut(&mut self) -> &mut SlotStorage<T> {
        self.0.recover_dense()
    }
}

impl<T: Clone> Slots<Option<T>> {
    /// Takes a value from an occupied optional slot without copying vacant slots.
    pub(super) fn take(&mut self, index: usize) -> Option<T> {
        match &mut self.0 {
            SlotStorage::Dense(values) => values.get_mut(index)?.take(),
            SlotStorage::Layered(_) => self.take_if(index, Option::is_some).flatten(),
        }
    }
}

impl<T> Index<usize> for Slots<T> {
    type Output = T;

    #[inline]
    #[track_caller]
    fn index(&self, index: usize) -> &T {
        match &self.0 {
            SlotStorage::Dense(values) => {
                let Some(value) = values.get(index) else { out_of_bounds(index, values.len()) };
                value
            }
            SlotStorage::Layered(layered) => layered.index(index),
        }
    }
}

impl<T: Clone> IndexMut<usize> for Slots<T> {
    #[inline]
    #[track_caller]
    fn index_mut(&mut self, index: usize) -> &mut T {
        match &mut self.0 {
            SlotStorage::Dense(values) => dense_slot_mut(values, index),
            layered @ SlotStorage::Layered(_) => layered.index_mut_layered(index),
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

/// The single value that represents an unoccupied slot in an arena table.
pub(super) trait VacantSlot {
    /// The value to write when freeing a slot.
    const VACANT: Self;
}

impl<T> VacantSlot for Option<T> {
    const VACANT: Self = None;
}

#[derive(Debug, Clone)]
enum SlotStorage<T> {
    Dense(Vec<T>),
    Layered(Box<LayeredSlots<T>>),
}

impl<T: Clone> SlotStorage<T> {
    /// Outline layered mutation so direct indexing of a dense slot only does a bounds check.
    #[inline(never)]
    #[track_caller]
    fn index_mut_layered(&mut self, index: usize) -> &mut T {
        match self.recover_dense() {
            Self::Dense(values) => dense_slot_mut(values, index),
            Self::Layered(layered) => layered.value_mut(index),
        }
    }

    /// Folds overrides into the base allocation once it becomes exclusively owned.
    #[inline]
    fn recover_dense(&mut self) -> &mut Self {
        if let Self::Layered(layered) = &self &&
            !layered.is_shared()
        {
            let Self::Layered(layered) = core::mem::replace(self, Self::Dense(Vec::new())) else {
                unreachable!("the slot table was just checked to be layered");
            };
            *self = Self::Dense(layered.into_dense());
        }
        self
    }
}

/// The base covers `0..base.len()`. Overrides may replace those slots and must cover every
/// appended slot in `base.len()..len`, with no entries at or beyond `len`.
#[derive(Debug, Clone)]
pub(super) struct LayeredSlots<T> {
    base: Arc<Vec<T>>,
    overrides: SlotOverrides<T>,
    /// Vacated slots in this table reuse one allocation, even across directory pages.
    vacancy: Option<Arc<T>>,
    root_shift: usize,
    len: usize,
}

impl<T> LayeredSlots<T> {
    pub(super) fn new(base: Vec<T>) -> Self {
        assert!(base.len() <= MAX_SLOTS, "slot base exceeds the maximum slot count");
        Self {
            len: base.len(),
            base: Arc::new(base),
            overrides: SlotOverrides::Empty,
            vacancy: None,
            root_shift: 0,
        }
    }

    pub(super) fn is_shared(&self) -> bool {
        Arc::strong_count(&self.base) > 1
    }

    /// Returns the backing length, including slots past a free list's logical cursor.
    pub(super) const fn len(&self) -> usize {
        self.len
    }

    fn get(&self, index: usize) -> Option<&T> {
        (index < self.len).then(|| self.value_at(index))
    }

    #[inline(never)]
    #[track_caller]
    fn index(&self, index: usize) -> &T {
        let Some(value) = self.get(index) else { out_of_bounds(index, self.len) };
        value
    }

    /// Returns a value at an index already known to be below the backing length.
    pub(super) fn value_at(&self, index: usize) -> &T {
        debug_assert!(index < self.len);
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
        let len = self.len;
        self.into_dense_prefix(len)
    }

    /// Recovers the exclusive base allocation and materializes only the requested prefix.
    #[cold]
    pub(super) fn into_dense_prefix(self, len: usize) -> Vec<T> {
        debug_assert!(len <= self.len);
        // The base had one strong reference before the move, and no weak references are made.
        let mut values = Arc::into_inner(self.base).expect("slot base must be exclusively owned");
        values.truncate(len);
        self.overrides.apply(&mut values, 0, self.root_shift, len);
        assert_eq!(values.len(), len, "merged slot count differs from the logical length");
        values
    }

    pub(super) fn push(&mut self, value: T) -> usize {
        let index = next_slot(self.len);
        self.write_override(index, value);
        self.len += 1;
        index
    }

    #[track_caller]
    pub(super) fn set(&mut self, index: usize, value: T) {
        self.check_index(index);
        self.write_override(index, value);
    }

    // Keep the snapshot override machinery outside the dense indexing path.
    #[inline(never)]
    #[track_caller]
    fn value_mut(&mut self, index: usize) -> &mut T {
        self.check_index(index);
        self.grow_overrides_for(index);
        let value = self.overrides.slot_mut(index, self.root_shift).get_or_insert_with(|| {
            Arc::new(self.base.get(index).expect("appended slot is missing its override").clone())
        });
        Arc::make_mut(value)
    }

    #[inline(never)]
    fn take(&mut self, index: usize) -> T
    where
        T: VacantSlot,
    {
        self.vacate(index).map_or_else(|| self.base_value(index).clone(), Arc::unwrap_or_clone)
    }

    #[inline(never)]
    fn write_override(&mut self, index: usize, value: T) {
        self.grow_overrides_for(index);
        *self.overrides.slot_mut(index, self.root_shift) = Some(Arc::new(value));
    }

    /// Writes the shared vacant value at an existing index, without copying the old value.
    fn vacate(&mut self, index: usize) -> Option<Arc<T>>
    where
        T: VacantSlot,
    {
        self.grow_overrides_for(index);
        let vacancy = self.vacancy.get_or_insert_with(|| Arc::new(T::VACANT));
        self.overrides.slot_mut(index, self.root_shift).replace(Arc::clone(vacancy))
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

    /// The value can be moved only if every directory and the page on its path are unique.
    fn owns_value(&self, index: usize, shift: usize) -> bool {
        let digit = (index >> shift) & (RADIX - 1);
        match self {
            Self::Empty => false,
            Self::Directory(children) => {
                Arc::strong_count(children) == 1 &&
                    children[digit].owns_value(index, shift - RADIX_BITS)
            }
            Self::Page(entries) => {
                Arc::strong_count(entries) == 1 &&
                    entries[digit].as_ref().is_some_and(|value| Arc::strong_count(value) == 1)
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
    fn apply(self, values: &mut Vec<T>, prefix: usize, shift: usize, len: usize) {
        if prefix >= len {
            return;
        }
        match self {
            Self::Empty => {}
            Self::Directory(children) => {
                assert!(shift >= RADIX_BITS, "unexpected slot override directory");
                for (digit, child) in Arc::unwrap_or_clone(children).into_iter().enumerate() {
                    child.apply(values, prefix | (digit << shift), shift - RADIX_BITS, len);
                }
            }
            Self::Page(page) => {
                assert_eq!(shift, 0, "unexpected slot override page");
                for (offset, entry) in
                    Arc::unwrap_or_clone(page).into_iter().take(len - prefix).enumerate()
                {
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
                    assert_eq!(slots[index], expected[index]);
                    slots.set(index, step);
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
    fn guarded_vacate_does_not_clone_shared_values() {
        struct NoClone(u8);

        impl Clone for NoClone {
            fn clone(&self) -> Self {
                assert_eq!(self.0, 2, "vacating must not clone the previous value");
                Self::VACANT
            }
        }

        impl VacantSlot for NoClone {
            const VACANT: Self = Self(2);
        }

        let mut original = Slots::new();
        assert_eq!(original.push(NoClone(0)), 0);
        original.prepare_snapshot();
        let mut snapshot = original.clone();

        assert!(snapshot.exclusive_slots().is_none());
        assert!(!snapshot.vacate_if(1, |_| true));
        assert!(!snapshot.vacate_if(0, |_| false));
        assert!(snapshot.vacate_if(0, |old| old.0 == 0));
        assert_eq!(snapshot[0].0, 2);
        assert_eq!(original[0].0, 0);

        drop(original);
        assert_eq!(snapshot.exclusive_slots().unwrap()[0].0, 2);
    }

    #[test]
    fn snapshot_vacancies_share_storage_across_pages() {
        let mut original = Slots::new();
        for value in 0..RADIX {
            original.push(Some(value));
        }
        original.prepare_snapshot();
        let mut fork = original.clone();
        assert_eq!(fork.push(Some(RADIX)), RADIX);

        assert!(fork.vacate_if(0, Option::is_some));
        assert!(!fork.vacate_if(0, Option::is_some));
        assert_eq!(fork.take(1), Some(1));
        assert_eq!(fork.take_slot(RADIX), Some(RADIX));
        assert!(core::ptr::eq(&raw const fork[0], &raw const fork[1]));
        assert!(core::ptr::eq(&raw const fork[0], &raw const fork[RADIX]));
        assert_eq!(original[0], Some(0));
        assert_eq!(original.get(RADIX), None);

        let mut next = fork.clone();
        next.set(0, Some(RADIX + 1));
        assert_eq!(fork[0], None);
        assert_eq!(next[0], Some(RADIX + 1));
        assert_eq!(next[RADIX], None);

        drop(original);
        drop(fork);
        let dense = next.exclusive_slots().unwrap();
        assert_eq!(dense.len(), RADIX + 1);
        assert_eq!(dense[0], Some(RADIX + 1));
        assert_eq!(dense[RADIX], None);
    }

    #[test]
    fn mutation_copies_only_when_the_slot_value_is_shared() {
        let mut original = Slots::new();
        let high = RADIX * RADIX;
        for val in 0..=high {
            original.push(val);
        }
        assert!(!original.mutation_copies(0));

        original.prepare_snapshot();
        let mut fork = original.clone();
        assert!(fork.mutation_copies(0));
        assert!(fork.mutation_copies(high));
        fork.set(0, high + 1);
        fork.set(high, high + 2);
        assert!(!fork.mutation_copies(0));
        assert!(!fork.mutation_copies(high));

        let mut next = fork.clone();
        assert!(next.mutation_copies(0));
        assert!(next.mutation_copies(high));
        next.set(1, high + 3);
        assert!(next.mutation_copies(0));
        assert!(!next.mutation_copies(1));
        assert!(next.mutation_copies(high));
        drop(fork);
        assert!(!next.mutation_copies(0));
        assert!(!next.mutation_copies(high));
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
    #[should_panic(expected = "slot base exceeds the maximum slot count")]
    fn layered_base_cannot_exceed_the_slot_limit() {
        LayeredSlots::new(Vec::from([(); MAX_SLOTS + 1]));
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
