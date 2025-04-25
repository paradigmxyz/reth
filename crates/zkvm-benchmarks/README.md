## ZKEVM benchmarks

## Tools required

In order to run these benchmarks, you must install sp1. [Guide here](https://docs.succinct.xyz/docs/sp1/getting-started/install)

## Instructions

### Compile the program

The first step is to compile the program that we will prove in RISCV32IM.

- cd `program`
- `cargo prove build`

### Executing the host

The next step is to run the executor/host to get the number of cycles for certain regions of the program.

- cd `host`
- `RUST_LOG=info cargo run --release`

### Inputs

The input to each program, is ClientInput, which consists of a block and the execution witness. The process to getting these
is a bit messy and unimportant right now. For adding new tests to benchmark, place the filled tests into `/testing/ef-tests/ethereum-tests/BlockchainTests/ValidBlocks`.

If you run `make ef-tests` it will download a particular version of the released execution specs to `/testing/ef-tests/ethereum-tests/` from the execution spec tests repo. If you want to know the exact version, check the `Makefile`.
