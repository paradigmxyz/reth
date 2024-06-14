//! Beacon consensus implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_consensus::{Consensus, ConsensusError, PostExecutionInput};
use reth_consensus_common::validation::{
    validate_4844_header_standalone, validate_against_parent_eip1559_base_fee,
    validate_against_parent_hash_number, validate_against_parent_timestamp,
    validate_header_base_fee, validate_header_extradata, validate_header_gas, validate_ommer_hash,
    validate_transaction_root,
};
use reth_primitives::{
    constants::MINIMUM_GAS_LIMIT, eip4844::calculate_excess_blob_gas, BlockWithSenders, Chain,
    ChainSpec, GotExpected, Hardfork, Header, SealedBlock, SealedHeader, EMPTY_OMMER_ROOT_HASH,
    U256,
};
use std::{sync::Arc, time::SystemTime};

mod validation;
pub use validation::validate_block_post_execution;

/// Ethereum beacon consensus
///
/// This consensus engine does basic checks as outlined in the execution specs.
#[derive(Debug)]
pub struct EthBeaconConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl EthBeaconConsensus {
    /// Create a new instance of [`EthBeaconConsensus`]
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }

    /// Validates that the EIP-4844 header fields are correct with respect to the parent block. This
    /// ensures that the `blob_gas_used` and `excess_blob_gas` fields exist in the child header, and
    /// that the `excess_blob_gas` field matches the expected `excess_blob_gas` calculated from the
    /// parent header fields.
    pub fn validate_against_parent_4844(
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        // From [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#header-extension):
        //
        // > For the first post-fork block, both parent.blob_gas_used and parent.excess_blob_gas
        // > are evaluated as 0.
        //
        // This means in the first post-fork block, calculate_excess_blob_gas will return 0.
        let parent_blob_gas_used = parent.blob_gas_used.unwrap_or(0);
        let parent_excess_blob_gas = parent.excess_blob_gas.unwrap_or(0);

        if header.blob_gas_used.is_none() {
            return Err(ConsensusError::BlobGasUsedMissing)
        }
        let excess_blob_gas = header.excess_blob_gas.ok_or(ConsensusError::ExcessBlobGasMissing)?;

        let expected_excess_blob_gas =
            calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used);
        if expected_excess_blob_gas != excess_blob_gas {
            return Err(ConsensusError::ExcessBlobGasDiff {
                diff: GotExpected { got: excess_blob_gas, expected: expected_excess_blob_gas },
                parent_excess_blob_gas,
                parent_blob_gas_used,
            })
        }

        Ok(())
    }

    /// Checks the gas limit for consistency between parent and self headers.
    ///
    /// The maximum allowable difference between self and parent gas limits is determined by the
    /// parent's gas limit divided by the elasticity multiplier (1024).
    fn validate_against_parent_gas_limit(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        // Determine the parent gas limit, considering elasticity multiplier on the London fork.
        let parent_gas_limit =
            if self.chain_spec.fork(Hardfork::London).transitions_at_block(header.number) {
                parent.gas_limit *
                    self.chain_spec
                        .base_fee_params_at_timestamp(header.timestamp)
                        .elasticity_multiplier as u64
            } else {
                parent.gas_limit
            };

        // Check for an increase in gas limit beyond the allowed threshold.
        if header.gas_limit > parent_gas_limit {
            if header.gas_limit - parent_gas_limit >= parent_gas_limit / 1024 {
                return Err(ConsensusError::GasLimitInvalidIncrease {
                    parent_gas_limit,
                    child_gas_limit: header.gas_limit,
                })
            }
        }
        // Check for a decrease in gas limit beyond the allowed threshold.
        else if parent_gas_limit - header.gas_limit >= parent_gas_limit / 1024 {
            return Err(ConsensusError::GasLimitInvalidDecrease {
                parent_gas_limit,
                child_gas_limit: header.gas_limit,
            })
        }
        // Check if the self gas limit is below the minimum required limit.
        else if header.gas_limit < MINIMUM_GAS_LIMIT {
            return Err(ConsensusError::GasLimitInvalidMinimum { child_gas_limit: header.gas_limit })
        }

        Ok(())
    }
}

impl Consensus for EthBeaconConsensus {
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        validate_header_gas(header)?;
        validate_header_base_fee(header, &self.chain_spec)?;

