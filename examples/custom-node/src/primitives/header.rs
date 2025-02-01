use alloy_consensus::Header;
use alloy_primitives::{
    private::derive_more, Address, BlockNumber, Bloom, Bytes, Sealable, B256, B64, U256,
};
use alloy_rlp::{Encodable, RlpDecodable, RlpEncodable};
use reth_codecs::Compact;
use reth_primitives_traits::InMemorySize;
use revm_primitives::keccak256;
use serde::{Deserialize, Serialize};

/// The header type of this node
///
/// This type extends the regular ethereum header with an extension.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    derive_more::AsRef,
    derive_more::Deref,
    Default,
    RlpEncodable,
    RlpDecodable,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct CustomHeader {
    /// The regular eth header
    #[as_ref]
    #[deref]
    #[serde(flatten)]
    pub eth_header: Header,
    /// The extended header
    pub extension: U256,
}

impl CustomHeader {}

impl AsRef<Self> for CustomHeader {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Sealable for CustomHeader {
    fn hash_slow(&self) -> B256 {
        let mut out = Vec::new();
        self.encode(&mut out);
        keccak256(&out)
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

impl InMemorySize for CustomHeader {
    fn size(&self) -> usize {
        self.eth_header.size() + self.extension.size()
    }
}

impl reth_codecs::Compact for CustomHeader {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        let identifier = self.eth_header.to_compact(buf);
        self.extension.to_compact(buf);

        identifier
    }

    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        let (eth_header, buf) = Compact::from_compact(buf, identifier);
        let (extension, buf) = Compact::from_compact(buf, buf.len());
        (Self { eth_header, extension }, buf)
    }
}
