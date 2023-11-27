use crate::{
    Address, Header, SealedHeader, TransactionSigned, TransactionSignedEcRecovered, Withdrawal,
    B256,
};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use reth_codecs::derive_arbitrary;
use serde::{Deserialize, Serialize};
use std::ops::Deref;

pub use reth_rpc_types::{
    BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag, ForkBlock, RpcBlockHash,
};

/// Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, RlpEncodable, RlpDecodable,
)]
#[derive_arbitrary(rlp, 25)]
#[rlp(trailing)]
pub struct Block {
    /// Block header.
    pub header: Header,
    /// Transactions in this block.
    pub body: Vec<TransactionSigned>,
    /// Ommers/uncles header.
    pub ommers: Vec<Header>,
    /// Block withdrawals.
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl Block {
    /// Create SealedBLock that will create all header hashes.
    pub fn seal_slow(self) -> SealedBlock {
        SealedBlock {
            header: self.header.seal_slow(),
            body: self.body,
            ommers: self.ommers,
            withdrawals: self.withdrawals,
        }
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    pub fn seal(self, hash: B256) -> SealedBlock {
        SealedBlock {
            header: self.header.seal(hash),
            body: self.body,
            ommers: self.ommers,
            withdrawals: self.withdrawals,
        }
    }

    /// Transform into a [`BlockWithSenders`].
    ///
    /// # Panics
    ///
    /// If the number of senders does not match the number of transactions in the block
    /// and the signer recovery for one of the transactions fails.
    #[track_caller]
    pub fn with_senders(self, senders: Vec<Address>) -> BlockWithSenders {
        let senders = if self.body.len() == senders.len() {
            senders
        } else {
            TransactionSigned::recover_signers(&self.body, self.body.len())
                .expect("stored block is valid")
        };

        BlockWithSenders { block: self, senders }
    }

    /// Returns whether or not the block contains any blob transactions.
    pub fn has_blob_transactions(&self) -> bool {
        self.body.iter().any(|tx| tx.is_eip4844())
    }

    /// Calculates a heuristic for the in-memory size of the [Block].
    #[inline]
    pub fn size(&self) -> usize {
        self.header.size() +
            // take into account capacity
            self.body.iter().map(TransactionSigned::size).sum::<usize>() + self.body.capacity() * std::mem::size_of::<TransactionSigned>() +
            self.ommers.iter().map(Header::size).sum::<usize>() + self.ommers.capacity() * std::mem::size_of::<Header>() +
            self.withdrawals.as_ref().map(|w| w.iter().map(Withdrawal::size).sum::<usize>() + w.capacity() * std::mem::size_of::<Withdrawal>()).unwrap_or(std::mem::size_of::<Option<Vec<Withdrawal>>>())
    }
}

impl Deref for Block {
    type Target = Header;
    fn deref(&self) -> &Self::Target {
        &self.header
    }
}

/// Sealed block with senders recovered from transactions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockWithSenders {
    /// Block
    pub block: Block,
    /// List of senders that match the transactions in the block
    pub senders: Vec<Address>,
}

impl BlockWithSenders {
    /// New block with senders. Return none if len of tx and senders does not match
    pub fn new(block: Block, senders: Vec<Address>) -> Option<Self> {
        (!block.body.len() != senders.len()).then_some(Self { block, senders })
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    #[inline]
    pub fn seal(self, hash: B256) -> SealedBlockWithSenders {
        let Self { block, senders } = self;
        SealedBlockWithSenders { block: block.seal(hash), senders }
    }

    /// Split Structure to its components
    #[inline]
    pub fn into_components(self) -> (Block, Vec<Address>) {
        (self.block, self.senders)
    }

    /// Returns an iterator over all transactions in the block.
    #[inline]
    pub fn transactions(&self) -> impl Iterator<Item = &TransactionSigned> + '_ {
        self.block.body.iter()
    }

    /// Returns an iterator over all transactions and their sender.
    #[inline]
    pub fn transactions_with_sender(
        &self,
    ) -> impl Iterator<Item = (&Address, &TransactionSigned)> + '_ {
        self.senders.iter().zip(self.block.body.iter())
    }

    /// Consumes the block and returns the transactions of the block.
    #[inline]
    pub fn into_transactions(self) -> Vec<TransactionSigned> {
        self.block.body
    }

    /// Returns an iterator over all transactions in the chain.
    #[inline]
    pub fn into_transactions_ecrecovered(
        self,
    ) -> impl Iterator<Item = TransactionSignedEcRecovered> {
        self.block.body.into_iter().zip(self.senders).map(|(tx, sender)| tx.with_signer(sender))
    }
}

