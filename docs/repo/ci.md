## CI

The CI runs a couple of workflows:

### Code

- **[unit]**: Runs unit tests (tests in `src/`) and doc tests
- **[integration]**: Runs integration tests (tests in `tests/` and sync tests)
- **[bench]**: Runs benchmarks
- **[eth-sync]**: Runs Ethereum mainnet sync tests
- **[op-sync]**: Runs base mainnet sync tests for Optimism
- **[stage]**: Runs all `stage run` commands

### Docs

- **[book]**: Builds, tests, and deploys the book.

### Meta

- **[deny]**: Runs `cargo deny` to check for license conflicts and security advisories in our dependencies
- **[release]**: Runs the release workflow
- **[release-dist]**: Publishes Reth to external package managers
- **[dependencies]**: Runs `cargo update` periodically to keep dependencies current
- **[stale]**: Marks issues as stale if there has been no activity
- **[docker]**: Publishes the Docker image.

### Integration Testing

- **[assertoor]**: Runs Assertoor tests on Reth pairs.
- **[hive]**: Runs `ethereum/hive` tests.

### Linting and Checks

- **[lint]**: Lints code using `cargo clippy` and other checks
- **[lint-actions]**: Lints GitHub Actions workflows
- **[label-pr]**: Automatically labels PRs

[unit]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/unit.yml
[integration]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/integration.yml
[bench]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/bench.yml
[eth-sync]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/eth-sync.yml
[op-sync]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/op-sync.yml
[stage]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/stage.yml
[book]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/book.yml
[deny]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/deny.yml
[release]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/release.yml
[release-dist]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/release-dist.yml
[dependencies]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/dependencies.yml
[stale]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/stale.yml
[docker]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/docker.yml
[assertoor]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/assertoor.yml
[hive]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/hive.yml
[lint]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/lint.yml
[lint-actions]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/lint-actions.yml
[label-pr]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/label-pr.yml
