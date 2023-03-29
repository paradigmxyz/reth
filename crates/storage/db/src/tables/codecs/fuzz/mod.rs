//! Implements fuzzing targets to be used by test-fuzz

mod inputs;

/// Fuzzer generates a random instance of the object and proceeds to encode and decode it. It then
/// makes sure that it matches the original object.
///
/// Some types like [`IntegerList`] might have some restrictions on how they're fuzzed. For example,
/// the list is assumed to be sorted before creating the object.
macro_rules! impl_fuzzer_with_input {
    ($(($name:tt, $input_type:tt, $encode:tt, $encode_method:tt, $decode:tt, $decode_method:tt)),+) => {
        $(
            /// Macro generated module to be used by test-fuzz and `bench` if it applies.
            #[allow(non_snake_case)]
            #[cfg(any(test, feature = "bench"))]
            pub mod $name {
                use crate::table;

                #[allow(unused_imports)]
                use reth_primitives::*;

                #[allow(unused_imports)]
                use super::inputs::*;

                #[allow(unused_imports)]
                use crate::tables::models::*;

                /// Encodes and decodes table types returning its encoded size and the decoded object.
                /// This method is used for benchmarking, so its parameter should be the actual type that is being tested.
                pub fn encode_and_decode(obj: $name) -> (usize, $name)
                {
                    let data = table::$encode::$encode_method(obj);
                    let size = data.len();

                    // Some `data` might be a fixed array.
                    (size, table::$decode::$decode_method(data.to_vec()).expect("failed to decode"))
                }

                #[cfg(test)]
                #[allow(dead_code)]
                #[test_fuzz::test_fuzz]
                pub fn fuzz(obj: $input_type)  {
                    let obj: $name = obj.into();
                    assert!(encode_and_decode(obj.clone()).1 == obj );
                }

                #[test]
                pub fn test() {
                    fuzz($input_type::default())
                }
            }

        )+
    };
}

/// Fuzzer generates a random instance of the object and proceeds to encode and decode it. It then
/// makes sure that it matches the original object.
macro_rules! impl_fuzzer_key {
    ($($name:tt),+) => {
        $(
            impl_fuzzer_with_input!(($name, $name, Encode, encode, Decode, decode));
        )+
    };
}

/// Fuzzer generates a random instance of the object and proceeds to compress and decompress it. It
/// then makes sure that it matches the original object.
#[allow(unused)]
macro_rules! impl_fuzzer_value {
    ($($name:tt),+) => {
        $(
            impl_fuzzer_with_input!(($name, $name, Compress, compress, Decompress, decompress));
        )+
    };
}

/// Fuzzer generates a random instance of the object and proceeds to compress and decompress it. It
/// then makes sure that it matches the original object. It supports being fed a different kind of
/// input, as long as it supports `Into<T>`.
macro_rules! impl_fuzzer_value_with_input {
    ($(($name:tt, $input:tt)),+) => {
        $(
            impl_fuzzer_with_input!(($name, $input, Compress, compress, Decompress, decompress));
        )+
    };
}

impl_fuzzer_key!(BlockNumHash, TransitionIdAddress);
impl_fuzzer_value_with_input!((IntegerList, IntegerListInput));
