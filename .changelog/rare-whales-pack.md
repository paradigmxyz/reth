---
reth-trie-sparse: minor
---

Added a comprehensive generic `SparseTrie` test suite covering `set_root`, `reveal_nodes`, `update_leaves`, `root`, `take_updates`, `commit_updates`, `prune`, `wipe`/`clear`, `get_leaf_value`, `find_leaf`, `size_hint`, and integration lifecycle scenarios. Tests are stamped out for all concrete `SparseTrie` implementations via a macro.
