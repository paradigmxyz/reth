## Release Workflow

1. Update the version in `Cargo.toml`
2. Update the changelog[^1]
    - Check that all issues marked https://github.com/paradigmxyz/reth/labels/M-changelog have been added to the changelog
    - Move the "Unreleased" section in the changelog under a new header with the new version, and fix up the links in the bottom of the file
3. Ensure tests and lints pass
4. Commit the new changelog and version bump
5. Tag the new version (no `v` prefix)
6. Push the new commit and tag
7. (Run release command)[^2]

[^1]: This can be automated somewhat.
[^2]: TBD.