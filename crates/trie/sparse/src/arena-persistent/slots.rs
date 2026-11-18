//! Index-addressed storage that copies individual slots on write after a fork.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::ops::{Index, IndexMut};

/// Dense storage on the unforked path. Freezing moves the vector into an immutable shared base;
/// each fork keeps a radix tree of replacements and appended slots. Unlike copy-on-write pages
/// of values, a write copies only directory entries and the value at the requested index.
#[derive(Debug, Clone)]
pub(super) enum Slots<T> {
    Dense(Vec<T>),
    Frozen(Box<Frozen<T>>),
}

#[derive(Debug, Clone)]
pub(super) struct Frozen<T> {
    base: Arc<Vec<T>>,
    edits: Edits<T>,
    len: usize,
}

impl<T: Clone> Slots<T> {
    pub(super) const fn new() -> Self {
        Self::Dense(Vec::new())
    }

    pub(super) fn freeze(&mut self) {
        if let Self::Dense(values) = self {
            let len = values.len();
            *self = Self::Frozen(Box::new(Frozen {
                base: Arc::new(core::mem::take(values)),
                edits: Edits::Empty,
                len,
            }));
        }
    }

    pub(super) const fn is_frozen(&self) -> bool {
        matches!(self, Self::Frozen(_))
    }

    pub(super) fn is_shared(&self) -> bool {
        matches!(self, Self::Frozen(frozen) if Arc::strong_count(&frozen.base) > 1)
    }

    /// Reclaims superseded values once no other fork owns the base. The base allocation can
    /// be reused, and subsequent operations return to the dense fast path.
    #[inline]
    fn thaw(&mut self) {
        if let Self::Frozen(frozen) = self &&
            let Some(values) = frozen.try_take_dense()
        {
            *self = Self::Dense(values);
        }
    }

    pub(super) const fn len(&self) -> usize {
        match self {
            Self::Dense(values) => values.len(),
            Self::Frozen(frozen) => frozen.len,
        }
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub(super) fn get(&self, index: usize) -> Option<&T> {
        match self {
            Self::Dense(values) => values.get(index),
            Self::Frozen(frozen) => frozen.get(index),
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &T> {
        let dense = match self {
            Self::Dense(values) => values.iter(),
            Self::Frozen(_) => [].iter(),
        };
        let frozen_len = if self.is_frozen() { self.len() } else { 0 };
        dense.chain((0..frozen_len).map(|index| &self[index]))
    }

    /// Visits selected slots without copying rejected values.
    pub(super) fn for_each_mut_if(
        &mut self,
        mut select: impl FnMut(&T) -> bool,
        mut visit: impl FnMut(usize, &mut T),
    ) {
        match self {
            Self::Dense(values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    if select(value) {
                        visit(index, value);
                    }
                }
            }
            Self::Frozen(frozen) => {
                for index in 0..frozen.len {
                    if select(&self[index]) {
                        visit(index, &mut self[index]);
                    }
                }
            }
        }
    }

    pub(super) fn push(&mut self, value: T) {
        self.thaw();
        match self {
            Self::Dense(values) => values.push(value),
            Self::Frozen(frozen) => {
                *frozen.edits.entry(frozen.len, TOP_SHIFT) = Some(Arc::new(value));
                frozen.len += 1;
            }
        }
    }

    pub(super) fn replace(&mut self, index: usize, value: T) {
        self.thaw();
        match self {
            Self::Dense(values) => values[index] = value,
            Self::Frozen(frozen) => {
                assert!(index < frozen.len, "arena slot out of bounds");
                *frozen.edits.entry(index, TOP_SHIFT) = Some(Arc::new(value));
            }
        }
    }

    pub(super) fn replace_get_old(&mut self, index: usize, value: T) -> T {
        self.thaw();
        match self {
            Self::Dense(values) => core::mem::replace(&mut values[index], value),
            Self::Frozen(frozen) => frozen.replace(index, value),
        }
    }

    pub(super) fn replace_if(
        &mut self,
        index: usize,
        value: T,
        condition: impl FnOnce(&T) -> bool,
    ) -> Option<T> {
        self.thaw();
        match self {
            Self::Dense(values) => {
                let slot = values.get_mut(index)?;
                condition(slot).then(|| core::mem::replace(slot, value))
            }
            Self::Frozen(frozen) => {
                condition(frozen.get(index)?).then(|| frozen.replace(index, value))
            }
        }
    }

    #[cfg(test)]
    pub(super) fn capacity(&self) -> usize {
        match self {
            Self::Dense(values) => values.capacity(),
            Self::Frozen(frozen) => frozen.base.capacity(),
        }
    }

    /// Only used while compacting a new arena or after exclusive ownership has been recovered.
    pub(super) fn dense_mut(&mut self) -> &mut Vec<T> {
        self.thaw();
        let Self::Dense(values) = self else {
            unreachable!("shared arena cannot be compacted");
        };
        values
    }
}

impl<T: Clone> Index<usize> for Slots<T> {
    type Output = T;

    #[inline]
    fn index(&self, index: usize) -> &T {
        match self {
            Self::Dense(values) => &values[index],
            Self::Frozen(frozen) => frozen.get(index).expect("arena slot out of bounds"),
        }
    }
}

impl<T: Clone> IndexMut<usize> for Slots<T> {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.thaw();
        match self {
            Self::Dense(values) => &mut values[index],
            Self::Frozen(frozen) => frozen.get_mut(index),
        }
    }
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self::Dense(Vec::new())
    }
}

