use alloy_consensus::Header;
use alloy_primitives::{Address, BlockNumber, Bloom, Bytes, Sealable, B256, B64, U256};

use alloy_rlp::{BufMut, Decodable, Encodable};
use reth_codecs::Compact;
use reth_primitives_traits::InMemorySize;
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CustomHeader {
    eth_header: Header,
    extra_data: String,
}

impl CustomHeader {
    pub fn new(eth_header: Header, extra_data: String) -> Self {
        Self { eth_header, extra_data }
    }

    pub fn eth_header(&self) -> &Header {
        &self.eth_header
    }

    pub fn extra_field(&self) -> String {
        self.extra_data.clone()
    }
}

impl Default for CustomHeader {
    fn default() -> Self {
        Self {
            eth_header: Default::default(),
            extra_data: "".to_string(),
        }
    }
}

// impl Deref for CustomHeader {
//     type Target = Header;

//     fn deref(&self) -> &Self::Target {
//         &self.eth_header
//     }
// }

impl AsRef<Self> for CustomHeader {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Sealable for CustomHeader {
    fn hash_slow(&self) -> B256 {
        self.eth_header.hash_slow()
    }
}

impl alloy_consensus::BlockHeader for CustomHeader {
    fn parent_hash(&self) -> B256 {
        self.eth_header.parent_hash()
    }

    fn ommers_hash(&self) -> B256 {
        self.eth_header.ommers_hash()
    }

    fn beneficiary(&self) -> Address {
        self.eth_header.beneficiary()
    }

    fn state_root(&self) -> B256 {
        self.eth_header.state_root()
    }

    fn transactions_root(&self) -> B256 {
        self.eth_header.transactions_root()
    }

    fn receipts_root(&self) -> B256 {
        self.eth_header.receipts_root()
    }

    fn withdrawals_root(&self) -> Option<B256> {
        self.eth_header.withdrawals_root()
    }

    fn logs_bloom(&self) -> Bloom {
        self.eth_header.logs_bloom()
    }

    fn difficulty(&self) -> U256 {
        self.eth_header.difficulty()
    }

    fn number(&self) -> BlockNumber {
        self.eth_header.number()
    }

    fn gas_limit(&self) -> u64 {
        self.eth_header.gas_limit()
    }

    fn gas_used(&self) -> u64 {
        self.eth_header.gas_used()
    }

    fn timestamp(&self) -> u64 {
        self.eth_header.timestamp()
    }

    fn mix_hash(&self) -> Option<B256> {
        self.eth_header.mix_hash()
    }

    fn nonce(&self) -> Option<B64> {
        self.eth_header.nonce()
    }

    fn base_fee_per_gas(&self) -> Option<u64> {
        self.eth_header.base_fee_per_gas()
    }

    fn blob_gas_used(&self) -> Option<u64> {
        self.eth_header.blob_gas_used()
    }

    fn excess_blob_gas(&self) -> Option<u64> {
        self.eth_header.excess_blob_gas()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.eth_header.parent_beacon_block_root()
    }

    fn requests_hash(&self) -> Option<B256> {
        self.eth_header.requests_hash()
    }

    fn extra_data(&self) -> &Bytes {
        self.eth_header.extra_data()
    }
}

impl Encodable for CustomHeader {
    fn encode(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        self.eth_header.encode(out);
        self.extra_data.encode(out);
    }

    fn length(&self) -> usize {
        self.eth_header.length() + self.extra_data.length()
    }
}

impl Decodable for CustomHeader {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self { eth_header: Header::decode(buf)?, extra_data: String::decode(buf)? })
    }
}

impl Compact for CustomHeader {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        let eth_header_len = self.eth_header.to_compact(buf);
        let extra_data_len = self.extra_data.to_compact(buf);
        eth_header_len + extra_data_len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (eth_header, buf) = Header::from_compact(buf, len);
        let (extra_data, buf) = String::from_compact(buf, buf.len());
        (Self { eth_header, extra_data }, buf)
    }
}

impl InMemorySize for CustomHeader {
    fn size(&self) -> usize {
        self.eth_header.size() + self.extra_data.len()
    }
}
