//! Round-trip encoding fuzzing for the `eth-wire` crate.
use serde::Serialize;
use reth_eth_wire::HelloMessage;
use reth_rlp::{Encodable, Decodable};
use test_fuzz::test_fuzz;

/// Creates a fuzz test for a type that should be [`Encodable`](reth_rlp::Encodable) and
/// [`Decodable`](reth_rlp::Decodable).
///
/// The test will create a random instance of the type, encode it, and then decode it.
#[test_fuzz]
fn fuzz_roundtrip<T: Encodable + Decodable + Clone + Serialize>(thing: T) {
}

macro_rules! fuzz_type {
    ( $( $x:ty ),* ) => {

    };
}

fuzz_type!(HelloMessage);
