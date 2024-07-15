## reth-codecs

This crate has helpers to implement the main codec used internally to save data in the different storage types.

Currently `Compact` is used when adding the derive macro `reth_codec`.

This crate implements the main codec (`Compact`) for:
* [primitive types](src/lib.rs)
* [alloy types](src/alloy/mod.rs): uses bridge types alongside `reth_codec` from [derive](derive/src/lib.rs)

### reth-codecs-derive

Provides derive macros that can be added and configured to stored data structs/enums
* `#[reth_codec]`: Implements `Compact` as well as `#[derive_arbitrary(compact)]`
* `#[reth_codec(rlp)]`: Implements `Compact` as well as `#[derive_arbitrary(compact, rlp)]`
* `#[reth_codec(no_arbitrary)]`: Implements `Compact` without `derive_arbitrary`.

* `#[derive_arbitrary]`: will derive arbitrary `Arbitrary` and `proptest::Arbitrary` with no generated tests.
* `#[derive_arbitrary(rlp)]`: will derive arbitrary and generate rlp roundtrip proptests.
* `#[derive_arbitrary(compact, rlp)]`. will derive arbitrary and generate rlp and compact roundtrip proptests.
* `#[derive_arbitrary(rlp, N)]`: will derive arbitrary and generate rlp roundtrip proptests. Limited to N cases.

In case the type wants to implement `Arbitrary` manually it's still possible to add generated tests with:
* `#[add_arbitrary_tests]`
* `#[add_arbitrary_tests(rlp)]`


### Compact

The general idea behind [`Compact`](src/lib.rs#L30) is to minimize the number of bytes that a data is serialized to without compressing. If an `uint32` only requires 1 byte to represent, do just that. It uses a mix of `proc_macro` and trait implementations to accomplish that.


#### Bitflag struct
`Compact` will generate a companion bitflag struct ([modular_bitfield](https://crates.io/crates/modular_bitfield)) to aid that (eg. `Receipt` and `ReceiptFlags`). **These aid struct fields can represent whatever is necessary**: the presence or absence of a value (eg. `Option<T>`), the number of bytes necessary to read a field (eg. `uint32` might only need 1 byte) or a variant of a serialized field (eg. `TxKind`). **The amount of bits required for each aid field is represented by this function**: [get_bit_size](derive/src/compact/mod.rs#L170). Any field that doesn't store any information in this bitflag struct handles their size on their own (eg. `Vec<T>`). 

This also means that types present in [get_bit_size](derive/src/compact/mod.rs#L170), even though implement the `Compact` trait, they cannot be used as standalone values and need a wrapper type. (eg. `U256` & `CompactU256`).


#### Restrictions
One hard restriction is that `Bytes` fields should be placed last. This allows us skipping storing the length of the field, and just read until the end of the buffer. **This restriction extends to [unknown types](derive/src/compact/generator.rs#55)**, since it's hard to know from the `proc_macro` perspective if they have `Bytes` fields or not. More about it [here](derive/src/compact/structs.rs#L117).

Another restriction is that since rust does not allow for specialized implementations over certain types like `Vec<T>`/`Option<T>` where `T` is a fixed size array, the trait itself has two additional methods: `specialized_to_compact` & `specialized_from_compact`.
For example, `Vec<T>::from_compact` will call `decode_varuint` before every element, while `Vec<B256>` will use `specialized_from_compact` so it can just read 32 bytes without any varuint decoding. This is generally handled by the derive macro.