        // EIP-4895: Beacon chain push withdrawals as operations
        if self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp) &&
            header.withdrawals_root.is_none()
        {
            return Err(ConsensusError::WithdrawalsRootMissing)
        } else if !self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp) &&
            header.withdrawals_root.is_some()
        {
            return Err(ConsensusError::WithdrawalsRootUnexpected)
        }

        // Ensures that EIP-4844 fields are valid once cancun is active.
        if self.chain_spec.is_cancun_active_at_timestamp(header.timestamp) {
            validate_4844_header_standalone(header)?;
        } else if header.blob_gas_used.is_some() {
            return Err(ConsensusError::BlobGasUsedUnexpected)
        } else if header.excess_blob_gas.is_some() {
            return Err(ConsensusError::ExcessBlobGasUnexpected)
        } else if header.parent_beacon_block_root.is_some() {
            return Err(ConsensusError::ParentBeaconBlockRootUnexpected)
        }

        if self.chain_spec.is_prague_active_at_timestamp(header.timestamp) {
            if header.requests_root.is_none() {
                return Err(ConsensusError::RequestsRootMissing)
            }
        } else if header.requests_root.is_some() {
            return Err(ConsensusError::RequestsRootUnexpected)
        }

        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        validate_against_parent_hash_number(header, parent)?;

        validate_against_parent_timestamp(header, parent)?;

        // TODO Check difficulty increment between parent and self
        // Ace age did increment it by some formula that we need to follow.
        self.validate_against_parent_gas_limit(header, parent)?;

        validate_against_parent_eip1559_base_fee(header, parent, &self.chain_spec)?;

        // ensure that the blob gas fields for this block
        if self.chain_spec.is_cancun_active_at_timestamp(header.timestamp) {
            Self::validate_against_parent_4844(header, parent)?;
        }

        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        let is_post_merge = self
            .chain_spec
            .fork(Hardfork::Paris)
            .active_at_ttd(total_difficulty, header.difficulty);

        if is_post_merge {
            if !header.is_zero_difficulty() {
                return Err(ConsensusError::TheMergeDifficultyIsNotZero)
            }

            if header.nonce != 0 {
                return Err(ConsensusError::TheMergeNonceIsNotZero)
            }

            if header.ommers_hash != EMPTY_OMMER_ROOT_HASH {
                return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty)
            }

            // Post-merge, the consensus layer is expected to perform checks such that the block
            // timestamp is a function of the slot. This is different from pre-merge, where blocks
            // are only allowed to be in the future (compared to the system's clock) by a certain
            // threshold.
            //
            // Block validation with respect to the parent should ensure that the block timestamp
            // is greater than its parent timestamp.

            // validate header extradata for all networks post merge
            validate_header_extradata(header)?;

            // mixHash is used instead of difficulty inside EVM
            // https://eips.ethereum.org/EIPS/eip-4399#using-mixhash-field-instead-of-difficulty
        } else {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order

            // Check if timestamp is in the future. Clock can drift but this can be consensus issue.
            let present_timestamp =
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();

            if header.exceeds_allowed_future_timestamp(present_timestamp) {
                return Err(ConsensusError::TimestampIsInFuture {
                    timestamp: header.timestamp,
                    present_timestamp,
                })
            }

            // Goerli and early OP exception:
            //  * If the network is goerli pre-merge, ignore the extradata check, since we do not
            //  support clique. Same goes for OP blocks below Bedrock.
            if self.chain_spec.chain != Chain::goerli() && !self.chain_spec.is_optimism() {
                validate_header_extradata(header)?;
            }
        }

        Ok(())
    }

    fn validate_block_pre_execution(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        validate_ommer_hash(block)?;
        validate_transaction_root(block)?;

        // EIP-4895: Beacon chain push withdrawals as operations
        if self.chain_spec.is_shanghai_active_at_timestamp(block.timestamp) {
            let withdrawals =
                block.withdrawals.as_ref().ok_or(ConsensusError::BodyWithdrawalsMissing)?;
            let withdrawals_root = reth_primitives::proofs::calculate_withdrawals_root(withdrawals);
            let header_withdrawals_root =
                block.withdrawals_root.as_ref().ok_or(ConsensusError::WithdrawalsRootMissing)?;
            if withdrawals_root != *header_withdrawals_root {
                return Err(ConsensusError::BodyWithdrawalsRootDiff(
                    GotExpected { got: withdrawals_root, expected: *header_withdrawals_root }
                        .into(),
                ))
            }
        }

        // EIP-4844: Shard Blob Transactions
        if self.chain_spec.is_cancun_active_at_timestamp(block.timestamp) {
            // Check that the blob gas used in the header matches the sum of the blob gas used by
            // each blob tx
            let header_blob_gas_used =
                block.blob_gas_used.ok_or(ConsensusError::BlobGasUsedMissing)?;
            let total_blob_gas = block.blob_gas_used();
            if total_blob_gas != header_blob_gas_used {
                return Err(ConsensusError::BlobGasUsedDiff(GotExpected {
                    got: header_blob_gas_used,
                    expected: total_blob_gas,
                }))
            }
        }

        // EIP-7685: General purpose execution layer requests
        if self.chain_spec.is_prague_active_at_timestamp(block.timestamp) {
            let requests = block.requests.as_ref().ok_or(ConsensusError::BodyRequestsMissing)?;
            let requests_root = reth_primitives::proofs::calculate_requests_root(&requests.0);
            let header_requests_root =
                block.requests_root.as_ref().ok_or(ConsensusError::RequestsRootMissing)?;
            if requests_root != *header_requests_root {
                return Err(ConsensusError::BodyRequestsRootDiff(
                    GotExpected { got: requests_root, expected: *header_requests_root }.into(),
                ))
            }
        }
        Ok(())
    }

    fn validate_block_post_execution(
        &self,
        block: &BlockWithSenders,
        input: PostExecutionInput<'_>,
    ) -> Result<(), ConsensusError> {
        validate_block_post_execution(block, &self.chain_spec, input.receipts, input.requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::mock;
    use rand::Rng;
    use reth_primitives::{
        constants::eip4844::DATA_GAS_PER_BLOB, hex_literal::hex, proofs, Account, Address,
        BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, Bytes, ChainSpecBuilder, Signature,
        Transaction, TransactionSigned, TxEip4844, Withdrawal, Withdrawals, B256, U256,
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
            placeholder: Some(()),
            to: Address::default(),
            value: U256::from(3_u64),
            input: Bytes::from(vec![1, 2]),
            access_list: Default::default(),
            blob_versioned_hashes: std::iter::repeat_with(|| rng.gen()).take(num_blobs).collect(),
        });

        let signature = Signature { odd_y_parity: true, r: U256::default(), s: U256::default() };

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
            nonce: 0x0000000000000000,
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
        let body = Vec::new();

        (
            SealedBlock {
                header: header.seal_slow(),
                body,
                ommers,
                withdrawals: None,
                requests: None,
            },
            parent,
        )
    }

    fn header_with_gas_limit(gas_limit: u64) -> SealedHeader {
        let header = Header { gas_limit, ..Default::default() };
        header.seal(B256::ZERO)
    }

    #[test]
    fn test_valid_gas_limit_increase() {
        let parent = header_with_gas_limit(1024 * 10);
        let child = header_with_gas_limit(parent.gas_limit + 5);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Ok(())
        );
    }

    #[test]
    fn test_gas_limit_below_minimum() {
        let parent = header_with_gas_limit(MINIMUM_GAS_LIMIT);
        let child = header_with_gas_limit(MINIMUM_GAS_LIMIT - 1);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Err(ConsensusError::GasLimitInvalidMinimum { child_gas_limit: child.gas_limit })
        );
    }

    #[test]
    fn test_invalid_gas_limit_increase_exceeding_limit() {
        let parent = header_with_gas_limit(1024 * 10);
        let child = header_with_gas_limit(parent.gas_limit + parent.gas_limit / 1024 + 1);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Err(ConsensusError::GasLimitInvalidIncrease {
                parent_gas_limit: parent.gas_limit,
                child_gas_limit: child.gas_limit,
            })
        );
    }

    #[test]
    fn test_valid_gas_limit_decrease_within_limit() {
        let parent = header_with_gas_limit(1024 * 10);
        let child = header_with_gas_limit(parent.gas_limit - 5);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Ok(())
        );
    }

    #[test]
    fn test_invalid_gas_limit_decrease_exceeding_limit() {
        let parent = header_with_gas_limit(1024 * 10);
        let child = header_with_gas_limit(parent.gas_limit - parent.gas_limit / 1024 - 1);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Err(ConsensusError::GasLimitInvalidDecrease {
                parent_gas_limit: parent.gas_limit,
                child_gas_limit: child.gas_limit,
            })
        );
    }

    #[test]
    fn shanghai_block_zero_withdrawals() {
        // ensures that if shanghai is activated, and we include a block with a withdrawals root,
        // that the header is valid
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().shanghai_activated().build());

        let header = Header {
            base_fee_per_gas: Some(1337u64),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            ..Default::default()
        }
        .seal_slow();

        assert_eq!(EthBeaconConsensus::new(chain_spec).validate_header(&header), Ok(()));
    }

    #[test]
    fn valid_withdrawal_index() {
        let consensus = EthBeaconConsensus::new(Arc::new(
            ChainSpecBuilder::mainnet().shanghai_activated().build(),
        ));
        let create_block_with_withdrawals = |indexes: &[u64]| {
            let withdrawals = Withdrawals::new(
                indexes
                    .iter()
                    .map(|idx| Withdrawal { index: *idx, ..Default::default() })
                    .collect(),
            );
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
        assert_eq!(consensus.validate_block_pre_execution(&block), Ok(()));

        // Multiple increasing withdrawals
        let block = create_block_with_withdrawals(&[1, 2, 3]);
        assert_eq!(consensus.validate_block_pre_execution(&block), Ok(()));
        let block = create_block_with_withdrawals(&[5, 6, 7, 8, 9]);
        assert_eq!(consensus.validate_block_pre_execution(&block), Ok(()));
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
        let consensus = EthBeaconConsensus::new(Arc::new(
            ChainSpecBuilder::mainnet().cancun_activated().build(),
        ));

        // create a tx with 10 blobs
        let transaction = mock_blob_tx(1, 10);

        let header = Header {
            base_fee_per_gas: Some(1337u64),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            blob_gas_used: Some(1),
            transactions_root: proofs::calculate_transaction_root(&[transaction.clone()]),
            ..Default::default()
        }
        .seal_slow();

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
            consensus.validate_block_pre_execution(&block),
            Err(ConsensusError::BlobGasUsedDiff(GotExpected {
                got: 1,
                expected: expected_blob_gas_used
            }))
        );
    }
}
