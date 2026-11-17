# Local metrics patch

Source: crates.io `metrics` 0.24.6, archive SHA-256
`89550ee9f79e88fef3119de263694973a8adb26c21d75322164fb8c493039fe2`.

The `fuzz` feature dispatches `with_recorder` to `NoopRecorder` without reading
recorder TLS or global state; its now-unused native lookup is cfg-gated. Native dispatch, public types, and macro argument
semantics are unchanged. Keys use deterministic, eagerly computed hashes.
Tests, examples, benchmarks, and their development dependencies are omitted.

External fuzz workload workspaces must repeat the root `metrics` patch to this
source: Cargo does not inherit patches from dependencies. Keep the patched
facade and all recorder/exporter consumers on one package identity.
