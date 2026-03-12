---
reth-payload-builder: minor
---

Added `payloads.oldest_active_job_age_seconds` gauge that tracks the age of the longest-running in-flight payload job. Enables alerting on stuck builds caused by deadlocked finalization or state root computation.
