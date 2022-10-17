# tests

## fuzz

Requirements:
`cargo install -f cargo-test-fuzz afl`

How to run:
`cargo test` - it will generate the corpus w/ list of fuzz targets.
`cargo test-fuzz Header` - will fuzz.