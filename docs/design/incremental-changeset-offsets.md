# Incremental Changeset Offset Storage

## Problem

Currently, `SegmentHeader.changeset_offsets` stores a `Vec<ChangesetOffset>` that gets fully serialized and written to the NippyJar config file on **every commit**:

```rust
pub struct SegmentHeader {
    // ...
    changeset_offsets: Option<Vec<ChangesetOffset>>,  // 16 bytes per block
}
```

For a segment with 500k blocks, this means **~8MB rewritten on every commit** - even when only appending a single block.

### Current Write Path
```
commit() 
  → NippyJarWriter::commit()
    → sync_all()           // flush data + row offsets
    → finalize()
      → freeze_config()    // atomic_write_file of ENTIRE NippyJar config
        → bincode serialize entire SegmentHeader (including Vec<ChangesetOffset>)
```

## Solution

Move changeset offsets to a **separate append-only sidecar file** with fixed-width records, keeping only a small descriptor in `SegmentHeader`.

### New Data Layout

```
segment_xxx.nippy      # existing data file
segment_xxx.off        # existing row offsets  
segment_xxx.conf       # config (now smaller - no Vec<ChangesetOffset>)
segment_xxx.csoff      # NEW: changeset offsets sidecar (fixed 16-byte records)
```

### New SegmentHeader Structure

```rust
pub struct SegmentHeader {
    expected_block_range: SegmentRangeInclusive,
    block_range: Option<SegmentRangeInclusive>,
    tx_range: Option<SegmentRangeInclusive>,
    segment: StaticFileSegment,
    // REMOVED: changeset_offsets: Option<Vec<ChangesetOffset>>
    // NEW: metadata only
    changeset_offsets_meta: Option<ChangesetOffsetsMeta>,
}

pub struct ChangesetOffsetsMeta {
    /// Number of valid entries (blocks) in the sidecar file
    len: u64,
    /// Format version for future compatibility
    version: u8,
}
```

### Sidecar File Format

Fixed-width binary records (16 bytes each):
```
[offset: u64 LE][num_changes: u64 LE]  // block N
[offset: u64 LE][num_changes: u64 LE]  // block N+1
...
```

Random access: `byte_position = block_index * 16`

## New Commit Protocol

### Append Path (adding blocks)

```
1. Write data rows to .nippy
2. Write row offsets to .off  
3. Append new ChangesetOffset records to .csoff
4. fdatasync(.csoff)                    // ensure offsets durable
5. Update SegmentHeader.changeset_offsets_meta.len += N
6. atomic_write_file(.conf)             // commit header last
```

**Crash safety**: If crash occurs:
- After step 3 but before 6: extra bytes in .csoff ignored (header has old len)
- After step 6: all data consistent

### Prune Path (removing blocks from tail)

```
1. Update SegmentHeader.changeset_offsets_meta.len = new_len
2. atomic_write_file(.conf)             // commit header
3. (Optional) Truncate .csoff to len * 16 bytes
```

**Crash safety**: Header is source of truth for valid length. Garbage beyond `len * 16` is ignored.

## Implementation Plan

### Phase 1: Add sidecar infrastructure
- [ ] Add `ChangesetOffsetsMeta` struct in `segment.rs`
- [ ] Add `.csoff` file path helpers in `NippyJar`
- [ ] Add `ChangesetOffsetWriter` for append-only writes
- [ ] Add `ChangesetOffsetReader` for O(1) lookups

### Phase 2: Modify SegmentHeader
- [ ] Replace `changeset_offsets: Option<Vec<ChangesetOffset>>` with `changeset_offsets_meta`
- [ ] Update serialize/deserialize with backwards compatibility
- [ ] Migrate existing data on open (one-time conversion)

### Phase 3: Update write path
- [ ] Modify `StaticFileProviderRW::commit()` to use new protocol
- [ ] Ensure fdatasync ordering: data → offsets → header
- [ ] Update prune logic to shrink `len` instead of rewriting

### Phase 4: Update read path  
- [ ] Modify `changeset_offsets()` to read from sidecar
- [ ] Cache hot entries in memory if needed
- [ ] Add mmap support for large segments

## API Changes

### Before
```rust
impl SegmentHeader {
    pub fn changeset_offsets(&self) -> Option<&Vec<ChangesetOffset>>;
}
```

### After
```rust
impl SegmentHeader {
    pub fn changeset_offsets_meta(&self) -> Option<&ChangesetOffsetsMeta>;
}

// New reader for lookups
pub struct ChangesetOffsetReader { ... }

impl ChangesetOffsetReader {
    pub fn get(&self, block_index: u64) -> Option<ChangesetOffset>;
    pub fn get_range(&self, range: Range<u64>) -> Vec<ChangesetOffset>;
}
```

## Performance Impact

| Operation | Before | After |
|-----------|--------|-------|
| Append 1 block | O(total_blocks) write | O(1) write (16 bytes) |
| Commit overhead | ~8MB for 500k blocks | ~100 bytes (header only) |
| Random lookup | O(1) from memory | O(1) from mmap/pread |
| Prune | O(remaining_blocks) | O(1) (len update only) |

## Migration Strategy

On segment open, if `changeset_offsets` (old Vec) is present and `changeset_offsets_meta` is absent:
1. Write Vec contents to new `.csoff` file
2. Set `changeset_offsets_meta.len = vec.len()`
3. Clear old `changeset_offsets` field
4. Commit header

This is a one-time migration per segment file.

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Commit ordering bug (header before offsets) | Enforce fdatasync on .csoff before header commit |
| Crash during append leaves partial record | Only advance `len` after fdatasync; validate `file_size >= len * 16` on open |
| Prune from middle (not just tail) | Currently unsupported; would need tombstones or compaction |
| Platform fsync semantics | Fsync directory after rename on Linux for full durability |
