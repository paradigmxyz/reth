## Codecs

This crate allows to easily configure different codecs for different purposes (benchmarks, user configuration) with minimal changes. Having them to be configurable through annotations allows us to contain their implementations/leakage to isolated portions of the project.

Examples:

- [`Header` struct](../../primitives/src/header.rs)
- [DB usage](../db/src/kv/codecs/scale.rs)
