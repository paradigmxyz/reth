# Reth Development Guide for AI Agents

This guide provides comprehensive instructions for AI agents working on the Reth codebase. It covers the architecture, development workflows, and critical guidelines for effective contributions.

## Project Overview

Reth is a high-performance Ethereum execution client written in Rust, focusing on modularity, performance, and contributor-friendliness. The codebase is organized into well-defined crates with clear boundaries and responsibilities.

## Architecture Overview

### Core Components

1. **Consensus (`crates/consensus/`)**: Validates blocks according to Ethereum consensus rules
2. **Storage (`crates/storage/`)**: Hybrid database using MDBX + static files for optimal performance
3. **Networking (`crates/net/`)**: P2P networking stack with discovery, sync, and transaction propagation
4. **RPC (`crates/rpc/`)**: JSON-RPC server supporting all standard Ethereum APIs
5. **Execution (`crates/evm/`, `crates/ethereum/`)**: Transaction execution and state transitions
6. **Pipeline (`crates/stages/`)**: Staged sync architecture for blockchain synchronization
7. **Trie (`crates/trie/`)**: Merkle Patricia Trie implementation with parallel state root computation
8. **Node Builder (`crates/node/`)**: High-level node orchestration and configuration
9  **The Consensus Engine (`crates/engine/`)**: Handles processing blocks received from the consensus layer with the Engine API (newPayload, forkchoiceUpdated)

### Key Design Principles

- **Modularity**: Each crate can be used as a standalone library
- **Performance**: Extensive use of parallelism, memory-mapped I/O, and optimized data structures
- **Extensibility**: Traits and generic types allow for different implementations (Ethereum, Optimism, etc.)
- **Type Safety**: Strong typing throughout with minimal use of dynamic dispatch

## Development Workflow

### Code Style and Standards

1. **Formatting**: Always use nightly rustfmt
   ```bash
   cargo +nightly fmt --all
   ```

2. **Linting**: Run clippy with all features
   ```bash
   RUSTFLAGS="-D warnings" cargo +nightly clippy --workspace --lib --examples --tests --benches --all-features --locked
   ```

3. **Testing**: Use nextest for faster test execution
   ```bash
   cargo nextest run --workspace
   ```

### Common Contribution Types

Based on actual recent PRs, here are typical contribution patterns:

