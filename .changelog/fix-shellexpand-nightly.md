---
reth-node-core: patch
reth-cli: patch
reth-bench-compare: patch
---

Replaced unmaintained `shellexpand` dependency with inline tilde (`~`) and environment variable (`$VAR`/`${VAR}`) expansion. The `shellexpand` crate fails to compile on nightly Rust due to a method resolution conflict from the stabilization of `OsStr::as_str()`.
