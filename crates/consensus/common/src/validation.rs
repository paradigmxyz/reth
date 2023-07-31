//! Collection of methods for block validation.
use reth_interfaces::{consensus::ConsensusError, Result as RethResult};
use reth_primitives::{
    blobfee::calculate_excess_blob_gas,
    constants::{
        self,
        eip4844::{DATA_GAS_PER_BLOB, MAX_DATA_GAS_PER_BLOCK},
    },
    BlockNumber, ChainSpec, Hardfork, Header, InvalidTransactionError, SealedBlock, SealedHeader,
    Transaction, TransactionSignedEcRecovered, TxEip1559, TxEip2930, TxLegacy,
};
use reth_provider::{AccountReader, HeaderProvider, WithdrawalsProvider};
use std::collections::{hash_map::Entry, HashMap};

/// Validate header standalone
pub fn validate_header_standalone(
    header: &SealedHeader,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    // Gas used needs to be less then gas limit. Gas used is going to be check after execution.
    if header.gas_used > header.gas_limit {
        return Err(ConsensusError::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used,
            gas_limit: header.gas_limit,
        })
    }

    // Check if base fee is set.
    if chain_spec.fork(Hardfork::London).active_at_block(header.number) &&
        header.base_fee_per_gas.is_none()
    {
        return Err(ConsensusError::BaseFeeMissing)
    }

    // EIP-4895: Beacon chain push withdrawals as operations
    if chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(header.timestamp) &&
        header.withdrawals_root.is_none()
    {
        return Err(ConsensusError::WithdrawalsRootMissing)
    } else if !chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(header.timestamp) &&
        header.withdrawals_root.is_some()
    {
        return Err(ConsensusError::WithdrawalsRootUnexpected)
    }

    // Ensures that EIP-4844 fields are valid once cancun is active.
    if chain_spec.fork(Hardfork::Cancun).active_at_timestamp(header.timestamp) {
        validate_4844_header_standalone(header)?;
    } else if header.blob_gas_used.is_some() {
        return Err(ConsensusError::BlobGasUsedUnexpected)
    } else if header.excess_blob_gas.is_some() {
        return Err(ConsensusError::ExcessBlobGasUnexpected)
    }

    Ok(())
}

/// Validate a transaction in regards to a block header.
///
/// The only parameter from the header that affects the transaction is `base_fee`.
pub fn validate_transaction_regarding_header(
    transaction: &Transaction,
    chain_spec: &ChainSpec,
    at_block_number: BlockNumber,
    base_fee: Option<u64>,
) -> Result<(), ConsensusError> {
    let chain_id = match transaction {
        Transaction::Legacy(TxLegacy { chain_id, .. }) => {
            // EIP-155: Simple replay attack protection: https://eips.ethereum.org/EIPS/eip-155
            if chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(at_block_number) &&
                chain_id.is_some()
            {
                return Err(InvalidTransactionError::OldLegacyChainId.into())
            }
            *chain_id
        }
        Transaction::Eip2930(TxEip2930 { chain_id, .. }) => {
            // EIP-2930: Optional access lists: https://eips.ethereum.org/EIPS/eip-2930 (New transaction type)
            if !chain_spec.fork(Hardfork::Berlin).active_at_block(at_block_number) {
                return Err(InvalidTransactionError::Eip2930Disabled.into())
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
                return Err(InvalidTransactionError::Eip1559Disabled.into())
            }

            // EIP-1559: add more constraints to the tx validation
            // https://github.com/ethereum/EIPs/pull/3594
            if max_priority_fee_per_gas > max_fee_per_gas {
                return Err(InvalidTransactionError::TipAboveFeeCap.into())
            }

            Some(*chain_id)
        }
    };
    if let Some(chain_id) = chain_id {
        if chain_id != chain_spec.chain().id() {
            return Err(InvalidTransactionError::ChainIdMismatch.into())
        }
    }
    // Check basefee and few checks that are related to that.
    // https://github.com/ethereum/EIPs/pull/3594
    if let Some(base_fee_per_gas) = base_fee {
        if transaction.max_fee_per_gas() < base_fee_per_gas as u128 {
            return Err(InvalidTransactionError::FeeCapTooLow.into())
        }
    }

    Ok(())
}

