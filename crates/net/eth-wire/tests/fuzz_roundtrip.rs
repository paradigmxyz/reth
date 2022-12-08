//! Round-trip encoding fuzzing for the `eth-wire` crate.
use std::fmt::Debug;

use reth_eth_wire::HelloMessage;
use reth_rlp::{Decodable, Encodable};
use serde::Serialize;
use test_fuzz::test_fuzz;

/// Creates a fuzz test for a type that should be [`Encodable`](reth_rlp::Encodable) and
/// [`Decodable`](reth_rlp::Decodable).
///
/// The test will create a random instance of the type, encode it, and then decode it.
#[test_fuzz]
fn roundtrip_encoding<T>(thing: T)
where
    T: Encodable + Decodable + Clone + Serialize + Debug + PartialEq + Eq,
{
    let mut encoded = Vec::new();
    thing.encode(&mut encoded);
    let decoded = T::decode(&mut &encoded[..]).unwrap();
    assert_eq!(thing, decoded, "expected: {thing:?}, got: {decoded:?}");
}

macro_rules! fuzz_type {
    ( $x:ty ) => {};
}

fuzz_type!(HelloMessage);
