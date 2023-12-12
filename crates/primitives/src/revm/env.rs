use crate::{
    constants::{BEACON_ROOTS_ADDRESS, SYSTEM_ADDRESS},
    recover_signer_unchecked,
    revm::config::revm_spec,
    revm_primitives::{AnalysisKind, BlockEnv, CfgEnv, Env, SpecId, TransactTo, TxEnv},
    Address, Bytes, Chain, ChainSpec, Head, Header, Transaction, TransactionKind,
    TransactionSignedEcRecovered, B256, U256,
};

#[cfg(feature = "optimism")]
use revm_primitives::OptimismFields;

/// Convenience function to call both [fill_cfg_env] and [fill_block_env]
pub fn fill_cfg_and_block_env(
    cfg: &mut CfgEnv,
    block_env: &mut BlockEnv,
    chain_spec: &ChainSpec,
    header: &Header,
    total_difficulty: U256,
) {
    fill_cfg_env(cfg, chain_spec, header, total_difficulty);
    let after_merge = cfg.spec_id >= SpecId::MERGE;
    fill_block_env(block_env, chain_spec, header, after_merge);
}

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

    cfg_env.chain_id = chain_spec.chain().id();
    cfg_env.spec_id = spec_id;
    cfg_env.perf_analyse_created_bytecodes = AnalysisKind::Analyse;

    #[cfg(feature = "optimism")]
    {
        cfg_env.optimism = chain_spec.is_optimism();
    }
}

/// Fill block environment from Block.
pub fn fill_block_env(
    block_env: &mut BlockEnv,
    chain_spec: &ChainSpec,
    header: &Header,
    after_merge: bool,
) {
    let coinbase = block_coinbase(chain_spec, header, after_merge);
    fill_block_env_with_coinbase(block_env, header, after_merge, coinbase);
}

/// Fill block environment with coinbase.
#[inline]
pub fn fill_block_env_with_coinbase(
    block_env: &mut BlockEnv,
    header: &Header,
    after_merge: bool,
    coinbase: Address,
) {
    block_env.number = U256::from(header.number);
    block_env.coinbase = coinbase;
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

    // EIP-4844 excess blob gas of this block, introduced in Cancun
    if let Some(excess_blob_gas) = header.excess_blob_gas {
        block_env.set_blob_excess_gas_and_price(excess_blob_gas);
    }
}

/// Return the coinbase address for the given header and chain spec.
pub fn block_coinbase(chain_spec: &ChainSpec, header: &Header, after_merge: bool) -> Address {
    if chain_spec.chain == Chain::goerli() && !after_merge {
        recover_header_signer(header).unwrap_or_else(|err| {
            panic!(
                "Failed to recover goerli Clique Consensus signer from header ({}, {}) using extradata {}: {:?}",
                header.number, header.hash_slow(), header.extra_data, err
            )
        })
    } else {
        header.beneficiary
    }
}

