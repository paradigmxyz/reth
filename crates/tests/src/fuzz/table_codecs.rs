//! Target encoding/decoding of different types

macro_rules! impl_fuzzer {
    ($($name:tt),+) => {
        $(
            #[allow(non_snake_case)]
            mod $name {
                use reth_primitives::$name;
                use reth_db::kv::table;

                #[allow(dead_code)]
                #[test_fuzz::test_fuzz]
                fn fuzz(obj: $name) {
                    let data = table::Encode::encode(obj.clone());
                    assert!(obj == table::Decode::decode(data).expect("failed to decode"));
                }

                #[test]
                fn test() {
                    fuzz($name::default());
                }
            }

        )+
    };
}

impl_fuzzer!(Header, Account);
