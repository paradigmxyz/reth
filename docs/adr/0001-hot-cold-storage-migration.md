# ADR: Hot/Cold Storage Migration & Legacy MDBX Deprecation

- **Status:** Proposed
- **Date:** 2026-01-28
- **Authors:** @joshie, @emma, @mattsse, @georgios, @Dan Cline
- **Discussion:** [Slack thread](https://tempoxyz.slack.com/archives/C09FQDW2ZRP/p1768925785386699)

## Context

Maintaining backwards compatibility for the database layer has become increasingly complex. The combinatorial explosion of supported configurations is creating significant technical debt:

- History indices can be stored in MDBX or RocksDB
- Changesets can be stored in MDBX or Static Files
- Each combination requires testing and maintenance

This complexity is unsustainable as we introduce hot/cold storage architecture.

## Decision

We will deprecate legacy MDBX-only configurations and establish a clear timeline for operators to transition to the new hot/cold storage architecture.

### Timeline

| Milestone | Target | Action |
|-----------|--------|--------|
| Hot/cold experimental (v1.11.0) | ~Feb 2026 | Release behind feature flag |
| Hot/cold GA | Q1 2026 | Announce deprecation of legacy configs |
| Deprecation window | +3 months post-GA | No guarantees of legacy support |
| Legacy MDBX cutoff | Before Glamsterdam (H1 2026) | Remove legacy support |

**Key constraint:** DB break must happen before Glamsterdam, not during devnets.

### Migration Strategy

**There is no in-place migration path.** The MDBX freelist structure makes `mdbx_copy`-style migrations infeasible.

**Solution: Snapshots**
- Operators will download fresh snapshots from new nodes
- This is preferred over migration tooling for:
  - Simplicity
  - Reduced downtime concerns
  - Easier communication to operators

### External Operators

**Scope:** Full node operators only (light clients and archive snapshot users unaffected)

**Requirements:**
1. Spin up new snapshot-serving nodes before release
2. Communicate breakage timeline clearly in advance
3. Provide clear instructions for snapshot download and fresh sync

**User considerations:**
- Hobbyist/limited-resource stakers may lack SSD space for snapshots
- Clear documentation and advance notice are critical

### Tempo Internal

**Key differences from external reth users:**
- Slightly easier to coordinate (smaller, more responsive user base)
- Protocol changes like state root are still on the table, so DB breaks are acceptable

**Requirements:**
1. Define cutover plan (coordinated restart vs. phased rollout)
2. Notify pandaops to integrate hot/cold metrics into dashboards before release
3. Ensure snapshots are ready for internal migration

## Consequences

### Positive
- Reduced maintenance burden from supporting multiple storage configurations
- Cleaner codebase with single blessed configuration
- Better performance from optimized hot/cold architecture

### Negative
- Operators must re-sync or download snapshots (downtime)
- Some operators with limited resources may struggle with snapshot downloads
- Coordination overhead for Tempo internal migration

### Risks
- Operators who miss deprecation announcements may be caught off-guard
- Snapshot infrastructure must be ready before deprecation deadline

## Action Items

- [ ] Release hot/cold behind flag in v1.11.0 (next week)
- [ ] Prepare snapshot-serving infrastructure for external operators
- [ ] Draft operator communication plan (deprecation announcement)
- [ ] Coordinate with pandaops on dashboard integration
- [ ] Define Tempo internal cutover procedure
- [ ] Finalize exact cutoff date (before Glamsterdam)