/// Iterate over all transactions, validate them against each other and against the block.
/// There is no gas check done as [REVM](https://github.com/bluealloy/revm/blob/fd0108381799662098b7ab2c429ea719d6dfbf28/crates/revm/src/evm_impl.rs#L113-L131) already checks that.
pub fn validate_all_transaction_regarding_block_and_nonces<
    'a,
    Provider: HeaderProvider + AccountReader,
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
                    return Err(ConsensusError::from(
                        InvalidTransactionError::SignerAccountHasBytecode,
                    )
                    .into())
                }
                let nonce = account.nonce;
                entry.insert(account.nonce + 1);
                nonce
            }
        };

        // check nonce
        if transaction.nonce() != nonce {
            return Err(ConsensusError::from(InvalidTransactionError::NonceNotConsistent).into())
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
pub fn validate_block_standalone(
    block: &SealedBlock,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    // Check ommers hash
    // TODO(onbjerg): This should probably be accessible directly on [Block]
    let ommers_hash = reth_primitives::proofs::calculate_ommers_root(&block.ommers);
    if block.header.ommers_hash != ommers_hash {
        return Err(ConsensusError::BodyOmmersHashDiff {
            got: ommers_hash,
            expected: block.header.ommers_hash,
        })
    }

    // Check transaction root
    // TODO(onbjerg): This should probably be accessible directly on [Block]
    let transaction_root = reth_primitives::proofs::calculate_transaction_root(&block.body);
    if block.header.transactions_root != transaction_root {
        return Err(ConsensusError::BodyTransactionRootDiff {
            got: transaction_root,
            expected: block.header.transactions_root,
        })
    }

    // EIP-4895: Beacon chain push withdrawals as operations
    if chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(block.timestamp) {
        let withdrawals =
            block.withdrawals.as_ref().ok_or(ConsensusError::BodyWithdrawalsMissing)?;
        let withdrawals_root = reth_primitives::proofs::calculate_withdrawals_root(withdrawals);
        let header_withdrawals_root =
            block.withdrawals_root.as_ref().ok_or(ConsensusError::WithdrawalsRootMissing)?;
        if withdrawals_root != *header_withdrawals_root {
            return Err(ConsensusError::BodyWithdrawalsRootDiff {
                got: withdrawals_root,
                expected: *header_withdrawals_root,
            })
        }

        // Validate that withdrawal index is monotonically increasing within a block.
        if let Some(first) = withdrawals.first() {
            let mut prev_index = first.index;
            for withdrawal in withdrawals.iter().skip(1) {
                let expected = prev_index + 1;
                if expected != withdrawal.index {
                    return Err(ConsensusError::WithdrawalIndexInvalid {
                        got: withdrawal.index,
                        expected,
                    })
                }
                prev_index = withdrawal.index;
            }
        }
    }

    Ok(())
}

/// Validate block in regards to parent
pub fn validate_header_regarding_parent(
    parent: &SealedHeader,
    child: &SealedHeader,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    // Parent number is consistent.
    if parent.number + 1 != child.number {
        return Err(ConsensusError::ParentBlockNumberMismatch {
            parent_block_number: parent.number,
            block_number: child.number,
        })
    }

    if parent.hash != child.parent_hash {
        return Err(ConsensusError::ParentHashMismatch {
            expected_parent_hash: parent.hash,
            got_parent_hash: child.parent_hash,
        })
    }

    // timestamp in past check
    if child.timestamp <= parent.timestamp {
        return Err(ConsensusError::TimestampIsInPast {
            parent_timestamp: parent.timestamp,
            timestamp: child.timestamp,
        })
    }

    // TODO Check difficulty increment between parent and child
    // Ace age did increment it by some formula that we need to follow.

    let mut parent_gas_limit = parent.gas_limit;

    // By consensus, gas_limit is multiplied by elasticity (*2) on
    // on exact block that hardfork happens.
    if chain_spec.fork(Hardfork::London).transitions_at_block(child.number) {
        parent_gas_limit = parent.gas_limit * constants::EIP1559_ELASTICITY_MULTIPLIER;
    }

    // Check gas limit, max diff between child/parent gas_limit should be  max_diff=parent_gas/1024
    if child.gas_limit > parent_gas_limit {
        if child.gas_limit - parent_gas_limit >= parent_gas_limit / 1024 {
            return Err(ConsensusError::GasLimitInvalidIncrease {
                parent_gas_limit,
                child_gas_limit: child.gas_limit,
            })
        }
    } else if parent_gas_limit - child.gas_limit >= parent_gas_limit / 1024 {
        return Err(ConsensusError::GasLimitInvalidDecrease {
            parent_gas_limit,
            child_gas_limit: child.gas_limit,
        })
    }

    // EIP-1559 check base fee
    if chain_spec.fork(Hardfork::London).active_at_block(child.number) {
        let base_fee = child.base_fee_per_gas.ok_or(ConsensusError::BaseFeeMissing)?;

        let expected_base_fee =
            if chain_spec.fork(Hardfork::London).transitions_at_block(child.number) {
                constants::EIP1559_INITIAL_BASE_FEE
            } else {
                // This BaseFeeMissing will not happen as previous blocks are checked to have them.
                parent.next_block_base_fee().ok_or(ConsensusError::BaseFeeMissing)?
            };
        if expected_base_fee != base_fee {
            return Err(ConsensusError::BaseFeeDiff { expected: expected_base_fee, got: base_fee })
        }
    }

    // ensure that the blob gas fields for this block
    if chain_spec.fork(Hardfork::Cancun).active_at_timestamp(child.timestamp) {
        validate_4844_header_with_parent(parent, child)?;
    }

    Ok(())
}

/// Validate block in regards to chain (parent)
///
/// Checks:
///  If we already know the block.
///  If parent is known
///  If withdrawals are valid
///
/// Returns parent block header
pub fn validate_block_regarding_chain<PROV: HeaderProvider + WithdrawalsProvider>(
    block: &SealedBlock,
    provider: &PROV,
) -> RethResult<SealedHeader> {
    let hash = block.header.hash();

    // Check if block is known.
    if provider.is_known(&hash)? {
        return Err(ConsensusError::BlockKnown { hash, number: block.header.number }.into())
    }

    // Check if parent is known.
    let parent = provider
        .header(&block.parent_hash)?
        .ok_or(ConsensusError::ParentUnknown { hash: block.parent_hash })?;

    // Check if withdrawals are valid.
    if let Some(withdrawals) = &block.withdrawals {
        if !withdrawals.is_empty() {
            let latest_withdrawal = provider.latest_withdrawal()?;
            match latest_withdrawal {
                Some(withdrawal) => {
                    if withdrawal.index + 1 != withdrawals.first().unwrap().index {
                        return Err(ConsensusError::WithdrawalIndexInvalid {
                            got: withdrawals.first().unwrap().index,
                            expected: withdrawal.index + 1,
                        }
                        .into())
                    }
                }
                None => {
                    if withdrawals.first().unwrap().index != 0 {
                        return Err(ConsensusError::WithdrawalIndexInvalid {
                            got: withdrawals.first().unwrap().index,
                            expected: 0,
                        }
                        .into())
                    }
                }
            }
        }
    }

    // Return parent header.
    Ok(parent.seal(block.parent_hash))
}

/// Full validation of block before execution.
pub fn full_validation<Provider: HeaderProvider + AccountReader + WithdrawalsProvider>(
    block: &SealedBlock,
    provider: Provider,
    chain_spec: &ChainSpec,
) -> RethResult<()> {
    validate_header_standalone(&block.header, chain_spec)?;
    validate_block_standalone(block, chain_spec)?;
    let parent = validate_block_regarding_chain(block, &provider)?;
    validate_header_regarding_parent(&parent, &block.header, chain_spec)?;

    // NOTE: depending on the need of the stages, recovery could be done in different place.
    let transactions = block
        .body
        .iter()
        .map(|tx| tx.try_ecrecovered().ok_or(ConsensusError::TransactionSignerRecoveryError))
        .collect::<Result<Vec<_>, _>>()?;

    validate_all_transaction_regarding_block_and_nonces(
        transactions.iter(),
        &block.header,
        provider,
        chain_spec,
    )?;
    Ok(())
}

/// Validates that the EIP-4844 header fields are correct with respect to the parent block. This
/// ensures that the `blob_gas_used` and `excess_blob_gas` fields exist in the child header, and
/// that the `excess_blob_gas` field matches the expected `excess_blob_gas` calculated from the
/// parent header fields.
pub fn validate_4844_header_with_parent(
    parent: &SealedHeader,
    child: &SealedHeader,
) -> Result<(), ConsensusError> {
    // From [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension):
    //
    // > For the first post-fork block, both parent.blob_gas_used and parent.excess_blob_gas
    // > are evaluated as 0.
    //
    // This means in the first post-fork block, calculate_excess_blob_gas will return 0.
    let parent_blob_gas_used = parent.blob_gas_used.unwrap_or(0);
    let parent_excess_blob_gas = parent.excess_blob_gas.unwrap_or(0);

    if child.blob_gas_used.is_none() {
        return Err(ConsensusError::BlobGasUsedMissing)
    }
    let excess_blob_gas = child.excess_blob_gas.ok_or(ConsensusError::ExcessBlobGasMissing)?;

    let expected_excess_blob_gas =
        calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used);
    if expected_excess_blob_gas != excess_blob_gas {
        return Err(ConsensusError::ExcessBlobGasDiff {
            expected: expected_excess_blob_gas,
            got: excess_blob_gas,
            parent_excess_blob_gas,
            parent_blob_gas_used,
        })
    }

    Ok(())
}

