//! Collection of methods for transaction validation.
use reth_interfaces::{consensus::Error, Result as RethResult};
use reth_primitives::{
    BlockNumber, ChainSpec, Hardfork, Header, Transaction, TransactionSignedEcRecovered, TxEip1559,
    TxEip2930, TxLegacy,
};
use reth_provider::{AccountProvider, HeaderProvider};
use std::collections::{hash_map::Entry, HashMap};

/// Validate a transaction in regards to a block header.
///
/// The only parameter from the header that affects the transaction is `base_fee`.
pub fn validate_transaction_regarding_header(
    transaction: &Transaction,
    chain_spec: &ChainSpec,
    at_block_number: BlockNumber,
    base_fee: Option<u64>,
) -> Result<(), Error> {
    let chain_id = match transaction {
        Transaction::Legacy(TxLegacy { chain_id, .. }) => {
            // EIP-155: Simple replay attack protection: https://eips.ethereum.org/EIPS/eip-155
            if chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(at_block_number) &&
                chain_id.is_some()
            {
                return Err(Error::TransactionOldLegacyChainId)
            }
            *chain_id
        }
        Transaction::Eip2930(TxEip2930 { chain_id, .. }) => {
            // EIP-2930: Optional access lists: https://eips.ethereum.org/EIPS/eip-2930 (New transaction type)
            if !chain_spec.fork(Hardfork::Berlin).active_at_block(at_block_number) {
                return Err(Error::TransactionEip2930Disabled)
            }
            Some(*chain_id)
        }
        Transaction::Eip1559(TxEip1559 {
            chain_id,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            ..
        }) => {
            // EIP-1559: Fee market change for ETH 1.0 chain https://eips.ethereum.org/EIPS/eip-1559
            if !chain_spec.fork(Hardfork::Berlin).active_at_block(at_block_number) {
                return Err(Error::TransactionEip1559Disabled)
            }

            // EIP-1559: add more constraints to the tx validation
            // https://github.com/ethereum/EIPs/pull/3594
            if max_priority_fee_per_gas > max_fee_per_gas {
                return Err(Error::TransactionPriorityFeeMoreThenMaxFee)
            }

            Some(*chain_id)
        }
    };
    if let Some(chain_id) = chain_id {
        if chain_id != chain_spec.chain().id() {
            return Err(Error::TransactionChainId)
        }
    }
    // Check basefee and few checks that are related to that.
    // https://github.com/ethereum/EIPs/pull/3594
    if let Some(base_fee_per_gas) = base_fee {
        if transaction.max_fee_per_gas() < base_fee_per_gas as u128 {
            return Err(Error::TransactionMaxFeeLessThenBaseFee)
        }
    }

    Ok(())
}

/// Iterate over all transactions, validate them against each other and against the block.
/// There is no gas check done as [REVM](https://github.com/bluealloy/revm/blob/fd0108381799662098b7ab2c429ea719d6dfbf28/crates/revm/src/evm_impl.rs#L113-L131) already checks that.
pub fn validate_all_transaction_regarding_block_and_nonces<
    'a,
    Provider: HeaderProvider + AccountProvider,
>(
    transactions: impl Iterator<Item = &'a TransactionSignedEcRecovered>,
    header: &Header,
    provider: Provider,
    chain_spec: &ChainSpec,
) -> RethResult<()> {
    let mut account_nonces = HashMap::new();

    for transaction in transactions {
        validate_transaction_regarding_header(
            transaction,
            chain_spec,
            header.number,
            header.base_fee_per_gas,
        )?;

        // Get nonce, if there is previous transaction from same sender we need
        // to take that nonce.
        let nonce = match account_nonces.entry(transaction.signer()) {
            Entry::Occupied(mut entry) => {
                let nonce = *entry.get();
                *entry.get_mut() += 1;
                nonce
            }
            Entry::Vacant(entry) => {
                let account = provider.basic_account(transaction.signer())?.unwrap_or_default();
                // Signer account shouldn't have bytecode. Presence of bytecode means this is a
                // smartcontract.
                if account.has_bytecode() {
                    return Err(Error::SignerAccountHasBytecode.into())
                }
                let nonce = account.nonce;
                entry.insert(account.nonce + 1);
                nonce
            }
        };

        // check nonce
        if transaction.nonce() != nonce {
            return Err(Error::TransactionNonceNotConsistent.into())
        }
    }

    Ok(())
}
