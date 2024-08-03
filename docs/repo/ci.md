## CI

The CI runs a couple of workflows:

### Code

- **[unit]**: Runs unit tests (tests in `src/`) and doc tests
- **[integration]**: Runs integration tests (tests in `tests/` and sync tests)
- **[bench]**: Runs benchmarks

### Docs

- **[book]**: Builds, tests, and deploys the book.

### Meta

- **[deny]**: Runs `cargo deny` to check for license conflicts and security advisories in our dependencies
- **[release]**: Runs the release workflow

[unit]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/unit.yml
[integration]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/integration.yml
[bench]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/bench.yml
[book]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/book.yml
[deny]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/deny.yml
[release]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/release.yml
