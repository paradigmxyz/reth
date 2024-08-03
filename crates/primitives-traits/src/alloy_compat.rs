use super::Header;
use alloy_rpc_types_eth::{ConversionError, Header as RpcHeader};

impl TryFrom<RpcHeader> for Header {
    type Error = ConversionError;

    fn try_from(header: RpcHeader) -> Result<Self, Self::Error> {
        Ok(Self {
            base_fee_per_gas: header
                .base_fee_per_gas
                .map(|base_fee_per_gas| {
                    base_fee_per_gas.try_into().map_err(ConversionError::BaseFeePerGasConversion)
                })
                .transpose()?,
            beneficiary: header.miner,
            blob_gas_used: header
                .blob_gas_used
                .map(|blob_gas_used| {
                    blob_gas_used.try_into().map_err(ConversionError::BlobGasUsedConversion)
                })
                .transpose()?,
            difficulty: header.difficulty,
            excess_blob_gas: header
                .excess_blob_gas
                .map(|excess_blob_gas| {
                    excess_blob_gas.try_into().map_err(ConversionError::ExcessBlobGasConversion)
                })
                .transpose()?,
            extra_data: header.extra_data,
            gas_limit: header.gas_limit.try_into().map_err(ConversionError::GasLimitConversion)?,
            gas_used: header.gas_used.try_into().map_err(ConversionError::GasUsedConversion)?,
            logs_bloom: header.logs_bloom,
            mix_hash: header.mix_hash.unwrap_or_default(),
            nonce: u64::from_be_bytes(header.nonce.unwrap_or_default().0),
            number: header.number.ok_or(ConversionError::MissingBlockNumber)?,
            ommers_hash: header.uncles_hash,
            parent_beacon_block_root: header.parent_beacon_block_root,
            parent_hash: header.parent_hash,
            receipts_root: header.receipts_root,
            state_root: header.state_root,
            timestamp: header.timestamp,
            transactions_root: header.transactions_root,
            withdrawals_root: header.withdrawals_root,
            // TODO: requests_root: header.requests_root,
            requests_root: None,
        })
    }
}
