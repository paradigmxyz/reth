//! Implements the `GetNodeData` and `NodeData` message types.

use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use reth_codecs_derive::add_arbitrary_tests;
use reth_primitives::{Bytes, B256};

/// A request for state tree nodes corresponding to the given hashes.
/// This message was removed in `eth/67`, only clients running `eth/66` or earlier will respond to
/// this message.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetNodeData(pub Vec<B256>);

/// The response to [`GetNodeData`], containing the state tree nodes or contract bytecode
/// corresponding to the requested hashes.
///
/// Not all nodes are guaranteed to be returned by the peer.
/// This message was removed in `eth/67`.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct NodeData(pub Vec<Bytes>);

#[cfg(test)]
mod tests {
    use reth_primitives::hex;

    use crate::{message::RequestPair, GetNodeData, NodeData};
    use alloy_rlp::{Decodable, Encodable};

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_get_node_data() {
        let expected = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let mut data = vec![];
        let request = RequestPair::<GetNodeData> {
            request_id: 1111,
            message: GetNodeData(vec![
                hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
            ]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_get_node_data() {
        let data = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let request = RequestPair::<GetNodeData>::decode(&mut &data[..]).unwrap();
        assert_eq!(
            request,
            RequestPair::<GetNodeData> {
                request_id: 1111,
                message: GetNodeData(vec![
                    hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                    hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
                ])
            }
        );
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_node_data() {
        let expected = hex!("ce820457ca84deadc0de84feedbeef");
        let mut data = vec![];
        let request = RequestPair::<NodeData> {
            request_id: 1111,
            message: NodeData(vec![
                hex!("deadc0de").as_slice().into(),
                hex!("feedbeef").as_slice().into(),
            ]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_node_data() {
        let data = hex!("ce820457ca84deadc0de84feedbeef");
        let request = RequestPair::<NodeData>::decode(&mut &data[..]).unwrap();
        assert_eq!(
            request,
            RequestPair::<NodeData> {
                request_id: 1111,
                message: NodeData(vec![
                    hex!("deadc0de").as_slice().into(),
                    hex!("feedbeef").as_slice().into(),
                ])
            }
        );
    }
}