impl Deref for BlockWithSenders {
    type Target = Block;
    fn deref(&self) -> &Self::Target {
        &self.block
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl std::ops::DerefMut for BlockWithSenders {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.block
    }
}

/// Sealed Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[derive_arbitrary(rlp, 10)]
#[derive(
    Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, RlpEncodable, RlpDecodable,
)]
#[rlp(trailing)]
pub struct SealedBlock {
    /// Locked block header.
    pub header: SealedHeader,
    /// Transactions with signatures.
    pub body: Vec<TransactionSigned>,
    /// Ommer/uncle headers
    pub ommers: Vec<Header>,
    /// Block withdrawals.
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl SealedBlock {
    /// Create a new sealed block instance using the sealed header and block body.
    #[inline]
    pub fn new(header: SealedHeader, body: BlockBody) -> Self {
        let BlockBody { transactions, ommers, withdrawals } = body;
        Self { header, body: transactions, ommers, withdrawals }
    }

    /// Header hash.
    #[inline]
    pub fn hash(&self) -> B256 {
        self.header.hash()
    }

    /// Splits the sealed block into underlying components
    #[inline]
    pub fn split(self) -> (SealedHeader, Vec<TransactionSigned>, Vec<Header>) {
        (self.header, self.body, self.ommers)
    }

    /// Splits the [BlockBody] and [SealedHeader] into separate components
    #[inline]
    pub fn split_header_body(self) -> (SealedHeader, BlockBody) {
        (
            self.header,
            BlockBody {
                transactions: self.body,
                ommers: self.ommers,
                withdrawals: self.withdrawals,
            },
        )
    }

    /// Returns only the blob transactions, if any, from the block body.
    pub fn blob_transactions(&self) -> Vec<&TransactionSigned> {
        self.body.iter().filter(|tx| tx.is_eip4844()).collect()
    }

    /// Expensive operation that recovers transaction signer. See [SealedBlockWithSenders].
    pub fn senders(&self) -> Option<Vec<Address>> {
        TransactionSigned::recover_signers(&self.body, self.body.len())
    }

    /// Seal sealed block with recovered transaction senders.
    pub fn seal_with_senders(self) -> Option<SealedBlockWithSenders> {
        self.try_seal_with_senders().ok()
    }

    /// Seal sealed block with recovered transaction senders.
    pub fn try_seal_with_senders(self) -> Result<SealedBlockWithSenders, Self> {
        match self.senders() {
            Some(senders) => Ok(SealedBlockWithSenders { block: self, senders }),
            None => Err(self),
        }
    }

    /// Unseal the block
    pub fn unseal(self) -> Block {
        Block {
            header: self.header.unseal(),
            body: self.body,
            ommers: self.ommers,
            withdrawals: self.withdrawals,
        }
    }

    /// Calculates a heuristic for the in-memory size of the [SealedBlock].
    #[inline]
    pub fn size(&self) -> usize {
        self.header.size() +
            // take into account capacity
            self.body.iter().map(TransactionSigned::size).sum::<usize>() + self.body.capacity() * std::mem::size_of::<TransactionSigned>() +
            self.ommers.iter().map(Header::size).sum::<usize>() + self.ommers.capacity() * std::mem::size_of::<Header>() +
            self.withdrawals.as_ref().map(|w| w.iter().map(Withdrawal::size).sum::<usize>() + w.capacity() * std::mem::size_of::<Withdrawal>()).unwrap_or(std::mem::size_of::<Option<Vec<Withdrawal>>>())
    }

    /// Calculates the total gas used by blob transactions in the sealed block.
    pub fn blob_gas_used(&self) -> u64 {
        self.blob_transactions().iter().filter_map(|tx| tx.blob_gas_used()).sum()
    }
}

impl From<SealedBlock> for Block {
    fn from(block: SealedBlock) -> Self {
        block.unseal()
    }
}

impl Deref for SealedBlock {
    type Target = SealedHeader;
    fn deref(&self) -> &Self::Target {
        &self.header
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl std::ops::DerefMut for SealedBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.header
    }
}

/// Sealed block with senders recovered from transactions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SealedBlockWithSenders {
    /// Sealed block
    pub block: SealedBlock,
    /// List of senders that match transactions from block.
    pub senders: Vec<Address>,
}

impl SealedBlockWithSenders {
    /// New sealed block with sender. Return none if len of tx and senders does not match
    pub fn new(block: SealedBlock, senders: Vec<Address>) -> Option<Self> {
        (!block.body.len() != senders.len()).then_some(Self { block, senders })
    }

    /// Split Structure to its components
    #[inline]
    pub fn into_components(self) -> (SealedBlock, Vec<Address>) {
        (self.block, self.senders)
    }

    /// Returns the unsealed [BlockWithSenders]
    #[inline]
    pub fn unseal(self) -> BlockWithSenders {
        let Self { block, senders } = self;
        BlockWithSenders { block: block.unseal(), senders }
    }

    /// Returns an iterator over all transactions in the block.
    #[inline]
    pub fn transactions(&self) -> impl Iterator<Item = &TransactionSigned> + '_ {
        self.block.body.iter()
    }

    /// Returns an iterator over all transactions and their sender.
    #[inline]
    pub fn transactions_with_sender(
        &self,
    ) -> impl Iterator<Item = (&Address, &TransactionSigned)> + '_ {
        self.senders.iter().zip(self.block.body.iter())
    }

    /// Consumes the block and returns the transactions of the block.
    #[inline]
    pub fn into_transactions(self) -> Vec<TransactionSigned> {
        self.block.body
    }

    /// Returns an iterator over all transactions in the chain.
    #[inline]
    pub fn into_transactions_ecrecovered(
        self,
    ) -> impl Iterator<Item = TransactionSignedEcRecovered> {
        self.block.body.into_iter().zip(self.senders).map(|(tx, sender)| tx.with_signer(sender))
    }
}

