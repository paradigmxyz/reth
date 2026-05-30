use std::{fmt, slice, sync::Arc};

const OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN: usize = 64;

#[derive(Debug)]
pub(crate) enum DbCursorState<K, V> {
    Unpositioned,
    Positioned((K, V)),
    Exhausted,
    Wiped,
}

impl<K, V> DbCursorState<K, V> {
    pub(crate) const fn new(cursor_wiped: bool) -> Self {
        if cursor_wiped {
            Self::Wiped
        } else {
            Self::Unpositioned
        }
    }

    pub(crate) const fn is_wiped(&self) -> bool {
        matches!(self, Self::Wiped)
    }

    pub(crate) const fn entry(&self) -> Option<&(K, V)> {
        match self {
            Self::Positioned(entry) => Some(entry),
            Self::Unpositioned | Self::Exhausted | Self::Wiped => None,
        }
    }

    pub(crate) fn set_entry(&mut self, entry: Option<(K, V)>) {
        if !self.is_wiped() {
            *self = entry.map(Self::Positioned).unwrap_or(Self::Exhausted);
        }
    }

    pub(crate) fn reset_position(&mut self) {
        if !self.is_wiped() {
            *self = Self::Unpositioned;
        }
    }
}

impl<K: Ord, V> DbCursorState<K, V> {
    pub(crate) fn should_seek(&self, key: &K) -> bool {
        match self {
            Self::Unpositioned => true,
            Self::Positioned((db_key, _)) => db_key < key,
            Self::Exhausted | Self::Wiped => false,
        }
    }

    pub(crate) fn exact_entry(&self, key: &K) -> Option<&(K, V)> {
        match self {
            Self::Positioned((db_key, _)) if db_key == key => self.entry(),
            Self::Unpositioned | Self::Positioned(_) | Self::Exhausted | Self::Wiped => None,
        }
    }

    pub(crate) fn may_contain_exact(&self, key: &K) -> bool {
        match self {
            Self::Unpositioned => true,
            Self::Positioned((db_key, _)) => db_key <= key,
            Self::Exhausted | Self::Wiped => false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PositionedOverlayCursor<'a, K, V> {
    cursors: Vec<PositionedOverlayLayerCursor<'a, K, V>>,
}

impl<K, V> Default for PositionedOverlayCursor<'_, K, V> {
    fn default() -> Self {
        Self { cursors: Vec::new() }
    }
}

impl<'a, K, V> PositionedOverlayCursor<'a, K, V> {
    #[cfg(test)]
    pub(crate) fn new<O>(layers: &'a [OverlayLayer<O, K, V>]) -> Self {
        Self::with_capacity(layers, layers.len())
    }

    pub(crate) fn with_capacity<O>(layers: &'a [OverlayLayer<O, K, V>], capacity: usize) -> Self {
        let mut this = Self { cursors: Vec::with_capacity(capacity.max(layers.len())) };
        this.retarget(layers);
        this
    }

    pub(crate) fn reset(&mut self) {
        for cursor in &mut self.cursors {
            cursor.reset();
        }
    }

    pub(crate) fn retarget<O>(&mut self, layers: &'a [OverlayLayer<O, K, V>]) {
        debug_assert!(self.cursors.capacity() >= layers.len());
        self.cursors.clear();
        self.cursors.extend(layers.iter().map(PositionedOverlayLayerCursor::new));
    }
}

impl<K, V> PositionedOverlayCursor<'_, K, V>
where
    K: Ord,
{
    #[inline(always)]
    pub(crate) fn seek_from(&mut self, start: usize, key: &K) {
        for cursor in self.cursors.iter_mut().skip(start) {
            let _ = cursor.seek(key);
        }
    }

    #[inline(always)]
    pub(crate) fn seek_until_exact(&mut self, key: &K) -> Option<(usize, &V)> {
        for (layer_idx, cursor) in self.cursors.iter_mut().enumerate() {
            if let Some((_, value)) = cursor.seek_exact(key) {
                return Some((layer_idx, value))
            }
        }

        None
    }

    #[inline(always)]
    pub(crate) fn first_after(&mut self, key: &K) {
        for cursor in &mut self.cursors {
            let _ = cursor.first_after(key);
        }
    }

    #[inline(always)]
    pub(crate) fn highest_priority_value_at(&self, key: &K) -> Option<&V> {
        self.cursors.iter().find_map(|cursor| {
            cursor.current().and_then(|(entry_key, value)| (entry_key == key).then_some(value))
        })
    }

    #[inline(always)]
    pub(crate) fn advance_key(&mut self, key: &K) {
        for cursor in &mut self.cursors {
            if cursor.current().is_some_and(|(entry_key, _)| entry_key == key) {
                let _ = cursor.first_after(key);
            }
        }
    }
}

impl<K, V> PositionedOverlayCursor<'_, K, V>
where
    K: Copy + Ord,
{
    #[inline(always)]
    pub(crate) fn min_current_key(&self) -> Option<K> {
        self.cursors.iter().filter_map(|cursor| cursor.current().map(|(key, _)| *key)).min()
    }
}

#[derive(Debug)]
struct PositionedOverlayLayerCursor<'a, K, V> {
    entries: &'a [(K, V)],
    position: usize,
}

