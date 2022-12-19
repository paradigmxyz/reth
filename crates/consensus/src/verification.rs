//! ALl functions for verification of block
use crate::{config, Config};
use reth_interfaces::{consensus::Error, Result as RethResult};
use reth_primitives::{
    BlockLocked, BlockNumber, Header, SealedHeader, Transaction, TransactionSignedEcRecovered,
    TxEip1559, TxEip2930, TxLegacy, EMPTY_OMMER_ROOT, H256, U256,
};
use reth_provider::{AccountProvider, HeaderProvider};
use std::{
    collections::{hash_map::Entry, HashMap},
    time::SystemTime,
};

/// Validate header standalone
pub fn validate_header_standalone(
    header: &SealedHeader,
    config: &config::Config,
) -> Result<(), Error> {
    // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
    if header.gas_used > header.gas_limit {
        return Err(Error::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used,
            gas_limit: header.gas_limit,
        })
    }

    // Check if timestamp is in future. Clock can drift but this can be consensus issue.
    let present_timestamp =
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
    if header.timestamp > present_timestamp {
        return Err(Error::TimestampIsInFuture { timestamp: header.timestamp, present_timestamp })
    }

    // From yellow papper: extraData: An arbitrary byte array containing data
    // relevant to this block. This must be 32 bytes or fewer; formally Hx.
    if header.extra_data.len() > 32 {
        return Err(Error::ExtraDataExceedsMax { len: header.extra_data.len() })
    }

    // Check if base fee is set.
    if header.number >= config.london_block && header.base_fee_per_gas.is_none() {
        return Err(Error::BaseFeeMissing)
    }

    // EIP-3675: Upgrade consensus to Proof-of-Stake:
    // https://eips.ethereum.org/EIPS/eip-3675#replacing-difficulty-with-0
    if header.number >= config.paris_block {
        if header.difficulty != U256::zero() {
            return Err(Error::TheMergeDifficultyIsNotZero)
        }

        if header.nonce != 0 {
            return Err(Error::TheMergeNonceIsNotZero)
        }

        if header.ommers_hash != EMPTY_OMMER_ROOT {
            return Err(Error::TheMergeOmmerRootIsNotEmpty)
        }

        if header.mix_hash != H256::zero() {
            return Err(Error::TheMergeMixHashIsNotZero)
        }
    }

    Ok(())
}

