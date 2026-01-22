---
title: Support for ERA (not ERA1) hosts in the downloader
labels:
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 16891
synced_at: 2026-01-21T11:32:15.989496Z
info:
    author: RomanHodulak
    created_at: 2025-06-18T13:38:10Z
    updated_at: 2025-11-10T07:18:42Z
---

### Describe the feature

Currently, the `reth_era_downloader` can deal with ERA1 hosts.

https://github.com/paradigmxyz/reth/blob/9ab57f70e3e5686b59a6f93fc41363d46707de73/crates/era-downloader/Cargo.toml#L2

The goal is to add support for ERA (not ERA1) hosts.

One notable difference is that compared to ERA1 hosts, these don't tend to contain checksums.txt, which is unfortunate as it makes file integrity checks impossible.

Another one is that, of course, the file format is different. However, the downloader is not dealing with the encoding at all, so this is not a concern.

### Additional context

_No response_
