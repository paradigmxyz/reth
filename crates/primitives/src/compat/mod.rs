#[cfg(feature = "alloy-compat")]
use crate::{Block, Header, TransactionSigned};
#[cfg(feature = "alloy-compat")]
impl TryFrom<alloy_rpc_types::Block> for Block {
    type Error = alloy_rpc_types::ConversionError;

    fn try_from(block: alloy_rpc_types::Block) -> Result<Self, Self::Error> {
        use alloy_rpc_types::ConversionError;

        let body = {
            let transactions: Result<Vec<TransactionSigned>, ConversionError> = match block
                .transactions
            {
                alloy_rpc_types::BlockTransactions::Full(transactions) => transactions
                    .into_iter()
                    .map(|tx| {
                        let signature = tx.signature.ok_or(ConversionError::MissingSignature)?;
                        Ok(TransactionSigned::from_transaction_and_signature(
                            tx.try_into()?,
                            crate::Signature {
                                r: signature.r,
                                s: signature.s,
                                odd_y_parity: signature
                                    .y_parity
                                    .unwrap_or(alloy_rpc_types::Parity(false))
                                    .0,
                            },
                        ))
                    })
                    .collect(),
                alloy_rpc_types::BlockTransactions::Hashes(_) |
                alloy_rpc_types::BlockTransactions::Uncle => {
                    return Err(ConversionError::MissingFullTransactions)
                }
            };
            transactions?
        };

        Ok(Self {
            header: block.header.try_into()?,
            body,
            ommers: Default::default(),
            withdrawals: block.withdrawals.map(Into::into),
        })
    }
}

#[cfg(feature = "alloy-compat")]
impl TryFrom<alloy_rpc_types::Header> for Header {
    type Error = alloy_rpc_types::ConversionError;

    fn try_from(header: alloy_rpc_types::Header) -> Result<Self, Self::Error> {
        use alloy_rpc_types::ConversionError;

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
        })
    }
}
