## Codecs

This crate allows to easily configure different codecs for different purposes (benchmarks, user configuration) with minimal changes. Having them to be configurable through annotations allows us to contain their implementations/leakage to isolated portions of the project.

Examples:

- [`Header` struct](../primitives/src/header.rs)
- [DB usage](../db/src/kv/codecs/scale.rs)

### Features

Feature defines what is the main codec used by `#[main_codec]`. However it is still possible to define them directly: `#[use_scale]`, `#[use_postcat]`, `#[no_codec]`.

```rust
default = ["scale"]
scale = ["codecs-derive/scale"]
postcard = ["codecs-derive/postcard"]
no_codec = ["codecs-derive/no_codec"]
```