#### 1. Small Bug Fixes (1-10 lines)
Real example: Fixing beacon block root handling ([#16767](https://github.com/paradigmxyz/reth/pull/16767))
```rust
// Changed a single line to fix logic error
- parent_beacon_block_root: parent.parent_beacon_block_root(),
+ parent_beacon_block_root: parent.parent_beacon_block_root().map(|_| B256::ZERO),
```

#### 2. Integration with Upstream Changes
Real example: Integrating revm updates ([#16752](https://github.com/paradigmxyz/reth/pull/16752))
```rust
// Update code to use new APIs from dependencies
- if self.fork_tracker.is_shanghai_activated() {
-     if let Err(err) = transaction.ensure_max_init_code_size(MAX_INIT_CODE_BYTE_SIZE) {
+ if let Some(init_code_size_limit) = self.fork_tracker.max_initcode_size() {
+     if let Err(err) = transaction.ensure_max_init_code_size(init_code_size_limit) {
```

#### 3. Adding Comprehensive Tests
Real example: ETH69 protocol tests ([#16759](https://github.com/paradigmxyz/reth/pull/16759))
```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_peers_can_connect() {
    // Create test network with specific protocol versions
    let p0 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth69.into()));
    // Test connection and version negotiation
}
```

#### 4. Making Components Generic
Real example: Making EthEvmConfig generic over chainspec ([#16758](https://github.com/paradigmxyz/reth/pull/16758))
```rust
// Before: Hardcoded to ChainSpec
- pub struct EthEvmConfig<EvmFactory = EthEvmFactory> {
-     pub executor_factory: EthBlockExecutorFactory<RethReceiptBuilder, Arc<ChainSpec>, EvmFactory>,

// After: Generic over any chain spec type
+ pub struct EthEvmConfig<C = ChainSpec, EvmFactory = EthEvmFactory>
+ where
+     C: EthereumHardforks,
+ {
+     pub executor_factory: EthBlockExecutorFactory<RethReceiptBuilder, Arc<C>, EvmFactory>,
```

#### 5. Resource Management Improvements
Real example: ETL directory cleanup ([#16770](https://github.com/paradigmxyz/reth/pull/16770))
```rust
// Add cleanup logic on startup
+ if let Err(err) = fs::remove_dir_all(&etl_path) {
+     warn!(target: "reth::cli", ?etl_path, %err, "Failed to remove ETL path on launch");
+ }
```

#### 6. Feature Additions
Real example: Sharded mempool support ([#16756](https://github.com/paradigmxyz/reth/pull/16756))
```rust
// Add new filtering policies for transaction announcements
pub struct ShardedMempoolAnnouncementFilter<T> {
    pub inner: T,
    pub shard_bits: u8,
    pub node_id: Option<B256>,
}
```

### Testing Guidelines

1. **Unit Tests**: Test individual functions and components
2. **Integration Tests**: Test interactions between components
3. **Benchmarks**: For performance-critical code
4. **Fuzz Tests**: For parsing and serialization code
5. **Property Tests**: For checking component correctness on a wide variety of inputs

Example test structure:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_component_behavior() {
        // Arrange
        let component = Component::new();
        
        // Act
        let result = component.operation();
        
        // Assert
        assert_eq!(result, expected);
    }
}
```

### Performance Considerations

1. **Avoid Allocations in Hot Paths**: Use references and borrowing
2. **Parallel Processing**: Use rayon for CPU-bound parallel work
3. **Async/Await**: Use tokio for I/O-bound operations
4. **File Operations**: Use `reth_fs_util` instead of `std::fs` for better error handling

### Common Pitfalls

1. **Don't Block Async Tasks**: Use `spawn_blocking` for CPU-intensive work or work with lots of blocking I/O
2. **Handle Errors Properly**: Use `?` operator and proper error types

### What to Avoid

Based on PR patterns, avoid:

1. **Large, sweeping changes**: Keep PRs focused and reviewable
2. **Mixing unrelated changes**: One logical change per PR
3. **Ignoring CI failures**: All checks must pass
4. **Incomplete implementations**: Finish features before submitting
5. **Modifying libmdbx sources**: Never modify files in `crates/storage/libmdbx-rs/mdbx-sys/libmdbx/` - this is vendored third-party code

### CI Requirements

Before submitting changes, ensure:

1. **Format Check**: `cargo +nightly fmt --all --check`
2. **Clippy**: No warnings with `RUSTFLAGS="-D warnings"`
3. **Tests Pass**: All unit and integration tests
4. **Documentation**: Update relevant docs and add doc comments with `cargo docs --document-private-items`
5. **Commit Messages**: Follow conventional format (feat:, fix:, chore:, etc.)


### Opening PRs against <https://github.com/paradigmxyz/reth>

Label PRs appropriately, first check the available labels and then apply the relevant ones:
* when changes are RPC related, add A-rpc label
* when changes are docs related, add C-docs label
* when changes are optimism related (e.g. new feature or exclusive changes to crates/optimism), add A-op-reth label
* ... and so on, check the available labels for more options.
* if being tasked to open a pr, ensure that all changes are properly formatted: `cargo +nightly fmt --all`

If changes in reth include changes to dependencies, run commands `zepter` and `make lint-toml` before finalizing the pr. Assume `zepter` binary is installed.

### Debugging Tips

1. **Logging**: Use `tracing` crate with appropriate levels
   ```rust
   tracing::debug!(target: "reth::component", ?value, "description");
   ```

2. **Metrics**: Add metrics for monitoring
   ```rust
   metrics::counter!("reth_component_operations").increment(1);
   ```

3. **Test Isolation**: Use separate test databases/directories

### Finding Where to Contribute

1. **Check Issues**: Look for issues labeled `good-first-issue` or `help-wanted`
2. **Review TODOs**: Search for `TODO` comments in the codebase
3. **Improve Tests**: Areas with low test coverage are good targets
4. **Documentation**: Improve code comments and documentation
5. **Performance**: Profile and optimize hot paths (with benchmarks)

### Common PR Patterns

#### Small, Focused Changes
Most PRs change only 1-5 files. Examples:
- Single-line bug fixes
- Adding a missing trait implementation
- Updating error messages
- Adding test cases for edge conditions

#### Integration Work
When dependencies update (especially revm), code needs updating:
- Check for breaking API changes
- Update to use new features (like EIP implementations)
- Ensure compatibility with new versions

#### Test Improvements
Tests often need expansion for:
- New protocol versions (ETH68, ETH69)
- Edge cases in state transitions
- Network behavior under specific conditions
- Concurrent operations

#### Making Code More Generic
Common refactoring pattern:
- Replace concrete types with generics
- Add trait bounds for flexibility
- Enable reuse across different chain types (Ethereum, Optimism)

### Example Contribution Workflow

Let's say you want to fix a bug where external IP resolution fails on startup:

1. **Create a branch**:
   ```bash
   git checkout -b fix-external-ip-resolution
   ```

2. **Find the relevant code**:
   ```bash
   # Search for IP resolution code
   rg "external.*ip" --type rust
   ```

3. **Reason about the problem, when the problem is identified, make the fix**:
   ```rust
   // In crates/net/discv4/src/lib.rs
   pub fn resolve_external_ip() -> Option<IpAddr> {
       // Add fallback mechanism
       nat::external_ip()
           .or_else(|| nat::external_ip_from_stun())
           .or_else(|| Some(DEFAULT_IP))
   }
   ```

4. **Add a test**:
   ```rust
   #[test]
   fn test_external_ip_fallback() {
       // Test that resolution has proper fallbacks
   }
   ```

5. **Run checks**:
   ```bash
   cargo +nightly fmt --all
   cargo clippy --all-features
   cargo test -p reth-discv4
   ```

6. **Commit with clear message**:
   ```bash
   git commit -m "fix: add fallback for external IP resolution

   Previously, node startup could fail if external IP resolution
   failed. This adds fallback mechanisms to ensure the node can
   always start with a reasonable default."
   ```

## Quick Reference

### Essential Commands

```bash
# Format code
cargo +nightly fmt --all

# Run lints
RUSTFLAGS="-D warnings" cargo +nightly clippy --workspace --all-features --locked

# Run tests
cargo nextest run --workspace

# Run specific benchmark
cargo bench --bench bench_name

# Build optimized binary
cargo build --release --features "jemalloc asm-keccak"

# Check compilation for all features
cargo check --workspace --all-features

# Check documentation
cargo docs --document-private-items 
```
