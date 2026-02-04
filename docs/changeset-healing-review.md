# Changeset Offset Healing - Code Review Report

**Date**: February 4, 2026  
**Status**: ✅ COMPLETE - All issues fixed, all tests passing  
**Reviewer**: AI Analysis (Oracle + Librarian)  
**Files Reviewed**:
- `crates/storage/provider/src/providers/static_file/writer.rs`
- `crates/static-file/types/src/changeset_offsets.rs`
- `crates/storage/provider/src/providers/static_file/writer_tests.rs`

---

## Executive Summary

The three-way healing implementation for changeset offset sidecars is **functionally correct** for the primary crash recovery scenarios. The implementation properly handles the most common failure modes and aligns with NippyJar's existing healing patterns. However, there are opportunities for hardening against edge cases and potential corruption scenarios.

---

## Implementation Overview

### What Changed

The `heal_changeset_sidecar()` function was rewritten to perform a **three-way consistency check** between:

1. **Header** (`header_claims_blocks`) - What the SegmentHeader says is committed
2. **NippyJar** (`actual_nippy_rows`) - Ground truth of actual data rows
3. **Sidecar File** (`actual_sidecar_blocks`) - Physical file size / 16 bytes

### Healing Algorithm

```
1. Read all three sources of truth
2. Validate each sidecar block: offset + num_changes <= actual_nippy_rows
3. Count valid blocks (stop at first invalid)
4. Truncate sidecar if it has invalid blocks
5. Update header via prune() if header doesn't match valid count
6. Commit header if modified
```

---

## Crash Scenarios Analysis

### ✅ Fully Covered Scenarios

| Scenario | Description | Healing Behavior |
|----------|-------------|------------------|
| **Partial Record Write** | Crash mid-write of 16-byte record | Align to 16-byte boundary, truncate partial |
| **Append Crash (Pre-Header)** | Sidecar synced, header not committed | Sidecar has more blocks than header; header stays authoritative |
| **Prune Crash (Sidecar First)** | Sidecar truncated, header stale | Header claims 100, sidecar has 80 → header pruned to 80 |
| **Invalid Offsets** | Sidecar points past NippyJar EOF | Validate each block, truncate invalid tail |
| **Empty Sidecar** | File doesn't exist or is empty | Creates new file with 0 blocks |
| **Empty NippyJar** | 0 rows in jar | Only zero-change blocks remain valid |

### ⚠️ Out of Scope (Not Crash Recovery)

| Scenario | Why Out of Scope |
|----------|------------------|
| **Corrupted middle sidecar entry** | Hard corruption, not crash recovery |
| **Missing sidecar with non-zero H/J** | File deletion, not crash recovery |
| **Non-monotonic offsets** | Data integrity issue, not crash safety |

---

## Detailed Findings

### 1. Header Enlargement Risk — FIXED ✅

**Location**: `writer.rs` line 414

**Status**: This issue has been **fixed**. The healing logic now uses the conservative approach:

```rust
// Never enlarge header during healing - only shrink
let correct_blocks = valid_blocks.min(header_claims_blocks);
```

This treats the header as the commit marker and never "commits" new blocks during healing, preventing the ambiguity between:
- "Append succeeded, header commit crashed" (would have been safe to enlarge)
- "Prune header updated, sidecar truncation crashed" (dangerous to enlarge - resurrects pruned data)

**Tests Added**: `test_healing_never_enlarges_header` verifies that when sidecar has more valid blocks than header claims, the header is NOT enlarged during healing.

---

### 2. Comment Accuracy in `changeset_offsets.rs`

**Location**: Lines 82-92

The new comment block is accurate and well-written. It clearly explains:
- Why this error indicates an invariant violation
- What could cause it (bug in healing, skipped healing, external corruption)
- That healing should have prevented reaching this code path

**No changes needed** - this is good documentation.

---

## Test Coverage Analysis

### Append Crash Scenarios

| Test | Scenario |
|------|----------|
| `test_append_clean_no_crash` | Normal append operation without crash |
| `test_append_crash_partial_sidecar_record` | Crash mid-write of 16-byte record |
| `test_append_crash_sidecar_synced_header_not_committed` | Sidecar synced but header not committed |
| `test_append_crash_sidecar_ahead_of_nippy_offsets` | Sidecar contains offsets pointing past NippyJar EOF |

