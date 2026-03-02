---
reth-db-api: patch
---

Changed `StoredNibblesSubKey` encoding to use a stack-allocated `[u8; 65]` array instead of a heap-allocated `Vec<u8>`, avoiding unnecessary heap allocation.
