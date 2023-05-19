## CI

The CI runs a couple of workflows:

### Code

- **[ci]**: A catch-all for small jobs. Currently only runs lints (rustfmt, clippy etc.)
- **[unit]**: Runs unit tests (tests in `src/`) and doc tests
- **[integration]**: Runs integration tests (tests in `tests/` and sync tests)
- **[fuzz]**: Runs fuzz tests
- **[bench]**: Runs benchmarks

### Docs

- **[book]**: Builds, tests, and deploys the book.

### Meta

- **[deny]**: Runs `cargo deny` to check for license conflicts and security advisories in our dependencies
- **[sanity]**: Runs a couple of sanity checks on the code every night, such as checking for unused dependencies
- **[release]**: Runs the release workflow

[ci]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/ci.yml
[unit]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/unit.yml
[integration]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/integration.yml
[fuzz]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/fuzz.yml
[bench]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/bench.yml
[book]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/book.yml
[deny]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/deny.yml
[sanity]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/sanity.yml
[release]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/release.yml
[gh-projects]: https://docs.github.com/en/issues/planning-and-tracking-with-projects/automating-your-project/automating-projects-using-actions