### Prune Crash Scenarios

| Test | Scenario |
|------|----------|
| `test_prune_clean_no_crash` | Normal prune operation without crash |
| `test_prune_crash_sidecar_truncated_header_stale` | Sidecar truncated but header not updated |
| `test_prune_crash_sidecar_offsets_past_nippy_rows` | Sidecar offsets point past NippyJar after prune |
| `test_prune_crash_nippy_offsets_truncated_data_stale` | NippyJar offsets truncated but data stale |
| `test_prune_crash_sidecar_truncate_not_synced` | Sidecar truncate not synced to disk |
| `test_prune_with_unflushed_current_offset` | Prune with unflushed current_changeset_offset in writer |
| `test_prune_with_uncommitted_sidecar_records` | Prune uses header block count, not sidecar file size |
| `test_prune_double_decrement_regression` | Regression test for double decrement bug |

### Edge Cases & Stability

| Test | Scenario |
|------|----------|
| `test_empty_segment_fresh_start` | Starting with empty/new segment |
| `test_empty_blocks_zero_changes` | Blocks with zero state changes |
| `test_healing_never_enlarges_header` | Sidecar has more valid blocks than header → header stays unchanged |
| `test_multiple_reopen_cycles_stable` | Repeated open/close cycles remain stable |
| `test_combined_partial_and_extra_blocks` | Multiple corruption types in single recovery |

### ✅ All 17 Tests Complete

All crash recovery tests use real NippyJar files via StaticFileProvider for end-to-end healing validation.

---

## NippyJar Alignment

### How NippyJar Healing Works

Per the Librarian analysis, NippyJar uses a similar three-tier model:
1. **Config file** (`.conf`) - The commit marker
2. **Offsets file** (`.off`) - Byte positions
3. **Data file** - Actual content

NippyJar healing truncates files to match config, while changeset healing truncates/updates to match file state. This is the correct complementary approach.

### Key Insight

> "NippyJar heals by truncating files to match config, while changeset offsets heal by recalculating/truncating the offset array to match file state"

The changeset healing runs **after** NippyJar healing (`ensure_end_range_consistency()`), so it can trust `actual_nippy_rows` as ground truth.

---

## Recommendations Summary

| Item | Status | Notes |
|------|--------|-------|
| Header enlargement policy | FIXED ✅ | Conservative approach: never enlarge header during healing |
| Test: never enlarge header | FIXED ✅ | `test_healing_never_enlarges_header` added |
| Issue #5: Unflushed current_offset | FIXED ✅ | `flush_current_changeset_offset()` before reading sidecar |
| Issue #7: Reader uses disk file size | FIXED ✅ | `with_len(changeset_offsets_len)` instead of `new()` |
| Integration tests with NippyJar | COMPLETE ✅ | All 17 tests use real NippyJar files |
| All crash scenarios | COMPLETE ✅ | Append and prune crashes fully covered |

---

## Conclusion

The implementation is **production-ready** for crash recovery. The three-way healing logic correctly handles all failure modes:

- ✅ Partial writes
- ✅ Uncommitted tails  
- ✅ Stale headers after prune
- ✅ Invalid offsets pointing past EOF
- ✅ Header enlargement prevention
- ✅ Orphaned sidecars (truncated on next use)

All 17 integration tests pass using real NippyJar files via StaticFileProvider.

---

## Appendix: Crash Scenario Diagrams

### Normal Commit Sequence
```
1. Append rows to NippyJar
2. Append offset to sidecar
3. sync_all() sidecar
4. commit() header ← COMMIT POINT
```

### Prune Sequence (Current Implementation)
```
1. Truncate NippyJar rows
2. Truncate sidecar file
3. Update header (prune)
4. commit() header ← COMMIT POINT
```

### Healing Decision Tree
```
                    ┌─────────────────────┐
                    │ Read 3 sources      │
                    │ header, jar, sidecar│
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │ Validate each block │
                    │ offset+len <= rows  │
                    └──────────┬──────────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
    ┌─────────▼─────────┐ ┌────▼────┐ ┌─────────▼─────────┐
    │ sidecar > valid   │ │ match   │ │ header > valid    │
    │ Truncate sidecar  │ │ No-op   │ │ Prune header      │
    └───────────────────┘ └─────────┘ └───────────────────┘
```
