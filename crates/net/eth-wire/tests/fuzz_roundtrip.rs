//! Round-trip encoding fuzzing for the `eth-wire` crate.

use alloy_rlp::{Decodable, Encodable};
use serde::Serialize;
use std::fmt::Debug;

/// Creates a fuzz test for a type that should be [`Encodable`](alloy_rlp::Encodable) and
/// [`Decodable`](alloy_rlp::Decodable).
///
/// The test will create a random instance of the type, encode it, and then decode it.
fn roundtrip_encoding<T>(thing: T)
where
    T: Encodable + Decodable + Debug + PartialEq + Eq,
{
    let mut encoded = Vec::new();
    thing.encode(&mut encoded);
    let decoded = T::decode(&mut &encoded[..]).unwrap();
    assert_eq!(thing, decoded, "expected: {thing:?}, got: {decoded:?}");
}

/// This method delegates to roundtrip_encoding, but is used to enforce that each type input to the
/// macro has a proper Default, Clone, and Serialize impl. These trait implementations are
/// necessary for test-fuzz to autogenerate a corpus.
///
/// If it makes sense to remove a Default impl from a type that we fuzz, this should prevent the
/// fuzz test from compiling, rather than failing at runtime.
/// In this case, we should implement a wrapper for the type that should no longer implement
/// Default, Clone, or Serialize, and fuzz the wrapper type instead.
fn roundtrip_fuzz<T>(thing: T)
where
    T: Encodable + Decodable + Clone + Serialize + Debug + PartialEq + Eq + Default,
{
    roundtrip_encoding::<T>(thing)
}

/// Creates a fuzz test for a rlp encodable and decodable type.
macro_rules! fuzz_type_and_name {
    ( $x:ty, $fuzzname:ident ) => {
        /// Fuzzes the round-trip encoding of the type.
        #[allow(non_snake_case)]
        #[test_fuzz]
        fn $fuzzname(thing: $x) {
            crate::roundtrip_fuzz::<$x>(thing)
        }
    };
}

#[cfg(any(test, feature = "bench"))]
pub mod fuzz_rlp {
    use crate::roundtrip_encoding;
    use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
    use reth_codecs::derive_arbitrary;
    use reth_eth_wire::{
        BlockBodies, BlockHeaders, DisconnectReason, GetBlockBodies, GetBlockHeaders, GetNodeData,
        GetPooledTransactions, GetReceipts, HelloMessage, NewBlock, NewBlockHashes,
        NewPooledTransactionHashes66, NewPooledTransactionHashes68, NodeData, P2PMessage,
        PooledTransactions, Receipts, Status, Transactions,
    };
    use reth_primitives::{BlockHashOrNumber, TransactionSigned};
    use serde::{Deserialize, Serialize};
    use test_fuzz::test_fuzz;

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

    // p2p subprotocol messages

    // see message below for why wrapper types are necessary for fuzzing types that do not have a
    // Default impl
    #[derive_arbitrary(rlp)]
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
    struct HelloMessageWrapper(HelloMessage);

    impl Default for HelloMessageWrapper {
        fn default() -> Self {
            HelloMessageWrapper(HelloMessage {
                client_version: Default::default(),
                capabilities: Default::default(),
                protocol_version: Default::default(),
                id: Default::default(),
                port: Default::default(),
            })
        }
    }

    fuzz_type_and_name!(HelloMessageWrapper, fuzz_HelloMessage);
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
    #[derive_arbitrary(rlp)]
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
    struct GetBlockHeadersWrapper(GetBlockHeaders);

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
    fuzz_type_and_name!(NewPooledTransactionHashes66, fuzz_NewPooledTransactionHashes66);
    fuzz_type_and_name!(NewPooledTransactionHashes68, fuzz_NewPooledTransactionHashes68);
    fuzz_type_and_name!(GetPooledTransactions, fuzz_GetPooledTransactions);
    fuzz_type_and_name!(PooledTransactions, fuzz_PooledTransactions);
    fuzz_type_and_name!(GetNodeData, fuzz_GetNodeData);
    fuzz_type_and_name!(NodeData, fuzz_NodeData);
    fuzz_type_and_name!(GetReceipts, fuzz_GetReceipts);
    fuzz_type_and_name!(Receipts, fuzz_Receipts);
    fuzz_type_and_name!(TransactionSigned, fuzz_TransactionSigned);
}
