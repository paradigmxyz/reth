# Benchmark hosting

Charts are uploaded to GitHub's user-attachments service using the same request
as [gh --attach](https://github.com/cli/cli/blob/v2.99.0/internal/attachments/client.go).
The uploader uses Node.js and the existing gh CLI; it does not require upgrading
gh to a version that exposes the attach flag.

Run locally with gh authenticated as a user with repository write access:

```sh
node .github/scripts/bench-upload-charts.js owner/repo /path/to/charts
node --test .github/scripts/bench-hosting.test.js
BENCH_LIVE_UPLOAD_REPO=owner/repo node --test --test-name-pattern='live GitHub image attachment' .github/scripts/bench-hosting.test.js
```

The last command uploads one synthetic PNG without posting a comment. CI uses
DEREK_TOKEN, which must be a supported user PAT or OAuth token with write access
to the target repository. GitHub App/Actions installation tokens cannot upload
attachments. Credential proxies must allow the token on uploads.github.com as
well as api.github.com.

All PNGs are validated before the first upload. Each successful URL is saved
immediately in charts/attachments.json with the image hash and repository ID;
rerunning against that directory reuses successful uploads. POST requests are
not retried automatically. charts/charts.md is shared by PR comments and job
summaries. Both files are included in the results artifact, including the
manifest after a partial upload failure. A fresh job/work directory will upload
new attachments.

Scheduled baseline SHAs are stored in Actions artifacts named for their series
(e.g. nightly-last-feature-ref or replay-nightly-mainnet-last-feature-ref).
Readers accept only successful runs of the expected workflow on the default
branch. API errors fail the resolver rather than silently resetting its state.
The artifacts expire after 90 days. The first run after migration, or after
expiry, uses the resolver's existing first-run baseline: self-comparison for
nightly, HEAD~1 for Reth hourly, and the latest release for Reth release mode.
Historical charts and old state in the former hosting repositories are not
deleted by these workflows.

Chrome trace JSON files remain in bench-results. Download that artifact and open
the files in https://ui.perfetto.dev/. One-click remote trace loading is no longer
provided because the attachment endpoint only accepts images and videos.
