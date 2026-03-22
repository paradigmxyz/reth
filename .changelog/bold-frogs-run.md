---
reth-transaction-pool: patch
---

Renamed and documented validation methods for clarity. `validate_one_no_state` and `validate_one_against_state` are now public methods `validate_stateless` and `validate_stateful` with improved documentation explaining their respective validation phases.
