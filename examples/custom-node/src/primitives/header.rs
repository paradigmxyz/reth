use alloy_consensus::Header;
use alloy_primitives::{
    private::derive_more, Address, BlockNumber, Bloom, Bytes, Sealable, B256, B64, U256,
};
use alloy_rlp::{Encodable, RlpDecodable, RlpEncodable};
use reth_codecs::Compact;
use reth_ethereum::primitives::{BlockHeader, InMemorySize};
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
    pub inner: Header,
    /// The extended header
    pub extension: u64,
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
        self.inner.parent_hash()
    }

    fn ommers_hash(&self) -> B256 {
        self.inner.ommers_hash()
    }

    fn beneficiary(&self) -> Address {
        self.inner.beneficiary()
    }

    fn state_root(&self) -> B256 {
        self.inner.state_root()
    }

    fn transactions_root(&self) -> B256 {
        self.inner.transactions_root()
    }

    fn receipts_root(&self) -> B256 {
        self.inner.receipts_root()
    }

    fn withdrawals_root(&self) -> Option<B256> {
        self.inner.withdrawals_root()
    }

    fn logs_bloom(&self) -> Bloom {
        self.inner.logs_bloom()
    }

    fn difficulty(&self) -> U256 {
        self.inner.difficulty()
    }

    fn number(&self) -> BlockNumber {
        self.inner.number()
    }

    fn gas_limit(&self) -> u64 {
        self.inner.gas_limit()
    }

    fn gas_used(&self) -> u64 {
        self.inner.gas_used()
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn mix_hash(&self) -> Option<B256> {
        self.inner.mix_hash()
    }

    fn nonce(&self) -> Option<B64> {
        self.inner.nonce()
    }

    fn base_fee_per_gas(&self) -> Option<u64> {
        self.inner.base_fee_per_gas()
    }

    fn blob_gas_used(&self) -> Option<u64> {
        self.inner.blob_gas_used()
    }

    fn excess_blob_gas(&self) -> Option<u64> {
        self.inner.excess_blob_gas()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.parent_beacon_block_root()
    }

    fn requests_hash(&self) -> Option<B256> {
        self.inner.requests_hash()
    }

    fn extra_data(&self) -> &Bytes {
        self.inner.extra_data()
    }
}

impl InMemorySize for CustomHeader {
    fn size(&self) -> usize {
        self.inner.size() + self.extension.size()
    }
}

impl reth_codecs::Compact for CustomHeader {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        let identifier = self.inner.to_compact(buf);
        self.extension.to_compact(buf);

        identifier
    }

    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        let (eth_header, buf) = Compact::from_compact(buf, identifier);
        let (extension, buf) = Compact::from_compact(buf, buf.len());
        (Self { inner: eth_header, extension }, buf)
    }
}

impl BlockHeader for CustomHeader {}

mod serde_bincode_compat {
    use alloy_consensus::serde_bincode_compat::Header;
    use reth_ethereum::primitives::serde_bincode_compat::SerdeBincodeCompat;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug)]
    pub struct CustomHeader<'a> {
        inner: Header<'a>,
        extension: u64,
    }

    impl From<CustomHeader<'_>> for super::CustomHeader {
        fn from(value: CustomHeader) -> Self {
            Self { inner: value.inner.into(), extension: value.extension }
        }
    }

    impl SerdeBincodeCompat for super::CustomHeader {
        type BincodeRepr<'a> = CustomHeader<'a>;

        fn as_repr(&self) -> Self::BincodeRepr<'_> {
            CustomHeader { inner: self.inner.as_repr(), extension: self.extension }
        }

        fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
            repr.into()
        }
    }
}
