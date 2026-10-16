use crate::{
    BlobFeesPerGas, ExecutionRequestsSnapshot, ExecutionSummary, ExecutionSummarySnapshot,
    GasAmounts, SummaryCodecError,
};
use alloy_eip7928::BlockAccessList;
use alloy_primitives::{keccak256, Address, Bytes, B256};
use alloy_rlp::Decodable;
use pureth_withdrawal_root::{WithdrawalSnapshot, WithdrawalSnapshotError};
use reth_pureth_receipt::eip6466::Eip6466ReceiptSnapshot;
use reth_pureth_ssz::{progressive_byte_list, RetainedNode, TreeConstructionError};
use reth_pureth_transaction::{TransactionError, TransactionSnapshot};
use ssz::{DecodeError, Encode, SszDecoderBuilder};
use std::fmt;

#[derive(Clone, Copy, Debug)]
pub struct PayloadLimits {
    pub bytes: usize,
    pub transactions: usize,
    pub withdrawals: usize,
    pub bal_bytes: usize,
}

impl Default for PayloadLimits {
    fn default() -> Self {
        Self {
            bytes: 64 * 1024 * 1024,
            transactions: 4096,
            withdrawals: 4096,
            bal_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum PayloadError {
    LimitExceeded(&'static str),
    InvalidEncoding(&'static str),
    ComponentMismatch(&'static str),
    Decode(DecodeError),
    Transaction(TransactionError),
    Withdrawal(WithdrawalSnapshotError),
    Summary(SummaryCodecError),
    Bal(alloy_rlp::Error),
    Tree(TreeConstructionError),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded(name) => write!(f, "payload exceeds {name} limit"),
            Self::InvalidEncoding(reason) => write!(f, "invalid payload encoding: {reason}"),
            Self::ComponentMismatch(name) => write!(f, "payload component mismatch: {name}"),
            Self::Decode(error) => write!(f, "payload decoding failed: {error:?}"),
            Self::Transaction(error) => write!(f, "payload transaction failed: {error}"),
            Self::Withdrawal(error) => write!(f, "payload withdrawal failed: {error}"),
            Self::Summary(error) => write!(f, "payload summary failed: {error}"),
            Self::Bal(error) => write!(f, "payload BAL decoding failed: {error}"),
            Self::Tree(error) => write!(f, "payload tree failed: {error}"),
        }
    }
}

impl std::error::Error for PayloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transaction(error) => Some(error),
            Self::Withdrawal(error) => Some(error),
            Self::Summary(error) => Some(error),
            Self::Bal(error) => Some(error),
            Self::Tree(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct BlockAccessListSnapshot {
    serialized: Bytes,
    tree: RetainedNode,
}

impl BlockAccessListSnapshot {
    pub fn from_rlp(bytes: Bytes, max_bytes: usize) -> Result<Self, PayloadError> {
        if bytes.len() > max_bytes {
            return Err(PayloadError::LimitExceeded("BAL bytes"));
        }
        let mut remaining = bytes.as_ref();
        let bal = BlockAccessList::decode(&mut remaining).map_err(PayloadError::Bal)?;
        if !remaining.is_empty() || alloy_rlp::encode(&bal).as_slice() != bytes.as_ref() {
            return Err(PayloadError::InvalidEncoding("noncanonical BAL"));
        }
        let tree = progressive_byte_list(&bytes).map_err(PayloadError::Tree)?;
        Ok(Self { serialized: bytes, tree })
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
    pub fn native_hash(&self) -> B256 {
        keccak256(&self.serialized)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionPayloadFields {
    pub parent_hash: B256,
    pub miner: Address,
    pub state_root: B256,
    pub number: u64,
    pub gas_limits: GasAmounts,
    pub gas_used: GasAmounts,
    pub timestamp: u64,
    pub extra_data: Bytes,
    pub mix_hash: B256,
    pub base_fees_per_gas: BlobFeesPerGas,
    pub excess_gas: GasAmounts,
    pub parent_beacon_block_root: B256,
    pub slot_number: u64,
}

#[derive(Debug)]
pub struct PayloadComponents {
    pub transactions: TransactionSnapshot,
    pub receipts: Eip6466ReceiptSnapshot,
    pub withdrawals: WithdrawalSnapshot,
    pub requests: ExecutionRequestsSnapshot,
    pub block_access_list: BlockAccessListSnapshot,
}

#[derive(Debug)]
pub struct ExecutionPayloadSnapshot {
    fields: ExecutionPayloadFields,
    components: PayloadComponents,
    summary: ExecutionSummarySnapshot,
    serialized: Bytes,
}

impl ExecutionPayloadSnapshot {
    pub fn build(
        fields: ExecutionPayloadFields,
        components: PayloadComponents,
        limits: PayloadLimits,
    ) -> Result<Self, PayloadError> {
        if components.transactions.transactions().len() > limits.transactions {
            return Err(PayloadError::LimitExceeded("transactions"));
        }
        if components.withdrawals.withdrawals().len() > limits.withdrawals {
            return Err(PayloadError::LimitExceeded("withdrawals"));
        }
        if components.transactions.transactions().len() != components.receipts.receipts().len() {
            return Err(PayloadError::ComponentMismatch("transaction/receipt count"));
        }
        if components.block_access_list.serialized().len() > limits.bal_bytes {
            return Err(PayloadError::LimitExceeded("BAL bytes"));
        }
        let length = [
            364,
            components.transactions.serialized().len(),
            fields.extra_data.len(),
            components.withdrawals.serialized().len(),
            components.block_access_list.serialized().len(),
        ]
        .into_iter()
        .try_fold(0usize, |total, bytes| total.checked_add(bytes))
        .ok_or(PayloadError::LimitExceeded("payload bytes"))?;
        let source_length = length
            .checked_add(components.receipts.serialized().len())
            .and_then(|bytes| bytes.checked_add(components.requests.serialized().len()))
            .ok_or(PayloadError::LimitExceeded("component bytes"))?;
        if source_length > limits.bytes || length > u32::MAX as usize {
            return Err(PayloadError::LimitExceeded("component bytes"));
        }
        let summary = ExecutionSummarySnapshot::build(ExecutionSummary {
            parent_hash: fields.parent_hash,
            miner: fields.miner,
            state_root: fields.state_root,
            transactions_root: components.transactions.root(),
            receipts_root: components.receipts.root(),
            number: fields.number,
            gas_limits: fields.gas_limits,
            gas_used: fields.gas_used,
            timestamp: fields.timestamp,
            extra_data: fields.extra_data.clone(),
            mix_hash: fields.mix_hash,
            base_fees_per_gas: fields.base_fees_per_gas,
            withdrawals_root: components.withdrawals.root(),
            excess_gas: fields.excess_gas,
            parent_beacon_block_root: fields.parent_beacon_block_root,
            requests_root: components.requests.root(),
            block_access_list_root: components.block_access_list.root(),
            slot_number: fields.slot_number,
        })
        .map_err(PayloadError::Summary)?;
        let mut bytes = Vec::with_capacity(length);
        let tx_offset = 364usize;
        let extra_offset = tx_offset + components.transactions.serialized().len();
        let withdrawal_offset = extra_offset + fields.extra_data.len();
        let bal_offset = withdrawal_offset + components.withdrawals.serialized().len();
        fields.parent_hash.ssz_append(&mut bytes);
        fields.miner.ssz_append(&mut bytes);
        fields.state_root.ssz_append(&mut bytes);
        (tx_offset as u32).ssz_append(&mut bytes);
        components.receipts.root().ssz_append(&mut bytes);
        fields.number.ssz_append(&mut bytes);
        fields.gas_limits.ssz_append(&mut bytes);
        fields.gas_used.ssz_append(&mut bytes);
        fields.timestamp.ssz_append(&mut bytes);
        (extra_offset as u32).ssz_append(&mut bytes);
        fields.mix_hash.ssz_append(&mut bytes);
        fields.base_fees_per_gas.ssz_append(&mut bytes);
        (withdrawal_offset as u32).ssz_append(&mut bytes);
        fields.excess_gas.ssz_append(&mut bytes);
        fields.parent_beacon_block_root.ssz_append(&mut bytes);
        components.requests.root().ssz_append(&mut bytes);
        (bal_offset as u32).ssz_append(&mut bytes);
        fields.slot_number.ssz_append(&mut bytes);
        bytes.extend_from_slice(components.transactions.serialized());
        bytes.extend_from_slice(&fields.extra_data);
        bytes.extend_from_slice(components.withdrawals.serialized());
        bytes.extend_from_slice(components.block_access_list.serialized());
        Ok(Self { fields, components, summary, serialized: Bytes::from(bytes) })
    }

    pub fn from_ssz(
        bytes: &[u8],
        receipts: Eip6466ReceiptSnapshot,
        requests: ExecutionRequestsSnapshot,
        limits: PayloadLimits,
    ) -> Result<Self, PayloadError> {
        if bytes
            .len()
            .checked_add(receipts.serialized().len())
            .and_then(|bytes| bytes.checked_add(requests.serialized().len()))
            .is_none_or(|bytes| bytes > limits.bytes)
        {
            return Err(PayloadError::LimitExceeded("payload bytes"));
        }
        let mut builder = SszDecoderBuilder::new(bytes);
        builder.register_type::<B256>().map_err(PayloadError::Decode)?;
        builder.register_type::<Address>().map_err(PayloadError::Decode)?;
        builder.register_type::<B256>().map_err(PayloadError::Decode)?;
        builder.register_type::<Bytes>().map_err(PayloadError::Decode)?;
        builder.register_type::<B256>().map_err(PayloadError::Decode)?;
        builder.register_type::<u64>().map_err(PayloadError::Decode)?;
        builder.register_type::<GasAmounts>().map_err(PayloadError::Decode)?;
        builder.register_type::<GasAmounts>().map_err(PayloadError::Decode)?;
        builder.register_type::<u64>().map_err(PayloadError::Decode)?;
        builder.register_type::<Bytes>().map_err(PayloadError::Decode)?;
        builder.register_type::<B256>().map_err(PayloadError::Decode)?;
        builder.register_type::<BlobFeesPerGas>().map_err(PayloadError::Decode)?;
        builder.register_type::<Bytes>().map_err(PayloadError::Decode)?;
        builder.register_type::<GasAmounts>().map_err(PayloadError::Decode)?;
        builder.register_type::<B256>().map_err(PayloadError::Decode)?;
        builder.register_type::<B256>().map_err(PayloadError::Decode)?;
        builder.register_type::<Bytes>().map_err(PayloadError::Decode)?;
        builder.register_type::<u64>().map_err(PayloadError::Decode)?;
        let mut decoder = builder.build().map_err(PayloadError::Decode)?;
        let parent_hash = decoder.decode_next().map_err(PayloadError::Decode)?;
        let miner = decoder.decode_next().map_err(PayloadError::Decode)?;
        let state_root = decoder.decode_next().map_err(PayloadError::Decode)?;
        let tx_bytes = decoder.decode_next_with(Ok).map_err(PayloadError::Decode)?;
        let receipts_root: B256 = decoder.decode_next().map_err(PayloadError::Decode)?;
        if receipts_root != receipts.root() {
            return Err(PayloadError::ComponentMismatch("receipts root"));
        }
        let number = decoder.decode_next().map_err(PayloadError::Decode)?;
        let gas_limits = decoder.decode_next().map_err(PayloadError::Decode)?;
        let gas_used = decoder.decode_next().map_err(PayloadError::Decode)?;
        let timestamp = decoder.decode_next().map_err(PayloadError::Decode)?;
        let extra_bytes = decoder.decode_next_with(Ok).map_err(PayloadError::Decode)?;
        if extra_bytes.len() > 32 {
            return Err(PayloadError::InvalidEncoding("extra data"));
        }
        let extra_data = Bytes::copy_from_slice(extra_bytes);
        let mix_hash = decoder.decode_next().map_err(PayloadError::Decode)?;
        let base_fees_per_gas = decoder.decode_next().map_err(PayloadError::Decode)?;
        let withdrawal_bytes = decoder.decode_next_with(Ok).map_err(PayloadError::Decode)?;
        let excess_gas = decoder.decode_next().map_err(PayloadError::Decode)?;
        let parent_beacon_block_root = decoder.decode_next().map_err(PayloadError::Decode)?;
        let requests_root: B256 = decoder.decode_next().map_err(PayloadError::Decode)?;
        if requests_root != requests.root() {
            return Err(PayloadError::ComponentMismatch("requests root"));
        }
        let bal_bytes = decoder.decode_next_with(Ok).map_err(PayloadError::Decode)?;
        let slot_number = decoder.decode_next().map_err(PayloadError::Decode)?;
        let count = if tx_bytes.is_empty() {
            0
        } else {
            let offset =
                tx_bytes.get(..4).ok_or(PayloadError::InvalidEncoding("transaction offsets"))?;
            let offset = u32::from_le_bytes(
                offset
                    .try_into()
                    .map_err(|_| PayloadError::InvalidEncoding("transaction offsets"))?,
            ) as usize;
            if offset == 0 || !offset.is_multiple_of(4) || offset > tx_bytes.len() {
                return Err(PayloadError::InvalidEncoding("transaction offsets"));
            }
            offset / 4
        };
        if count > limits.transactions {
            return Err(PayloadError::LimitExceeded("transactions"));
        }
        if count != receipts.receipts().len() {
            return Err(PayloadError::ComponentMismatch("transaction/receipt count"));
        }
        if !withdrawal_bytes.len().is_multiple_of(44) {
            return Err(PayloadError::InvalidEncoding("withdrawal list"));
        }
        if withdrawal_bytes.len() / 44 > limits.withdrawals {
            return Err(PayloadError::LimitExceeded("withdrawals"));
        }
        if bal_bytes.len() > limits.bal_bytes {
            return Err(PayloadError::LimitExceeded("BAL bytes"));
        }
        let fields = ExecutionPayloadFields {
            parent_hash,
            miner,
            state_root,
            number,
            gas_limits,
            gas_used,
            timestamp,
            extra_data,
            mix_hash,
            base_fees_per_gas,
            excess_gas,
            parent_beacon_block_root,
            slot_number,
        };
        let components = PayloadComponents {
            transactions: TransactionSnapshot::from_ssz(tx_bytes)
                .map_err(PayloadError::Transaction)?,
            receipts,
            withdrawals: WithdrawalSnapshot::from_ssz(withdrawal_bytes)
                .map_err(PayloadError::Withdrawal)?,
            requests,
            block_access_list: BlockAccessListSnapshot::from_rlp(
                Bytes::copy_from_slice(bal_bytes),
                limits.bal_bytes,
            )?,
        };
        let snapshot = Self::build(fields, components, limits)?;
        if snapshot.serialized() != bytes {
            return Err(PayloadError::InvalidEncoding("noncanonical payload"));
        }
        Ok(snapshot)
    }

    pub const fn fields(&self) -> &ExecutionPayloadFields {
        &self.fields
    }
    pub const fn components(&self) -> &PayloadComponents {
        &self.components
    }
    pub const fn summary(&self) -> &ExecutionSummarySnapshot {
        &self.summary
    }
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }
    pub const fn root(&self) -> B256 {
        self.summary.root()
    }

    pub fn node(&self, index: u128) -> Option<&RetainedNode> {
        if index == 0 {
            return None;
        }
        let depth = u128::BITS - 1 - index.leading_zeros();
        let mut current = self.summary.tree();
        let mut position = 1;
        for shift in (0..depth).rev() {
            current = match position {
                0x2a => self.components.transactions.tree(),
                0x167 => self.components.withdrawals.tree(),
                0x16b => self.components.block_access_list.tree(),
                _ => current,
            };
            current = &current.children()?[((index >> shift) & 1) as usize];
            position = index >> shift;
        }
        Some(match position {
            0x2a => self.components.transactions.tree(),
            0x167 => self.components.withdrawals.tree(),
            0x16b => self.components.block_access_list.tree(),
            _ => current,
        })
    }
}

#[cfg(test)]
mod tests;
