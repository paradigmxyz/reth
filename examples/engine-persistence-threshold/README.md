# Engine Persistence Threshold Example

Demonstrates `--engine.persistence-threshold 0` for immediate block persistence.

Related: [#16511](https://github.com/paradigmxyz/reth/issues/16511) - external DB readers lag behind when using default threshold.

## Usage

```sh
cargo run -p example-engine-persistence-threshold
```

Compare threshold 0 vs 2:

```sh
cargo run -p example-engine-persistence-threshold -- --compare
```

## What it does

1. Starts a dev node with configurable persistence threshold
2. Submits a transaction to produce a block
3. Checks if the block is visible on disk immediately

With threshold 0, blocks are visible immediately. With threshold 2 (default), blocks remain in memory until the threshold is exceeded.
