use reth_primitives::{
    constants::{MAXIMUM_EXTRA_DATA_SIZE, MIN_PROTOCOL_BASE_FEE_U256},
    proofs::{self, EMPTY_LIST_HASH},
    Header, SealedBlock, TransactionSigned, UintTryTo, Withdrawal,
};
use reth_rlp::Decodable;
use reth_rpc_types::{engine::StandaloneWithdraw, ExecutionPayload, PayloadError};

fn try_convert_from_execution_payload_to_sealed_block(
    payload: ExecutionPayload,
) -> Result<SealedBlock, PayloadError> {
    if payload.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
        return Err(PayloadError::ExtraData(payload.extra_data))
    }

    if payload.base_fee_per_gas < MIN_PROTOCOL_BASE_FEE_U256 {
        return Err(PayloadError::BaseFee(payload.base_fee_per_gas))
    }

    let transactions = payload
        .transactions
        .iter()
        .map(|tx| TransactionSigned::decode(&mut tx.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    let transactions_root = proofs::calculate_transaction_root(&transactions);
    let withdraw = payload.withdrawals.as_ref().map(|withdrawals| {
        withdrawals
            .iter()
            .map(|withdrawal| Withdrawal::from(withdrawal.clone())) 
            .collect::<Vec<_>>()
    });
    let withdrawals_root = withdraw.as_ref().map(|w| proofs::calculate_withdrawals_root(w));

    let header = Header {
        parent_hash: payload.parent_hash,
        beneficiary: payload.fee_recipient,
        state_root: payload.state_root,
        transactions_root,
        receipts_root: payload.receipts_root,
        withdrawals_root,
        logs_bloom: payload.logs_bloom,
        number: payload.block_number.as_u64(),
        gas_limit: payload.gas_limit.as_u64(),
        gas_used: payload.gas_used.as_u64(),
        timestamp: payload.timestamp.as_u64(),
        mix_hash: payload.prev_randao,
        base_fee_per_gas: Some(
            payload
                .base_fee_per_gas
                .uint_try_to()
                .map_err(|_| PayloadError::BaseFee(payload.base_fee_per_gas))?,
        ),
        blob_gas_used: payload.blob_gas_used.map(|blob_gas_used| blob_gas_used.as_u64()),
        excess_blob_gas: payload.excess_blob_gas.map(|excess_blob_gas| excess_blob_gas.as_u64()),
        extra_data: payload.extra_data,
        // Defaults
        ommers_hash: EMPTY_LIST_HASH,
        difficulty: Default::default(),
        nonce: Default::default(),
    }
    .seal_slow();

    if payload.block_hash != header.hash() {
        return Err(PayloadError::BlockHash {
            execution: header.hash(),
            consensus: payload.block_hash,
        })
    }

    Ok(SealedBlock {
        header,
        body: transactions,
        withdrawals: withdraw,
        ommers: Default::default(),
    })
}
