use std::{fmt, slice, sync::Arc};

const OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN: usize = 64;

#[derive(Debug)]
pub(crate) enum DbCursorState<K, V> {
    Unpositioned,
    Positioned((K, V)),
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
            Self::Unpositioned | Self::Wiped => None,
        }
    }

    pub(crate) fn set_entry(&mut self, entry: Option<(K, V)>) {
        if !self.is_wiped() {
            *self = entry.map(Self::Positioned).unwrap_or(Self::Unpositioned);
        }
    }
}

impl<K: PartialEq, V> DbCursorState<K, V> {
    pub(crate) fn is_positioned_at(&self, key: &K) -> bool {
        matches!(self, Self::Positioned((db_key, _)) if db_key == key)
    }
}

#[derive(Debug)]
pub(crate) struct PositionedOverlayCursor<'a, K, V> {
    layers: Vec<PositionedOverlayLayer<'a, K, V>>,
}

impl<K, V> Default for PositionedOverlayCursor<'_, K, V> {
    fn default() -> Self {
        Self { layers: Vec::new() }
    }
}

impl<'a, K, V> PositionedOverlayCursor<'a, K, V> {
    pub(crate) fn new<O>(layers: &'a [OverlayLayer<O, K, V>]) -> Self {
        let mut cursor = Self { layers: Vec::with_capacity(layers.len()) };
        cursor.retarget(layers);
        cursor
    }

    pub(crate) fn reset(&mut self) {
        for layer in &mut self.layers {
            layer.reset();
        }
    }

    pub(crate) fn retarget<O>(&mut self, layers: &'a [OverlayLayer<O, K, V>]) {
        self.layers.clear();
        self.layers.extend(layers.iter().map(|layer| PositionedOverlayLayer::new(layer.entries())));
    }
}

impl<K, V> PositionedOverlayCursor<'_, K, V>
where
    K: Ord,
{
    #[inline(always)]
    pub(crate) fn seek_from(&mut self, start: usize, key: &K) {
        for layer in &mut self.layers[start..] {
            let _ = layer.seek(key);
        }
    }

    #[inline(always)]
    pub(crate) fn seek_until_exact(&mut self, key: &K) -> Option<(usize, &V)> {
        for (layer_idx, layer) in self.layers.iter_mut().enumerate() {
            let Some(entry) = layer.seek_exact(key) else {
                continue;
            };
            return Some((layer_idx, &entry.1))
        }

        None
    }

    #[inline(always)]
    pub(crate) fn first_after(&mut self, key: &K) {
        for layer in &mut self.layers {
            let _ = layer.first_after(key);
        }
    }

    #[inline(always)]
    pub(crate) fn highest_priority_value_at(&self, key: &K) -> Option<&V> {
        self.layers.iter().find_map(|layer| {
            layer.current().and_then(|(entry_key, value)| (entry_key == key).then_some(value))
        })
    }

    #[inline(always)]
    pub(crate) fn advance_key(&mut self, key: &K) {
        for layer in &mut self.layers {
            if layer.current().is_some_and(|(entry_key, _)| entry_key == key) {
                let _ = layer.first_after(key);
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
        self.layers.iter().filter_map(|layer| layer.current().map(|(key, _)| *key)).min()
    }
}

#[derive(Debug)]
struct PositionedOverlayLayer<'a, K, V> {
    entries: &'a [(K, V)],
    position: usize,
}

impl<'a, K, V> PositionedOverlayLayer<'a, K, V> {
    const fn new(entries: &'a [(K, V)]) -> Self {
        Self { entries, position: 0 }
    }

    fn current(&self) -> Option<&'a (K, V)> {
        self.entries.get(self.position)
    }

    const fn reset(&mut self) {
        self.position = 0;
    }
}

