use super::{BlobFeesPerGas, BlockCommitmentError, ExecutionSummary, GasAmounts};
use alloy_primitives::{Address, Bytes, B256};
use reth_pureth_ssz::RetainedNode;
use ssz::{DecodeError, Encode, SszDecoderBuilder};
use std::fmt;

#[derive(Debug)]
pub enum SummaryCodecError {
    Decode(DecodeError),
    Commitment(BlockCommitmentError),
}

impl fmt::Display for SummaryCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "execution summary decoding failed: {error:?}"),
            Self::Commitment(error) => write!(f, "invalid execution summary: {error:?}"),
        }
    }
}
impl std::error::Error for SummaryCodecError {}
impl From<DecodeError> for SummaryCodecError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}
impl From<BlockCommitmentError> for SummaryCodecError {
    fn from(error: BlockCommitmentError) -> Self {
        Self::Commitment(error)
    }
}

impl ExecutionSummary {
    pub fn to_ssz_bytes(&self) -> Result<Vec<u8>, SummaryCodecError> {
        self.validate()?;
        Ok(self.as_ssz_bytes())
    }

    pub fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, SummaryCodecError> {
        if !(448..=480).contains(&bytes.len()) {
            return Err(
                DecodeError::BytesInvalid("execution summary length is invalid".into()).into()
            );
        }
        let mut builder = SszDecoderBuilder::new(bytes);
        builder.register_type::<B256>()?;
        builder.register_type::<Address>()?;
        for _ in 0..3 {
            builder.register_type::<B256>()?;
        }
        builder.register_type::<u64>()?;
        builder.register_type::<GasAmounts>()?;
        builder.register_type::<GasAmounts>()?;
        builder.register_type::<u64>()?;
        builder.register_type::<Bytes>()?;
        builder.register_type::<B256>()?;
        builder.register_type::<BlobFeesPerGas>()?;
        builder.register_type::<B256>()?;
        builder.register_type::<GasAmounts>()?;
        for _ in 0..3 {
            builder.register_type::<B256>()?;
        }
        builder.register_type::<u64>()?;
        let mut decoder = builder.build()?;
        let summary = Self {
            parent_hash: decoder.decode_next()?,
            miner: decoder.decode_next()?,
            state_root: decoder.decode_next()?,
            transactions_root: decoder.decode_next()?,
            receipts_root: decoder.decode_next()?,
            number: decoder.decode_next()?,
            gas_limits: decoder.decode_next()?,
            gas_used: decoder.decode_next()?,
            timestamp: decoder.decode_next()?,
            extra_data: decoder.decode_next()?,
            mix_hash: decoder.decode_next()?,
            base_fees_per_gas: decoder.decode_next()?,
            withdrawals_root: decoder.decode_next()?,
            excess_gas: decoder.decode_next()?,
            parent_beacon_block_root: decoder.decode_next()?,
            requests_root: decoder.decode_next()?,
            block_access_list_root: decoder.decode_next()?,
            slot_number: decoder.decode_next()?,
        };
        summary.validate()?;
        Ok(summary)
    }
}

#[derive(Debug)]
pub struct ExecutionSummarySnapshot {
    summary: ExecutionSummary,
    serialized: Bytes,
    tree: RetainedNode,
}
impl ExecutionSummarySnapshot {
    pub fn build(summary: ExecutionSummary) -> Result<Self, SummaryCodecError> {
        let serialized = Bytes::from(summary.to_ssz_bytes()?);
        let tree = summary.tree()?;
        Ok(Self { summary, serialized, tree })
    }
    pub fn from_ssz(bytes: &[u8]) -> Result<Self, SummaryCodecError> {
        Self::build(ExecutionSummary::from_ssz_bytes(bytes)?)
    }
    pub const fn summary(&self) -> &ExecutionSummary {
        &self.summary
    }
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }
    pub const fn tree(&self) -> &RetainedNode {
        &self.tree
    }
    pub const fn root(&self) -> B256 {
        self.tree.root()
    }
}
