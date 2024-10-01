//! Collection of methods for block validation.

use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_consensus::ConsensusError;
use reth_primitives::{
    constants::{
        eip4844::{DATA_GAS_PER_BLOB, MAX_DATA_GAS_PER_BLOCK},
        MAXIMUM_EXTRA_DATA_SIZE,
    },
    EthereumHardfork, GotExpected, Header, SealedBlock, SealedHeader,
};
use revm_primitives::calc_excess_blob_gas;

/// Gas used needs to be less than gas limit. Gas used is going to be checked after execution.
#[inline]
pub const fn validate_header_gas(header: &Header) -> Result<(), ConsensusError> {
    if header.gas_used > header.gas_limit {
        return Err(ConsensusError::HeaderGasUsedExceedsGasLimit {
            gas_used: header.gas_used,
            gas_limit: header.gas_limit,
        })
    }
    Ok(())
}

/// Ensure the EIP-1559 base fee is set if the London hardfork is active.
#[inline]
pub fn validate_header_base_fee<ChainSpec: EthereumHardforks>(
    header: &Header,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    if chain_spec.is_fork_active_at_block(EthereumHardfork::London, header.number) &&
        header.base_fee_per_gas.is_none()
    {
        return Err(ConsensusError::BaseFeeMissing)
    }
    Ok(())
}

/// Validate that withdrawals are present in Shanghai
///
/// See [EIP-4895]: Beacon chain push withdrawals as operations
///
/// [EIP-4895]: https://eips.ethereum.org/EIPS/eip-4895
#[inline]
pub fn validate_shanghai_withdrawals(block: &SealedBlock) -> Result<(), ConsensusError> {
    let withdrawals =
        block.body.withdrawals.as_ref().ok_or(ConsensusError::BodyWithdrawalsMissing)?;
    let withdrawals_root = reth_primitives::proofs::calculate_withdrawals_root(withdrawals);
    let header_withdrawals_root =
        block.withdrawals_root.as_ref().ok_or(ConsensusError::WithdrawalsRootMissing)?;
    if withdrawals_root != *header_withdrawals_root {
        return Err(ConsensusError::BodyWithdrawalsRootDiff(
            GotExpected { got: withdrawals_root, expected: *header_withdrawals_root }.into(),
        ));
    }
    Ok(())
}

/// Validate that blob gas is present in the block if Cancun is active.
///
/// See [EIP-4844]: Shard Blob Transactions
///
/// [EIP-4844]: https://eips.ethereum.org/EIPS/eip-4844
#[inline]
pub fn validate_cancun_gas(block: &SealedBlock) -> Result<(), ConsensusError> {
    // Check that the blob gas used in the header matches the sum of the blob gas used by each
    // blob tx
    let header_blob_gas_used = block.blob_gas_used.ok_or(ConsensusError::BlobGasUsedMissing)?;
    let total_blob_gas = block.blob_gas_used();
    if total_blob_gas != header_blob_gas_used {
        return Err(ConsensusError::BlobGasUsedDiff(GotExpected {
            got: header_blob_gas_used,
            expected: total_blob_gas,
        }));
    }
    Ok(())
}

/// Validate that requests root is present if Prague is active.
///
/// See [EIP-7685]: General purpose execution layer requests
///
/// [EIP-7685]: https://eips.ethereum.org/EIPS/eip-7685
#[inline]
pub fn validate_prague_request(block: &SealedBlock) -> Result<(), ConsensusError> {
    let requests_root =
        block.body.calculate_requests_root().ok_or(ConsensusError::BodyRequestsMissing)?;
    let header_requests_root = block.requests_root.ok_or(ConsensusError::RequestsRootMissing)?;
    if requests_root != *header_requests_root {
        return Err(ConsensusError::BodyRequestsRootDiff(
            GotExpected { got: requests_root, expected: header_requests_root }.into(),
        ));
    }
    Ok(())
}

/// Validate a block without regard for state:
///
/// - Compares the ommer hash in the block header to the block body
/// - Compares the transactions root in the block header to the block body
/// - Pre-execution transaction validation
/// - (Optionally) Compares the receipts root in the block header to the block body
pub fn validate_block_pre_execution<ChainSpec: EthereumHardforks>(
    block: &SealedBlock,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    // Check ommers hash
    let ommers_hash = block.body.calculate_ommers_root();
    if block.header.ommers_hash != ommers_hash {
        return Err(ConsensusError::BodyOmmersHashDiff(
            GotExpected { got: ommers_hash, expected: block.header.ommers_hash }.into(),
        ))
    }

    // Check transaction root
    if let Err(error) = block.ensure_transaction_root_valid() {
        return Err(ConsensusError::BodyTransactionRootDiff(error.into()))
    }

    // EIP-4895: Beacon chain push withdrawals as operations
    if chain_spec.is_shanghai_active_at_timestamp(block.timestamp) {
        validate_shanghai_withdrawals(block)?;
    }

    if chain_spec.is_cancun_active_at_timestamp(block.timestamp) {
        validate_cancun_gas(block)?;
    }

    if chain_spec.is_prague_active_at_timestamp(block.timestamp) {
        validate_prague_request(block)?;
    }

    Ok(())
}

