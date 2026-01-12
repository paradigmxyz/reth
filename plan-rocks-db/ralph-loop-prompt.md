# Ralph Loop Prompt: RocksDB Historical Indexes Implementation for Reth

## Mission Statement

Implement and stabilize RocksDB as the storage backend for historical indexes in Reth, achieving three completion criteria through iterative development and bug fixing:

1. **Local CI Success**: All tests pass with proper formatting
2. **Remote CI Success**: GitHub workflows pass with RocksDB enabled in edge mode
3. **Integration Test Success**: Live Hoodi testnet operation with reth-bench validation

## Context

### What We're Building
RocksDB support for three historical index tables in Reth:
- **TransactionHashNumbers**: Maps transaction hash → transaction number
- **StoragesHistory**: Storage history index
- **AccountsHistory**: Account history index

### Current State
- RocksDB implementation exists but with flags disabled
- Base work in PR #20544 (open, needs rebase)
- Issue #20384 tracks remaining work with **3 critical open sub-issues**
- Storage settings at `crates/storage/db-api/src/models/metadata.rs:42-51` has all RocksDB flags set to `false`

### Remaining Open Sub-Issues from #20384
**MUST complete these three before finishing:**

1. **#20388 - Use EitherReader/EitherWriter in DatabaseProvider and HistoricalStateProvider (REOPENED)**
   - Ensure DatabaseProvider properly uses EitherReader/EitherWriter abstractions
   - Update HistoricalStateProvider to read from RocksDB when enabled
   - Fix any issues that caused this to be reopened

