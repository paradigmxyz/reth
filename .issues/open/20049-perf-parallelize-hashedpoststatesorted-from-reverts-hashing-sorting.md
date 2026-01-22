---
title: 'perf: parallelize HashedPostStateSorted::from_reverts hashing/sorting'
labels:
    - A-trie
    - C-perf
    - D-good-first-issue
    - S-needs-benchmark
assignees:
    - Andrurachi
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:16.009604Z
info:
    author: yongkangc
    created_at: 2025-12-01T04:50:03Z
    updated_at: 2025-12-01T04:56:02Z
---

## Summary
Explore a perf experiment for `HashedPostStateSorted::from_reverts` to parallelize the hashing/sorting step (not the DB cursor walks) so we can speed up large revert ranges without extra allocations.

## Motivation
- PR #20047 added the sorted-from-reverts path; benchmarks show ~6% win vs unsorted+sort on a small synthetic dataset (256 accounts x 4 slots). For larger reorg ranges, hashing/sorting could dominate.
- Tracker: #17920 (perf experiments).

## Proposal
- Keep DB cursor iteration sequential (required), but parallelize the post-collection hashing/sorting with rayon behind an opt-in feature flag (e.g. `parallel-from-reverts`).
- Use thresholds to avoid rayon overhead on small inputs (e.g. accounts > 1k and per-account slots > 32 use `par_sort_unstable_by_key`, else stay sequential).
- Hash keys during collection to avoid an extra map pass if it helps CPU profiles.

## Acceptance Criteria
- Implement gated parallel path with sensible thresholds and benchmarks comparing sequential vs parallel on large synthetic revert sets (e.g. 10k+ accounts, skewed slot distributions).
- Measure CPU and wall-time deltas; document when parallel path wins/loses.
- Keep default behavior unchanged (feature flag off) to avoid footprint changes.

## Risks / Notes
- rayon dependency and build footprint (hence feature flag).
- Parallel sorting only helps when key counts are large; overhead may regress small cases.
