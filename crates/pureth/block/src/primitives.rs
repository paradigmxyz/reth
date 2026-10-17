use crate::{
    ExecutionPayloadSnapshot, ExecutionSummary, ExecutionSummarySnapshot, SummaryCodecError,
};
use alloy_consensus::{BlockHeader as AlloyBlockHeader, Header, InMemorySize};
use alloy_eips::{eip4895::Withdrawals, eip7840::BlobParams};
use alloy_primitives::{Address, Bloom, Bytes, Sealable, B256, B64, U256};
use alloy_rlp::{bytes::BufMut, Decodable, Encodable};
use pureth_withdrawal_root::{WithdrawalSnapshot, WithdrawalSnapshotError};
use reth_codecs::{Compact, Compress, Decompress, DecompressError};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_primitives_traits::{Block, BlockBody, BlockHeader, NodePrimitives};
use reth_pureth_transaction::{convert_transactions, TransactionError, TransactionSnapshot};
use serde::{Deserialize, Serialize};

type EthereumBody = alloy_consensus::BlockBody<TransactionSigned, ExecutionHeader>;
type EthereumBlock = alloy_consensus::Block<TransactionSigned, ExecutionHeader>;
const SSZ_HEADER_PREFIX: [u8; 4] = [0xff; 4];
const EMPTY_SSZ_LIST_ROOT: B256 =
    alloy_primitives::b256!("f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b");

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "HeaderEncoding", into = "HeaderEncoding")]
pub struct ExecutionHeader {
    native: Header,
    summary: Option<Bytes>,
    hash: B256,
    blob_fee: Option<u128>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum HeaderEncoding {
    Legacy(Box<Header>),
    Ssz(Bytes),
}

impl ExecutionHeader {
    pub fn legacy(header: Header) -> Self {
        let hash = header.hash_slow();
        Self { native: header, summary: None, hash, blob_fee: None }
    }

    pub fn from_summary(summary: ExecutionSummary) -> Result<Self, PrimitiveError> {
        Self::from_summary_snapshot(&ExecutionSummarySnapshot::build(summary)?)
    }

    fn from_summary_snapshot(snapshot: &ExecutionSummarySnapshot) -> Result<Self, PrimitiveError> {
        let summary = snapshot.summary();
        let base_fee = u64::try_from(summary.base_fees_per_gas.regular)
            .map_err(|_| PrimitiveError::UnsupportedFeeRange)?;
        let blob_fee = u128::try_from(summary.base_fees_per_gas.blob)
            .map_err(|_| PrimitiveError::UnsupportedFeeRange)?;
        let native = Header {
            parent_hash: summary.parent_hash,
            ommers_hash: alloy_primitives::keccak256([0xc0]),
            beneficiary: summary.miner,
            state_root: summary.state_root,
            transactions_root: summary.transactions_root,
            receipts_root: summary.receipts_root,
            logs_bloom: Bloom::repeat_byte(0xff),
            number: summary.number,
            gas_limit: summary.gas_limits.regular,
            gas_used: summary.gas_used.regular,
            timestamp: summary.timestamp,
            extra_data: summary.extra_data.clone(),
            mix_hash: summary.mix_hash,
            base_fee_per_gas: Some(base_fee),
            withdrawals_root: Some(summary.withdrawals_root),
            blob_gas_used: Some(summary.gas_used.blob),
            excess_blob_gas: Some(summary.excess_gas.blob),
            parent_beacon_block_root: Some(summary.parent_beacon_block_root),
            requests_hash: Some(summary.requests_root),
            block_access_list_hash: Some(summary.block_access_list_root),
            slot_number: Some(summary.slot_number),
            ..Default::default()
        };
        Ok(Self {
            native,
            summary: Some(Bytes::copy_from_slice(snapshot.serialized())),
            hash: snapshot.root(),
            blob_fee: Some(blob_fee),
        })
    }

    pub fn from_payload(payload: &ExecutionPayloadSnapshot) -> Result<Self, PrimitiveError> {
        Self::from_summary_snapshot(payload.summary())
    }

    pub const fn is_ssz(&self) -> bool {
        self.summary.is_some()
    }

