---
reth-cli-commands: minor
reth-node-core: minor
reth: patch
---

Made v2 storage the default for all new databases, deprecating the `--storage.v2` flag to a hidden no-op kept for backwards compatibility. Updated CLI reference docs to remove the now-hidden flag from all command help pages.
