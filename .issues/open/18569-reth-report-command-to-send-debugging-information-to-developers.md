---
title: '`reth report` command to send debugging information to developers'
labels:
    - A-cli
    - C-enhancement
assignees:
    - jenpaff
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.998829Z
info:
    author: shekhirin
    created_at: 2025-09-19T09:26:15Z
    updated_at: 2025-09-28T14:17:48Z
---

### Describe the feature

When an issue like state root mismatch or EVM execution error happens, we usually want to get as much information as possible from the user. This includes, but not limited to:
- Debug logs from `$LOGS_DIR/reth.log*`
- Invalid block hook data (`$DATADIR/invalid_block_hooks`)
- `VersionHistory` full table dump

To help automate this, we should introduce `reth report` command that collects all this data, archives it, and uploads to a write-only S3 bucket that only devs can read. We should confirm with the user that they're fine with uploading that data. Additionally, we should log a prepared [GitHub issue creation link](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-an-issue#creating-an-issue-from-a-url-query) that will include the link to upload S3 archive.

When we have this command implemented, we should log a suggestion to run it when a block validation error happens, for example state root mismatch or EVM execution error.

### Additional context

_No response_
