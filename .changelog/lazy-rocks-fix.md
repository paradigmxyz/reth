---
reth-codecs-derive: patch
---

Removed intermediate `BytesMut` allocation in the `Compact` derive macro for non-zstd types. Fields are now written directly to the output buffer with placeholder flag bytes that are patched in-place, eliminating a copy. The zstd path retains its intermediate buffer since it needs to inspect the length before deciding whether to compress.
