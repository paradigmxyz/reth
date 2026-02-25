---
reth-network: minor
---

Added `fork_id` as a tiebreaker in peer selection when reputations are equal, preferring peers with a discovered `fork_id` as it indicates fork compatibility. Added a test to verify the tiebreaker behavior.
