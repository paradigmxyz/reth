use super::nodes::ArenaSparseNode;
use alloc::vec::Vec;
use core::{
    iter::Enumerate,
    mem,
    ops::{Index as OpsIndex, IndexMut},
    slice,
};

/// Index into [`NodeArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Index {
    slot: u32,
    generation: u32,
}

impl Index {
    const fn new(slot: usize, generation: u32) -> Self {
        debug_assert!(slot <= u32::MAX as usize);
        Self { slot: slot as u32, generation }
    }

    const fn slot(self) -> usize {
        self.slot as usize
    }
}

/// Arena allocating sparse trie nodes with stable, generation-checked indices.
#[derive(Debug, Default, Clone)]
pub(super) struct NodeArena {
    slots: Vec<ArenaSlot>,
    free: Vec<u32>,
    len: usize,
}

impl NodeArena {
    pub(super) const fn new() -> Self {
        Self { slots: Vec::new(), free: Vec::new(), len: 0 }
    }

    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self { slots: Vec::with_capacity(capacity), free: Vec::new(), len: 0 }
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn reserve(&mut self, additional: usize) {
        self.slots.reserve(additional.saturating_sub(self.free.len()));
    }

    pub(super) fn memory_size(&self) -> usize {
        self.slots.capacity() * mem::size_of::<ArenaSlot>() +
            self.free.capacity() * mem::size_of::<u32>()
    }

    #[cfg(test)]
    pub(super) const fn slot_size() -> usize {
        mem::size_of::<ArenaSlot>()
    }

    pub(super) fn insert(&mut self, value: ArenaSparseNode) -> Index {
        self.len += 1;

        if let Some(slot_idx) = self.free.pop() {
            let slot_idx = slot_idx as usize;
            let slot = &mut self.slots[slot_idx];
            debug_assert!(slot.value.is_none());
            slot.value = Some(value);
            return Index::new(slot_idx, slot.generation);
        }

        let slot_idx = self.slots.len();
        self.slots.push(ArenaSlot { generation: 1, value: Some(value) });
        Index::new(slot_idx, 1)
    }

    pub(super) fn remove(&mut self, idx: Index) -> Option<ArenaSparseNode> {
        let slot = self.slots.get_mut(idx.slot())?;
        if slot.generation != idx.generation {
            return None;
        }

        let value = slot.value.take()?;
        slot.generation = slot.generation.wrapping_add(1).max(1);
        self.free.push(idx.slot);
        self.len -= 1;
        Some(value)
    }

    pub(super) fn get(&self, idx: Index) -> Option<&ArenaSparseNode> {
        let slot = self.slots.get(idx.slot())?;
        (slot.generation == idx.generation).then(|| slot.value.as_ref()).flatten()
    }

    pub(super) fn contains_key(&self, idx: Index) -> bool {
        self.get(idx).is_some()
    }

    pub(super) fn iter(&self) -> ArenaIter<'_> {
        ArenaIter { inner: self.slots.iter().enumerate() }
    }

    /// Prefetches the cache line containing the slot for `idx`.
    ///
    /// This intentionally avoids validating the generation or loading the node value. AMAC-style
    /// traversal wants to issue the memory request before the actual dereference.
    #[allow(dead_code)]
    #[inline(always)]
    pub(super) fn prefetch(&self, idx: Index) {
        if idx.slot() < self.slots.len() {
            // SAFETY: The bounds check above guarantees the computed address is within the
            // allocation. The pointer is used only as a prefetch hint and is never dereferenced.
            prefetch_read(unsafe { self.slots.as_ptr().add(idx.slot()) });
        }
    }
}

impl OpsIndex<Index> for NodeArena {
    type Output = ArenaSparseNode;

    fn index(&self, idx: Index) -> &Self::Output {
        self.get(idx).expect("invalid arena index")
    }
}

impl IndexMut<Index> for NodeArena {
    fn index_mut(&mut self, idx: Index) -> &mut Self::Output {
        let slot = self.slots.get_mut(idx.slot()).expect("invalid arena index");
        if slot.generation != idx.generation {
            panic!("invalid arena index");
        }
        slot.value.as_mut().expect("invalid arena index")
    }
}

impl<'a> IntoIterator for &'a NodeArena {
    type Item = (Index, &'a ArenaSparseNode);
    type IntoIter = ArenaIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut NodeArena {
    type Item = (Index, &'a mut ArenaSparseNode);
    type IntoIter = ArenaIterMut<'a>;

    fn into_iter(self) -> Self::IntoIter {
        ArenaIterMut { inner: self.slots.iter_mut().enumerate() }
    }
}

#[derive(Debug, Clone)]
struct ArenaSlot {
    generation: u32,
    value: Option<ArenaSparseNode>,
}

pub(super) struct ArenaIter<'a> {
    inner: Enumerate<slice::Iter<'a, ArenaSlot>>,
}

impl<'a> Iterator for ArenaIter<'a> {
    type Item = (Index, &'a ArenaSparseNode);

    fn next(&mut self) -> Option<Self::Item> {
        for (slot_idx, slot) in self.inner.by_ref() {
            if let Some(value) = slot.value.as_ref() {
                return Some((Index::new(slot_idx, slot.generation), value));
            }
        }
        None
    }
}

pub(super) struct ArenaIterMut<'a> {
    inner: Enumerate<slice::IterMut<'a, ArenaSlot>>,
}

impl<'a> Iterator for ArenaIterMut<'a> {
    type Item = (Index, &'a mut ArenaSparseNode);

    fn next(&mut self) -> Option<Self::Item> {
        for (slot_idx, slot) in self.inner.by_ref() {
            if let Some(value) = slot.value.as_mut() {
                return Some((Index::new(slot_idx, slot.generation), value));
            }
        }
        None
    }
}

#[inline(always)]
fn prefetch_read<T>(ptr: *const T) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // SAFETY: `prfm` is a non-faulting prefetch hint on supported AArch64 targets. The pointer
        // is not dereferenced by Rust code and may refer to any address.
        core::arch::asm!(
            "prfm pldl1keep, [{addr}]",
            addr = in(reg) ptr,
            options(nostack, readonly)
        );
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        // SAFETY: `_mm_prefetch` is a cache hint. It does not dereference the pointer in Rust and
        // imposes no validity requirement beyond forming the raw pointer value.
        core::arch::x86_64::_mm_prefetch(ptr.cast::<i8>(), core::arch::x86_64::_MM_HINT_T0);
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        let _ = ptr;
    }
}
