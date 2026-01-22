---
title: Use packed representation for StoredNibbles and StoredNibblesSubkey
labels:
    - A-db
    - C-enhancement
    - C-perf
    - M-prevent-stale
    - S-needs-design
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.995115Z
info:
    author: Rjected
    created_at: 2025-08-13T02:12:41Z
    updated_at: 2025-08-13T03:14:29Z
---

### Describe the feature

Right now `StoredNibbles` and `StoredNibblesSubkey` have inefficient encodings.


First, `StoredNibbles` uses the unpacked representation on disk:
https://github.com/paradigmxyz/reth/blob/b2d9cc1670a10cf4a0ae74346caed1fdd9f44062/crates/trie/common/src/nibbles.rs#L25-L44

Next, `StoredNibblesSubkey` pads everything to 65 bytes total, with the format being `unpacked nibbles || zeros (up to 64 bytes) || len`:
https://github.com/paradigmxyz/reth/blob/b2d9cc1670a10cf4a0ae74346caed1fdd9f44062/crates/trie/common/src/nibbles.rs#L73-L96

We can improve this by using an efficient encoding for both values on-disk. This would be `len || packed nibbles`.

Here are some stats on path length in existing mainnet account tables:

Length | Count | Percentage
-- | -- | --
1 | 16 | 0.00%
2 | 256 | 0.00%
3 | 4096 | 0.02%
4 | 65536 | 0.27%
5 | 1048576 | 4.33%
6 | 16726496 | 69.11%
7 | 6315318 | 26.09%
8 | 41590 | 0.17%
9 | 167 | 0.00%
10 | 2 | 0.00%

```
--- Most Common Nibble Lengths ---
1. Length 6 nibbles: 16726496 entries (69.11%)
2. Length 7 nibbles: 6315318 entries (26.09%)
3. Length 5 nibbles: 1048576 entries (4.33%)
4. Length 4 nibbles: 65536 entries (0.27%)
5. Length 8 nibbles: 41590 entries (0.17%)
```


And here is the distribution for storage tries:

Length | Count | Percentage
-- | -- | --
1 | 6502357 | 5.47%
2 | 17094720 | 14.38%
3 | 32187105 | 27.08%
4 | 27030971 | 22.74%
5 | 25716256 | 21.63%
6 | 10112317 | 8.51%
7 | 219679 | 0.18%
8 | 974 | 0.00%
9 | 5 | 0.00%

```
--- Most Common Nibble Lengths ---
1. Length 3 nibbles: 32187105 entries (27.08%)
2. Length 4 nibbles: 27030971 entries (22.74%)
3. Length 5 nibbles: 25716256 entries (21.63%)
4. Length 2 nibbles: 17094720 entries (14.38%)
5. Length 6 nibbles: 10112317 entries (8.51%)
```

Given we use 64 bytes for the path part of the subkey, which is mostly zeroes and has no compression, we would be able to get rid of over 90% of the space used by the key field.


## Unsolved problems / TODO
This would probably change the ordering of the storage keys. This would impact where things are on disk, and impact performance. Given lexicographic order, this would cause nodes with the same level to be closer on disk, due to the length prefix. However this might break iterators that rely on `next` values with the current format

This also requires backwards compatibility - this is a more difficult problem as we would still have padded nodes in older DBs, which we would have to disambiguate. We would have to support both formats unless we did some sort of online migration (which still requires this backwards compatibility work), or decide to do a breaking change. Luckily backwards compatibility can be easily tested with existing DB data.


### Additional context

_No response_
