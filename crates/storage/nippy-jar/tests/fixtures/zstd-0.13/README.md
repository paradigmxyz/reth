These fixtures were generated from Reth commit
`b51bb683f8` with the unchanged lockfile dependencies:

- `zstd 0.13.3`
- `zstd-safe 7.3.0`
- `zstd-sys 2.1.0+zstd.1.5.7`

Both jars contain 100 rows and two 32-byte columns. Column 0 repeats the row
number; column 1 contains the row number plus each byte offset, wrapping at 256.
`dictionary.conf` also stores the trained dictionaries. The current jar writer
writes streaming frames even when dictionaries are configured, so
`dictionary.block` separately exercises a frame compressed with the first trained
dictionary. It contains column 0 of row 5.

To reproduce, add the following temporary test to `src/lib.rs`'s existing `tests`
module at the producer revision, then run:

```sh
cargo tree --offline --locked -p reth-nippy-jar -i zstd@0.13.3
cargo test --offline --locked -p reth-nippy-jar generate_zstd_013_fixtures
```

```rust
#[test]
fn generate_zstd_013_fixtures() {
    let col1 = (0..100u8).map(|i| vec![i; 32]).collect::<Vec<_>>();
    let col2 = (0..100u8)
        .map(|i| (0..32u8).map(|j| i.wrapping_add(j)).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zstd-0.13");
    reth_fs_util::create_dir_all(&directory).unwrap();
    for (name, dictionary) in [("dictionary", true), ("plain", false)] {
        let path = directory.join(name);
        let mut jar = NippyJar::new_without_header(2, &path).with_zstd(dictionary, 1024);
        if dictionary {
            jar.prepare_compression(vec![col1.clone(), col2.clone()]).unwrap();
            let Compressors::Zstd(zstd) = jar.compressor().unwrap() else { unreachable!() };
            let encoded = zstd.compressors().unwrap().unwrap()[0].compress(&col1[5]).unwrap();
            reth_fs_util::write(directory.join("dictionary.block"), encoded).unwrap();
        }
        jar.freeze(vec![clone_with_result(&col1), clone_with_result(&col2)], 100).unwrap();
        assert_jar_rows(&path, &col1, &col2, 100);
    }
}
```