/// Error type for recovering Clique signer from a header.
#[derive(Debug, thiserror::Error)]
pub enum CliqueSignerRecoveryError {
    /// Header extradata is too short.
    #[error("Invalid extra data length")]
    InvalidExtraData,
    /// Recovery failed.
    #[error("Invalid signature: {0}")]
    InvalidSignature(#[from] secp256k1::Error),
}

/// Recover the account from signed header per clique consensus rules.
pub fn recover_header_signer(header: &Header) -> Result<Address, CliqueSignerRecoveryError> {
    let extra_data_len = header.extra_data.len();
    // Fixed number of extra-data suffix bytes reserved for signer signature.
    // 65 bytes fixed as signatures are based on the standard secp256k1 curve.
    // Filled with zeros on genesis block.
    let signature_start_byte = extra_data_len - 65;
    let signature: [u8; 65] = header.extra_data[signature_start_byte..]
        .try_into()
        .map_err(|_| CliqueSignerRecoveryError::InvalidExtraData)?;
    let seal_hash = {
        let mut header_to_seal = header.clone();
        header_to_seal.extra_data = Bytes::from(header.extra_data[..signature_start_byte].to_vec());
        header_to_seal.hash_slow()
    };

    // TODO: this is currently unchecked recovery, does this need to be checked w.r.t EIP-2?
    recover_signer_unchecked(&signature, &seal_hash.0)
        .map_err(CliqueSignerRecoveryError::InvalidSignature)
}

/// Returns a new [TxEnv] filled with the transaction's data.
pub fn tx_env_with_recovered(transaction: &TransactionSignedEcRecovered) -> TxEnv {
    let mut tx_env = TxEnv::default();

    #[cfg(not(feature = "optimism"))]
    fill_tx_env(&mut tx_env, transaction.as_ref(), transaction.signer());

    #[cfg(feature = "optimism")]
    {
        let mut envelope_buf = Vec::with_capacity(transaction.length_without_header());
        transaction.encode_enveloped(&mut envelope_buf);
        fill_tx_env(&mut tx_env, transaction.as_ref(), transaction.signer(), envelope_buf.into());
    }

    tx_env
}

/// Fill transaction environment with the EIP-4788 system contract message data.
///
/// This requirements for the beacon root contract call defined by
/// [EIP-4788](https://eips.ethereum.org/EIPS/eip-4788) are:
///
/// At the start of processing any execution block where `block.timestamp >= FORK_TIMESTAMP` (i.e.
/// before processing any transactions), call `BEACON_ROOTS_ADDRESS` as `SYSTEM_ADDRESS` with the
/// 32-byte input of `header.parent_beacon_block_root`, a gas limit of `30_000_000`, and `0` value.
/// This will trigger the `set()` routine of the beacon roots contract. This is a system operation
/// and therefore:
///  * the call must execute to completion
///  * the call does not count against the block’s gas limit
///  * the call does not follow the EIP-1559 burn semantics - no value should be transferred as
///  part of the call
///  * if no code exists at `BEACON_ROOTS_ADDRESS`, the call must fail silently
pub fn fill_tx_env_with_beacon_root_contract_call(env: &mut Env, parent_beacon_block_root: B256) {
    env.tx = TxEnv {
        caller: SYSTEM_ADDRESS,
        transact_to: TransactTo::Call(BEACON_ROOTS_ADDRESS),
        // Explicitly set nonce to None so revm does not do any nonce checks
        nonce: None,
        gas_limit: 30_000_000,
        value: U256::ZERO,
        data: parent_beacon_block_root.0.to_vec().into(),
        // Setting the gas price to zero enforces that no value is transferred as part of the call,
        // and that the call will not count against the block's gas limit
        gas_price: U256::ZERO,
        // The chain ID check is not relevant here and is disabled if set to None
        chain_id: None,
        // Setting the gas priority fee to None ensures the effective gas price is derived from the
        // `gas_price` field, which we need to be zero
        gas_priority_fee: None,
        access_list: Vec::new(),
        // blob fields can be None for this tx
        blob_hashes: Vec::new(),
        max_fee_per_blob_gas: None,
        #[cfg(feature = "optimism")]
        optimism: OptimismFields {
            source_hash: None,
            mint: None,
            is_system_transaction: Some(false),
            enveloped_tx: None,
        },
    };

    // ensure the block gas limit is >= the tx
    env.block.gas_limit = U256::from(env.tx.gas_limit);

    // disable the base fee check for this call by setting the base fee to zero
    env.block.basefee = U256::ZERO;
}

/// Fill transaction environment from [TransactionSignedEcRecovered].
#[cfg(not(feature = "optimism"))]
pub fn fill_tx_env_with_recovered(tx_env: &mut TxEnv, transaction: &TransactionSignedEcRecovered) {
    fill_tx_env(tx_env, transaction.as_ref(), transaction.signer());
}

/// Fill transaction environment from [TransactionSignedEcRecovered] and the given envelope.
#[cfg(feature = "optimism")]
pub fn fill_tx_env_with_recovered(
    tx_env: &mut TxEnv,
    transaction: &TransactionSignedEcRecovered,
    envelope: Bytes,
) {
    fill_tx_env(tx_env, transaction.as_ref(), transaction.signer(), envelope);
}

/// Fill transaction environment from a [Transaction] and the given sender address.
pub fn fill_tx_env<T>(
    tx_env: &mut TxEnv,
    transaction: T,
    sender: Address,
    #[cfg(feature = "optimism")] envelope: Bytes,
) where
    T: AsRef<Transaction>,
{
    tx_env.caller = sender;
    match transaction.as_ref() {
        Transaction::Legacy(tx) => {
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::from(tx.gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match tx.to {
                TransactionKind::Call(to) => TransactTo::Call(to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = tx.value.into();
            tx_env.data = tx.input.clone();
            tx_env.chain_id = tx.chain_id;
            tx_env.nonce = Some(tx.nonce);
            tx_env.access_list.clear();
            tx_env.blob_hashes.clear();
            tx_env.max_fee_per_blob_gas.take();

            #[cfg(feature = "optimism")]
            fill_op_tx_env(tx_env, transaction, envelope);
        }
        Transaction::Eip2930(tx) => {
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::from(tx.gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match tx.to {
                TransactionKind::Call(to) => TransactTo::Call(to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = tx.value.into();
            tx_env.data = tx.input.clone();
            tx_env.chain_id = Some(tx.chain_id);
            tx_env.nonce = Some(tx.nonce);
            tx_env.access_list = tx
                .access_list
                .0
                .iter()
                .map(|l| {
                    (l.address, l.storage_keys.iter().map(|k| U256::from_be_bytes(k.0)).collect())
                })
                .collect();
            tx_env.blob_hashes.clear();
            tx_env.max_fee_per_blob_gas.take();

            #[cfg(feature = "optimism")]
            fill_op_tx_env(tx_env, transaction, envelope);
        }
        Transaction::Eip1559(tx) => {
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::from(tx.max_fee_per_gas);
            tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
            tx_env.transact_to = match tx.to {
                TransactionKind::Call(to) => TransactTo::Call(to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = tx.value.into();
            tx_env.data = tx.input.clone();
            tx_env.chain_id = Some(tx.chain_id);
            tx_env.nonce = Some(tx.nonce);
            tx_env.access_list = tx
                .access_list
                .0
                .iter()
                .map(|l| {
                    (l.address, l.storage_keys.iter().map(|k| U256::from_be_bytes(k.0)).collect())
                })
                .collect();
            tx_env.blob_hashes.clear();
            tx_env.max_fee_per_blob_gas.take();

            #[cfg(feature = "optimism")]
            fill_op_tx_env(tx_env, transaction, envelope);
        }
        Transaction::Eip4844(tx) => {
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::from(tx.max_fee_per_gas);
            tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
            tx_env.transact_to = match tx.to {
                TransactionKind::Call(to) => TransactTo::Call(to),
                TransactionKind::Create => TransactTo::create(),
            };
            tx_env.value = tx.value.into();
            tx_env.data = tx.input.clone();
            tx_env.chain_id = Some(tx.chain_id);
            tx_env.nonce = Some(tx.nonce);
            tx_env.access_list = tx
                .access_list
                .0
                .iter()
                .map(|l| {
                    (l.address, l.storage_keys.iter().map(|k| U256::from_be_bytes(k.0)).collect())
                })
                .collect();
            tx_env.blob_hashes = tx.blob_versioned_hashes.clone();
            tx_env.max_fee_per_blob_gas = Some(U256::from(tx.max_fee_per_blob_gas));

            #[cfg(feature = "optimism")]
            fill_op_tx_env(tx_env, transaction, envelope);
        }
        #[cfg(feature = "optimism")]
        Transaction::Deposit(tx) => {
            tx_env.access_list.clear();
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::ZERO;
            tx_env.gas_priority_fee = None;
            match tx.to {
                TransactionKind::Call(to) => tx_env.transact_to = TransactTo::Call(to),
                TransactionKind::Create => tx_env.transact_to = TransactTo::create(),
            }
            tx_env.value = tx.value.into();
            tx_env.data = tx.input.clone();
            tx_env.chain_id = None;
            tx_env.nonce = None;

            fill_op_tx_env(tx_env, transaction, envelope);
        }
    }
}

#[cfg(feature = "optimism")]
#[inline(always)]
fn fill_op_tx_env<T: AsRef<Transaction>>(tx_env: &mut TxEnv, transaction: T, envelope: Bytes) {
    match transaction.as_ref() {
        Transaction::Deposit(tx) => {
            tx_env.optimism = OptimismFields {
                source_hash: Some(tx.source_hash),
                mint: tx.mint,
                is_system_transaction: Some(tx.is_system_transaction),
                enveloped_tx: Some(envelope),
            };
        }
        _ => {
            tx_env.optimism = OptimismFields {
                source_hash: None,
                mint: None,
                is_system_transaction: Some(false),
                enveloped_tx: Some(envelope),
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn test_fill_cfg_and_block_env() {
        let mut cfg_env = CfgEnv::default();
        let mut block_env = BlockEnv::default();
        let header = Header::default();
        let chain_spec = ChainSpec::default();
        let total_difficulty = U256::ZERO;

        fill_cfg_and_block_env(
            &mut cfg_env,
            &mut block_env,
            &chain_spec,
            &header,
            total_difficulty,
        );

        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }
}
