---
reth-bench: minor
---

Removed temporary OP-stack transaction and payload handling from `reth-bench`, making the replay and standalone payload tooling Ethereum-only again. This drops the new abstraction from PR #23262 and keeps the existing CLI behavior while unsupported OP-style blocks now fail during conversion.
