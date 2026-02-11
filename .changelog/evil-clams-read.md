---
reth-bench-compare: minor
reth-bench: minor
---

Added gas range filtering for benchmark measurements. Blocks with gas usage outside the specified `--measure-gas-min` and `--measure-gas-max` range are now executed but excluded from timing statistics, allowing more focused performance analysis of blocks within specific gas usage ranges.
