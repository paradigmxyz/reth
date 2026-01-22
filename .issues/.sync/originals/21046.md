---
title: 'perf(db): Zero-copy database serialization - eliminate heap allocations in MDBX writes'
labels:
    - A-db
    - C-perf
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:16.019453Z
info:
    author: DaniPopes
    created_at: 2026-01-14T18:03:51Z
    updated_at: 2026-01-15T18:42:30Z
---

# Zero-Copy Database Serialization

Design document for eliminating heap allocations in MDBX write operations.

**Issue**: [#21046](https://github.com/paradigmxyz/reth/issues/21046)  
**Related**: [#11296](https://github.com/paradigmxyz/reth/issues/11296)

## Problem

The current `Tx::put` allocates heap memory on every write:

```rust
fn put<T: Table>(&self, kind: PutKind, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
    let key = key.encode();       // heap allocation
    let value = value.compress(); // Vec<u8> allocation
    // ...
    tx.put(self.get_dbi::<T>()?, key.as_ref(), value, flags)
}
```

The same pattern exists in cursor operations (`upsert`, `insert`, `append`, `append_dup`).

## Solution

Use MDBX's `reserve` API to serialize directly into MDBX-managed memory, and use stack buffers for keys.

### Target API

```rust
let val_buf = tx.reserve(db, key.encode_key(key_buf), value.encoded_size(), flags)?;
value.encode_value_to(val_buf);
```

Arguments should be `&T::Key, &T::Value` to avoid moves.

## Proposed Traits

### `KeySer` - Stack-only key serialization

```rust
/// Maximum allowed key size in bytes.
pub const MAX_KEY_SIZE: usize = 64;

pub trait KeySer: Ord + Sized {
    /// Fixed size of serialized key. Must satisfy `SIZE <= MAX_KEY_SIZE`.
    const SIZE: usize;

    const ASSERT: () = {
        assert!(Self::SIZE <= MAX_KEY_SIZE);
        assert!(Self::SIZE > 0);
    };

    /// Encode key into buffer. Returns slice of encoded bytes (<= SIZE).
    /// Encoding must preserve ordering: k1 > k2 => bytes(k1) > bytes(k2).
    fn encode_key<'a: 'c, 'b: 'c, 'c>(&'a self, buf: &'b mut [u8; MAX_KEY_SIZE]) -> &'c [u8];

    fn decode_key(data: &[u8]) -> Result<Self, DeserError>;
}
```

Key properties:
- Never allocates heap space.
- Buffer is reusable across multiple kv pairs.
- MDBX perf degrades with key size, so enforcing maximum is desirable.
- Lifetime dance (`'a: 'c, 'b: 'c, 'c`) allows returning either a slice of buf or a borrow from self.

### `ValSer` - Value serialization with size reporting

```rust
pub trait ValSer {
    /// Encoded size in bytes. MUST be accurate for `reserve` to work.
    fn encoded_size(&self) -> usize;

    fn encode_value_to<B>(&self, buf: &mut B)
    where
        B: bytes::BufMut + AsMut<[u8]>;

    fn decode_value(data: &[u8]) -> Result<Self, DeserError>
    where
        Self: Sized;
}
```

Key properties:
- `encoded_size()` enables `mdbx::reserve` - no Rust-side allocation.
- Values must be self-describing (include length prefixes where needed).

## Implementation Notes

### Compress trait compatibility

Current `Compress` allows variable-size keys (e.g., compact `U256`). In this layout, `U256` key remains 32 bytes. Variable-length key encoding is possible but adds complexity.

### MDBX reserve semantics

`tx.reserve(db, key, len, flags)` allocates exactly `len` bytes in MDBX and returns `&mut [u8]`. The buffer must be filled precisely - no partial writes allowed.

### Cursor operations

Need cursor-level reserve wrapper. Current `Cursor::put` takes `&[u8]` data, need to add reserve variant.

## Benefits

| Metric | Before | After |
|--------|--------|-------|
| Key allocations per write | 1 heap | 0 (stack) |
| Value allocations per write | 1 `Vec<u8>` | 0 (MDBX buffer) |
| Memory copies per write | 2 | 0 (direct encode) |

## Files to Modify

- `crates/storage/db-api/src/table.rs` - New traits
- `crates/storage/db/src/implementation/mdbx/tx.rs` - Update `put`
- `crates/storage/db/src/implementation/mdbx/cursor.rs` - Update cursor writes
- `crates/storage/libmdbx-rs/src/cursor.rs` - Add cursor reserve if needed

