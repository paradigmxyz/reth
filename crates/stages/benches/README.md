# Stage Benchmarks

Test vectors are automatically generated if they cannot be found. Futhermore, for certain stages you can link an external database to run the benchmarks.

## Usage

It will run the normal criterion benchmark.
```
cargo bench --package reth-stages --bench criterion --features test-utils
```

It will generate a flamegraph report without running any criterion analysis.
```
cargo bench --package reth-stages --bench criterion --features test-utils -- --profile-time=2
```
Flamegraph reports can be find at `target/criterion/Stages/$STAGE_LABEL/profile/flamegraph.svg` 


## External DB support
To choose an external DB, just pass an environment variable to the `cargo bench` command.

* Account Hashing Stage: `ACCOUNT_HASHING_DB=`