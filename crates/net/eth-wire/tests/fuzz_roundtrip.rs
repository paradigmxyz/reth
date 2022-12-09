//! Round-trip encoding fuzzing for the `eth-wire` crate.
use std::fmt::Debug;

use reth_eth_wire::{DisconnectReason, HelloMessage};
use reth_rlp::{Decodable, Encodable};
use serde::Serialize;
use test_fuzz::test_fuzz;

/// Creates a fuzz test for a type that should be [`Encodable`](reth_rlp::Encodable) and
/// [`Decodable`](reth_rlp::Decodable).
///
/// The test will create a random instance of the type, encode it, and then decode it.
fn roundtrip_encoding<T>(thing: T)
where
    T: Encodable + Decodable + Clone + Serialize + Debug + PartialEq + Eq,
{
    let mut encoded = Vec::new();
    thing.encode(&mut encoded);
    let decoded = T::decode(&mut &encoded[..]).unwrap();
    assert_eq!(thing, decoded, "expected: {thing:?}, got: {decoded:?}");
}

/// Takes as input a type and testname, using the type as the type being fuzzed.
macro_rules! fuzz_type_and_name {
    ( $x:ty, $testname:ident) => {
        /// Fuzzes the round-trip encoding of the type.
        #[cfg(test)]
        #[allow(dead_code)]
        #[test_fuzz]
        fn $testname(thing: $x) {
            roundtrip_encoding::<$x>(thing)
        }
    };
}

#[allow(non_snake_case)]
#[cfg(any(test, feature = "bench"))]
pub mod fuzz_rlp {
    use super::*;

    fuzz_type_and_name!(HelloMessage, fuzz_HelloMessage);
    fuzz_type_and_name!(DisconnectReason, fuzz_DisconnectReason);
}