impl<'a, K, V> PositionedOverlayLayer<'a, K, V>
where
    K: Ord,
{
    #[inline(always)]
    fn seek(&mut self, key: &K) -> Option<&'a (K, V)> {
        let _ = seek_overlay_entries(self.entries, &mut self.position, key);
        self.current()
    }

    #[inline(always)]
    fn seek_exact(&mut self, key: &K) -> Option<&'a (K, V)> {
        let _ = seek_overlay_entries_exact(self.entries, &mut self.position, key)?;
        self.current()
    }

    #[inline(always)]
    fn first_after(&mut self, key: &K) -> Option<&'a (K, V)> {
        let _ = seek_overlay_entries_after(self.entries, &mut self.position, key);
        self.current()
    }
}

#[inline(always)]
fn seek_overlay_entries<K, V>(entries: &[(K, V)], position: &mut usize, key: &K) -> Option<usize>
where
    K: Ord,
{
    if let Some((entry_key, _)) = entries.get(*position) {
        match entry_key.cmp(key) {
            std::cmp::Ordering::Less => *position += 1,
            std::cmp::Ordering::Equal | std::cmp::Ordering::Greater => return Some(*position),
        }
    }

    let remaining = &entries[*position..];
    let advance = if remaining.len() >= OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN {
        remaining.partition_point(|(entry_key, _)| entry_key < key)
    } else {
        let mut advance = 0;
        while advance < remaining.len() && &remaining[advance].0 < key {
            advance += 1;
        }
        advance
    };

    *position += advance;
    (*position < entries.len()).then_some(*position)
}

#[inline(always)]
fn seek_overlay_entries_exact<K, V>(
    entries: &[(K, V)],
    position: &mut usize,
    key: &K,
) -> Option<usize>
where
    K: Ord,
{
    if let Some((entry_key, _)) = entries.get(*position) {
        match entry_key.cmp(key) {
            std::cmp::Ordering::Less => *position += 1,
            std::cmp::Ordering::Equal => return Some(*position),
            std::cmp::Ordering::Greater => return None,
        }
    }

    let remaining = &entries[*position..];
    if remaining.len() >= OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN {
        *position += remaining.partition_point(|(entry_key, _)| entry_key < key);
        return entries
            .get(*position)
            .and_then(|(entry_key, _)| (entry_key == key).then_some(*position))
    }

    for (advance, (entry_key, _)) in remaining.iter().enumerate() {
        match entry_key.cmp(key) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => {
                *position += advance;
                return Some(*position)
            }
            std::cmp::Ordering::Greater => {
                *position += advance;
                return None
            }
        }
    }

    *position = entries.len();
    None
}

#[inline(always)]
fn seek_overlay_entries_after<K, V>(
    entries: &[(K, V)],
    position: &mut usize,
    key: &K,
) -> Option<usize>
where
    K: Ord,
{
    if let Some((entry_key, _)) = entries.get(*position) {
        match entry_key.cmp(key) {
            std::cmp::Ordering::Greater => return Some(*position),
            std::cmp::Ordering::Less | std::cmp::Ordering::Equal => *position += 1,
        }
    }

    let remaining = &entries[*position..];
    let advance = if remaining.len() >= OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN {
        remaining.partition_point(|(entry_key, _)| entry_key <= key)
    } else {
        let mut advance = 0;
        while advance < remaining.len() && &remaining[advance].0 <= key {
            advance += 1;
        }
        advance
    };

    *position += advance;
    (*position < entries.len()).then_some(*position)
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
        assert_eq!(cursor.layers[0].position, 201);

        assert_eq!(cursor.seek_until_exact(&25), None);
        assert_eq!(cursor.layers[0].position, 201);

        cursor.first_after(&250);
        assert_eq!(cursor.min_current_key(), None);
        assert_eq!(cursor.layers[0].position, 201);

        cursor.first_after(&25);
        assert_eq!(cursor.min_current_key(), None);
        assert_eq!(cursor.layers[0].position, 201);
    }
}