/// Validate a transaction in regards to a block header.
///
/// The only parameter from the header that affects the transaction is `base_fee`.
pub fn validate_transaction_regarding_header(
    transaction: &Transaction,
    config: &Config,
    at_block_number: BlockNumber,
    base_fee: Option<u64>,
) -> Result<(), Error> {
    let chain_id = match transaction {
        Transaction::Legacy(TxLegacy { chain_id, .. }) => {
            // EIP-155: Simple replay attack protection: https://eips.ethereum.org/EIPS/eip-155
            if config.eip_155_block <= at_block_number && chain_id.is_some() {
                return Err(Error::TransactionOldLegacyChainId)
            }
            *chain_id
        }
        Transaction::Eip2930(TxEip2930 { chain_id, .. }) => {
            // EIP-2930: Optional access lists: https://eips.ethereum.org/EIPS/eip-2930 (New transaction type)
            if config.berlin_block > at_block_number {
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
            if config.berlin_block > at_block_number {
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
        if chain_id != config.chain_id {
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

/// Iterate over all transactions, verify them agains each other and against the block.
/// There is no gas check done as [REVM](https://github.com/bluealloy/revm/blob/fd0108381799662098b7ab2c429ea719d6dfbf28/crates/revm/src/evm_impl.rs#L113-L131) already checks that.
pub fn validate_all_transaction_regarding_block_and_nonces<
    'a,
    Provider: HeaderProvider + AccountProvider,
>(
    transactions: impl Iterator<Item = &'a TransactionSignedEcRecovered>,
    header: &Header,
    provider: Provider,
    config: &Config,
) -> RethResult<()> {
    let mut account_nonces = HashMap::new();

    for transaction in transactions {
        validate_transaction_regarding_header(
            transaction,
            config,
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
                // Signer account shoudn't have bytecode. Presence of bytecode means this is a
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

/// Validate a block without regard for state:
///
/// - Compares the ommer hash in the block header to the block body
/// - Compares the transactions root in the block header to the block body
/// - Pre-execution transaction validation
/// - (Optionally) Compares the receipts root in the block header to the block body
pub fn validate_block_standalone(block: &BlockLocked) -> Result<(), Error> {
    // Check ommers hash
    // TODO(onbjerg): This should probably be accessible directly on [Block]
    let ommers_hash =
        reth_primitives::proofs::calculate_ommers_root(block.ommers.iter().map(|h| h.as_ref()));
    if block.header.ommers_hash != ommers_hash {
        return Err(Error::BodyOmmersHashDiff {
            got: ommers_hash,
            expected: block.header.ommers_hash,
        })
    }

    // Check transaction root
    // TODO(onbjerg): This should probably be accessible directly on [Block]
    let transaction_root = reth_primitives::proofs::calculate_transaction_root(block.body.iter());
    if block.header.transactions_root != transaction_root {
        return Err(Error::BodyTransactionRootDiff {
            got: transaction_root,
            expected: block.header.transactions_root,
        })
    }

    Ok(())
}

/// Calculate base fee for next block. EIP-1559 spec
pub fn calculate_next_block_base_fee(gas_used: u64, gas_limit: u64, base_fee: u64) -> u64 {
    let gas_target = gas_limit / config::EIP1559_ELASTICITY_MULTIPLIER;

    if gas_used == gas_target {
        return base_fee
    }
    if gas_used > gas_target {
        let gas_used_delta = gas_used - gas_target;
        let base_fee_delta = std::cmp::max(
            1,
            base_fee as u128 * gas_used_delta as u128 /
                gas_target as u128 /
                config::EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR as u128,
        );
        base_fee + (base_fee_delta as u64)
    } else {
        let gas_used_delta = gas_target - gas_used;
        let base_fee_per_gas_delta = base_fee as u128 * gas_used_delta as u128 /
            gas_target as u128 /
            config::EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR as u128;

        base_fee.saturating_sub(base_fee_per_gas_delta as u64)
    }
}

/// Validate block in regards to parent
pub fn validate_header_regarding_parent(
    parent: &SealedHeader,
    child: &SealedHeader,
    config: &config::Config,
) -> Result<(), Error> {
    // Parent number is consistent.
    if parent.number + 1 != child.number {
        return Err(Error::ParentBlockNumberMismatch {
            parent_block_number: parent.number,
            block_number: child.number,
        })
    }

    // timestamp in past check
    if child.timestamp < parent.timestamp {
        return Err(Error::TimestampIsInPast {
            parent_timestamp: parent.timestamp,
            timestamp: child.timestamp,
        })
    }

    // difficulty check is done by consensus.
    if config.paris_block > child.number {
        // TODO how this needs to be checked? As ice age did increment it by some formula
    }

    let mut parent_gas_limit = parent.gas_limit;

    // By consensus, gas_limit is multiplied by elasticity (*2) on
    // on exact block that hardfork happens.
    if config.london_block == child.number {
        parent_gas_limit = parent.gas_limit * config::EIP1559_ELASTICITY_MULTIPLIER;
    }

    // Check gas limit, max diff between child/parent gas_limit should be  max_diff=parent_gas/1024
    if child.gas_limit > parent_gas_limit {
        if child.gas_limit - parent_gas_limit >= parent_gas_limit / 1024 {
            return Err(Error::GasLimitInvalidIncrease {
                parent_gas_limit,
                child_gas_limit: child.gas_limit,
            })
        }
    } else if parent_gas_limit - child.gas_limit >= parent_gas_limit / 1024 {
        return Err(Error::GasLimitInvalidDecrease {
            parent_gas_limit,
            child_gas_limit: child.gas_limit,
        })
    }

    // EIP-1559 check base fee
    if child.number >= config.london_block {
        let base_fee = child.base_fee_per_gas.ok_or(Error::BaseFeeMissing)?;

        let expected_base_fee = if config.london_block == child.number {
            config::EIP1559_INITIAL_BASE_FEE
        } else {
            // This BaseFeeMissing will not happen as previous blocks are checked to have them.
            calculate_next_block_base_fee(
                parent.gas_used,
                parent.gas_limit,
                parent.base_fee_per_gas.ok_or(Error::BaseFeeMissing)?,
            )
        };
        if expected_base_fee != base_fee {
            return Err(Error::BaseFeeDiff { expected: expected_base_fee, got: base_fee })
        }
    }

    Ok(())
}

/// Validate block in regards to chain (parent)
///
/// Checks:
///  If we already know the block.
///  If parent is known
///
/// Returns parent block header  
pub fn validate_block_regarding_chain<PROV: HeaderProvider>(
    block: &BlockLocked,
    provider: &PROV,
) -> RethResult<SealedHeader> {
    let hash = block.header.hash();

    // Check if block is known.
    if provider.is_known(&hash)? {
        return Err(Error::BlockKnown { hash, number: block.header.number }.into())
    }

    // Check if parent is known.
    let parent = provider
        .header(&block.parent_hash)?
        .ok_or(Error::ParentUnknown { hash: block.parent_hash })?;

    // Return parent header.
    Ok(parent.seal())
}

/// Full validation of block before execution.
pub fn full_validation<Provider: HeaderProvider + AccountProvider>(
    block: &BlockLocked,
    provider: Provider,
    config: &Config,
) -> RethResult<()> {
    validate_header_standalone(&block.header, config)?;
    validate_block_standalone(block)?;
    let parent = validate_block_regarding_chain(block, &provider)?;
    validate_header_regarding_parent(&parent, &block.header, config)?;

    // NOTE: depending on the need of the stages, recovery could be done in different place.
    let transactions = block
        .body
        .iter()
        .map(|tx| tx.try_ecrecovered().ok_or(Error::TransactionSignerRecoveryError))
        .collect::<Result<Vec<_>, _>>()?;

    validate_all_transaction_regarding_block_and_nonces(
        transactions.iter(),
        &block.header,
        provider,
        config,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use reth_interfaces::Result;
    use reth_primitives::{
        hex_literal::hex, Account, Address, BlockHash, Bytes, Header, Signature, TransactionKind,
        TransactionSigned,
    };

    use super::*;

    #[test]
    fn calculate_base_fee_success() {
        let base_fee = [
            1000000000, 1000000000, 1000000000, 1072671875, 1059263476, 1049238967, 1049238967, 0,
            1, 2,
        ];
        let gas_used = [
            10000000, 10000000, 10000000, 9000000, 10001000, 0, 10000000, 10000000, 10000000,
            10000000,
        ];
        let gas_limit = [
            10000000, 12000000, 14000000, 10000000, 14000000, 2000000, 18000000, 18000000,
            18000000, 18000000,
        ];
        let next_base_fee = [
            1125000000, 1083333333, 1053571428, 1179939062, 1116028649, 918084097, 1063811730, 1,
            2, 3,
        ];

        for i in 0..base_fee.len() {
            assert_eq!(
                next_base_fee[i],
                calculate_next_block_base_fee(gas_used[i], gas_limit[i], base_fee[i])
            );
        }
    }

    struct Provider {
        is_known: bool,
        parent: Option<Header>,
        account: Option<Account>,
    }

    impl Provider {
        /// New provider with parent
        fn new(parent: Option<Header>) -> Self {
            Self { is_known: false, parent, account: None }
        }
        /// New provider where is_known is always true
        fn new_known() -> Self {
            Self { is_known: true, parent: None, account: None }
        }
    }

    impl AccountProvider for Provider {
        fn basic_account(&self, _address: Address) -> Result<Option<Account>> {
            Ok(self.account)
        }
    }

    impl HeaderProvider for Provider {
        fn is_known(&self, _block_hash: &BlockHash) -> Result<bool> {
            Ok(self.is_known)
        }

        fn header(&self, _block_number: &BlockHash) -> Result<Option<Header>> {
            Ok(self.parent.clone())
        }

        fn header_by_number(&self, _num: u64) -> Result<Option<Header>> {
            Ok(self.parent.clone())
        }
    }

    fn mock_tx(nonce: u64) -> TransactionSignedEcRecovered {
        let request = Transaction::Eip2930(TxEip2930 {
            chain_id: 1u64,
            nonce,
            gas_price: 0x28f000fff,
            gas_limit: 10,
            to: TransactionKind::Call(Address::default()),
            value: 3,
            input: Bytes::from(vec![1, 2]),
            access_list: Default::default(),
        });

        let signature = Signature { odd_y_parity: true, r: U256::default(), s: U256::default() };

        let tx = TransactionSigned::from_transaction_and_signature(request, signature);
        let signer = Address::zero();
        TransactionSignedEcRecovered::from_signed_transaction(tx, signer)
    }
    /// got test block
    fn mock_block() -> (BlockLocked, Header) {
        // https://etherscan.io/block/15867168 where transaction root and receipts root are cleared
        // empty merkle tree: 0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421

        let header = Header {
            parent_hash: hex!("859fad46e75d9be177c2584843501f2270c7e5231711e90848290d12d7c6dcdd").into(),
            ommers_hash: hex!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347").into(),
            beneficiary: hex!("4675c7e5baafbffbca748158becba61ef3b0a263").into(),
            state_root: hex!("8337403406e368b3e40411138f4868f79f6d835825d55fd0c2f6e17b1a3948e9").into(),
            transactions_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
            receipts_root: hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into(),
            logs_bloom: hex!("002400000000004000220000800002000000000000000000000000000000100000000000000000100000000000000021020000000800000006000000002100040000000c0004000000000008000008200000000000000000000000008000000001040000020000020000002000000800000002000020000000022010000000000000010002001000000000020200000000000001000200880000004000000900020000000000020000000040000000000000000000000000000080000000000001000002000000000000012000200020000000000000001000000000000020000010321400000000100000000000000000000000000000400000000000000000").into(),
            difficulty: 0x00.into(), // total difficulty: 0xc70d815d562d3cfa955).into(),
            number: 0xf21d20,
            gas_limit: 0x1c9c380,
            gas_used: 0x6e813,
            timestamp: 0x635f9657,
            extra_data: hex!("")[..].into(),
            mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
            nonce: 0x0000000000000000,
            base_fee_per_gas: 0x28f0001df.into(),
        };
        // size: 0x9b5

        let mut parent = header.clone();
        parent.gas_used = 17763076;
        parent.gas_limit = 30000000;
        parent.base_fee_per_gas = Some(0x28041f7f5);
        parent.number -= 1;

        let ommers = Vec::new();
        let body = Vec::new();

        (BlockLocked { header: header.seal(), body, ommers }, parent)
    }

    #[test]
    fn sanity_check() {
        let (block, parent) = mock_block();
        let provider = Provider::new(Some(parent));
        let config = Config::default();

        assert_eq!(full_validation(&block, provider, &config), Ok(()), "Validation should pass");
    }

    #[test]
    fn validate_known_block() {
        let (block, _) = mock_block();
        let provider = Provider::new_known();
        let config = Config::default();

        assert_eq!(
            full_validation(&block, provider, &config),
            Err(Error::BlockKnown { hash: block.hash(), number: block.number }.into()),
            "Should fail with error"
        );
    }

    #[test]
    fn sanity_tx_nonce_check() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(0);
        let tx2 = mock_tx(1);
        let provider = Provider::new_known();
        let config = Config::default();

        let txs = vec![tx1, tx2];
        validate_all_transaction_regarding_block_and_nonces(
            txs.iter(),
            &block.header,
            provider,
            &config,
        )
        .expect("To Pass");
    }

    #[test]
    fn nonce_gap_in_first_transaction() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(1);
        let provider = Provider::new_known();
        let config = Config::default();

        let txs = vec![tx1];
        assert_eq!(
            validate_all_transaction_regarding_block_and_nonces(
                txs.iter(),
                &block.header,
                provider,
                &config,
            ),
            Err(Error::TransactionNonceNotConsistent.into())
        )
    }

    #[test]
    fn nonce_gap_on_second_tx_from_same_signer() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(0);
        let tx2 = mock_tx(3);
        let provider = Provider::new_known();
        let config = Config::default();

        let txs = vec![tx1, tx2];
        assert_eq!(
            validate_all_transaction_regarding_block_and_nonces(
                txs.iter(),
                &block.header,
                provider,
                &config,
            ),
            Err(Error::TransactionNonceNotConsistent.into())
        );
    }
}
