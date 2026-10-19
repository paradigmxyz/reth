## CI

The CI runs a couple of workflows:

### Code

- **[unit]**: Runs unit tests (tests in `src/`) and doc tests
- **[integration]**: Runs integration tests (tests in `tests/` and sync tests)
- **[bench]**: Runs benchmarks
- **[sync]**: Runs sync tests
- **[stage]**: Runs all `stage run` commands

### Docs

- **[book]**: Builds, tests, and deploys the book.

### Meta
- **[release]**: Runs the release workflow
- **[release-dist]**: Publishes Reth to external package managers
- **[dependencies]**: Runs `cargo update` periodically to keep dependencies current
- **[stale]**: Marks issues as stale if there has been no activity
- **[docker]**: Publishes the Docker image.

#### Docker workflow changes

Docker changes usually affect more than one workflow. Before merging changes to Dockerfiles,
Docker build inputs, or files copied into images:

- Check all Dockerfiles and bake targets that share the repository build context:
  `Dockerfile`, `Dockerfile.depot`, `.github/scripts/hive/Dockerfile`, and `docker-bake.hcl`.
- Keep `.dockerignore` in sync with every file or directory copied from the repository context.
  A file that exists in git can still be missing from Docker builds if it is not allowlisted there.
- Check the workflow users of those targets: `docker.yml`, reusable `docker-test.yml`, and callers
  such as `hive.yml` and `kurtosis.yml`.
- Run local syntax checks before opening or merging the PR when BuildKit/buildx is available, for
  example `docker build --check -f Dockerfile .`, `docker build --check -f Dockerfile.depot .`,
  and `docker buildx bake --print -f docker-bake.hcl`.
- Run at least one GitHub Actions dry run for the affected workflow. For the publishing workflow,
  use `docker.yml` with `workflow_dispatch`, `build_type=git-sha`, and `dry_run=true`.
- When updating shared image contents, look at recent Docker-related PRs and existing workflow
  history to identify when failures started and whether sibling workflows are affected.

### Integration Testing

- **[kurtosis]**: Spins up a Kurtosis testnet and runs Assertoor tests on Reth pairs.
- **[hive]**: Runs `ethereum/hive` tests.

### Linting and Checks

- **[lint]**: Lints code using `cargo clippy` and other checks
- **[lint-actions]**: Lints GitHub Actions workflows
- **[label-pr]**: Automatically labels PRs

[unit]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/unit.yml
[integration]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/integration.yml
[bench]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/bench.yml
[sync]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/sync.yml
[stage]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/stage.yml
[book]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/book.yml
[release]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/release.yml
[release-dist]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/release-dist.yml
[dependencies]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/dependencies.yml
[stale]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/stale.yml
[docker]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/docker.yml
[kurtosis]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/kurtosis.yml
[hive]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/hive.yml
[lint]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/lint.yml
[lint-actions]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/lint-actions.yml
[label-pr]: https://github.com/paradigmxyz/reth/blob/main/.github/workflows/label-pr.yml
