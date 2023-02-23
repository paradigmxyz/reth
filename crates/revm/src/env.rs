use crate::config::revm_spec;
use reth_primitives::{
    Address, ChainSpec, Head, Header, Transaction, TransactionKind, TransactionSigned, TxEip1559,
    TxEip2930, TxLegacy, U256,
};
use revm::primitives::{AnalysisKind, BlockEnv, CfgEnv, TransactTo, TxEnv};

/// Fill [CfgEnv] fields according to the chain spec and given header
pub fn fill_cfg_env(
    cfg_env: &mut CfgEnv,
    chain_spec: &ChainSpec,
    header: &Header,
    total_difficulty: U256,
) {
    let spec_id = revm_spec(
        chain_spec,
        Head {
            number: header.number,
            timestamp: header.timestamp,
            difficulty: header.difficulty,
            total_difficulty,
            hash: Default::default(),
        },
    );

    cfg_env.chain_id = U256::from(chain_spec.chain().id());
    cfg_env.spec_id = spec_id;
    cfg_env.perf_all_precompiles_have_balance = false;
    cfg_env.perf_analyse_created_bytecodes = AnalysisKind::Raw;
}
/// Fill block environment from Block.
pub fn fill_block_env(block_env: &mut BlockEnv, header: &Header, after_merge: bool) {
    block_env.number = U256::from(header.number);
    block_env.coinbase = header.beneficiary;
    block_env.timestamp = U256::from(header.timestamp);
    if after_merge {
        block_env.prevrandao = Some(header.mix_hash);
        block_env.difficulty = U256::ZERO;
    } else {
        block_env.difficulty = header.difficulty;
        block_env.prevrandao = None;
    }
    block_env.basefee = U256::from(header.base_fee_per_gas.unwrap_or_default());
    block_env.gas_limit = U256::from(header.gas_limit);
}

/// Fill transaction environment from Transaction.
pub fn fill_tx_env(tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address) {
    tx_env.caller = sender;
    match transaction.as_ref() {
        Transaction::Legacy(TxLegacy {
            nonce,
            chain_id,
            gas_price,
            gas_limit,
            to,
            value,
            input,
        }) => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = U256::from(*gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = U256::from(*value);
            tx_env.data = input.0.clone();
            tx_env.chain_id = *chain_id;
            tx_env.nonce = Some(*nonce);
        }
        Transaction::Eip2930(TxEip2930 {
            nonce,
            chain_id,
            gas_price,
            gas_limit,
            to,
            value,
            input,
            access_list,
        }) => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = U256::from(*gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = U256::from(*value);
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
                .0
                .iter()
                .map(|l| {
                    (
                        l.address,
                        l.storage_keys
                            .iter()
                            .map(|k| U256::from_be_bytes(k.to_fixed_bytes()))
                            .collect(),
                    )
                })
                .collect();
        }
        Transaction::Eip1559(TxEip1559 {
            nonce,
            chain_id,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            input,
            access_list,
        }) => {
            tx_env.gas_limit = *gas_limit;
            tx_env.gas_price = U256::from(*max_fee_per_gas);
            tx_env.gas_priority_fee = Some(U256::from(*max_priority_fee_per_gas));
            tx_env.transact_to = match to {
                TransactionKind::Call(to) => TransactTo::Call(*to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = U256::from(*value);
            tx_env.data = input.0.clone();
            tx_env.chain_id = Some(*chain_id);
            tx_env.nonce = Some(*nonce);
            tx_env.access_list = access_list
                .0
                .iter()
                .map(|l| {
                    (
                        l.address,
                        l.storage_keys
                            .iter()
                            .map(|k| U256::from_be_bytes(k.to_fixed_bytes()))
                            .collect(),
                    )
                })
                .collect();
        }
    }
}
