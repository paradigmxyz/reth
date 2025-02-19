use crate::{
    recover_signer_unchecked,
    revm_primitives::{BlockEnv, Env, TransactTo, TxEnv},
    Address, Bytes, Header, Transaction, TransactionSignedEcRecovered, TxKind, B256, U256,
};
use reth_chainspec::{Chain, ChainSpec};

use alloy_eips::{eip4788::BEACON_ROOTS_ADDRESS, eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS};
#[cfg(feature = "optimism")]
use revm_primitives::OptimismFields;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

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
    // Clique consensus fills the EXTRA_SEAL (last 65 bytes) of the extra data with the
    // signer's signature.
    //
    // On the genesis block, the extra data is filled with zeros, so we should not attempt to
    // recover the signer on the genesis block.
    //
    // From EIP-225:
    //
    // * `EXTRA_SEAL`: Fixed number of extra-data suffix bytes reserved for signer seal.
    //   * 65 bytes fixed as signatures are based on the standard `secp256k1` curve.
    //   * Filled with zeros on genesis block.
    if chain_spec.chain == Chain::goerli() && !after_merge && header.number > 0 {
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
#[derive(Debug, thiserror_no_std::Error)]
pub enum CliqueSignerRecoveryError {
    /// Header extradata is too short.
    #[error("Invalid extra data length")]
    InvalidExtraData,
    /// Recovery failed.
    #[cfg(feature = "k256")]
    #[error("Invalid signature: {0}")]
    InvalidSignature(#[from] k256::ecdsa::Error),
    /// Recovery failed.
    #[cfg(not(feature = "k256"))]
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

/// Returns a new [`TxEnv`] filled with the transaction's data.
pub fn tx_env_with_recovered(transaction: &TransactionSignedEcRecovered) -> TxEnv {
    let mut tx_env = TxEnv::default();

    #[cfg(not(feature = "optimism"))]
    fill_tx_env(&mut tx_env, transaction.as_ref(), transaction.signer());

    #[cfg(feature = "optimism")]
    {
        let mut envelope_buf = Vec::with_capacity(transaction.length_without_header());
        transaction.encode_enveloped(&mut envelope_buf);
        fill_op_tx_env(
            &mut tx_env,
            transaction.as_ref(),
            transaction.signer(),
            envelope_buf.into(),
        );
    }

    tx_env
}

/// Fill transaction environment with the EIP-4788 system contract message data.
///
/// This requirements for the beacon root contract call defined by
/// [EIP-4788](https://eips.ethereum.org/EIPS/eip-4788) are:
///
/// At the start of processing any execution block where `block.timestamp >= FORK_TIMESTAMP` (i.e.
/// before processing any transactions), call [`BEACON_ROOTS_ADDRESS`] as
/// [`SYSTEM_ADDRESS`](alloy_eips::eip4788::SYSTEM_ADDRESS) with the 32-byte input of
/// `header.parent_beacon_block_root`. This will trigger the `set()` routine of the beacon roots
/// contract.
pub fn fill_tx_env_with_beacon_root_contract_call(env: &mut Env, parent_beacon_block_root: B256) {
    fill_tx_env_with_system_contract_call(
        env,
        alloy_eips::eip4788::SYSTEM_ADDRESS,
        BEACON_ROOTS_ADDRESS,
        parent_beacon_block_root.0.into(),
    );
}

/// Fill transaction environment with the EIP-7002 withdrawal requests contract message data.
//
/// This requirement for the withdrawal requests contract call defined by
/// [EIP-7002](https://eips.ethereum.org/EIPS/eip-7002) is:
//
/// At the end of processing any execution block where `block.timestamp >= FORK_TIMESTAMP` (i.e.
/// after processing all transactions and after performing the block body withdrawal requests
/// validations), call the contract as `SYSTEM_ADDRESS`.
pub fn fill_tx_env_with_withdrawal_requests_contract_call(env: &mut Env) {
    fill_tx_env_with_system_contract_call(
        env,
        alloy_eips::eip7002::SYSTEM_ADDRESS,
        WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
        Bytes::new(),
    );
}

/// Fill transaction environment with the system caller and the system contract address and message
/// data.
///
/// This is a system operation and therefore:
///  * the call must execute to completion
///  * the call does not count against the block’s gas limit
///  * the call does not follow the EIP-1559 burn semantics - no value should be transferred as part
///    of the call
///  * if no code exists at the provided address, the call will fail silently
fn fill_tx_env_with_system_contract_call(
    env: &mut Env,
    caller: Address,
    contract: Address,
    data: Bytes,
) {
    env.tx = TxEnv {
        caller,
        transact_to: TransactTo::Call(contract),
        // Explicitly set nonce to None so revm does not do any nonce checks
        nonce: None,
        gas_limit: 30_000_000,
        value: U256::ZERO,
        data,
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
        eof_initcodes: Vec::new(),
        eof_initcodes_hashed: std::collections::HashMap::new(),
        #[cfg(feature = "optimism")]
        optimism: OptimismFields {
            source_hash: None,
            mint: None,
            is_system_transaction: Some(false),
            // The L1 fee is not charged for the EIP-4788 transaction, submit zero bytes for the
            // enveloped tx size.
            enveloped_tx: Some(Bytes::default()),
        },
        #[cfg(feature = "taiko")]
        taiko: revm_primitives::TaikoFields {
            treasury: Address::default(),
            is_anchor: false,
            basefee_ratio: 0u8,
        },
    };

    // ensure the block gas limit is >= the tx
    env.block.gas_limit = U256::from(env.tx.gas_limit);

    // disable the base fee check for this call by setting the base fee to zero
    env.block.basefee = U256::ZERO;
}

/// Fill transaction environment from [`TransactionSignedEcRecovered`].
#[cfg(not(feature = "optimism"))]
pub fn fill_tx_env_with_recovered(tx_env: &mut TxEnv, transaction: &TransactionSignedEcRecovered) {
    fill_tx_env(tx_env, transaction.as_ref(), transaction.signer());
}

/// Fill transaction environment from [`TransactionSignedEcRecovered`] and the given envelope.
#[cfg(feature = "optimism")]
pub fn fill_tx_env_with_recovered(
    tx_env: &mut TxEnv,
    transaction: &TransactionSignedEcRecovered,
    envelope: Bytes,
) {
    fill_op_tx_env(tx_env, transaction.as_ref(), transaction.signer(), envelope);
}

/// Fill transaction environment from a [Transaction] and the given sender address.
pub fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address)
where
    T: AsRef<Transaction>,
{
    tx_env.caller = sender;
    match transaction.as_ref() {
        Transaction::Legacy(tx) => {
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::from(tx.gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match tx.to {
                TxKind::Call(to) => TransactTo::Call(to),
                TxKind::Create => TransactTo::create(),
            };
            tx_env.value = tx.value;
            tx_env.data = tx.input.clone();
            tx_env.chain_id = tx.chain_id;
            tx_env.nonce = Some(tx.nonce);
            tx_env.access_list.clear();
            tx_env.blob_hashes.clear();
            tx_env.max_fee_per_blob_gas.take();
        }
        Transaction::Eip2930(tx) => {
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::from(tx.gas_price);
            tx_env.gas_priority_fee = None;
            tx_env.transact_to = match tx.to {
                TxKind::Call(to) => TransactTo::Call(to),
                TxKind::Create => TransactTo::create(),
            };
            tx_env.value = tx.value;
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
        }
        Transaction::Eip1559(tx) => {
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::from(tx.max_fee_per_gas);
            tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
            tx_env.transact_to = match tx.to {
                TxKind::Call(to) => TransactTo::Call(to),
                TxKind::Create => TransactTo::create(),
            };
            tx_env.value = tx.value;
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
        }
        Transaction::Eip4844(tx) => {
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::from(tx.max_fee_per_gas);
            tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
            tx_env.transact_to = TransactTo::Call(tx.to);
            tx_env.value = tx.value;
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
            tx_env.blob_hashes.clone_from(&tx.blob_versioned_hashes);
            tx_env.max_fee_per_blob_gas = Some(U256::from(tx.max_fee_per_blob_gas));
        }
        #[cfg(feature = "optimism")]
        Transaction::Deposit(tx) => {
            tx_env.access_list.clear();
            tx_env.gas_limit = tx.gas_limit;
            tx_env.gas_price = U256::ZERO;
            tx_env.gas_priority_fee = None;
            match tx.to {
                TxKind::Call(to) => tx_env.transact_to = TransactTo::Call(to),
                TxKind::Create => tx_env.transact_to = TransactTo::create(),
            }
            tx_env.value = tx.value;
            tx_env.data = tx.input.clone();
            tx_env.chain_id = None;
            tx_env.nonce = None;
        }
    }
}

/// Fill transaction environment from a [Transaction], envelope, and the given sender address.
#[cfg(feature = "optimism")]
#[inline(always)]
pub fn fill_op_tx_env<T: AsRef<Transaction>>(
    tx_env: &mut TxEnv,
    transaction: T,
    sender: Address,
    envelope: Bytes,
) {
    fill_tx_env(tx_env, &transaction, sender);
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
    use reth_chainspec::GOERLI;

    #[test]
    fn test_recover_genesis_goerli_signer() {
        // just ensures that `block_coinbase` does not panic on the genesis block
        let chain_spec = GOERLI.clone();
        let header = chain_spec.genesis_header();
        let block_coinbase = block_coinbase(&chain_spec, &header, false);
        assert_eq!(block_coinbase, header.beneficiary);
    }
}