    pub fn summary_ssz(&self) -> Option<&[u8]> {
        self.summary.as_ref().map(|bytes| bytes.as_ref())
    }
}

impl Default for ExecutionHeader {
    fn default() -> Self {
        Self::legacy(Header::default())
    }
}

impl From<ExecutionHeader> for HeaderEncoding {
    fn from(header: ExecutionHeader) -> Self {
        match header.summary {
            Some(bytes) => Self::Ssz(bytes),
            None => Self::Legacy(Box::new(header.native)),
        }
    }
}

impl TryFrom<HeaderEncoding> for ExecutionHeader {
    type Error = PrimitiveError;

    fn try_from(header: HeaderEncoding) -> Result<Self, Self::Error> {
        match header {
            HeaderEncoding::Legacy(header) => Ok(Self::legacy(*header)),
            HeaderEncoding::Ssz(bytes) => {
                Self::from_summary(ExecutionSummary::from_ssz_bytes(&bytes)?)
            }
        }
    }
}

impl AsRef<Self> for ExecutionHeader {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Sealable for ExecutionHeader {
    fn hash_slow(&self) -> B256 {
        self.hash
    }
}

impl InMemorySize for ExecutionHeader {
    fn size(&self) -> usize {
        std::mem::size_of::<Self>() +
            self.native.extra_data.len() +
            self.summary.as_ref().map_or(0, |bytes| bytes.len())
    }
}

macro_rules! header_fields {
    ($($name:ident: $ty:ty),* $(,)?) => {
        $(fn $name(&self) -> $ty { self.native.$name() })*
    };
}

impl AlloyBlockHeader for ExecutionHeader {
    header_fields! {
        parent_hash: B256,
        ommers_hash: B256,
        beneficiary: Address,
        state_root: B256,
        transactions_root: B256,
        receipts_root: B256,
        withdrawals_root: Option<B256>,
        logs_bloom: Bloom,
        difficulty: U256,
        number: u64,
        gas_limit: u64,
        gas_used: u64,
        timestamp: u64,
        mix_hash: Option<B256>,
        nonce: Option<B64>,
        base_fee_per_gas: Option<u64>,
        blob_gas_used: Option<u64>,
        excess_blob_gas: Option<u64>,
        parent_beacon_block_root: Option<B256>,
        requests_hash: Option<B256>,
        block_access_list_hash: Option<B256>,
        slot_number: Option<u64>,
    }

    fn extra_data(&self) -> &Bytes {
        &self.native.extra_data
    }

    fn blob_fee(&self, blob_params: BlobParams) -> Option<u128> {
        self.blob_fee.or_else(|| self.native.blob_fee(blob_params))
    }

    fn is_empty(&self) -> bool {
        if self.is_ssz() {
            self.native.transactions_root == EMPTY_SSZ_LIST_ROOT &&
                self.native.withdrawals_root == Some(EMPTY_SSZ_LIST_ROOT) &&
                self.native.ommers_hash == alloy_consensus::EMPTY_OMMER_ROOT_HASH
        } else {
            self.native.is_empty()
        }
    }
}

impl BlockHeader for ExecutionHeader {}

impl Encodable for ExecutionHeader {
    fn encode(&self, out: &mut dyn BufMut) {
        match &self.summary {
            Some(bytes) => bytes.encode(out),
            None => self.native.encode(out),
        }
    }

    fn length(&self) -> usize {
        self.summary.as_ref().map_or_else(|| self.native.length(), Encodable::length)
    }
}

impl Decodable for ExecutionHeader {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let mut prefix = *buf;
        if alloy_rlp::Header::decode(&mut prefix)?.list {
            return Ok(Self::legacy(Header::decode(buf)?));
        }
        let bytes = Bytes::decode(buf)?;
        Self::from_summary(
            ExecutionSummary::from_ssz_bytes(&bytes)
                .map_err(|_| alloy_rlp::Error::Custom("invalid execution summary"))?,
        )
        .map_err(|_| alloy_rlp::Error::Custom("unsupported execution header"))
    }
}

impl Compact for ExecutionHeader {
    fn to_compact<B: BufMut + AsMut<[u8]>>(&self, buf: &mut B) -> usize {
        match &self.summary {
            Some(bytes) => {
                buf.put_slice(&SSZ_HEADER_PREFIX);
                buf.put_slice(bytes);
                SSZ_HEADER_PREFIX.len() + bytes.len()
            }
            None => self.native.to_compact(buf),
        }
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (encoded, rest) = buf.split_at(len);
        let header = Self::decompress(encoded).expect("valid stored execution header");
        (header, rest)
    }
}

impl Compress for ExecutionHeader {
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        self.to_compact(buf);
    }
}

