use reth_ethereum_primitives::TransactionSigned;
#[cfg(any(test, feature = "arbitrary"))]
pub use reth_primitives_traits::test_utils::{generate_valid_header, valid_header_strategy};

/// Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
pub type Block<T = TransactionSigned> = alloy_consensus::Block<T>;

/// A response to `GetBlockBodies`, containing bodies if any bodies were found.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
pub type BlockBody<T = TransactionSigned> = alloy_consensus::BlockBody<T>;

/// Ethereum sealed block type
pub type SealedBlock<B = Block> = reth_primitives_traits::block::SealedBlock<B>;

/// Helper type for constructing the block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type SealedBlockFor<B = Block> = reth_primitives_traits::block::SealedBlock<B>;

/// Ethereum recovered block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type BlockWithSenders<B = Block> = reth_primitives_traits::block::RecoveredBlock<B>;

/// Ethereum recovered block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type SealedBlockWithSenders<B = Block> = reth_primitives_traits::block::RecoveredBlock<B>;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::{
        eip1898::HexStringMissingPrefixError, BlockId, BlockNumberOrTag, BlockNumberOrTag::*,
        RpcBlockHash,
    };
    use alloy_primitives::{hex_literal::hex, B256};
    use alloy_rlp::{Decodable, Encodable};
    use reth_primitives_traits::{BlockBody, RecoveredBlock};
    use std::str::FromStr;

    const fn _traits() {
        const fn assert_block<T: reth_primitives_traits::Block>() {}
        assert_block::<Block>();
    }

    /// Check parsing according to EIP-1898.
    #[test]
    fn can_parse_blockid_u64() {
        let num = serde_json::json!(
            {"blockNumber": "0xaf"}
        );

        let id = serde_json::from_value::<BlockId>(num);
        assert_eq!(id.unwrap(), BlockId::from(175));
    }
    #[test]
    fn can_parse_block_hash() {
        let block_hash =
            B256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        let block_hash_json = serde_json::json!(
            { "blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}
        );
        let id = serde_json::from_value::<BlockId>(block_hash_json).unwrap();
        assert_eq!(id, BlockId::from(block_hash,));
    }
    #[test]
    fn can_parse_block_hash_with_canonical() {
        let block_hash =
            B256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        let block_id = BlockId::Hash(RpcBlockHash::from_hash(block_hash, Some(true)));
        let block_hash_json = serde_json::json!(
            { "blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3", "requireCanonical": true }
        );
        let id = serde_json::from_value::<BlockId>(block_hash_json).unwrap();
        assert_eq!(id, block_id)
    }
    #[test]
    fn can_parse_blockid_tags() {
        let tags =
            [("latest", Latest), ("finalized", Finalized), ("safe", Safe), ("pending", Pending)];
        for (value, tag) in tags {
            let num = serde_json::json!({ "blockNumber": value });
            let id = serde_json::from_value::<BlockId>(num);
            assert_eq!(id.unwrap(), BlockId::from(tag))
        }
    }
    #[test]
    fn repeated_keys_is_err() {
        let num = serde_json::json!({"blockNumber": 1, "requireCanonical": true, "requireCanonical": false});
        assert!(serde_json::from_value::<BlockId>(num).is_err());
        let num =
            serde_json::json!({"blockNumber": 1, "requireCanonical": true, "blockNumber": 23});
        assert!(serde_json::from_value::<BlockId>(num).is_err());
    }
    /// Serde tests
    #[test]
    fn serde_blockid_tags() {
        let block_ids = [Latest, Finalized, Safe, Pending].map(BlockId::from);
        for block_id in &block_ids {
            let serialized = serde_json::to_string(&block_id).unwrap();
            let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, *block_id)
        }
    }

    #[test]
    fn serde_blockid_number() {
        let block_id = BlockId::from(100u64);
        let serialized = serde_json::to_string(&block_id).unwrap();
        let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, block_id)
    }

    #[test]
    fn serde_blockid_hash() {
        let block_id = BlockId::from(B256::default());
        let serialized = serde_json::to_string(&block_id).unwrap();
        let deserialized: BlockId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, block_id)
    }

    #[test]
    fn serde_blockid_hash_from_str() {
        let val = "\"0x898753d8fdd8d92c1907ca21e68c7970abd290c647a202091181deec3f30a0b2\"";
        let block_hash: B256 = serde_json::from_str(val).unwrap();
        let block_id: BlockId = serde_json::from_str(val).unwrap();
        assert_eq!(block_id, BlockId::Hash(block_hash.into()));
    }

    #[test]
    fn serde_rpc_payload_block_tag() {
        let payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},"latest"],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap();
        let block_id: BlockId = serde_json::from_value::<BlockId>(block_id_param.clone()).unwrap();
        assert_eq!(BlockId::Number(BlockNumberOrTag::Latest), block_id);
    }
    #[test]
    fn serde_rpc_payload_block_object() {
        let example_payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},{"blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(example_payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap().to_string();
        let block_id: BlockId = serde_json::from_str::<BlockId>(&block_id_param).unwrap();
        let hash =
            B256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap();
        assert_eq!(BlockId::from(hash), block_id);
        let serialized = serde_json::to_string(&BlockId::from(hash)).unwrap();
        assert_eq!("{\"blockHash\":\"0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3\"}", serialized)
    }
    #[test]
    fn serde_rpc_payload_block_number() {
        let example_payload = r#"{"method":"eth_call","params":[{"to":"0xebe8efa441b9302a0d7eaecc277c09d20d684540","data":"0x45848dfc"},{"blockNumber": "0x0"}],"id":1,"jsonrpc":"2.0"}"#;
        let value: serde_json::Value = serde_json::from_str(example_payload).unwrap();
        let block_id_param = value.pointer("/params/1").unwrap().to_string();
        let block_id: BlockId = serde_json::from_str::<BlockId>(&block_id_param).unwrap();
        assert_eq!(BlockId::from(0u64), block_id);
        let serialized = serde_json::to_string(&BlockId::from(0u64)).unwrap();
        assert_eq!("\"0x0\"", serialized)
    }
    #[test]
    #[should_panic]
    fn serde_rpc_payload_block_number_duplicate_key() {
        let payload = r#"{"blockNumber": "0x132", "blockNumber": "0x133"}"#;
        let parsed_block_id = serde_json::from_str::<BlockId>(payload);
        parsed_block_id.unwrap();
    }
    #[test]
    fn serde_rpc_payload_block_hash() {
        let payload = r#"{"blockHash": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"}"#;
        let parsed = serde_json::from_str::<BlockId>(payload).unwrap();
        let expected = BlockId::from(
            B256::from_str("0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
                .unwrap(),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn encode_decode_raw_block() {
        let bytes = hex!("f90288f90218a0fe21bb173f43067a9f90cfc59bbb6830a7a2929b5de4a61f372a9db28e87f9aea01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347940000000000000000000000000000000000000000a061effbbcca94f0d3e02e5bd22e986ad57142acabf0cb3d129a6ad8d0f8752e94a0d911c25e97e27898680d242b7780b6faef30995c355a2d5de92e6b9a7212ad3aa0056b23fbba480696b65fe5a59b8f2148a1299103c4f57df839233af2cf4ca2d2b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008003834c4b408252081e80a00000000000000000000000000000000000000000000000000000000000000000880000000000000000842806be9da056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421f869f86702842806be9e82520894658bdf435d810c91414ec09147daa6db624063798203e880820a95a040ce7918eeb045ebf8c8b1887ca139d076bda00fa828a07881d442a72626c42da0156576a68e456e295e4c9cf67cf9f53151f329438916e0f24fc69d6bbb7fbacfc0c0");
        let bytes_buf = &mut bytes.as_ref();
        let block: Block = Block::decode(bytes_buf).unwrap();
        let mut encoded_buf = Vec::with_capacity(bytes.len());
        block.encode(&mut encoded_buf);
        assert_eq!(bytes[..], encoded_buf);
    }

    #[test]
    fn serde_blocknumber_non_0xprefix() {
        let s = "\"2\"";
        let err = serde_json::from_str::<BlockNumberOrTag>(s).unwrap_err();
        assert_eq!(err.to_string(), HexStringMissingPrefixError::default().to_string());
    }

    #[test]
    fn block_with_senders() {
        let mut block: Block = Block::default();
        block.body.transactions.push(TransactionSigned::default());
        let block = RecoveredBlock::try_new_unhashed(block.clone(), vec![]).unwrap();
        assert_eq!(block.senders().len(), 1);
    }

    #[test]
    fn empty_block_rlp() {
        let body = alloy_consensus::BlockBody::<TransactionSigned>::default();
        let mut buf = Vec::new();
        body.encode(&mut buf);
        let decoded = alloy_consensus::BlockBody::decode(&mut buf.as_slice()).unwrap();
        assert_eq!(body, decoded);
    }

    #[test]
    fn test_transaction_count() {
        let mut block = Block::default();
        assert_eq!(block.body.transaction_count(), 0);
        block.body.transactions.push(TransactionSigned::default());
        assert_eq!(block.body.transaction_count(), 1);
        block.body.transactions.push(TransactionSigned::default());
        assert_eq!(block.body.transaction_count(), 2);
    }
}