2. **#20390 - Modify IndexStorageHistoryStage to use EitherWriter for RocksDB writes (OPEN)**
   - Update `IndexStorageHistoryStage` to write `StoragesHistory` to RocksDB
   - Follow the same pattern as `TransactionLookupStage` (already completed in #20389)
   - Implement proper batch writes with `with_rocksdb_batch()`

3. **#20393 - Add CLI flags to enable RocksDB storage (REOPENED)**
   - Add command-line flags to control RocksDB enabling
   - Integrate with storage settings configuration
   - Ensure backward compatibility with existing nodes
   - Fix any issues that caused this to be reopened

### Technical Stack
- **Language**: Rust with nightly toolchain for formatting
- **Database**: RocksDB (Unix-only, feature-gated)
- **Test Framework**: cargo-nextest + rstest for parameterized tests
- **CI**: GitHub Actions with storage matrix [stable, edge]

## Your Mission

### Phase 1: Investigation & Setup
1. Review issue #20384 sub-issues - **Focus on the 3 open/reopened issues listed above**
2. Checkout PR #20544's branch and rebase onto latest `main`
3. Study PR #20871 for parameterized test patterns using rstest
4. Review completed sub-issues (#20386, #20387, #20389, #20391, #20392) to understand patterns
5. Identify what needs to be implemented/fixed for the 3 remaining issues

### Phase 2: Implementation Loop

**Iterative Cycle**:
```
Implement → Format → Lint → Test → Fix Bugs → Repeat
```

**Core Tasks** (addressing the 3 open sub-issues):
1. **#20388**: Fix DatabaseProvider and HistoricalStateProvider to use EitherReader/EitherWriter
2. **#20390**: Implement IndexStorageHistoryStage RocksDB writes for `StoragesHistory` table
3. **#20393**: Add and fix CLI flags for RocksDB enabling
4. Add comprehensive tests using rstest parameterized testing
5. Ensure proper invariant checking (see `invariants.rs`)
6. Add metrics and logging for debugging

**Quality Gates** (must pass before moving to next criterion):
- `cargo +nightly fmt --all` (no changes)
- `RUSTFLAGS="-D warnings" cargo +nightly clippy --workspace --all-features --locked` (zero warnings)
- `cargo nextest run --features "asm-keccak ethereum edge" --locked --workspace` (all pass)

### Phase 3: Completion Criterion 1 - Local CI
**Goal**: All tests pass locally with zero warnings

**Checklist**:
- [ ] All code properly formatted with nightly rustfmt
- [ ] Zero clippy warnings with `-D warnings` flag
- [ ] All unit tests pass with `edge` feature enabled
- [ ] Integration tests pass
- [ ] No panics or errors in test output

**Blocking Issues**: Do not proceed to Criterion 2 until ALL local tests pass

### Phase 4: Completion Criterion 2 - Remote CI
**Goal**: GitHub CI workflows pass with RocksDB enabled

**Critical Configuration Change**:

**File**: `crates/storage/db-api/src/models/metadata.rs` (lines 47-49)

Change these three fields from `false` to `true`:
```rust
pub const fn edge() -> Self {
    Self {
        receipts_in_static_files: true,
        transaction_senders_in_static_files: true,
        account_changesets_in_static_files: true,
        storages_history_in_rocksdb: true,           // ← Change to true
        transaction_hash_numbers_in_rocksdb: true,   // ← Change to true
        account_history_in_rocksdb: true,            // ← Change to true
    }
}
```

**Process**:
1. Make the three-field configuration change above
2. Commit with message: `feat: enable RocksDB for historical indexes in edge mode`
3. Push to remote branch (based off PR #20544)
4. Monitor these CI workflows:
   - `.github/workflows/unit.yml` (storage: edge matrix jobs)
   - `.github/workflows/hive.yml` (edge build tests)
   - `.github/workflows/lint.yml` (formatting/linting)

**Debugging CI Failures**:
- Check CI logs for RocksDB-specific errors
- Verify feature flags are correctly propagated
- Ensure Unix platform checks pass
- Check for race conditions in concurrent tests

**Blocking Issues**: Do not proceed to Criterion 3 until ALL GitHub CI checks are green

### Phase 5: Completion Criterion 3 - Hoodi Integration Test
**Goal**: Live testnet operation validates RocksDB works end-to-end

**Hoodi Testnet Details**:
- Chain ID: 560048
- Snapshot location: `~/.local/share/reth/hoodi`
- Approach: Use existing snapshot (not fresh sync)

**Integration Test Script** (create as `scripts/test_rocksdb_hoodi.sh`):

```bash
#!/bin/bash
set -e

HOODI_DATA_DIR="$HOME/.local/share/reth/hoodi"
LOG_FILE="rocksdb_test_$(date +%Y%m%d_%H%M%S).log"

# Build with RocksDB support
echo "Building reth with edge features..."
cargo build --release --features "asm-keccak ethereum edge"

# Start Hoodi node
echo "Starting Hoodi node with RocksDB..."
./target/release/reth node \
  --chain hoodi \
  --datadir "$HOODI_DATA_DIR" \
  --authrpc.port 8551 \
  --authrpc.jwtsecret "$HOODI_DATA_DIR/jwt.hex" \
  --http \
  --http.port 8545 \
  --log.file "$LOG_FILE" &

NODE_PID=$!
echo "Node PID: $NODE_PID"

# Wait for node readiness
sleep 15

# Run reth-bench to stress test
echo "Running reth-bench new-payload-fcu..."
./target/release/reth-bench new-payload-fcu \
  --rpc-url "http://localhost:8545" \
  --engine-rpc-url "http://localhost:8551" \
  --auth-jwtsecret "$HOODI_DATA_DIR/jwt.hex" \
  --from 1 \
  --advance 100 \
  --wait-for-persistence

# Check for errors
if grep -i "rocksdb.*error\|panic\|fatal" "$LOG_FILE"; then
    echo "❌ ERRORS FOUND"
    kill $NODE_PID
    exit 1
fi

echo "✅ Integration test passed"
kill $NODE_PID
```

**Validation Checklist**:
- [ ] Node starts without panics
- [ ] RocksDB files created in datadir
- [ ] reth-bench completes without errors
- [ ] No RocksDB errors in logs
- [ ] Historical index queries work (verify via RPC)
- [ ] Node shutdown is clean

**Debugging Runtime Issues**:
- Check RocksDB metrics: `crates/storage/provider/src/providers/rocksdb/metrics.rs`
- Verify invariants: `crates/storage/provider/src/providers/rocksdb/invariants.rs`
- Test batch writes: Ensure `with_rocksdb_batch()` pattern is correct
- Check stage progress: TransactionLookupStage should populate TransactionHashNumbers

**Blocking Issues**: Do not consider task complete until integration test passes cleanly

## Bug Fixing Strategy

When tests fail at any stage:

1. **Identify Root Cause**:
   - Read error messages carefully
   - Check stack traces for RocksDB-specific panics
   - Review logs for invariant violations
   - Check for concurrency issues (race conditions)

2. **Fix with Minimal Changes**:
   - Target the specific bug, don't refactor unnecessarily
   - Add tests that reproduce the bug before fixing
   - Verify fix resolves the issue without breaking others

3. **Validate Fix**:
   - Re-run failed test to confirm fix
   - Run full test suite to catch regressions
   - Check that all three criteria still pass

4. **Iterate**:
   - Return to appropriate phase based on where failure occurred
   - Re-run quality gates
   - Progress through criteria again

## Critical Files Reference

### Implementation
- `crates/storage/provider/src/providers/rocksdb/provider.rs` - Main RocksDB provider
- `crates/storage/provider/src/providers/rocksdb/invariants.rs` - Consistency checking
- `crates/storage/provider/src/either_writer.rs` - MDBX/RocksDB abstraction
- `crates/stages/stages/src/stages/tx_lookup.rs` - TransactionLookup stage

### Configuration (KEY FOR CRITERION 2)
- `crates/storage/db-api/src/models/metadata.rs` - **Lines 47-49 must change to true**
- `crates/storage/db-common/Cargo.toml` - Edge feature definition

### Tests
- `crates/stages/stages/src/test_utils/test_db.rs` - Test database setup
- Reference PR #20871 for rstest parameterized test patterns

### CI
- `.github/workflows/unit.yml` - Storage matrix testing
- `.github/workflows/hive.yml` - Integration tests
- `.github/workflows/lint.yml` - Code quality checks

## Commands Reference

### Formatting
```bash
cargo +nightly fmt --all
```

### Linting
```bash
RUSTFLAGS="-D warnings" cargo +nightly clippy --workspace --all-features --locked
```

### Testing
```bash
# Unit tests with edge feature
cargo nextest run --features "asm-keccak ethereum edge" --locked --workspace --exclude ef-tests

# Specific stage tests
cargo test -p reth-stages --features edge

# All features
cargo nextest run --workspace --all-features
```

### Building for Integration Test
```bash
cargo build --release --features "asm-keccak ethereum edge"
```

### Running reth-bench
```bash
./target/release/reth-bench new-payload-fcu \
  --rpc-url "http://localhost:8545" \
  --engine-rpc-url "http://localhost:8551" \
  --auth-jwtsecret ~/.local/share/reth/hoodi/jwt.hex \
  --from 1 \
  --advance 100 \
  --wait-for-persistence
```

## Success Definition

**Mission Complete When**:
1. ✅ All local tests pass (Criterion 1)
2. ✅ All GitHub CI workflows pass (Criterion 2)
3. ✅ Hoodi integration test passes (Criterion 3)
4. ✅ No outstanding bugs or panics
5. ✅ Code is properly formatted and linted
6. ✅ Comprehensive tests added with rstest

## Behavioral Guidelines

1. **Be Thorough**: Don't skip quality gates to move faster
2. **Fix Properly**: Understand root cause before patching symptoms
3. **Test Extensively**: Add tests for edge cases and error conditions
4. **Document Findings**: Note any quirks or gotchas discovered
5. **Iterate Relentlessly**: Keep fixing until all three criteria pass

## Initial Actions

When starting this task:

1. Read issue #20384 to understand full scope
2. Review PR #20544 to see current implementation state
3. Rebase PR #20544's branch onto latest main
4. Run full local test suite to establish baseline
5. Create todo list tracking progress through all three criteria

## Final Note

This is an **iterative mission**. You will encounter bugs and failures - that's expected. The goal is to methodically work through each criterion, fixing issues as they arise, until all three completion criteria are achieved with stable, production-ready code.

Keep iterating. Keep fixing. Keep testing. Success is achieved when all three criteria pass cleanly.

---

**Good luck! 🚀**