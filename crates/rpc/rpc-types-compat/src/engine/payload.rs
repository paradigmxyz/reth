use reth_primitives::{
    constants::{MAXIMUM_EXTRA_DATA_SIZE, MIN_PROTOCOL_BASE_FEE_U256},
    proofs::{self, EMPTY_LIST_HASH},
    Header, SealedBlock, TransactionSigned, UintTryTo, Withdrawal,Block,U256, U64,
};
use reth_rlp::Decodable;
use reth_rpc_types::{ExecutionPayload, PayloadError};
use reth_rpc_types::engine::payload::{ExecutionPayloadBodyV1,StandaloneWithdraw};

pub fn try_convert_from_execution_payload_to_sealed_block(
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
    let withdrawals: Vec<reth_primitives::Withdrawal> = payload.withdrawals.unwrap_or_else(Vec::new).into_iter().map(|withdrawal| {
        convert_standalonewithdraw_to_withdrawal(withdrawal)
    }).collect();
    let withdrawals_root =  Some(proofs::calculate_withdrawals_root(&withdrawals));

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
        withdrawals: Some(withdrawals),
        ommers: Default::default(),
    })
}


 pub fn convert_withdrawal_to_standalonewithdraw(withdrawal: &Withdrawal) -> StandaloneWithdraw {
        StandaloneWithdraw {
            index: withdrawal.index,
            validator_index: withdrawal.validator_index,
            address: withdrawal.address,
            amount: withdrawal.amount,
        }
    }


 pub  fn convert_standalonewithdraw_to_withdrawal(standalone: StandaloneWithdraw) -> Withdrawal {
        Withdrawal {
            index: standalone.index,
            validator_index: standalone.validator_index,
            address: standalone.address,
            amount: standalone.amount,
        }
    }

  pub fn convert_to_execution_payload(value: SealedBlock) -> ExecutionPayload {
            let transactions = value
                .body
                .iter()
                .map(|tx| {
                    let mut encoded = Vec::new();
                    tx.encode_enveloped(&mut encoded);
                    encoded.into()
                })
                .collect();
            let withdraw = value.withdrawals.as_ref().map(|withdrawals| {
                withdrawals
                    .iter()
                    .map(|withdrawal| convert_withdrawal_to_standalonewithdraw(withdrawal))
                    .collect::<Vec<_>>()
            });
          
            ExecutionPayload {
                parent_hash: value.parent_hash,
                fee_recipient: value.beneficiary,
                state_root: value.state_root,
                receipts_root: value.receipts_root,
                logs_bloom: value.logs_bloom,
                prev_randao: value.mix_hash,
                block_number: value.number.into(),
                gas_limit: value.gas_limit.into(),
                gas_used: value.gas_used.into(),
                timestamp: value.timestamp.into(),
                extra_data: value.extra_data.clone(),
                base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
                blob_gas_used: value.blob_gas_used.map(U64::from),
                excess_blob_gas: value.excess_blob_gas.map(U64::from),
                block_hash: value.hash(),
                transactions,
                withdrawals: withdraw,
            }
        }
    

    pub fn convert_to_execution_payloadv1(value: Block) -> ExecutionPayloadBodyV1 {
        let transactions = value.body.into_iter().map(|tx| {
            let mut out = Vec::new();
            tx.encode_enveloped(&mut out);
            out.into()
        });
        let withdraw = value.withdrawals.map(|withdrawals| {
            withdrawals
                .into_iter()
                .map(|withdrawal| convert_withdrawal_to_standalonewithdraw(&withdrawal))
                .collect::<Vec<_>>()
        });
        ExecutionPayloadBodyV1 {
            transactions: transactions.collect(),
            withdrawals: withdraw,
        }
    }


