---
title: 'db-api: trait Compress and Encode use reference &self'
labels:
    - A-db
    - C-debt
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.976495Z
info:
    author: nkysg
    created_at: 2024-09-25T06:10:31Z
    updated_at: 2025-02-18T16:10:56Z
---

### Describe the feature

Trait Encode fn encodes use self
https://github.com/paradigmxyz/reth/blob/1994959fb28c9f9a17128ca27b6f550b2d8df1d3/crates/storage/db-api/src/table.rs#L55

It's a bit weird. I see many libraries use &self, because when you have an object use fn encode, maybe we need to clone.

This is alloyed rlp  Encode trait.
https://github.com/alloy-rs/rlp/blob/9aef28e6da7c2769460dd6bf22eeacd53adf0ffc/crates/rlp/src/encode.rs#L15

```rust
/// A type that can be encoded via RLP.
pub trait Encodable {
    /// Encodes the type into the `out` buffer.
    fn encode(&self, out: &mut dyn BufMut);

    /// Returns the length of the encoding of this type in bytes.
    ///
    /// The default implementation computes this by encoding the type.
    /// When possible, we recommender implementers override this with a
    /// specialized implementation.
    #[inline]
    fn length(&self) -> usize {
        let mut out = Vec::new();
        self.encode(&mut out);
        out.len()
    }
}
```
It same as trait Compress
https://github.com/paradigmxyz/reth/blob/1994959fb28c9f9a17128ca27b6f550b2d8df1d3/crates/storage/db-api/src/table.rs#L28

I think we can use &self, maybe we can remove some clone. And It seems some db fn like put, also can use reference.
 

### Additional context

_No response_
