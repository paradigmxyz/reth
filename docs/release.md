## Releases

### Release cadence

**reth does not currently have a regular release cadence while it is still experimental software**.

### For maintainers

This section outlines how to cut a new release.

It is assumed that the commit that is being considered for release has been marked as stable, i.e. that there is an expectation of no major bugs.

#### Release PR

- [ ] Create a new branch (e.g. `release/vx.y.z`) and open a pull request for it
- [ ] Ensure *all* tests and lints pass for the chosen commit
- [ ] Version bump
  - [ ] Update the version in *all* `Cargo.toml`'s
- [ ] Commit the changes
  - The message format should be `release: vx.y.z`, substituting `x.y.z` for the semver.
- [ ] The PR should be reviewed to see if anything was missed
- [ ] Once reviewed, merge the PR

#### Releasing

- [ ] Tag the new commit on main with `vx.y.z` (`git tag vx.y.z SHA`)
- [ ] Push the tag (`git push origin vx.y.z`)[^1]
- [ ] Run the release commit on testing infrastructure for 1-3 days to check for inconsistencies and bugs
  - This testing infrastructure is going to sync and keep up with a live testnet, and includes monitoring of bandwidth, CPU, disk space etc.

> **Note**
> 
> The `v` prefix for the tag is important! If it is missing, the release workflow **will not run**.

When the tag is pushed, the artifacts are built automatically and a draft release is added to the repository. This draft release includes a template that must be filled out, including:

- A summary of the release (highlights etc.)
- The update priority (see below)
- An auto-generated changelog

The release artifacts are automatically added to the draft release. Once ready, simply publish the release.

#### Release summaries

The release summary should include general notes on what the release contains that is important to operators. These changes can be found using the https://github.com/paradigmxyz/reth/labels/M-changelog label.

[^1]: It is possible to use `git push --tags`, but this is discouraged since it can be very difficult to get rid of bad tags.