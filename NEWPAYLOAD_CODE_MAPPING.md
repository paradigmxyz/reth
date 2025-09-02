# NewPayload Code Flow Mapping

## Complete Flow Path

### A. RPC Entry Points
- **File**: `crates/rpc/rpc-engine-api/src/engine_api.rs`
- **Functions**: `new_payload_v1_metered`, `new_payload_v2_metered`, `new_payload_v3_metered`, `new_payload_v4_metered`
- **Timing Point A1**: RPC method entry point
- **Current Timing**: Already has basic start/end timing with `Instant::now()`

### B. Beacon Consensus Engine Message
- **File**: `crates/engine/primitives/src/message.rs`
- **Function**: `BeaconConsensusEngine::new_payload()`
- **Action**: Sends `BeaconEngineMessage::NewPayload` to engine
- **Timing Point A2**: Message sent to engine

### C. Engine Handler Processing
- **File**: `crates/engine/tree/src/engine.rs` 
- **Handler**: `EngineApiRequestHandler`
- **Message Processing**: Receives `BeaconEngineMessage::NewPayload`
- **Timing Point A3**: Engine handler receives message

### D. Tree Processing
- **File**: `crates/engine/tree/src/tree/mod.rs`
- **Tree Operations**: Insert payload into the tree
- **Timing Point A4**: Tree processing starts

### E. Block Execution
- **File**: `crates/engine/tree/src/tree/payload_validator.rs`
- **Function**: `execute_block()` (Line ~737)
- **Timing Points**:
  - **B1**: Prewarming starts (if enabled)
  - **C1**: Regular execution starts  
  - **C2**: Per-transaction execution with cache validation
  - **D1-D4**: Cache validation phases

### F. State Root Computation
- **File**: `crates/trie/parallel/src/root.rs`
- **Function**: State root computation (called from payload_validator.rs)
- **Timing Point E1-E3**: State root computation phases

### G. Response Path
- **Back through**: Engine → BeaconConsensusEngine → RPC Engine API
- **Timing Point F4**: Complete end-to-end timing

## Key Files to Instrument

1. **`crates/rpc/rpc-engine-api/src/engine_api.rs`** - Entry point timing
2. **`crates/engine/tree/src/tree/payload_validator.rs`** - Block execution timing
3. **`crates/engine/tree/src/tree/mod.rs`** - Tree operations
4. **`crates/trie/parallel/src/root.rs`** - State root timing

## Current State Analysis

### Already Instrumented (Basic):
- RPC entry points have start/end timing
- Block execution has some transaction-level timing
- Cache validation has detailed timing (our implementation)

### Missing Instrumentation:
- Pre-execution setup phases
- Prewarming phase boundaries  
- State root computation breakdown
- Post-processing phases
- End-to-end phase transitions