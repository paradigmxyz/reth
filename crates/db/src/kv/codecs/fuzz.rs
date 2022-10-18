//! Implements fuzzing targets to be used by test-fuzz

macro_rules! impl_fuzzer {
    ($($name:tt),+) => {
        $(
            /// Macro generated module to be used by test-fuzz and `bench` if it applies.
            #[allow(non_snake_case)]
            #[cfg(any(test, feature = "bench"))]
            pub mod $name {
                use reth_primitives::$name;
                use crate::kv::table;

                /// Encodes and decodes table types returning its encoded size and the decoded object.
                pub fn encode_and_decode(obj: $name) -> (usize, $name) {
                    let data = table::Encode::encode(obj);
                    let size = data.len();
                    (size, table::Decode::decode(data).expect("failed to decode"))
                }

                #[cfg(test)]
                #[allow(dead_code)]
                #[test_fuzz::test_fuzz]
                pub fn fuzz(obj: $name) {
                    assert!(encode_and_decode(obj.clone()).1 == obj );
                }

                #[test]
                pub fn test() {
                    encode_and_decode($name::default());
                }
            }

        )+
    };
}

impl_fuzzer!(Header, Account);
