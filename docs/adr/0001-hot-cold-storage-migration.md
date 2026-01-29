# ADR: Hot/Cold Storage Migration & Legacy MDBX Deprecation

- **Status:** Draft
- **Date:** 2026-01-28

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
| Hot/cold experimental (v1.11.0) | w/c 2nd of Feb 2026 | Release behind feature flag |
| Hot/cold General Annoucnement (GA) | w/c 2nd of Feb 2026| Announce deprecation of legacy configs |
| Deprecation window | w/c 9th of Feb 2026 | No guarantees of legacy support |
| Legacy MDBX cutoff | w/c 16th of Feb 2026 | Remove legacy support |
| Glamsterdam | 27th of Feb 2026 | Glamsterdam Live |

**Key constraint:** DB break must happen before Glamsterdam, not during devnets.

### Migration Strategy

**There is no in-place migration path.** 

**Solution: Snapshots**
- Operators will download fresh snapshots from new nodes
- This is preferred over migration tooling for:
  - Simplicity
  - Reduced downtime concerns
  - Easier communication to operators

**Solution: Full resync**
- Operators can also do a full resync if they prefer

### External Operators

**Scope:** Full node operators only (light clients and archive snapshot users unaffected)

**Requirements:**
1. Share snapshot URLs & link here
2. Communicate breakage timeline clearly in advance
3. Provide clear instructions for snapshot download and fresh sync

### Tempo Internal

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

- [ ] Release hot/cold behind flag in v1.11.0 (due: w/c 2nd of Feb 2026)
- [ ] Prepare snapshots infrastructure for external operators (due: w/c 26th of Jan 2026)
- [ ] Draft deprecation announcement for external operators (due: w/c 26th of Jan 2026)
- [ ] Define Tempo internal cutover procedure (due: w/c 26th of Jan 2026)
- [ ] Coordinate with pandaops on dashboard integration (due: w/c 9th of Feb 2026)
- [ ] Finalize exact cutoff date (before Glamsterdam)

## Updates Log
[Make sure to update the status of each action item here]: # 


