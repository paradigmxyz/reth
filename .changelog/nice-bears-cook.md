---
reth-trie: patch
---

Removed the local `increment_and_strip_trailing_zeros` function and `PATH_ALL_ZEROS` static in `proof_v2`, replacing them with the equivalent `Nibbles::next_without_prefix` and `Nibbles::is_zeroes` builtins. Also replaced manual `.get()` calls on `state_mask`/`hash_mask` with direct field access and switched to `Nibbles::unpack_array` over the unsafe `unpack_unchecked`.