/// Validates that the EIP-4844 header fields exist and conform to the spec. This ensures that:
///
///  * `blob_gas_used` exists as a header field
///  * `excess_blob_gas` exists as a header field
///  * `blob_gas_used` is less than or equal to `MAX_DATA_GAS_PER_BLOCK`
///  * `blob_gas_used` is a multiple of `DATA_GAS_PER_BLOB`
pub fn validate_4844_header_standalone(header: &SealedHeader) -> Result<(), ConsensusError> {
    let blob_gas_used = header.blob_gas_used.ok_or(ConsensusError::BlobGasUsedMissing)?;

    if header.excess_blob_gas.is_none() {
        return Err(ConsensusError::ExcessBlobGasMissing)
    }

    if blob_gas_used > MAX_DATA_GAS_PER_BLOCK {
        return Err(ConsensusError::BlobGasUsedExceedsMaxBlobGasPerBlock {
            blob_gas_used,
            max_blob_gas_per_block: MAX_DATA_GAS_PER_BLOCK,
        })
    }

    if blob_gas_used % DATA_GAS_PER_BLOB != 0 {
        return Err(ConsensusError::BlobGasUsedNotMultipleOfBlobGasPerBlob {
            blob_gas_used,
            blob_gas_per_blob: DATA_GAS_PER_BLOB,
        })
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use mockall::mock;
    use reth_interfaces::{Error::Consensus, Result};
    use reth_primitives::{
        hex_literal::hex, proofs, Account, Address, BlockHash, BlockHashOrNumber, Bytes,
        ChainSpecBuilder, Header, Signature, TransactionKind, TransactionSigned, Withdrawal,
        MAINNET, U256,
    };
    use std::ops::RangeBounds;

    mock! {
        WithdrawalsProvider {}

        impl WithdrawalsProvider for WithdrawalsProvider {
            fn latest_withdrawal(&self) -> Result<Option<Withdrawal>> ;

            fn withdrawals_by_block(
                &self,
                _id: BlockHashOrNumber,
                _timestamp: u64,
            ) -> RethResult<Option<Vec<Withdrawal>>> ;
        }
    }

    struct Provider {
        is_known: bool,
        parent: Option<Header>,
        account: Option<Account>,
        withdrawals_provider: MockWithdrawalsProvider,
    }

    impl Provider {
        /// New provider with parent
        fn new(parent: Option<Header>) -> Self {
            Self {
                is_known: false,
                parent,
                account: None,
                withdrawals_provider: MockWithdrawalsProvider::new(),
            }
        }
        /// New provider where is_known is always true
        fn new_known() -> Self {
            Self {
                is_known: true,
                parent: None,
                account: None,
                withdrawals_provider: MockWithdrawalsProvider::new(),
            }
        }
    }

    impl AccountReader for Provider {
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

        fn header_td(&self, _hash: &BlockHash) -> Result<Option<U256>> {
            Ok(None)
        }

        fn header_td_by_number(&self, _number: BlockNumber) -> Result<Option<U256>> {
            Ok(None)
        }

        fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
            Ok(vec![])
        }

        fn sealed_headers_range(
            &self,
            _range: impl RangeBounds<BlockNumber>,
        ) -> Result<Vec<SealedHeader>> {
            Ok(vec![])
        }

        fn sealed_header(&self, _block_number: BlockNumber) -> Result<Option<SealedHeader>> {
            Ok(None)
        }
    }

    impl WithdrawalsProvider for Provider {
        fn withdrawals_by_block(
            &self,
            _id: BlockHashOrNumber,
            _timestamp: u64,
        ) -> RethResult<Option<Vec<Withdrawal>>> {
            self.withdrawals_provider.withdrawals_by_block(_id, _timestamp)
        }

        fn latest_withdrawal(&self) -> Result<Option<Withdrawal>> {
            self.withdrawals_provider.latest_withdrawal()
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
    fn mock_block() -> (SealedBlock, Header) {
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
            difficulty: U256::ZERO, // total difficulty: 0xc70d815d562d3cfa955).into(),
            number: 0xf21d20,
            gas_limit: 0x1c9c380,
            gas_used: 0x6e813,
            timestamp: 0x635f9657,
            extra_data: hex!("")[..].into(),
            mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
            nonce: 0x0000000000000000,
            base_fee_per_gas: 0x28f0001df.into(),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
        };
        // size: 0x9b5

        let mut parent = header.clone();
        parent.gas_used = 17763076;
        parent.gas_limit = 30000000;
        parent.base_fee_per_gas = Some(0x28041f7f5);
        parent.number -= 1;
        parent.timestamp -= 1;

        let ommers = Vec::new();
        let body = Vec::new();

        (SealedBlock { header: header.seal_slow(), body, ommers, withdrawals: None }, parent)
    }

    #[test]
    fn sanity_check() {
        let (block, parent) = mock_block();
        let provider = Provider::new(Some(parent));

        assert_eq!(full_validation(&block, provider, &MAINNET), Ok(()), "Validation should pass");
    }

    #[test]
    fn validate_known_block() {
        let (block, _) = mock_block();
        let provider = Provider::new_known();

        assert_eq!(
            full_validation(&block, provider, &MAINNET),
            Err(ConsensusError::BlockKnown { hash: block.hash(), number: block.number }.into()),
            "Should fail with error"
        );
    }

    #[test]
    fn sanity_tx_nonce_check() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(0);
        let tx2 = mock_tx(1);
        let provider = Provider::new_known();

        let txs = vec![tx1, tx2];
        validate_all_transaction_regarding_block_and_nonces(
            txs.iter(),
            &block.header,
            provider,
            &MAINNET,
        )
        .expect("To Pass");
    }

    #[test]
    fn nonce_gap_in_first_transaction() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(1);
        let provider = Provider::new_known();

        let txs = vec![tx1];
        assert_eq!(
            validate_all_transaction_regarding_block_and_nonces(
                txs.iter(),
                &block.header,
                provider,
                &MAINNET,
            ),
            Err(ConsensusError::from(InvalidTransactionError::NonceNotConsistent).into())
        )
    }

    #[test]
    fn nonce_gap_on_second_tx_from_same_signer() {
        let (block, _) = mock_block();
        let tx1 = mock_tx(0);
        let tx2 = mock_tx(3);
        let provider = Provider::new_known();

        let txs = vec![tx1, tx2];
        assert_eq!(
            validate_all_transaction_regarding_block_and_nonces(
                txs.iter(),
                &block.header,
                provider,
                &MAINNET,
            ),
            Err(ConsensusError::from(InvalidTransactionError::NonceNotConsistent).into())
        );
    }

    #[test]
    fn valid_withdrawal_index() {
        let chain_spec = ChainSpecBuilder::mainnet().shanghai_activated().build();

        let create_block_with_withdrawals = |indexes: &[u64]| {
            let withdrawals = indexes
                .iter()
                .map(|idx| Withdrawal { index: *idx, ..Default::default() })
                .collect::<Vec<_>>();
            SealedBlock {
                header: Header {
                    withdrawals_root: Some(proofs::calculate_withdrawals_root(&withdrawals)),
                    ..Default::default()
                }
                .seal_slow(),
                withdrawals: Some(withdrawals),
                ..Default::default()
            }
        };

        // Single withdrawal
        let block = create_block_with_withdrawals(&[1]);
        assert_eq!(validate_block_standalone(&block, &chain_spec), Ok(()));

        // Multiple increasing withdrawals
        let block = create_block_with_withdrawals(&[1, 2, 3]);
        assert_eq!(validate_block_standalone(&block, &chain_spec), Ok(()));
        let block = create_block_with_withdrawals(&[5, 6, 7, 8, 9]);
        assert_eq!(validate_block_standalone(&block, &chain_spec), Ok(()));

        // Invalid withdrawal index
        let block = create_block_with_withdrawals(&[100, 102]);
        assert_matches!(
            validate_block_standalone(&block, &chain_spec),
            Err(ConsensusError::WithdrawalIndexInvalid { .. })
        );
        let block = create_block_with_withdrawals(&[5, 6, 7, 9]);
        assert_matches!(
            validate_block_standalone(&block, &chain_spec),
            Err(ConsensusError::WithdrawalIndexInvalid { .. })
        );

        let (_, parent) = mock_block();
        let mut provider = Provider::new(Some(parent.clone()));
        // Withdrawal index should be 0 if there are no withdrawals in the chain
        let block = create_block_with_withdrawals(&[1, 2, 3]);
        provider.withdrawals_provider.expect_latest_withdrawal().return_const(Ok(None));
        assert_matches!(
            validate_block_regarding_chain(&block, &provider),
            Err(Consensus(ConsensusError::WithdrawalIndexInvalid { got: 1, expected: 0 }))
        );
        let block = create_block_with_withdrawals(&[0, 1, 2]);
        let res = validate_block_regarding_chain(&block, &provider);
        assert!(res.is_ok());

        // Withdrawal index should be the last withdrawal index + 1
        let mut provider = Provider::new(Some(parent));
        let block = create_block_with_withdrawals(&[4, 5, 6]);
        provider
            .withdrawals_provider
            .expect_latest_withdrawal()
            .return_const(Ok(Some(Withdrawal { index: 2, ..Default::default() })));
        assert_matches!(
            validate_block_regarding_chain(&block, &provider),
            Err(Consensus(ConsensusError::WithdrawalIndexInvalid { got: 4, expected: 3 }))
        );

        let block = create_block_with_withdrawals(&[3, 4, 5]);
        provider
            .withdrawals_provider
            .expect_latest_withdrawal()
            .return_const(Ok(Some(Withdrawal { index: 2, ..Default::default() })));
        let res = validate_block_regarding_chain(&block, &provider);
        assert!(res.is_ok());
    }

    #[test]
    fn shanghai_block_zero_withdrawals() {
        // ensures that if shanghai is activated, and we include a block with a withdrawals root,
        // that the header is valid
        let chain_spec = ChainSpecBuilder::mainnet().shanghai_activated().build();

        let header = Header {
            base_fee_per_gas: Some(1337u64),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            ..Default::default()
        }
        .seal_slow();

        assert_eq!(validate_header_standalone(&header, &chain_spec), Ok(()));
    }
}