impl Deref for SealedBlockWithSenders {
    type Target = SealedBlock;
    fn deref(&self) -> &Self::Target {
        &self.block
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl std::ops::DerefMut for SealedBlockWithSenders {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.block
    }
}

/// A response to `GetBlockBodies`, containing bodies if any bodies were found.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[derive_arbitrary(rlp, 10)]
#[derive(
    Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, RlpEncodable, RlpDecodable,
)]
#[rlp(trailing)]
pub struct BlockBody {
    /// Transactions in the block
    #[cfg_attr(
        any(test, feature = "arbitrary"),
        proptest(
            strategy = "proptest::collection::vec(proptest::arbitrary::any::<TransactionSigned>(), 0..=100)"
        )
    )]
    pub transactions: Vec<TransactionSigned>,
    /// Uncle headers for the given block
    #[cfg_attr(
        any(test, feature = "arbitrary"),
        proptest(
            strategy = "proptest::collection::vec(proptest::arbitrary::any::<Header>(), 0..=2)"
        )
    )]
    pub ommers: Vec<Header>,
    /// Withdrawals in the block.
    #[cfg_attr(
        any(test, feature = "arbitrary"),
        proptest(
            strategy = "proptest::option::of(proptest::collection::vec(proptest::arbitrary::any::<Withdrawal>(), 0..=16))"
        )
    )]
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl BlockBody {
    /// Create a [`Block`] from the body and its header.
    pub fn create_block(&self, header: Header) -> Block {
        Block {
            header,
            body: self.transactions.clone(),
            ommers: self.ommers.clone(),
            withdrawals: self.withdrawals.clone(),
        }
    }

    /// Calculate the transaction root for the block body.
    pub fn calculate_tx_root(&self) -> B256 {
        crate::proofs::calculate_transaction_root(&self.transactions)
    }

    /// Calculate the ommers root for the block body.
    pub fn calculate_ommers_root(&self) -> B256 {
        crate::proofs::calculate_ommers_root(&self.ommers)
    }

    /// Calculate the withdrawals root for the block body, if withdrawals exist. If there are no
    /// withdrawals, this will return `None`.
    pub fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.withdrawals.as_ref().map(|w| crate::proofs::calculate_withdrawals_root(w))
    }

    /// Calculate all roots (transaction, ommers, withdrawals) for the block body.
    pub fn calculate_roots(&self) -> BlockBodyRoots {
        BlockBodyRoots {
            tx_root: self.calculate_tx_root(),
            ommers_hash: self.calculate_ommers_root(),
            withdrawals_root: self.calculate_withdrawals_root(),
        }
    }

    /// Calculates a heuristic for the in-memory size of the [BlockBody].
    #[inline]
    pub fn size(&self) -> usize {
        self.transactions.iter().map(TransactionSigned::size).sum::<usize>() +
            self.transactions.capacity() * std::mem::size_of::<TransactionSigned>() +
            self.ommers.iter().map(Header::size).sum::<usize>() +
            self.ommers.capacity() * std::mem::size_of::<Header>() +
            self.withdrawals
                .as_ref()
                .map(|w| {
                    w.iter().map(Withdrawal::size).sum::<usize>() +
                        w.capacity() * std::mem::size_of::<Withdrawal>()
                })
                .unwrap_or(std::mem::size_of::<Option<Vec<Withdrawal>>>())
    }
}

/// A struct that represents roots associated with a block body. This can be used to correlate
/// block body responses with headers.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, Hash)]
pub struct BlockBodyRoots {
    /// The transaction root for the block body.
    pub tx_root: B256,
    /// The ommers hash for the block body.
    pub ommers_hash: B256,
    /// The withdrawals root for the block body, if withdrawals exist.
    pub withdrawals_root: Option<B256>,
}

#[cfg(test)]
mod test {
    use super::{BlockId, BlockNumberOrTag::*, *};
    use crate::hex_literal::hex;
    use alloy_rlp::{Decodable, Encodable};
    use reth_rpc_types::HexStringMissingPrefixError;
    use std::str::FromStr;

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
        let block = Block::decode(bytes_buf).unwrap();
        let mut encoded_buf = Vec::new();
        block.encode(&mut encoded_buf);
        assert_eq!(bytes[..], encoded_buf);
    }

    #[test]
    fn serde_blocknumber_non_0xprefix() {
        let s = "\"2\"";
        let err = serde_json::from_str::<BlockNumberOrTag>(s).unwrap_err();
        assert_eq!(err.to_string(), HexStringMissingPrefixError::default().to_string());
    }
}
