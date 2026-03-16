---
reth-engine-tree: patch
reth-trie-sparse: patch
reth-tasks: patch
---

Offloaded deallocation of expensive proof node buffers to a persistent background thread (`Runtime::spawn_drop`) to avoid blocking state root computation or lock-holding code.