impl<'a, K, V> PositionedOverlayLayerCursor<'a, K, V> {
    fn new<O>(layer: &'a OverlayLayer<O, K, V>) -> Self {
        Self { entries: layer.entries(), position: 0 }
    }

    #[inline(always)]
    fn current(&self) -> Option<&'a (K, V)> {
        self.entries.get(self.position)
    }

    const fn reset(&mut self) {
        self.position = 0;
    }
}

impl<'a, K, V> PositionedOverlayLayerCursor<'a, K, V>
where
    K: Ord,
{
    #[inline(always)]
    fn seek(&mut self, key: &K) -> Option<&'a (K, V)> {
        if let Some((entry_key, _)) = self.current() {
            match entry_key.cmp(key) {
                std::cmp::Ordering::Less => self.position += 1,
                std::cmp::Ordering::Equal | std::cmp::Ordering::Greater => return self.current(),
            }
        }

        let remaining = &self.entries[self.position..];
        let advance = if remaining.len() >= OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN {
            remaining.partition_point(|(entry_key, _)| entry_key < key)
        } else {
            let mut advance = 0;
            while advance < remaining.len() && &remaining[advance].0 < key {
                advance += 1;
            }
            advance
        };

        self.position += advance;
        self.current()
    }

    #[inline(always)]
    fn seek_exact(&mut self, key: &K) -> Option<&'a (K, V)> {
        if let Some(current @ (entry_key, _)) = self.current() {
            match entry_key.cmp(key) {
                std::cmp::Ordering::Less => self.position += 1,
                std::cmp::Ordering::Equal => return Some(current),
                std::cmp::Ordering::Greater => return None,
            }
        }

        let remaining = &self.entries[self.position..];
        if remaining.len() >= OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN {
            self.position += remaining.partition_point(|(entry_key, _)| entry_key < key);
            let current = self.current()?;
            return (&current.0 == key).then_some(current)
        }

        for (advance, (entry_key, _)) in remaining.iter().enumerate() {
            match entry_key.cmp(key) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    self.position += advance;
                    return self.current()
                }
                std::cmp::Ordering::Greater => {
                    self.position += advance;
                    return None
                }
            }
        }

        self.position = self.entries.len();
        None
    }

    #[inline(always)]
    fn first_after(&mut self, key: &K) -> Option<&'a (K, V)> {
        if let Some((entry_key, _)) = self.current() {
            match entry_key.cmp(key) {
                std::cmp::Ordering::Greater => return self.current(),
                std::cmp::Ordering::Less | std::cmp::Ordering::Equal => self.position += 1,
            }
        }

        let remaining = &self.entries[self.position..];
        let advance = if remaining.len() >= OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN {
            remaining.partition_point(|(entry_key, _)| entry_key <= key)
        } else {
            let mut advance = 0;
            while advance < remaining.len() && &remaining[advance].0 <= key {
                advance += 1;
            }
            advance
        };

        self.position += advance;
        self.current()
    }
}

#[derive(Clone)]
pub(crate) struct OverlayLayer<O, K, V> {
    _owner: Arc<O>,
    entries_ptr: *const (K, V),
    entries_len: usize,
}

impl<O, K, V> OverlayLayer<O, K, V> {
    pub(crate) const fn new(owner: Arc<O>, entries: &[(K, V)]) -> Self {
        Self { _owner: owner, entries_ptr: entries.as_ptr(), entries_len: entries.len() }
    }

