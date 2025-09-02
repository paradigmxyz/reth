# NewPayload Process Timing Observability Plan

## Objective
Create comprehensive timing logs for the entire `newPayload` process to identify bottlenecks and compare performance between branches.

## Target Timing Breakdown

### A. Pre-Execution Phase
- **A1**: `newPayload` API call received → validation setup start
- **A2**: Block validation (consensus rules, header checks) 
- **A3**: Database setup and state provider initialization
- **A4**: EVM executor creation and precompile setup

### B. Prewarming Phase  
- **B1**: Prewarming start → first transaction begins
- **B2**: Per-transaction prewarming execution times
- **B3**: Total prewarming completion time
- **B4**: Cache population metrics (entries stored)

### C. Regular Execution Phase
- **C1**: Regular execution start → first transaction
- **C2**: Per-transaction execution times (with cache hit/miss)
- **C3**: Cache validation overhead per transaction
- **C4**: Total regular execution completion time

### D. Validation Phase
- **D1**: Cache validation start
- **D2**: Database I/O time for validation
- **D3**: Deduplication time
- **D4**: Total validation completion time

### E. State Root Computation
- **E1**: State root computation start
- **E2**: Trie building/parallel computation time
- **E3**: State root completion and verification

### F. Post-Processing
- **F1**: Block finalization start
- **F2**: Persistence preparation
- **F3**: Response preparation and return
- **F4**: Total `newPayload` end-to-end time

## Implementation Strategy

### Phase 1: Identify Code Locations
1. Map each timing point to specific functions/code locations
2. Find the main `newPayload` entry point
3. Locate prewarming execution logic
4. Find regular execution with cache validation
5. Identify state root computation calls
6. Map post-processing steps

### Phase 2: Add Timing Infrastructure
1. Create structured timing logger with consistent format
2. Add timing points at each phase boundary
3. Include contextual information (block number, tx count, cache stats)
4. Ensure thread-safe logging across worker threads

### Phase 3: Branch Implementation
1. **Current Branch**: Add full timing to InvalidatedAccessCache implementation
2. **Main Branch**: Create clean branch from main with same timing additions
3. Ensure identical timing instrumentation on both branches

### Phase 4: Analysis Tools
1. Create log parsing tools to extract timing data
2. Generate comparative timing reports
3. Create visualization for bottleneck identification

## Expected Log Format

```
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 block_hash=0x... total_txs=150 phase=A1 event="newPayload_start" 
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=A2 event="validation_start" elapsed_since_start_us=45
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=A4 event="executor_ready" elapsed_since_start_us=123
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=B1 event="prewarming_start" elapsed_since_start_us=234
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=B3 event="prewarming_complete" elapsed_since_start_us=5432 cache_entries=1245
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=C1 event="execution_start" elapsed_since_start_us=5456
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=C4 event="execution_complete" elapsed_since_start_us=12345 cache_hits=89 cache_misses=61
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=D4 event="validation_complete" elapsed_since_start_us=12456 db_calls=234
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=E3 event="state_root_complete" elapsed_since_start_us=15678
2025-09-01T10:39:20.123456Z INFO newpayload::timing block_number=12345678 phase=F4 event="newPayload_complete" total_time_us=16789
```

## Code Locations to Instrument

Based on Reth codebase structure:

### 1. NewPayload Entry Point
- `crates/rpc/rpc-engine-api/src/engine_api.rs` - `new_payload_v*` methods
- `crates/engine/tree/src/engine.rs` - Engine API implementation

### 2. Block Execution Pipeline  
- `crates/engine/tree/src/tree/payload_validator.rs` - `execute_block` function
- Transaction execution loop with cache validation
- State root computation calls

### 3. Prewarming Logic
- `crates/engine/tree/src/tree/payload_processor/` - Prewarming execution
- Cache population and storage

### 4. Validation and State Root
- Database validation timing (our current implementation)
- `crates/trie/parallel/` - State root computation

## Metrics to Track

### Performance Metrics
- Total end-to-end time per block
- Time per transaction (prewarming vs execution)
- Cache hit rate and validation overhead
- Database I/O time breakdown
- State root computation time

### Comparative Metrics (Current vs Main)
- Cache effectiveness (time saved vs validation overhead)
- Transaction throughput differences
- Memory usage patterns
- Database contention patterns

## Success Criteria

1. **Complete Visibility**: Every major phase of `newPayload` is timed and logged
2. **Comparative Analysis**: Can compare identical blocks between current and main branches
3. **Bottleneck Identification**: Clear identification of slowest phases
4. **Cache Impact Measurement**: Quantify InvalidatedAccessCache benefits vs overhead
5. **Actionable Insights**: Data leads to specific optimization opportunities

## Deliverables

1. Comprehensive timing instrumentation on current branch
2. Identical timing instrumentation on clean main branch
3. Log parsing and analysis tools
4. Comparative performance report
5. Bottleneck identification and optimization recommendations