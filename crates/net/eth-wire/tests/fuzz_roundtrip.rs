//! Round-trip encoding fuzzing for the `eth-wire` crate.
use reth_rlp::{Decodable, Encodable};
use serde::Serialize;
use std::fmt::Debug;

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

/// Creates a fuzz test for a rlp encodable and decodable type.
macro_rules! fuzz_type_and_name {
    ( $x:ty, $fuzzname:ident ) => {
        /// Fuzzes the round-trip encoding of the type.
        #[test_fuzz]
        #[allow(non_snake_case)]
        fn $fuzzname(thing: $x) {
            roundtrip_encoding::<$x>(thing)
        }
    };
}

#[allow(non_snake_case)]
#[cfg(any(test, feature = "bench"))]
pub mod fuzz_rlp {
    use reth_eth_wire::{
        BlockBodies, BlockHeaders, DisconnectReason, GetBlockBodies, GetBlockHeaders, GetNodeData,
        GetPooledTransactions, GetReceipts, HelloMessage, NewBlock, NewBlockHashes,
        NewPooledTransactionHashes, NodeData, P2PMessage, PooledTransactions, Receipts, Status,
        Transactions,
    };
    use reth_primitives::BlockHashOrNumber;
    use reth_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
    use serde::{Deserialize, Serialize};
    use test_fuzz::test_fuzz;

    use crate::roundtrip_encoding;

    // p2p subprotocol messages
    fuzz_type_and_name!(HelloMessage, fuzz_HelloMessage);
    fuzz_type_and_name!(DisconnectReason, fuzz_DisconnectReason);

    // eth subprotocol messages
    fuzz_type_and_name!(Status, fuzz_Status);
    fuzz_type_and_name!(NewBlockHashes, fuzz_NewBlockHashes);
    fuzz_type_and_name!(Transactions, fuzz_Transactions);

    // GetBlockHeaders implements all the traits required for roundtrip_encoding, so why is this
    // wrapper type needed?
    //
    // While GetBlockHeaders implements all traits needed to work for test-fuzz, it does not have
    // an obvious Default implementation since BlockHashOrNumber can be either a hash or number,
    // and the default value of BlockHashOrNumber is not obvious.
    //
    // We just provide a default value here so test-fuzz can auto-generate a corpus file for the
    // type.
    #[derive(
        Clone,
        Debug,
        PartialEq,
        Eq,
        Serialize,
        Deserialize,
        RlpEncodableWrapper,
        RlpDecodableWrapper,
    )]
    struct GetBlockHeadersWrapper(pub GetBlockHeaders);

    impl Default for GetBlockHeadersWrapper {
        fn default() -> Self {
            GetBlockHeadersWrapper(GetBlockHeaders {
                start_block: BlockHashOrNumber::Number(0),
                limit: Default::default(),
                skip: Default::default(),
                direction: Default::default(),
            })
        }
    }

    fuzz_type_and_name!(GetBlockHeadersWrapper, fuzz_GetBlockHeaders);

    fuzz_type_and_name!(BlockHeaders, fuzz_BlockHeaders);
    fuzz_type_and_name!(GetBlockBodies, fuzz_GetBlockBodies);
    fuzz_type_and_name!(BlockBodies, fuzz_BlockBodies);
    fuzz_type_and_name!(NewBlock, fuzz_NewBlock);
    fuzz_type_and_name!(NewPooledTransactionHashes, fuzz_NewPooledTransactionHashes);
    fuzz_type_and_name!(GetPooledTransactions, fuzz_GetPooledTransactions);
    fuzz_type_and_name!(PooledTransactions, fuzz_PooledTransactions);
    fuzz_type_and_name!(GetNodeData, fuzz_GetNodeData);
    fuzz_type_and_name!(NodeData, fuzz_NodeData);
    fuzz_type_and_name!(GetReceipts, fuzz_GetReceipts);
    fuzz_type_and_name!(Receipts, fuzz_Receipts);

    // manually test Ping and Pong which are not covered by the above

    /// Tests the round-trip encoding of Ping
    #[test]
    fn roundtrip_ping() {
        roundtrip_encoding::<P2PMessage>(P2PMessage::Ping)
    }

    /// Tests the round-trip encoding of Pong
    #[test]
    fn roundtrip_pong() {
        roundtrip_encoding::<P2PMessage>(P2PMessage::Pong)
    }
}
