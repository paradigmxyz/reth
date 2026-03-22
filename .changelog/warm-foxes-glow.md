---
reth-network: minor
---

Added direction labels to `closed_sessions` and `pending_session_failures` metrics. Operators can now distinguish session closures and failures by direction (`active`, `incoming_pending`, `outgoing_pending` for closed sessions; `inbound`, `outbound` for pending session failures).
