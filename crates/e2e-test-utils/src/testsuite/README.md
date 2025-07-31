# E2E Test Suite Framework

This directory contains the framework for writing end-to-end (e2e) tests in Reth. The framework provides utilities for setting up test environments, performing actions, and verifying blockchain behavior.

## Test Organization

E2E tests using this framework follow a consistent structure across the codebase:

### Directory Structure
Each crate that requires e2e tests should organize them as follows:
```
<crate-name>/
├── src/
│   └── ... (implementation code)
├── tests/
│   └── e2e-testsuite/
│       └── main.rs (or other test files)
└── Cargo.toml
```

### Cargo.toml Configuration
In your crate's `Cargo.toml`, define the e2e test binary:
```toml
[[test]]
name = "e2e_testsuite"
path = "tests/e2e-testsuite/main.rs"
harness = true
```

**Important**: The test binary MUST be named `e2e_testsuite` to be properly recognized by the nextest filter and CI workflows.

## Running E2E Tests

### Run all e2e tests across the workspace
```bash
cargo nextest run --workspace \
  --exclude 'example-*' \
  --exclude 'exex-subscription' \
  --exclude 'reth-bench' \
  --exclude 'ef-tests' \
  --exclude 'op-reth' \
  --exclude 'reth' \
  -E 'binary(e2e_testsuite)'
```

Note: The `--exclude` flags prevent compilation of crates that don't contain e2e tests (examples, benchmarks, binaries, and EF tests), significantly reducing build time.

### Run e2e tests for a specific crate
```bash
cargo nextest run -p <crate-name> -E 'binary(e2e_testsuite)'
```

### Run with additional features
```bash
cargo nextest run --locked --features "asm-keccak" --workspace -E 'binary(e2e_testsuite)'
```

### Run a specific test
```bash
cargo nextest run --workspace -E 'binary(e2e_testsuite) and test(test_name)'
```

## Writing E2E Tests

Tests use the framework components from this directory:

```rust
use reth_e2e_test_utils::{setup_import, Environment, TestBuilder};

#[tokio::test]
async fn test_example() -> eyre::Result<()> {
    // Create test environment
    let (mut env, mut handle) = TestBuilder::new()
        .build()
        .await?;

    // Perform test actions...
    
    Ok(())
}
```

## Framework Components

- **Environment**: Core test environment managing nodes and network state
- **TestBuilder**: Builder pattern for configuring test environments
- **Actions** (`actions/`): Pre-built test actions like block production, reorgs, etc.
- **Setup utilities**: Helper functions for common test scenarios

## CI Integration

E2E tests run in a dedicated GitHub Actions workflow (`.github/workflows/e2e.yml`) with:
- Extended timeouts (2 minutes per test, with 3 retries)
- Isolation from unit and integration tests
- Parallel execution support

## Nextest Configuration

The framework uses custom nextest settings (`.config/nextest.toml`):
```toml
[[profile.default.overrides]]
filter = "binary(e2e_testsuite)"
slow-timeout = { period = "2m", terminate-after = 3 }
```

This ensures all e2e tests get appropriate timeouts for complex blockchain operations.