/// Validates that the EIP-4844 header fields exist and conform to the spec. This ensures that:
///
///  * `blob_gas_used` exists as a header field
///  * `excess_blob_gas` exists as a header field
///  * `parent_beacon_block_root` exists as a header field
///  * `blob_gas_used` is less than or equal to `MAX_DATA_GAS_PER_BLOCK`
///  * `blob_gas_used` is a multiple of `DATA_GAS_PER_BLOB`
///  * `excess_blob_gas` is a multiple of `DATA_GAS_PER_BLOB`
pub fn validate_4844_header_standalone(header: &Header) -> Result<(), ConsensusError> {
    let blob_gas_used = header.blob_gas_used.ok_or(ConsensusError::BlobGasUsedMissing)?;
    let excess_blob_gas = header.excess_blob_gas.ok_or(ConsensusError::ExcessBlobGasMissing)?;

    if header.parent_beacon_block_root.is_none() {
        return Err(ConsensusError::ParentBeaconBlockRootMissing)
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

    // `excess_blob_gas` must also be a multiple of `DATA_GAS_PER_BLOB`. This will be checked later
    // (via `calc_excess_blob_gas`), but it doesn't hurt to catch the problem sooner.
    if excess_blob_gas % DATA_GAS_PER_BLOB != 0 {
        return Err(ConsensusError::ExcessBlobGasNotMultipleOfBlobGasPerBlob {
            excess_blob_gas,
            blob_gas_per_blob: DATA_GAS_PER_BLOB,
        })
    }

    Ok(())
}

/// Validates the header's extradata according to the beacon consensus rules.
///
/// From yellow paper: extraData: An arbitrary byte array containing data relevant to this block.
/// This must be 32 bytes or fewer; formally Hx.
#[inline]
pub fn validate_header_extradata(header: &Header) -> Result<(), ConsensusError> {
    let extradata_len = header.extra_data.len();
    if extradata_len > MAXIMUM_EXTRA_DATA_SIZE {
        Err(ConsensusError::ExtraDataExceedsMax { len: extradata_len })
    } else {
        Ok(())
    }
}

/// Validates against the parent hash and number.
///
/// This function ensures that the header block number is sequential and that the hash of the parent
/// header matches the parent hash in the header.
#[inline]
pub fn validate_against_parent_hash_number(
    header: &Header,
    parent: &SealedHeader,
) -> Result<(), ConsensusError> {
    // Parent number is consistent.
    if parent.number + 1 != header.number {
        return Err(ConsensusError::ParentBlockNumberMismatch {
            parent_block_number: parent.number,
            block_number: header.number,
        })
    }

    if parent.hash() != header.parent_hash {
        return Err(ConsensusError::ParentHashMismatch(
            GotExpected { got: header.parent_hash, expected: parent.hash() }.into(),
        ))
    }

    Ok(())
}

/// Validates the base fee against the parent and EIP-1559 rules.
#[inline]
pub fn validate_against_parent_eip1559_base_fee<ChainSpec: EthChainSpec + EthereumHardforks>(
    header: &Header,
    parent: &Header,
    chain_spec: &ChainSpec,
) -> Result<(), ConsensusError> {
    if chain_spec.fork(EthereumHardfork::London).active_at_block(header.number) {
        let base_fee = header.base_fee_per_gas.ok_or(ConsensusError::BaseFeeMissing)?;

        let expected_base_fee =
            if chain_spec.fork(EthereumHardfork::London).transitions_at_block(header.number) {
                reth_primitives::constants::EIP1559_INITIAL_BASE_FEE
            } else {
                // This BaseFeeMissing will not happen as previous blocks are checked to have
                // them.
                parent
                    .next_block_base_fee(chain_spec.base_fee_params_at_timestamp(header.timestamp))
                    .ok_or(ConsensusError::BaseFeeMissing)?
            };
        if expected_base_fee != base_fee {
            return Err(ConsensusError::BaseFeeDiff(GotExpected {
                expected: expected_base_fee,
                got: base_fee,
            }))
        }
    }

    Ok(())
}

/// Validates the timestamp against the parent to make sure it is in the past.
#[inline]
pub const fn validate_against_parent_timestamp(
    header: &Header,
    parent: &Header,
) -> Result<(), ConsensusError> {
    if header.timestamp <= parent.timestamp {
        return Err(ConsensusError::TimestampIsInPast {
            parent_timestamp: parent.timestamp,
            timestamp: header.timestamp,
        })
    }
    Ok(())
}

/// Validates that the EIP-4844 header fields are correct with respect to the parent block. This
/// ensures that the `blob_gas_used` and `excess_blob_gas` fields exist in the child header, and
/// that the `excess_blob_gas` field matches the expected `excess_blob_gas` calculated from the
/// parent header fields.
pub fn validate_against_parent_4844(
    header: &Header,
    parent: &Header,
) -> Result<(), ConsensusError> {
    // From [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension):
    //
    // > For the first post-fork block, both parent.blob_gas_used and parent.excess_blob_gas
    // > are evaluated as 0.
    //
    // This means in the first post-fork block, calc_excess_blob_gas will return 0.
    let parent_blob_gas_used = parent.blob_gas_used.unwrap_or(0);
    let parent_excess_blob_gas = parent.excess_blob_gas.unwrap_or(0);

    if header.blob_gas_used.is_none() {
        return Err(ConsensusError::BlobGasUsedMissing)
    }
    let excess_blob_gas = header.excess_blob_gas.ok_or(ConsensusError::ExcessBlobGasMissing)?;

    let expected_excess_blob_gas =
        calc_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used);
    if expected_excess_blob_gas != excess_blob_gas {
        return Err(ConsensusError::ExcessBlobGasDiff {
            diff: GotExpected { got: excess_blob_gas, expected: expected_excess_blob_gas },
            parent_excess_blob_gas,
            parent_blob_gas_used,
        })
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxEip4844;
    use alloy_primitives::{
        hex_literal::hex, Address, BlockHash, BlockNumber, Bytes, Parity, Sealable, U256,
    };
    use mockall::mock;
    use rand::Rng;
    use reth_chainspec::ChainSpecBuilder;
    use reth_primitives::{
        proofs, Account, BlockBody, BlockHashOrNumber, Signature, Transaction, TransactionSigned,
        Withdrawal, Withdrawals,
    };
    use reth_storage_api::{
        errors::provider::ProviderResult, AccountReader, HeaderProvider, WithdrawalsProvider,
    };
    use std::ops::RangeBounds;

    mock! {
        WithdrawalsProvider {}

        impl WithdrawalsProvider for WithdrawalsProvider {
            fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> ;

            fn withdrawals_by_block(
                &self,
                _id: BlockHashOrNumber,
                _timestamp: u64,
            ) -> ProviderResult<Option<Withdrawals>> ;
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
    }

    impl AccountReader for Provider {
        fn basic_account(&self, _address: Address) -> ProviderResult<Option<Account>> {
            Ok(self.account)
        }
    }

    impl HeaderProvider for Provider {
        fn is_known(&self, _block_hash: &BlockHash) -> ProviderResult<bool> {
            Ok(self.is_known)
        }

        fn header(&self, _block_number: &BlockHash) -> ProviderResult<Option<Header>> {
            Ok(self.parent.clone())
        }

        fn header_by_number(&self, _num: u64) -> ProviderResult<Option<Header>> {
            Ok(self.parent.clone())
        }

        fn header_td(&self, _hash: &BlockHash) -> ProviderResult<Option<U256>> {
            Ok(None)
        }

        fn header_td_by_number(&self, _number: BlockNumber) -> ProviderResult<Option<U256>> {
            Ok(None)
        }

        fn headers_range(
            &self,
            _range: impl RangeBounds<BlockNumber>,
        ) -> ProviderResult<Vec<Header>> {
            Ok(vec![])
        }

        fn sealed_header(
            &self,
            _block_number: BlockNumber,
        ) -> ProviderResult<Option<SealedHeader>> {
            Ok(None)
        }

        fn sealed_headers_while(
            &self,
            _range: impl RangeBounds<BlockNumber>,
            _predicate: impl FnMut(&SealedHeader) -> bool,
        ) -> ProviderResult<Vec<SealedHeader>> {
            Ok(vec![])
        }
    }

    impl WithdrawalsProvider for Provider {
        fn withdrawals_by_block(
            &self,
            _id: BlockHashOrNumber,
            _timestamp: u64,
        ) -> ProviderResult<Option<Withdrawals>> {
            self.withdrawals_provider.withdrawals_by_block(_id, _timestamp)
        }

        fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
            self.withdrawals_provider.latest_withdrawal()
        }
    }

    fn mock_blob_tx(nonce: u64, num_blobs: usize) -> TransactionSigned {
        let mut rng = rand::thread_rng();
        let request = Transaction::Eip4844(TxEip4844 {
            chain_id: 1u64,
            nonce,
            max_fee_per_gas: 0x28f000fff,
            max_priority_fee_per_gas: 0x28f000fff,
            max_fee_per_blob_gas: 0x7,
            gas_limit: 10,
            to: Address::default(),
            value: U256::from(3_u64),
            input: Bytes::from(vec![1, 2]),
            access_list: Default::default(),
            blob_versioned_hashes: std::iter::repeat_with(|| rng.gen()).take(num_blobs).collect(),
        });

        let signature = Signature::new(U256::default(), U256::default(), Parity::Parity(true));

        TransactionSigned::from_transaction_and_signature(request, signature)
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
            nonce: 0x0000000000000000u64.into(),
            base_fee_per_gas: 0x28f0001df.into(),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_root: None
        };
        // size: 0x9b5

        let mut parent = header.clone();
        parent.gas_used = 17763076;
        parent.gas_limit = 30000000;
        parent.base_fee_per_gas = Some(0x28041f7f5);
        parent.number -= 1;
        parent.timestamp -= 1;

        let ommers = Vec::new();
        let transactions = Vec::new();

        let sealed = header.seal_slow();
        let (header, seal) = sealed.into_parts();

        (
            SealedBlock {
                header: SealedHeader::new(header, seal),
                body: BlockBody { transactions, ommers, withdrawals: None, requests: None },
            },
            parent,
        )
    }

    #[test]
    fn valid_withdrawal_index() {
        let chain_spec = ChainSpecBuilder::mainnet().shanghai_activated().build();

        let create_block_with_withdrawals = |indexes: &[u64]| {
            let withdrawals = Withdrawals::new(
                indexes
                    .iter()
                    .map(|idx| Withdrawal { index: *idx, ..Default::default() })
                    .collect(),
            );

            let sealed = Header {
                withdrawals_root: Some(proofs::calculate_withdrawals_root(&withdrawals)),
                ..Default::default()
            }
            .seal_slow();
            let (header, seal) = sealed.into_parts();

            SealedBlock {
                header: SealedHeader::new(header, seal),
                body: BlockBody { withdrawals: Some(withdrawals), ..Default::default() },
            }
        };

        // Single withdrawal
        let block = create_block_with_withdrawals(&[1]);
        assert_eq!(validate_block_pre_execution(&block, &chain_spec), Ok(()));

        // Multiple increasing withdrawals
        let block = create_block_with_withdrawals(&[1, 2, 3]);
        assert_eq!(validate_block_pre_execution(&block, &chain_spec), Ok(()));
        let block = create_block_with_withdrawals(&[5, 6, 7, 8, 9]);
        assert_eq!(validate_block_pre_execution(&block, &chain_spec), Ok(()));
        let (_, parent) = mock_block();

        // Withdrawal index should be the last withdrawal index + 1
        let mut provider = Provider::new(Some(parent));
        provider
            .withdrawals_provider
            .expect_latest_withdrawal()
            .return_const(Ok(Some(Withdrawal { index: 2, ..Default::default() })));
    }

    #[test]
    fn cancun_block_incorrect_blob_gas_used() {
        let chain_spec = ChainSpecBuilder::mainnet().cancun_activated().build();

        // create a tx with 10 blobs
        let transaction = mock_blob_tx(1, 10);

        let sealed = Header {
            base_fee_per_gas: Some(1337),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            blob_gas_used: Some(1),
            transactions_root: proofs::calculate_transaction_root(&[transaction.clone()]),
            ..Default::default()
        }
        .seal_slow();
        let (header, seal) = sealed.into_parts();
        let header = SealedHeader::new(header, seal);

        let body = BlockBody {
            transactions: vec![transaction],
            ommers: vec![],
            withdrawals: Some(Withdrawals::default()),
            requests: None,
        };

        let block = SealedBlock::new(header, body);

        // 10 blobs times the blob gas per blob.
        let expected_blob_gas_used = 10 * DATA_GAS_PER_BLOB;

        // validate blob, it should fail blob gas used validation
        assert_eq!(
            validate_block_pre_execution(&block, &chain_spec),
            Err(ConsensusError::BlobGasUsedDiff(GotExpected {
                got: 1,
                expected: expected_blob_gas_used
            }))
        );
    }
}
