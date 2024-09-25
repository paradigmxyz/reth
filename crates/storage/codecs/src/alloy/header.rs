use crate::Compact;
use alloy_consensus::Header as AlloyHeader;
use alloy_primitives::{Address, BlockNumber, Bloom, Bytes, B256, U256};
use serde::{Deserialize, Serialize};

/// Block header
///
/// This is a helper type to use derive on it instead of manually managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Make sure this struct is 1:1 with [`alloy_consensus::Header`]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Compact)]
struct Header {
    parent_hash: B256,
    ommers_hash: B256,
    beneficiary: Address,
    state_root: B256,
    transactions_root: B256,
    receipts_root: B256,
    withdrawals_root: Option<B256>,
    logs_bloom: Bloom,
    difficulty: U256,
    number: BlockNumber,
    gas_limit: u64,
    gas_used: u64,
    timestamp: u64,
    mix_hash: B256,
    nonce: u64,
    base_fee_per_gas: Option<u64>,
    blob_gas_used: Option<u64>,
    excess_blob_gas: Option<u64>,
    parent_beacon_block_root: Option<B256>,
    requests_root: Option<B256>,
    extra_data: Bytes,
}

impl Compact for AlloyHeader {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let header = Header {
            parent_hash: self.parent_hash,
            ommers_hash: self.ommers_hash,
            beneficiary: self.beneficiary,
            state_root: self.state_root,
            transactions_root: self.transactions_root,
            receipts_root: self.receipts_root,
            withdrawals_root: self.withdrawals_root,
            logs_bloom: self.logs_bloom,
            difficulty: self.difficulty,
            number: self.number,
            gas_limit: self.gas_limit as u64,
            gas_used: self.gas_used as u64,
            timestamp: self.timestamp,
            mix_hash: self.mix_hash,
            nonce: self.nonce.into(),
            base_fee_per_gas: self.base_fee_per_gas.map(|base_fee| base_fee as u64),
            blob_gas_used: self.blob_gas_used.map(|blob_gas| blob_gas as u64),
            excess_blob_gas: self.excess_blob_gas.map(|excess_blob| excess_blob as u64),
            parent_beacon_block_root: self.parent_beacon_block_root,
            requests_root: self.requests_root,
            extra_data: self.extra_data.clone(),
        };
        header.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (header, _) = Header::from_compact(buf, len);
        let alloy_header = Self {
            parent_hash: header.parent_hash,
            ommers_hash: header.ommers_hash,
            beneficiary: header.beneficiary,
            state_root: header.state_root,
            transactions_root: header.transactions_root,
            receipts_root: header.receipts_root,
            withdrawals_root: header.withdrawals_root,
            logs_bloom: header.logs_bloom,
            difficulty: header.difficulty,
            number: header.number,
            gas_limit: header.gas_limit.into(),
            gas_used: header.gas_used.into(),
            timestamp: header.timestamp,
            mix_hash: header.mix_hash,
            nonce: header.nonce.into(),
            base_fee_per_gas: header.base_fee_per_gas.map(Into::into),
            blob_gas_used: header.blob_gas_used.map(Into::into),
            excess_blob_gas: header.excess_blob_gas.map(Into::into),
            parent_beacon_block_root: header.parent_beacon_block_root,
            requests_root: header.requests_root,
            extra_data: header.extra_data,
        };
        (alloy_header, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_backwards_compatibility() {
        assert_eq!(Header::bitflag_encoded_bytes(), 4);
    }
}