impl Decompress for ExecutionHeader {
    fn decompress(value: &[u8]) -> Result<Self, DecompressError> {
        if let Some(bytes) = value.strip_prefix(&SSZ_HEADER_PREFIX) {
            let summary = ExecutionSummary::from_ssz_bytes(bytes).map_err(DecompressError::new)?;
            Self::from_summary(summary).map_err(DecompressError::new)
        } else if value.len() < SSZ_HEADER_PREFIX.len() {
            Err(DecompressError::new(PrimitiveError::InvalidStorage))
        } else {
            Header::decompress(value).map(Self::legacy)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BodyEncoding", into = "BodyEncoding")]
pub struct ExecutionBody {
    native: EthereumBody,
    roots: Option<(B256, B256)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BodyEncoding {
    native: EthereumBody,
    ssz: bool,
}

impl ExecutionBody {
    pub fn from_header(
        header: &ExecutionHeader,
        native: EthereumBody,
    ) -> Result<Self, PrimitiveError> {
        Self::try_from(BodyEncoding { native, ssz: header.is_ssz() })
    }

    pub fn from_payload(
        payload: &ExecutionPayloadSnapshot,
        native: EthereumBody,
    ) -> Result<Self, PrimitiveError> {
        let components = payload.components();
        if !native.ommers.is_empty() ||
            native.withdrawals.as_ref().map(|values| values.as_slice()) !=
                Some(components.withdrawals.withdrawals()) ||
            convert_transactions(&native.transactions)? != components.transactions.transactions()
        {
            return Err(PrimitiveError::InvalidBody);
        }
        Ok(Self {
            native,
            roots: Some((components.transactions.root(), components.withdrawals.root())),
        })
    }

    pub const fn is_ssz(&self) -> bool {
        self.roots.is_some()
    }
}

impl From<ExecutionBody> for BodyEncoding {
    fn from(body: ExecutionBody) -> Self {
        Self { ssz: body.is_ssz(), native: body.native }
    }
}

impl TryFrom<BodyEncoding> for ExecutionBody {
    type Error = PrimitiveError;

    fn try_from(body: BodyEncoding) -> Result<Self, Self::Error> {
        if body.native.ommers.iter().any(ExecutionHeader::is_ssz) ||
            (body.ssz && !body.native.ommers.is_empty())
        {
            return Err(PrimitiveError::InvalidBody);
        }
        let roots = if body.ssz {
            let withdrawals =
                body.native.withdrawals.as_ref().ok_or(PrimitiveError::InvalidBody)?;
            Some((
                TransactionSnapshot::build(convert_transactions(&body.native.transactions)?)?
                    .root(),
                WithdrawalSnapshot::build(withdrawals.to_vec())?.root(),
            ))
        } else {
            None
        };
        Ok(Self { native: body.native, roots })
    }
}

impl InMemorySize for ExecutionBody {
    fn size(&self) -> usize {
        std::mem::size_of::<Self>() - std::mem::size_of::<EthereumBody>() + self.native.size()
    }
}

impl BlockBody for ExecutionBody {
    type Transaction = TransactionSigned;
    type OmmerHeader = ExecutionHeader;

    fn transactions(&self) -> &[Self::Transaction] {
        &self.native.transactions
    }

    fn into_ethereum_body(self) -> EthereumBody {
        self.native
    }

    fn into_transactions(self) -> Vec<Self::Transaction> {
        self.native.transactions
    }

    fn withdrawals(&self) -> Option<&Withdrawals> {
        self.native.withdrawals.as_ref()
    }

    fn ommers(&self) -> Option<&[ExecutionHeader]> {
        if self.is_ssz() {
            None
        } else {
            Some(&self.native.ommers)
        }
    }

    fn calculate_tx_root(&self) -> B256 {
        self.roots.map_or_else(|| self.native.calculate_tx_root(), |roots| roots.0)
    }

    fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.roots.map(|roots| roots.1).or_else(|| self.native.calculate_withdrawals_root())
    }
}

impl Encodable for ExecutionBody {
    fn encode(&self, out: &mut dyn BufMut) {
        if self.is_ssz() {
            alloy_rlp::encode(&self.native).as_slice().encode(out);
        } else {
            self.native.encode(out);
        }
    }

    fn length(&self) -> usize {
        if self.is_ssz() {
            let len = self.native.length();
            alloy_rlp::length_of_length(len) + len
        } else {
            self.native.length()
        }
    }
}

impl Decodable for ExecutionBody {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let mut prefix = *buf;
        let ssz = !alloy_rlp::Header::decode(&mut prefix)?.list;
        let native = if ssz {
            let bytes = Bytes::decode(buf)?;
            let mut encoded = bytes.as_ref();
            let native = EthereumBody::decode(&mut encoded)?;
            if !encoded.is_empty() {
                return Err(alloy_rlp::Error::Custom("trailing execution body bytes"));
            }
            native
        } else {
            EthereumBody::decode(buf)?
        };
        Self::try_from(BodyEncoding { native, ssz })
            .map_err(|_| alloy_rlp::Error::Custom("invalid execution body"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EthereumBlock", into = "EthereumBlock")]
pub struct ExecutionBlock {
    header: ExecutionHeader,
    body: ExecutionBody,
}

impl ExecutionBlock {
    pub fn try_new(header: ExecutionHeader, body: ExecutionBody) -> Result<Self, PrimitiveError> {
        if header.is_ssz() != body.is_ssz() {
            return Err(PrimitiveError::InvalidBody);
        }
        if (header.transactions_root(), header.withdrawals_root()) !=
            (body.calculate_tx_root(), body.calculate_withdrawals_root())
        {
            return Err(PrimitiveError::InvalidBody);
        }
        Ok(Self { header, body })
    }
}

impl From<ExecutionBlock> for EthereumBlock {
    fn from(block: ExecutionBlock) -> Self {
        block.into_ethereum_block()
    }
}

impl TryFrom<EthereumBlock> for ExecutionBlock {
    type Error = PrimitiveError;

    fn try_from(block: EthereumBlock) -> Result<Self, Self::Error> {
        let body = ExecutionBody::from_header(&block.header, block.body)?;
        Self::try_new(block.header, body)
    }
}

impl InMemorySize for ExecutionBlock {
    fn size(&self) -> usize {
        self.header.size() + self.body.size()
    }
}

impl Block for ExecutionBlock {
    type Header = ExecutionHeader;
    type Body = ExecutionBody;

    fn new(header: ExecutionHeader, body: ExecutionBody) -> Self {
        Self { header, body }
    }

    fn header(&self) -> &ExecutionHeader {
        &self.header
    }

    fn body(&self) -> &ExecutionBody {
        &self.body
    }

    fn split(self) -> (ExecutionHeader, ExecutionBody) {
        (self.header, self.body)
    }

    fn rlp_length(header: &ExecutionHeader, body: &ExecutionBody) -> usize {
        EthereumBlock::rlp_length_for(header, &body.native)
    }

    fn into_ethereum_block(self) -> EthereumBlock {
        EthereumBlock { header: self.header, body: self.body.native }
    }
}

impl Encodable for ExecutionBlock {
    fn encode(&self, out: &mut dyn BufMut) {
        EthereumBlock::rlp_encode_from_parts(&self.header, &self.body.native, out);
    }

    fn length(&self) -> usize {
        Self::rlp_length(&self.header, &self.body)
    }
}

impl Decodable for ExecutionBlock {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let native = EthereumBlock::decode(buf)?;
        let body = ExecutionBody::from_header(&native.header, native.body)
            .map_err(|_| alloy_rlp::Error::Custom("invalid execution block body"))?;
        Self::try_new(native.header, body)
            .map_err(|_| alloy_rlp::Error::Custom("execution block body root mismatch"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecutionPrimitives;

impl NodePrimitives for ExecutionPrimitives {
    type Block = ExecutionBlock;
    type BlockHeader = ExecutionHeader;
    type BlockBody = ExecutionBody;
    type SignedTx = TransactionSigned;
    type Receipt = Receipt;
}

#[derive(Debug, thiserror::Error)]
pub enum PrimitiveError {
    #[error("invalid stored execution header")]
    InvalidStorage,
    #[error("invalid execution body or component roots")]
    InvalidBody,
    #[error("fee exceeds the native execution field width")]
    UnsupportedFeeRange,
    #[error(transparent)]
    Summary(#[from] SummaryCodecError),
    #[error(transparent)]
    Transactions(#[from] TransactionError),
    #[error(transparent)]
    Withdrawals(#[from] WithdrawalSnapshotError),
}

#[cfg(test)]
mod tests;
