---
title: Remove intermediate allocation in Compact derive
labels:
    - C-perf
    - M-prevent-stale
    - S-blocked
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.976724Z
info:
    author: DaniPopes
    created_at: 2024-09-27T17:11:54Z
    updated_at: 2025-09-15T10:09:20Z
---

### Describe the feature

Generated code for `Account`:
```rust
// Recursive expansion of Compact macro
// =====================================

impl Account {
    #[doc = "Used bytes by [`AccountFlags`]"]
    pub const fn bitflag_encoded_bytes() -> usize {
        2u8 as usize
    }
    #[doc = "Unused bits for new fields by [`AccountFlags`]"]
    pub const fn bitflag_unused_bits() -> usize {
        5u8 as usize
    }
}
pub use Account_flags::AccountFlags;
#[allow(non_snake_case)]
mod Account_flags {
    // ... omitted
}
// ... omitted
impl Compact for Account {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut flags = AccountFlags::default();
        let mut total_length = 0;
        let mut buffer = bytes::BytesMut::new();
        let nonce_len = self.nonce.to_compact(&mut buffer);
        flags.set_nonce_len(nonce_len as u8);
        let balance_len = self.balance.to_compact(&mut buffer);
        flags.set_balance_len(balance_len as u8);
        let bytecode_hash_len = self.bytecode_hash.specialized_to_compact(&mut buffer);
        flags.set_bytecode_hash_len(bytecode_hash_len as u8);
        let flags = flags.into_bytes();
        total_length += flags.len() + buffer.len();
        buf.put_slice(&flags);
        buf.put(buffer);
        total_length
    }
    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        // ... omitted
    }
}
```

There's a `BytesMut` in `to_compact` which is used as a temporary buffer to call compact on the fields, so as to be able to encode the flags before the actual data.

We should be able to eliminate this temporary buffer by pushing directly to the provided buffer and using `AsMut<[u8]>`

Something like pushing `[0; Self::bitflag_encoded_bytes()]`, doing the normal encoding routine, then going back and setting the flag bits

cc @joshieDo 

### Additional context

_No response_
