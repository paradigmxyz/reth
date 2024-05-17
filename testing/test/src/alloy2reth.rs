//! Common conversions between Alloy and Reth types.

use alloy_rpc_types::{
    AccessListItem as AlloyAccessListItem, Header as AlloyHeader, Signature as AlloySignature,
    Transaction as AlloyTransaction, Withdrawal as AlloyWithdrawal,
};
use reth_primitives::{
    AccessListItem as RethAccessListItem, Header as RethHeader, Signature as RethSignature,
    Transaction, TransactionSignedNoHash as RethTransaction, TransactionSignedNoHash, TxEip1559,
    TxEip2930, TxLegacy, Withdrawal as RethWithdrawal, U256, U64,
};

/// A trait to convert from Alloy types to Reth types.
pub trait IntoReth<T> {
    fn into_reth(self) -> T;
}

impl IntoReth<RethWithdrawal> for AlloyWithdrawal {
    fn into_reth(self) -> RethWithdrawal {
        RethWithdrawal {
            index: self.index,
            validator_index: self.validator_index,
            amount: self.amount,
            address: self.address,
        }
    }
}

impl IntoReth<RethTransaction> for AlloyTransaction {
    fn into_reth(self) -> RethTransaction {
        let tx_type: u64 = self.transaction_type.unwrap_or(0u8).try_into().unwrap();
        let inner_tx = match tx_type {
            0 => Transaction::Legacy(TxLegacy {
                chain_id: self.chain_id.map(|chain_id| chain_id.try_into().unwrap()),
                nonce: self.nonce.try_into().unwrap(),
                gas_price: self.gas_price.unwrap().try_into().unwrap(),
                gas_limit: self.gas.try_into().unwrap(),
                to: match self.to {
                    None => reth_primitives::TxKind::Create,
                    Some(to) => reth_primitives::TxKind::Call(to),
                },
                value: self.value.into(),
                input: self.input,
            }),
            1 => Transaction::Eip2930(TxEip2930 {
                chain_id: self.chain_id.unwrap().try_into().unwrap(),
                nonce: self.nonce.try_into().unwrap(),
                gas_price: self.gas_price.unwrap().try_into().unwrap(),
                gas_limit: self.gas.try_into().unwrap(),
                to: match self.to {
                    None => reth_primitives::TxKind::Create,
                    Some(to) => reth_primitives::TxKind::Call(to),
                },
                value: self.value.into(),
                input: self.input,
                access_list: reth_primitives::AccessList(
                    self.access_list
                        .unwrap()
                        .clone()
                        .iter()
                        .map(|item| item.clone().into_reth())
                        .collect(),
                ),
            }),
            2 => Transaction::Eip1559(TxEip1559 {
                chain_id: self.chain_id.unwrap().try_into().unwrap(),
                nonce: self.nonce.try_into().unwrap(),
                max_fee_per_gas: self.max_fee_per_gas.unwrap().try_into().unwrap(),
                max_priority_fee_per_gas: self
                    .max_priority_fee_per_gas
                    .unwrap()
                    .try_into()
                    .unwrap(),
                gas_limit: self.gas.try_into().unwrap(),
                to: match self.to {
                    None => reth_primitives::TxKind::Create,
                    Some(to) => reth_primitives::TxKind::Call(to),
                },
                value: self.value.into(),
                input: self.input,
                access_list: reth_primitives::AccessList(
                    self.access_list
                        .unwrap()
                        .clone()
                        .iter()
                        .map(|item| item.clone().into_reth())
                        .collect(),
                ),
            }),
            _ => panic!("invalid tx type: {}", tx_type),
        };
        TransactionSignedNoHash {
            signature: self.signature.unwrap().into_reth(),
            transaction: inner_tx,
        }
    }
}

impl IntoReth<RethAccessListItem> for AlloyAccessListItem {
    fn into_reth(self) -> RethAccessListItem {
        RethAccessListItem { address: self.address, storage_keys: self.storage_keys }
    }
}

impl IntoReth<RethSignature> for AlloySignature {
    fn into_reth(self) -> RethSignature {
        // TODO: should be chain_id * 2 + 35.
        let recovery_id = if self.v > U256::from(1) { self.v - U256::from(37) } else { self.v };

        RethSignature { r: self.r, s: self.s, odd_y_parity: recovery_id == U256::from(1) }
    }
}

impl IntoReth<RethHeader> for AlloyHeader {
    fn into_reth(self) -> RethHeader {
        RethHeader {
            parent_hash: self.parent_hash.0.into(),
            ommers_hash: self.uncles_hash.0.into(),
            beneficiary: self.miner.0.into(),
            state_root: self.state_root.0.into(),
            transactions_root: self.transactions_root.0.into(),
            receipts_root: self.receipts_root.0.into(),
            withdrawals_root: self.withdrawals_root,
            logs_bloom: self.logs_bloom.0.into(),
            difficulty: self.difficulty,
            number: self.number.unwrap().try_into().unwrap(),
            gas_limit: self.gas_limit.try_into().unwrap(),
            gas_used: self.gas_used.try_into().unwrap(),
            timestamp: self.timestamp.try_into().unwrap(),
            extra_data: self.extra_data.0.into(),
            mix_hash: self.mix_hash.unwrap(),
            nonce: u64::from_be_bytes(self.nonce.unwrap().0),
            base_fee_per_gas: Some(self.base_fee_per_gas.unwrap().try_into().unwrap()),
            blob_gas_used: self.blob_gas_used.map(|x| x.try_into().unwrap()),
            excess_blob_gas: self.excess_blob_gas.map(|x| x.try_into().unwrap()),
            parent_beacon_block_root: self.parent_beacon_block_root,
        }
    }
}
