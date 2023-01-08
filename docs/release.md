## Releasing

This document outlines how to cut a new release.

### Checklist

It is assumed that the commit that is being considered for release has been marked as stable, i.e. that there is an expectation of no major bugs.

- [ ] Ensure *all* tests and lints pass
- [ ] Changelog
  - [ ] Ensure the changelog is up to date by going over https://github.com/paradigmxyz/reth/labels/M-changelog
  - [ ] Move the "Unreleased" section in [CHANGELOG.md](./CHANGELOG.md) into a header with the new version, and add a link at the bottom of the changelog to compare the new version to HEAD.
- [ ] Version bump
  - [ ] Update the version in `Cargo.toml`
- [ ] Commit the changes
  - The message format should be `release: x.y.z`, substituting `x.y.z` for the semver.
- [ ] Tag the new commit with `x.y.z`
- [ ] Push the changes, including the tag

After pushing the tag, CI will start building the release, bundling it, and publishing it to package managers and crates.io. (TBD)

> **Note**
> This is a work in progress. There are still some unknowns:
>
> - Should we require that the commit is signed?
> - Should we publish crates to crates.io? Should each crate use the same version?
>
> Additionally, a workflow needs to be added that:
>
> - Builds the release (cross-platform)
> - Runs *all* tests once more, even the long ones
> - Bundles the release with any aux files (license, config, ...)
> - Publishes the release to GitHub, brew, Dockerhub, Arch, ...
> - Publishes crates to crates.io
> - Creates a GitHub release, copying over the relevant section of the changelog