    pub(crate) const fn entries(&self) -> &[(K, V)] {
        // SAFETY: `entries_ptr` and `entries_len` are captured from a slice inside `_owner`.
        // The `Arc` keeps that allocation alive, and the overlay owners are never mutated through
        // this layer.
        unsafe { slice::from_raw_parts(self.entries_ptr, self.entries_len) }
    }
}

// SAFETY: the raw pointer only targets immutable data owned by `_owner`, and `_owner` is retained
// for at least as long as the pointer is used.
unsafe impl<O: Send + Sync, K: Sync, V: Sync> Send for OverlayLayer<O, K, V> {}
// SAFETY: see the `Send` impl; shared access only exposes immutable slices.
unsafe impl<O: Send + Sync, K: Sync, V: Sync> Sync for OverlayLayer<O, K, V> {}

impl<O, K, V> fmt::Debug for OverlayLayer<O, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayLayer").field("entries_len", &self.entries_len).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(entries: Arc<Vec<(u8, u8)>>) -> OverlayLayer<Vec<(u8, u8)>, u8, u8> {
        OverlayLayer::new(Arc::clone(&entries), entries.as_slice())
    }

    #[test]
    fn seek_reuses_current_position_when_it_already_satisfies_bound() {
        let entries = Arc::new((0..=200).map(|value| (value, value)).collect::<Vec<_>>());
        let overlay = [layer(entries)];
        let mut cursor = PositionedOverlayCursor::new(&overlay);

        cursor.seek_from(0, &100);
        assert_eq!(cursor.min_current_key(), Some(100));
        cursor.seek_from(0, &100);
        assert_eq!(cursor.min_current_key(), Some(100));
        cursor.first_after(&99);
        assert_eq!(cursor.min_current_key(), Some(100));
        cursor.first_after(&100);
        assert_eq!(cursor.min_current_key(), Some(101));
    }

    #[test]
    fn seek_does_not_move_backwards_from_current_position() {
        let entries = Arc::new((0..=200).map(|value| (value, value)).collect::<Vec<_>>());
        let overlay = [layer(entries)];
        let mut cursor = PositionedOverlayCursor::new(&overlay);

        cursor.seek_from(0, &150);
        assert_eq!(cursor.min_current_key(), Some(150));
        cursor.seek_from(0, &75);
        assert_eq!(cursor.min_current_key(), Some(150));
        assert_eq!(cursor.seek_until_exact(&25), None);
        assert_eq!(cursor.min_current_key(), Some(150));
    }

    #[test]
    fn seek_does_not_recover_after_past_end() {
        let entries = Arc::new((0..=200).map(|value| (value, value)).collect::<Vec<_>>());
        let overlay = [layer(entries)];
        let mut cursor = PositionedOverlayCursor::new(&overlay);

        cursor.seek_from(0, &250);
        assert_eq!(cursor.min_current_key(), None);
        assert_eq!(
            cursor.cursors.iter().map(|cursor| cursor.position).collect::<Vec<_>>(),
            vec![201]
        );

        assert_eq!(cursor.seek_until_exact(&25), None);
        assert_eq!(
            cursor.cursors.iter().map(|cursor| cursor.position).collect::<Vec<_>>(),
            vec![201]
        );

        cursor.first_after(&250);
        assert_eq!(cursor.min_current_key(), None);
        assert_eq!(
            cursor.cursors.iter().map(|cursor| cursor.position).collect::<Vec<_>>(),
            vec![201]
        );

        cursor.first_after(&25);
        assert_eq!(cursor.min_current_key(), None);
        assert_eq!(
            cursor.cursors.iter().map(|cursor| cursor.position).collect::<Vec<_>>(),
            vec![201]
        );
    }

    #[test]
    fn retarget_reuses_cursor_allocation() {
        let first_entries = Arc::new(vec![(1, 1)]);
        let second_entries = Arc::new(vec![(2, 2)]);
        let first_overlay = [layer(Arc::clone(&first_entries))];
        let second_overlay = [layer(first_entries), layer(second_entries)];
        let mut cursor = PositionedOverlayCursor::with_capacity(&first_overlay, 2);
        let capacity = cursor.cursors.capacity();
        let ptr = cursor.cursors.as_ptr();

        cursor.retarget(&second_overlay);
        assert_eq!(cursor.cursors.capacity(), capacity);
        assert_eq!(cursor.cursors.as_ptr(), ptr);

        cursor.reset();
        assert_eq!(cursor.cursors.capacity(), capacity);
        assert_eq!(cursor.cursors.as_ptr(), ptr);
    }
}
