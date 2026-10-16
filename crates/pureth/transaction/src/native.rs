use crate::{
    AccessListPayload, BlobPayload, FeeMarketPayload, LegacyPayload, SetCodePayload,
    TransactionError, TransactionPayload, TransactionSsz,
};
use alloy_consensus::{
    EthereumTxEnvelope, Signed, Transaction, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy,
};
use alloy_primitives::U256;

pub(crate) fn payload(source: &EthereumTxEnvelope<TxEip4844>) -> TransactionPayload {
    let nonce = source.nonce();
    let gas_limit = source.gas_limit();
    let to = source.kind();
    let value = source.value();
    let input = source.input().clone();
    match source {
        EthereumTxEnvelope::Legacy(signed) => TransactionPayload::Legacy(LegacyPayload {
            chain_id: signed.tx().chain_id.map(U256::from),
            nonce,
            gas_price: U256::from(signed.tx().gas_price),
            gas_limit,
            to,
            value,
            input,
        }),
        EthereumTxEnvelope::Eip2930(signed) => TransactionPayload::AccessList(AccessListPayload {
            chain_id: U256::from(signed.tx().chain_id),
            nonce,
            gas_price: U256::from(signed.tx().gas_price),
            gas_limit,
            to,
            value,
            input,
            access_list: signed.tx().access_list.clone(),
        }),
        EthereumTxEnvelope::Eip1559(signed) => TransactionPayload::FeeMarket(FeeMarketPayload {
            chain_id: U256::from(signed.tx().chain_id),
            nonce,
            max_fee_per_gas: U256::from(signed.tx().max_fee_per_gas),
            max_priority_fee_per_gas: U256::from(signed.tx().max_priority_fee_per_gas),
            gas_limit,
            to,
            value,
            input,
            access_list: signed.tx().access_list.clone(),
        }),
        EthereumTxEnvelope::Eip4844(signed) => TransactionPayload::Blob(BlobPayload {
            chain_id: U256::from(signed.tx().chain_id),
            nonce,
            max_fee_per_gas: U256::from(signed.tx().max_fee_per_gas),
            max_priority_fee_per_gas: U256::from(signed.tx().max_priority_fee_per_gas),
            max_fee_per_blob_gas: U256::from(signed.tx().max_fee_per_blob_gas),
            gas_limit,
            to: signed.tx().to,
            value,
            input,
            access_list: signed.tx().access_list.clone(),
            blob_versioned_hashes: signed.tx().blob_versioned_hashes.clone(),
        }),
        EthereumTxEnvelope::Eip7702(signed) => TransactionPayload::SetCode(SetCodePayload {
            chain_id: U256::from(signed.tx().chain_id),
            nonce,
            max_fee_per_gas: U256::from(signed.tx().max_fee_per_gas),
            max_priority_fee_per_gas: U256::from(signed.tx().max_priority_fee_per_gas),
            gas_limit,
            to: signed.tx().to,
            value,
            input,
            access_list: signed.tx().access_list.clone(),
            authorization_list: signed.tx().authorization_list.clone(),
        }),
    }
}

pub(crate) fn envelope(
    transaction: &TransactionSsz,
) -> Result<EthereumTxEnvelope<TxEip4844>, TransactionError> {
    let signature = *transaction.signature();
    Ok(match transaction.payload() {
        TransactionPayload::Legacy(p) => EthereumTxEnvelope::Legacy(Signed::new_unhashed(
            TxLegacy {
                chain_id: p.chain_id.map(chain).transpose()?,
                nonce: p.nonce,
                gas_price: fee(p.gas_price, "gas_price")?,
                gas_limit: p.gas_limit,
                to: p.to,
                value: p.value,
                input: p.input.clone(),
            },
            signature,
        )),
        TransactionPayload::AccessList(p) => EthereumTxEnvelope::Eip2930(Signed::new_unhashed(
            TxEip2930 {
                chain_id: chain(p.chain_id)?,
                nonce: p.nonce,
                gas_price: fee(p.gas_price, "gas_price")?,
                gas_limit: p.gas_limit,
                to: p.to,
                value: p.value,
                input: p.input.clone(),
                access_list: p.access_list.clone(),
            },
            signature,
        )),
        TransactionPayload::FeeMarket(p) => EthereumTxEnvelope::Eip1559(Signed::new_unhashed(
            TxEip1559 {
                chain_id: chain(p.chain_id)?,
                nonce: p.nonce,
                max_fee_per_gas: fee(p.max_fee_per_gas, "max_fee_per_gas")?,
                max_priority_fee_per_gas: fee(
                    p.max_priority_fee_per_gas,
                    "max_priority_fee_per_gas",
                )?,
                gas_limit: p.gas_limit,
                to: p.to,
                value: p.value,
                input: p.input.clone(),
                access_list: p.access_list.clone(),
            },
            signature,
        )),
        TransactionPayload::Blob(p) => EthereumTxEnvelope::Eip4844(Signed::new_unhashed(
            TxEip4844 {
                chain_id: chain(p.chain_id)?,
                nonce: p.nonce,
                max_fee_per_gas: fee(p.max_fee_per_gas, "max_fee_per_gas")?,
                max_priority_fee_per_gas: fee(
                    p.max_priority_fee_per_gas,
                    "max_priority_fee_per_gas",
                )?,
                max_fee_per_blob_gas: fee(p.max_fee_per_blob_gas, "max_fee_per_blob_gas")?,
                gas_limit: p.gas_limit,
                to: p.to,
                value: p.value,
                input: p.input.clone(),
                access_list: p.access_list.clone(),
                blob_versioned_hashes: p.blob_versioned_hashes.clone(),
            },
            signature,
        )),
        TransactionPayload::SetCode(p) => EthereumTxEnvelope::Eip7702(Signed::new_unhashed(
            TxEip7702 {
                chain_id: chain(p.chain_id)?,
                nonce: p.nonce,
                max_fee_per_gas: fee(p.max_fee_per_gas, "max_fee_per_gas")?,
                max_priority_fee_per_gas: fee(
                    p.max_priority_fee_per_gas,
                    "max_priority_fee_per_gas",
                )?,
                gas_limit: p.gas_limit,
                to: p.to,
                value: p.value,
                input: p.input.clone(),
                access_list: p.access_list.clone(),
                authorization_list: p.authorization_list.clone(),
            },
            signature,
        )),
    })
}

fn chain(value: U256) -> Result<u64, TransactionError> {
    u64::try_from(value).map_err(|_| TransactionError::UnsupportedWidth("chain_id"))
}

fn fee(value: U256, field: &'static str) -> Result<u128, TransactionError> {
    u128::try_from(value).map_err(|_| TransactionError::UnsupportedWidth(field))
}
