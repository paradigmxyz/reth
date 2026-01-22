---
title: Make sure that we can just delete static files and it will not break provider access
labels:
    - A-static-files
    - C-enhancement
assignees:
    - shekhirin
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 18682
synced_at: 2026-01-21T11:32:16.003689Z
info:
    author: shekhirin
    created_at: 2025-11-07T17:30:49Z
    updated_at: 2025-11-24T13:42:04Z
---

### Describe the feature

```console
❯ ls /mnt/nvme/reth/mainnet/static_files/static_file_* | first 6
╭───┬────────────────────────────────────────────────────────────────────────────────┬──────┬──────────┬────────────╮
│ # │                                      name                                      │ type │   size   │  modified  │
├───┼────────────────────────────────────────────────────────────────────────────────┼──────┼──────────┼────────────┤
│ 0 │ /mnt/nvme/reth/mainnet/static_files/static_file_headers_0_499999               │ file │ 158.2 MB │ a year ago │
│ 1 │ /mnt/nvme/reth/mainnet/static_files/static_file_headers_0_499999.conf          │ file │     75 B │ a year ago │
│ 2 │ /mnt/nvme/reth/mainnet/static_files/static_file_headers_0_499999.off           │ file │  12.0 MB │ a year ago │
│ 3 │ /mnt/nvme/reth/mainnet/static_files/static_file_headers_10000000_10499999      │ file │ 276.8 MB │ a year ago │
│ 4 │ /mnt/nvme/reth/mainnet/static_files/static_file_headers_10000000_10499999.conf │ file │     75 B │ a year ago │
│ 5 │ /mnt/nvme/reth/mainnet/static_files/static_file_headers_10000000_10499999.off  │ file │  12.0 MB │ a year ago │
╰───┴────────────────────────────────────────────────────────────────────────────────┴──────┴──────────┴────────────╯
```

Make sure that I can just delete `static_file_headers_0_499999`, `static_file_headers_0_499999.conf` and `static_file_headers_0_499999.off` files, and the provider will start returning `None` for all headers in that range. Same for other segments.

### Additional context

_No response_