impl<T: Clone> Frozen<T> {
    #[cold]
    fn try_take_dense(&mut self) -> Option<Vec<T>> {
        let mut values = core::mem::take(Arc::get_mut(&mut self.base)?);
        core::mem::take(&mut self.edits).apply(&mut values, 0, TOP_SHIFT);
        debug_assert_eq!(values.len(), self.len);
        Some(values)
    }

    #[cold]
    fn get(&self, index: usize) -> Option<&T> {
        if index < self.len {
            self.edits.get(index, TOP_SHIFT).or_else(|| self.base.get(index))
        } else {
            None
        }
    }

    #[cold]
    fn get_mut(&mut self, index: usize) -> &mut T {
        assert!(index < self.len, "arena slot out of bounds");
        let value = self
            .edits
            .entry(index, TOP_SHIFT)
            .get_or_insert_with(|| Arc::new(self.base[index].clone()));
        Arc::make_mut(value)
    }

    #[cold]
    fn replace(&mut self, index: usize, value: T) -> T {
        assert!(index < self.len, "arena slot out of bounds");
        self.edits
            .entry(index, TOP_SHIFT)
            .replace(Arc::new(value))
            .map_or_else(|| self.base[index].clone(), Arc::unwrap_or_clone)
    }
}

const RADIX_BITS: usize = 4;
const RADIX: usize = 1 << RADIX_BITS;
/// The arena's indices fit into 31 bits, including those of the blinded side table.
const TOP_SHIFT: usize = 28;

#[derive(Debug, Clone, Default)]
enum Edits<T> {
    #[default]
    Empty,
    Branch(Arc<[Self; RADIX]>),
    Leaf(Arc<[Option<Arc<T>>; RADIX]>),
}

impl<T: Clone> Edits<T> {
    fn get(&self, index: usize, shift: usize) -> Option<&T> {
        let digit = (index >> shift) & (RADIX - 1);
        match self {
            Self::Empty => None,
            Self::Branch(children) => children[digit].get(index, shift - RADIX_BITS),
            Self::Leaf(values) => values[digit].as_deref(),
        }
    }

    fn entry(&mut self, index: usize, shift: usize) -> &mut Option<Arc<T>> {
        let digit = (index >> shift) & (RADIX - 1);
        if matches!(self, Self::Empty) {
            *self = if shift == 0 {
                Self::Leaf(Arc::new(core::array::from_fn(|_| None)))
            } else {
                Self::Branch(Arc::new(core::array::from_fn(|_| Self::Empty)))
            };
        }

        match self {
            Self::Empty => unreachable!(),
            Self::Branch(children) => {
                Arc::make_mut(children)[digit].entry(index, shift - RADIX_BITS)
            }
            Self::Leaf(values) => &mut Arc::make_mut(values)[digit],
        }
    }

    /// Applies replacements in ascending index order so appended slots can reuse the base Vec.
    fn apply(self, values: &mut Vec<T>, prefix: usize, shift: usize) {
        match self {
            Self::Empty => {}
            Self::Branch(children) => {
                for (digit, child) in Arc::unwrap_or_clone(children).into_iter().enumerate() {
                    child.apply(values, prefix | (digit << shift), shift - RADIX_BITS);
                }
            }
            Self::Leaf(page) => {
                for (offset, entry) in Arc::unwrap_or_clone(page).into_iter().enumerate() {
                    if let Some(entry) = entry {
                        let index = prefix | offset;
                        let value = Arc::unwrap_or_clone(entry);
                        if index < values.len() {
                            values[index] = value;
                        } else {
                            debug_assert_eq!(index, values.len());
                            values.push(value);
                        }
                    }
                }
            }
        }
    }
}
