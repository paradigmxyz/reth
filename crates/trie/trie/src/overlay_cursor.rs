use std::{fmt, slice, sync::Arc};

const OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN: usize = 64;

#[derive(Debug)]
pub(crate) enum DbCursorState<K, V> {
    Unpositioned,
    Positioned { entry: (K, V), position_valid: bool },
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
            Self::Positioned { entry, .. } => Some(entry),
            Self::Unpositioned | Self::Wiped => None,
        }
    }

    pub(crate) const fn position_valid(&self) -> bool {
        matches!(self, Self::Positioned { position_valid: true, .. })
    }

    pub(crate) fn set_entry(&mut self, entry: Option<(K, V)>) {
        if !self.is_wiped() {
            *self = entry
                .map(|entry| Self::Positioned { entry, position_valid: true })
                .unwrap_or(Self::Unpositioned);
        }
    }

    pub(crate) const fn validate_position(&mut self) {
        if let Self::Positioned { position_valid, .. } = self {
            *position_valid = true;
        }
    }

    pub(crate) const fn invalidate_position(&mut self) {
        if let Self::Positioned { position_valid, .. } = self {
            *position_valid = false;
        }
    }
}

impl<K: PartialEq, V> DbCursorState<K, V> {
    pub(crate) fn is_positioned_at(&self, key: &K) -> bool {
        matches!(self, Self::Positioned { entry: (db_key, _), .. } if db_key == key)
    }
}

#[derive(Debug)]
pub(crate) struct PositionedOverlayCursor<'a, O, K, V> {
    layers: &'a [OverlayLayer<O, K, V>],
    positions: Vec<usize>,
}

impl<O, K, V> Default for PositionedOverlayCursor<'_, O, K, V> {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl<'a, O, K, V> PositionedOverlayCursor<'a, O, K, V> {
    pub(crate) fn new(layers: &'a [OverlayLayer<O, K, V>]) -> Self {
        Self { layers, positions: vec![0; layers.len()] }
    }

    pub(crate) fn reset(&mut self) {
        self.positions.fill(0);
    }

    pub(crate) fn retarget(&mut self, layers: &'a [OverlayLayer<O, K, V>]) {
        self.layers = layers;
        self.positions.clear();
        self.positions.resize(layers.len(), 0);
    }
}

impl<O, K, V> PositionedOverlayCursor<'_, O, K, V>
where
    K: Ord,
{
    pub(crate) fn seek_exact(&mut self, key: &K) -> Option<&V> {
        let Self { layers, positions } = self;

        layers.iter().enumerate().find_map(|(layer_idx, layer)| {
            let entries = layer.entries();
            let idx = seek_overlay_entries(
                entries,
                positions.get_mut(layer_idx),
                key,
                OverlaySeekMode::Inclusive,
            )?;
            (&entries[idx].0 == key).then_some(&entries[idx].1)
        })
    }

    pub(crate) fn highest_priority_value_at(&self, key: &K) -> Option<&V> {
        self.layers.iter().enumerate().find_map(|(layer_idx, layer)| {
            let entries = layer.entries();
            if let Some(position) = self.positions.get(layer_idx) {
                entries
                    .get(*position)
                    .and_then(|(entry_key, value)| (entry_key == key).then_some(value))
            } else {
                let idx = entries.binary_search_by(|(entry_key, _)| entry_key.cmp(key)).ok()?;
                Some(&entries[idx].1)
            }
        })
    }
}

impl<O, K, V> PositionedOverlayCursor<'_, O, K, V>
where
    K: Copy + Ord,
{
    pub(crate) fn next_key(&mut self, key: &K, inclusive: bool) -> Option<K> {
        let mode = if inclusive { OverlaySeekMode::Inclusive } else { OverlaySeekMode::Exclusive };
        let Self { layers, positions } = self;

        layers
            .iter()
            .enumerate()
            .filter_map(|(layer_idx, layer)| {
                let entries = layer.entries();
                let idx = seek_overlay_entries(entries, positions.get_mut(layer_idx), key, mode)?;
                Some(entries[idx].0)
            })
            .min()
    }
}

#[derive(Clone, Copy)]
enum OverlaySeekMode {
    Inclusive,
    Exclusive,
}

impl OverlaySeekMode {
    fn skips<K: Ord>(self, entry_key: &K, bound: &K) -> bool {
        match self {
            Self::Inclusive => entry_key < bound,
            Self::Exclusive => entry_key <= bound,
        }
    }
}

fn seek_overlay_entries<K, V>(
    entries: &[(K, V)],
    mut position: Option<&mut usize>,
    key: &K,
    mode: OverlaySeekMode,
) -> Option<usize>
where
    K: Ord,
{
    let mut start =
        position.as_ref().map(|position| **position).unwrap_or_default().min(entries.len());
    if start > 0 && !mode.skips(&entries[start - 1].0, key) {
        start = 0;
    }

    let remaining = &entries[start..];
    let advance = if remaining.len() >= OVERLAY_CURSOR_PARTITION_POINT_MIN_LEN {
        remaining.partition_point(|(entry_key, _)| mode.skips(entry_key, key))
    } else {
        let mut advance = 0;
        while advance < remaining.len() && mode.skips(&remaining[advance].0, key) {
            advance += 1;
        }
        advance
    };

    let idx = start + advance;
    if let Some(position) = position.as_mut() {
        **position = idx;
    }
    (idx < entries.len()).then_some(idx)
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
