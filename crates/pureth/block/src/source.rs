use crate::{
    BlobFeesPerGas, BlockAccessListSnapshot, ExecutionPayloadFields, ExecutionPayloadSnapshot,
    ExecutionRequestsSnapshot, GasAmounts, PayloadComponents, PayloadError, PayloadLimits,
    RequestsError,
};
use alloy_consensus::{BlockHeader, Header, Transaction};
use alloy_eip7928::{bal::Bal, BlockAccessListGasError, BlockAccessListValidationError};
use alloy_eips::{eip4844::DATA_GAS_PER_BLOB, eip7840::BlobParams, BlockNumHash};
use alloy_primitives::{keccak256, Bytes, B256, U256, U512};
use alloy_rlp::Decodable;
use pureth_withdrawal_root::{WithdrawalSnapshot, WithdrawalSnapshotError};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_consensus::{ConsensusError, HeaderValidator};
use reth_consensus_common::validation::validate_body_against_header;
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt};
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome, BlockExecutionError, BlockExecutor},
    Evm, NextBlockEnvAttributes,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{RecoveredBlock, SealedHeader};
use reth_provider::{
    providers::{BlockchainProvider, ProviderNodeTypes},
    BalStore, BlockHashReader, BlockReader, ChainSpecProvider, ProviderError, StateProvider,
    TransactionVariant,
};
use reth_pureth_receipt::{
    CanonicalityStatus, Eip6466ReceiptSnapshot, Eip6466SnapshotError, RootContext,
};
use reth_pureth_transaction::{convert_transactions, TransactionError, TransactionSnapshot};
use std::{fmt, sync::Arc};

#[derive(Debug)]
pub struct SourcePayloadSnapshot {
    source: PayloadSourceMetadata,
    payload: ExecutionPayloadSnapshot,
}

impl SourcePayloadSnapshot {
    pub fn finish_builder<S>(
        builder: impl BlockBuilder<Primitives = EthPrimitives>,
        state_provider: impl StateProvider,
        parent: &SealedHeader<Header>,
        attributes: &NextBlockEnvAttributes,
        chain_spec: &S,
        limits: PayloadLimits,
    ) -> Result<(BlockBuilderOutcome<EthPrimitives>, Self), PayloadSourceError>
    where
        S: EthChainSpec<Header = Header> + EthereumHardforks,
    {
        let evm = builder.executor().evm();
        let cfg = evm.cfg_env();
        let amsterdam = chain_spec.is_amsterdam_active_at_timestamp(attributes.timestamp);
        if evm.chain_id() != chain_spec.chain().id() ||
            cfg.enable_amsterdam_eip8037 != amsterdam ||
            cfg.enable_amsterdam_eip2780 != amsterdam
        {
            return Err(PayloadSourceError::UnsupportedFork);
        }
        let outcome =
            builder.finish(state_provider, None).map_err(PayloadSourceError::Execution)?;
        let snapshot = Self::from_builder_outcome(
            &outcome,
            parent,
            attributes,
            chain_spec,
            StateRootStatus::Computed,
            limits,
        )?;
        Ok((outcome, snapshot))
    }

    pub fn from_builder_outcome<S>(
        outcome: &BlockBuilderOutcome<EthPrimitives>,
        parent: &SealedHeader<Header>,
        attributes: &NextBlockEnvAttributes,
        chain_spec: &S,
        state_root_status: StateRootStatus,
        limits: PayloadLimits,
    ) -> Result<Self, PayloadSourceError>
    where
        S: EthChainSpec<Header = Header> + EthereumHardforks,
    {
        if state_root_status == StateRootStatus::Skipped {
            return Err(PayloadSourceError::StateRootSkipped);
        }
        let header = outcome.block.header();
        let params = chain_spec
            .blob_params_at_timestamp(header.timestamp)
            .ok_or(PayloadSourceError::UnsupportedFork)?;
        if params.update_fraction == 0 ||
            params.max_blob_count.checked_mul(DATA_GAS_PER_BLOB).is_none() ||
            params.target_blob_count.checked_mul(DATA_GAS_PER_BLOB).is_none()
        {
            return Err(PayloadSourceError::InvalidBlobParameters);
        }
        if header.parent_hash != parent.hash() ||
            header.timestamp != attributes.timestamp ||
            header.beneficiary != attributes.suggested_fee_recipient ||
            header.mix_hash != attributes.prev_randao ||
            header.gas_limit != attributes.gas_limit ||
            header.extra_data != attributes.extra_data ||
            header.parent_beacon_block_root != attributes.parent_beacon_block_root ||
            header.slot_number != attributes.slot_number ||
            outcome.block.body().withdrawals.as_ref() != attributes.withdrawals.as_ref()
        {
            return Err(PayloadSourceError::SourceMismatch);
        }
        EthBeaconConsensus::new(Arc::new(chain_spec))
            .validate_header_against_parent(outcome.block.sealed_header(), parent)
            .map_err(PayloadSourceError::NativeValidation)?;
        let bal = outcome
            .block_access_list
            .as_ref()
            .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::BlockAccessList))?;
        let raw = bal.as_raw().clone();
        if raw.len() > limits.bal_bytes ||
            outcome.block.body().transactions.len() > limits.transactions
        {
            return Err(PayloadSourceError::LimitExceeded);
        }
        if keccak256(&raw) != bal.hash() ||
            alloy_rlp::encode(bal.as_bal()).as_slice() != raw.as_ref()
        {
            return Err(PayloadSourceError::SourceMismatch);
        }
        Self::build(
            &outcome.block,
            &outcome.execution_result,
            raw,
            chain_spec,
            PayloadSourceKind::Built,
            CanonicalityStatus::Unknown,
            limits,
        )
    }

    pub fn from_reth_historical<N>(
        provider: &BlockchainProvider<N>,
        bal_store: &impl BalStore,
        block_hash: B256,
        limits: PayloadLimits,
    ) -> Result<Self, PayloadSourceError>
    where
        N: ProviderNodeTypes<Primitives = EthPrimitives>,
        N::ChainSpec: EthChainSpec<Header = Header> + EthereumHardforks,
    {
        let view = provider.consistent_provider().map_err(|error| {
            PayloadSourceError::ProviderRead { input: PayloadSourceInput::ConsistentView, error }
        })?;
        let block = view
            .recovered_block(block_hash.into(), TransactionVariant::WithHash)
            .map_err(|error| PayloadSourceError::ProviderRead {
                input: PayloadSourceInput::Block,
                error,
            })?
            .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Block))?;
        if block.hash() != block_hash {
            return Err(PayloadSourceError::SourceMismatch);
        }
        let canonical_hash = view.block_hash(block.number()).map_err(|error| {
            PayloadSourceError::ProviderRead { input: PayloadSourceInput::CanonicalHash, error }
        })?;
        let canonicality = match canonical_hash {
            Some(hash) if hash == block_hash => CanonicalityStatus::Canonical,
            Some(_) => CanonicalityStatus::NonCanonical,
            None => CanonicalityStatus::Unknown,
        };
        let execution = view
            .execution_source_by_hash(block_hash)
            .map_err(|error| PayloadSourceError::ProviderRead {
                input: PayloadSourceInput::Requests,
                error,
            })?
            .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Requests))?;
        if execution.block_hash != block_hash {
            return Err(PayloadSourceError::SourceMismatch);
        }
        drop(view);
        let raw = match execution.block_access_list {
            Some(raw) => raw,
            None => bal_store
                .get_by_hash(block_hash)
                .map_err(|error| PayloadSourceError::ProviderRead {
                    input: PayloadSourceInput::BlockAccessList,
                    error,
                })?
                .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::BlockAccessList))?,
        };
        Self::build(
            &block,
            &execution.result,
            raw,
            provider.chain_spec().as_ref(),
            PayloadSourceKind::Historical,
            canonicality,
            limits,
        )
    }

    pub const fn source(&self) -> &PayloadSourceMetadata {
        &self.source
    }

    pub const fn payload(&self) -> &ExecutionPayloadSnapshot {
        &self.payload
    }

    pub const fn root(&self) -> B256 {
        self.payload.root()
    }

    fn build<S>(
        block: &RecoveredBlock<Block>,
        result: &BlockExecutionResult<Receipt>,
        raw_bal: Bytes,
        chain_spec: &S,
        source_kind: PayloadSourceKind,
        canonicality: CanonicalityStatus,
        limits: PayloadLimits,
    ) -> Result<Self, PayloadSourceError>
    where
        S: EthChainSpec<Header = Header> + EthereumHardforks,
    {
        let header = block.header();
        if header.hash_slow() != block.hash() {
            return Err(PayloadSourceError::SourceMismatch);
        }
        if block.body().transactions.len() > limits.transactions || raw_bal.len() > limits.bal_bytes
        {
            return Err(PayloadSourceError::LimitExceeded);
        }
        let withdrawals = block
            .body()
            .withdrawals
            .as_ref()
            .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Withdrawals))?;
        if withdrawals.len() > limits.withdrawals {
            return Err(PayloadSourceError::LimitExceeded);
        }
        let slot_number =
            header.slot_number.ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Slot))?;
        let parent_beacon_block_root = header
            .parent_beacon_block_root
            .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::ParentBeaconRoot))?;
        let base_fee = header
            .base_fee_per_gas
            .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::BaseFee))?;
        let blob_gas_used = header
            .blob_gas_used
            .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::BlobGasUsed))?;
        let excess_blob_gas = header
            .excess_blob_gas
            .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::ExcessBlobGas))?;
        let params = chain_spec
            .blob_params_at_timestamp(header.timestamp)
            .ok_or(PayloadSourceError::UnsupportedFork)?;
        if params.target_blob_count.checked_mul(DATA_GAS_PER_BLOB).is_none() {
            return Err(PayloadSourceError::InvalidBlobParameters);
        }
        let blob_limit = params
            .max_blob_count
            .checked_mul(DATA_GAS_PER_BLOB)
            .ok_or(PayloadSourceError::InvalidBlobParameters)?;
        let blob_fee = blob_fee(&params, excess_blob_gas)?;
        if blob_fee > U256::from(u128::MAX) ||
            U256::from(params.calc_blob_fee(excess_blob_gas)) != blob_fee
        {
            return Err(PayloadSourceError::UnsupportedFeeRange);
        }
        if !chain_spec.is_prague_active_at_timestamp(header.timestamp) ||
            !chain_spec.is_amsterdam_active_at_timestamp(header.timestamp)
        {
            return Err(PayloadSourceError::UnsupportedFork);
        }
        EthBeaconConsensus::new(Arc::new(chain_spec))
            .validate_header(block.sealed_header())
            .map_err(PayloadSourceError::NativeValidation)?;
        validate_body_against_header(block.body(), header)
            .map_err(PayloadSourceError::NativeValidation)?;
        let expected_blob_gas = block.body().transactions.iter().try_fold(0u64, |sum, tx| {
            sum.checked_add(tx.blob_gas_used().unwrap_or_default())
                .ok_or(PayloadSourceError::SourceMismatch)
        })?;
        if expected_blob_gas != blob_gas_used || result.blob_gas_used != blob_gas_used {
            return Err(PayloadSourceError::SourceMismatch);
        }
        let mut remaining = raw_bal.as_ref();
        let bal = Bal::decode(&mut remaining).map_err(PayloadSourceError::BalDecode)?;
        if !remaining.is_empty() || alloy_rlp::encode(&bal).as_slice() != raw_bal.as_ref() {
            return Err(PayloadSourceError::NonCanonicalBal);
        }
        bal.validate_structure(block.body().transactions.len())
            .map_err(PayloadSourceError::BalStructure)?;
        bal.validate_gas_limit(header.gas_limit).map_err(PayloadSourceError::BalGas)?;
        let bal_hash = keccak256(&raw_bal);
        if header.block_access_list_hash != Some(bal_hash) {
            return Err(PayloadSourceError::NativeCommitmentMismatch(
                PayloadSourceInput::BlockAccessList,
            ));
        }
        let request_values = crate::ExecutionRequests::from_native_groups(&result.requests)
            .map_err(PayloadSourceError::Requests)?;
        if header.requests_hash != Some(result.requests.requests_hash()) {
            return Err(PayloadSourceError::NativeCommitmentMismatch(PayloadSourceInput::Requests));
        }
        validate_block_post_execution(block, chain_spec, result, None, Some(bal_hash))
            .map_err(PayloadSourceError::NativeValidation)?;
        let transactions = TransactionSnapshot::build(
            convert_transactions(&block.body().transactions)
                .map_err(PayloadSourceError::Transactions)?,
        )
        .map_err(PayloadSourceError::Transactions)?;
        let receipts = Eip6466ReceiptSnapshot::from_block(block, &result.receipts, chain_spec)
            .map_err(PayloadSourceError::Receipts)?;
        let withdrawals = WithdrawalSnapshot::build(withdrawals.to_vec())
            .map_err(PayloadSourceError::Withdrawals)?;
        let requests = ExecutionRequestsSnapshot::build(request_values)
            .map_err(PayloadSourceError::Requests)?;
        let block_access_list = BlockAccessListSnapshot::from_rlp(raw_bal, limits.bal_bytes)
            .map_err(PayloadSourceError::Payload)?;
        let payload = ExecutionPayloadSnapshot::build(
            ExecutionPayloadFields {
                parent_hash: header.parent_hash,
                miner: header.beneficiary,
                state_root: header.state_root,
                number: header.number,
                gas_limits: GasAmounts { regular: header.gas_limit, blob: blob_limit },
                gas_used: GasAmounts { regular: header.gas_used, blob: blob_gas_used },
                timestamp: header.timestamp,
                extra_data: header.extra_data.clone(),
                mix_hash: header.mix_hash,
                base_fees_per_gas: BlobFeesPerGas { regular: U256::from(base_fee), blob: blob_fee },
                excess_gas: GasAmounts { regular: 0, blob: excess_blob_gas },
                parent_beacon_block_root,
                slot_number,
            },
            PayloadComponents { transactions, receipts, withdrawals, requests, block_access_list },
            limits,
        )
        .map_err(PayloadSourceError::Payload)?;
        Ok(Self {
            source: PayloadSourceMetadata {
                native_block: BlockNumHash::new(header.number, block.hash()),
                source_kind,
                canonicality,
                root_context: RootContext::RethExperimentalUnanchored,
            },
            payload,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadSourceMetadata {
    pub native_block: BlockNumHash,
    pub source_kind: PayloadSourceKind,
    pub canonicality: CanonicalityStatus,
    pub root_context: RootContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadSourceKind {
    Built,
    Historical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateRootStatus {
    Computed,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadSourceInput {
    ConsistentView,
    Block,
    CanonicalHash,
    Requests,
    Withdrawals,
    BlockAccessList,
    Slot,
    ParentBeaconRoot,
    BaseFee,
    BlobGasUsed,
    ExcessBlobGas,
}

#[derive(Debug)]
pub enum PayloadSourceError {
    ProviderRead { input: PayloadSourceInput, error: ProviderError },
    MissingInput(PayloadSourceInput),
    SourceMismatch,
    NativeCommitmentMismatch(PayloadSourceInput),
    StateRootSkipped,
    UnsupportedFork,
    InvalidBlobParameters,
    UnsupportedFeeRange,
    LimitExceeded,
    NonCanonicalBal,
    BalDecode(alloy_rlp::Error),
    BalStructure(BlockAccessListValidationError),
    BalGas(BlockAccessListGasError),
    NativeValidation(ConsensusError),
    Execution(BlockExecutionError),
    Transactions(TransactionError),
    Receipts(Eip6466SnapshotError),
    Withdrawals(WithdrawalSnapshotError),
    Requests(RequestsError),
    Payload(PayloadError),
}

impl fmt::Display for PayloadSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "payload source failed: {self:?}")
    }
}

impl std::error::Error for PayloadSourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProviderRead { error, .. } => Some(error),
            Self::BalDecode(error) => Some(error),
            Self::BalStructure(error) => Some(error),
            Self::BalGas(error) => Some(error),
            Self::NativeValidation(error) => Some(error),
            Self::Execution(error) => Some(error),
            Self::Transactions(error) => Some(error),
            Self::Receipts(error) => Some(error),
            Self::Withdrawals(error) => Some(error),
            Self::Requests(error) => Some(error),
            Self::Payload(error) => Some(error),
            _ => None,
        }
    }
}

fn blob_fee(params: &BlobParams, excess: u64) -> Result<U256, PayloadSourceError> {
    if params.update_fraction == 0 {
        return Err(PayloadSourceError::InvalidBlobParameters);
    }
    let denominator = U512::from(params.update_fraction);
    let numerator = U512::from(excess);
    let mut term = U512::from(params.min_blob_fee) * denominator;
    let mut sum = U512::ZERO;
    let mut index = U512::from(1);
    let maximum = U512::from(U256::MAX);
    while !term.is_zero() {
        sum = sum.checked_add(term).ok_or(PayloadSourceError::UnsupportedFeeRange)?;
        if sum / denominator > maximum {
            return Err(PayloadSourceError::UnsupportedFeeRange);
        }
        let divisor =
            denominator.checked_mul(index).ok_or(PayloadSourceError::UnsupportedFeeRange)?;
        term =
            term.checked_mul(numerator).ok_or(PayloadSourceError::UnsupportedFeeRange)? / divisor;
        index = index.checked_add(U512::from(1)).ok_or(PayloadSourceError::UnsupportedFeeRange)?;
    }
    U256::checked_from_limbs_slice((sum / denominator).as_limbs())
        .ok_or(PayloadSourceError::UnsupportedFeeRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{
        proofs::calculate_receipt_root, SignableTransaction, TxEip7702, TxLegacy, TxReceipt,
    };
    use alloy_eips::{
        eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
        eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
        eip4895::Withdrawal,
        eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
        eip7251::{CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, CONSOLIDATION_REQUEST_PREDEPLOY_CODE},
        eip7702::Authorization,
        eip7840::BlobParams,
    };
    use alloy_genesis::{Genesis, GenesisAccount};
    use alloy_primitives::{Address, TxKind};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::TransactionSigned;
    use reth_evm::{execute::BlockBuilder, ConfigureEvm};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
    use reth_primitives_traits::Recovered;
    use reth_provider::{
        providers::RocksDBBuilder,
        test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
        BlockWriter, EvmStateProviderAdapter, InMemoryBalStore, NoopBalStore, OriginalValuesKnown,
        ProviderFactory, RawBal, RocksDBBalStore, StateWriteConfig, StateWriter,
    };
    use reth_revm::database::StateProviderDatabase;
    use revm::database::State;

    #[cfg(feature = "native-primitives")]
    use crate::{ExecutionBlock, ExecutionBody, ExecutionHeader};
    #[cfg(feature = "native-primitives")]
    use alloy_primitives::Sealable;
    #[cfg(feature = "native-primitives")]
    use reth_primitives_traits::Block as _;

    fn signer(byte: u8) -> PrivateKeySigner {
        B256::repeat_byte(byte).to_string().parse().unwrap()
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum BuildCase {
        Transfer,
        SetCode,
        WithdrawalRequest,
        ResourceGas,
    }

    type NativeBuild = (
        ProviderFactory<MockNodeTypesWithDB>,
        BlockBuilderOutcome<EthPrimitives>,
        BlockExecutionOutput<Receipt>,
        SealedHeader<Header>,
        NextBlockEnvAttributes,
        Arc<ChainSpec>,
        SourcePayloadSnapshot,
    );

    fn build(case: BuildCase) -> NativeBuild {
        let sender = signer(9);
        let authority = signer(1);
        let recipient = Address::repeat_byte(0x44);
        let withdrawal_recipient = Address::repeat_byte(0x55);
        let mut genesis = Genesis {
            gas_limit: if case == BuildCase::ResourceGas { 1_000_000 } else { 30_000_000 },
            base_fee_per_gas: Some(1_000_000_000),
            ..Default::default()
        };
        for address in [sender.address(), authority.address(), recipient, withdrawal_recipient] {
            genesis.alloc.insert(
                address,
                GenesisAccount { balance: U256::from(10u64.pow(18)), ..Default::default() },
            );
        }
        for (address, code) in [
            (HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE.clone()),
            (BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE.clone()),
            (WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone()),
            (CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, CONSOLIDATION_REQUEST_PREDEPLOY_CODE.clone()),
        ] {
            genesis.alloc.insert(
                address,
                GenesisAccount { nonce: Some(1), code: Some(code), ..Default::default() },
            );
        }
        let spec =
            Arc::new(ChainSpecBuilder::mainnet().genesis(genesis).amsterdam_activated().build());
        let factory = create_test_provider_factory_with_chain_spec(spec.clone());
        init_genesis(&factory).unwrap();
        let parent = SealedHeader::seal_slow(spec.genesis_header().clone());
        let attributes = NextBlockEnvAttributes {
            timestamp: 12,
            suggested_fee_recipient: Address::repeat_byte(0x66),
            prev_randao: B256::repeat_byte(0x77),
            gas_limit: parent.gas_limit,
            parent_beacon_block_root: Some(B256::repeat_byte(0x88)),
            withdrawals: Some(
                vec![Withdrawal {
                    index: 1,
                    validator_index: 2,
                    address: withdrawal_recipient,
                    amount: 3,
                }]
                .into(),
            ),
            extra_data: Bytes::from_static(b"execution-source"),
            slot_number: Some(1),
        };
        let source = factory.latest().unwrap();
        let evm_source = EvmStateProviderAdapter(source.as_ref());
        let mut state = State::builder()
            .with_database(StateProviderDatabase::new(&evm_source))
            .with_bundle_update()
            .with_bal_builder()
            .build();
        let config = EthEvmConfig::ethereum(spec.clone());
        let mut builder =
            config.builder_for_next_block(&mut state, &parent, attributes.clone()).unwrap();
        builder.apply_pre_execution_changes().unwrap();
        let signed: TransactionSigned = if case == BuildCase::SetCode {
            let make_authorization = |chain_id, nonce| {
                let value =
                    Authorization { chain_id: U256::from(chain_id), address: recipient, nonce };
                let signature = authority.sign_hash_sync(&value.signature_hash()).unwrap();
                value.into_signed(signature)
            };
            let matching = make_authorization(1u64, 0);
            let mut invalid = make_authorization(1u64, 2);
            invalid = alloy_eips::eip7702::SignedAuthorization::new_unchecked(
                invalid.inner().clone(),
                invalid.y_parity(),
                U256::ZERO,
                invalid.s(),
            );
            let tx = TxEip7702 {
                chain_id: 1,
                gas_limit: 500_000,
                max_fee_per_gas: 2_000_000_000,
                to: recipient,
                authorization_list: vec![
                    make_authorization(2u64, 0),
                    matching.clone(),
                    matching,
                    invalid,
                ],
                ..Default::default()
            };
            let signature = sender.sign_hash_sync(&tx.signature_hash()).unwrap();
            tx.into_signed(signature).into()
        } else {
            let tx = TxLegacy {
                chain_id: Some(1),
                gas_limit: if matches!(case, BuildCase::WithdrawalRequest | BuildCase::ResourceGas)
                {
                    1_000_000
                } else {
                    100_000
                },
                gas_price: 2_000_000_000,
                to: TxKind::Call(
                    if matches!(case, BuildCase::WithdrawalRequest | BuildCase::ResourceGas) {
                        WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS
                    } else {
                        recipient
                    },
                ),
                input: if matches!(case, BuildCase::WithdrawalRequest | BuildCase::ResourceGas) {
                    let mut input = vec![0x11; 48];
                    input.extend_from_slice(&3_u64.to_be_bytes());
                    input.into()
                } else {
                    Bytes::new()
                },
                value: U256::from(1),
                ..Default::default()
            };
            let signature = sender.sign_hash_sync(&tx.signature_hash()).unwrap();
            tx.into_signed(signature).into()
        };
        builder.execute_transaction(Recovered::new_unchecked(signed, sender.address())).unwrap();
        if case == BuildCase::ResourceGas {
            let tx = TxLegacy {
                chain_id: Some(1),
                nonce: 1,
                gas_limit: 480_000,
                gas_price: 2_000_000_000,
                to: TxKind::Call(recipient),
                input: vec![0; 7_000].into(),
                ..Default::default()
            };
            let signature = sender.sign_hash_sync(&tx.signature_hash()).unwrap();
            let tx: TransactionSigned = tx.into_signed(signature).into();
            builder.execute_transaction(Recovered::new_unchecked(tx, sender.address())).unwrap();
        }
        let (outcome, snapshot) = SourcePayloadSnapshot::finish_builder(
            builder,
            source.as_ref(),
            &parent,
            &attributes,
            spec.as_ref(),
            PayloadLimits::default(),
        )
        .unwrap();
        let output = BlockExecutionOutput {
            state: state.take_bundle(),
            result: outcome.execution_result.clone(),
        };
        (factory, outcome, output, parent, attributes, spec, snapshot)
    }

    fn captured_provider(
        factory: ProviderFactory<MockNodeTypesWithDB>,
        outcome: &BlockBuilderOutcome<EthPrimitives>,
        output: BlockExecutionOutput<Receipt>,
    ) -> BlockchainProvider<MockNodeTypesWithDB> {
        let provider = BlockchainProvider::new(factory).unwrap();
        provider.canonical_in_memory_state().update_chain(NewCanonicalChain::Commit {
            new: vec![ExecutedBlock::new(
                Arc::new(outcome.block.clone()),
                Arc::new(output),
                Default::default(),
            )],
        });
        provider
    }

    #[test]
    fn real_builder_and_captured_provider_produce_the_same_complete_payload() {
        let (factory, outcome, output, parent, _, spec, fresh) = build(BuildCase::Transfer);
        assert_ne!(outcome.block.header().state_root, parent.state_root);
        assert_eq!(fresh.source().native_block.hash, outcome.block.hash());
        assert_eq!(fresh.source().source_kind, PayloadSourceKind::Built);
        assert_eq!(fresh.source().canonicality, CanonicalityStatus::Unknown);
        assert_ne!(fresh.root(), outcome.block.hash());
        let raw = outcome.block_access_list.as_ref().unwrap().as_raw().clone();
        assert!(raw.len() > 1);
        let receipts = outcome
            .execution_result
            .receipts
            .iter()
            .map(TxReceipt::with_bloom_ref)
            .collect::<Vec<_>>();
        let receipt_root = calculate_receipt_root(&receipts);
        let bloom = receipts
            .iter()
            .fold(alloy_primitives::Bloom::ZERO, |bloom, receipt| bloom | receipt.bloom_ref());
        for precomputed in [None, Some((receipt_root, bloom))] {
            validate_block_post_execution(
                &outcome.block,
                spec.as_ref(),
                &outcome.execution_result,
                precomputed,
                Some(keccak256(&raw)),
            )
            .unwrap();
        }
        assert!(matches!(
            validate_block_post_execution(
                &outcome.block,
                spec.as_ref(),
                &outcome.execution_result,
                Some((fresh.payload().components().receipts.root(), bloom)),
                Some(keccak256(&raw))
            ),
            Err(ConsensusError::BodyReceiptRootDiff(_))
        ));
        let store = InMemoryBalStore::default();
        store.insert(fresh.source().native_block, RawBal::new(raw.clone())).unwrap();
        let provider = captured_provider(factory, &outcome, output);
        let historical = SourcePayloadSnapshot::from_reth_historical(
            &provider,
            &store,
            outcome.block.hash(),
            PayloadLimits::default(),
        )
        .unwrap();
        assert_eq!(historical.source().canonicality, CanonicalityStatus::Canonical);
        assert_eq!(fresh.payload().serialized(), historical.payload().serialized());
        assert_eq!(fresh.root(), historical.root());
        assert_eq!(fresh.payload().components().block_access_list.serialized(), raw.as_ref());
        assert!(std::ptr::eq(
            fresh.payload().node(0x2a).unwrap(),
            fresh.payload().components().transactions.tree()
        ));
        let receipts = Eip6466ReceiptSnapshot::from_block(
            &outcome.block,
            &outcome.execution_result.receipts,
            spec.as_ref(),
        )
        .unwrap();
        let requests =
            ExecutionRequestsSnapshot::from_native_groups(&outcome.execution_result.requests)
                .unwrap();
        let received = ExecutionPayloadSnapshot::from_ssz(
            fresh.payload().serialized(),
            receipts,
            requests,
            PayloadLimits::default(),
        )
        .unwrap();
        assert_eq!(received.root(), fresh.root());
        assert_eq!(
            fresh.root(),
            alloy_primitives::b256!(
                "52453dd36381348f20d4bfa8a994409cd10310ce96addb7107043c66e26c9e72"
            )
        );
    }

    #[test]
    fn source_context_skipped_root_missing_inputs_and_mismatches_fail_closed() {
        let (factory, outcome, output, parent, mut attributes, spec, _) =
            build(BuildCase::Transfer);
        assert!(matches!(
            SourcePayloadSnapshot::from_builder_outcome(
                &outcome,
                &parent,
                &attributes,
                spec.as_ref(),
                StateRootStatus::Skipped,
                PayloadLimits::default()
            ),
            Err(PayloadSourceError::StateRootSkipped)
        ));
        attributes.slot_number = Some(2);
        assert!(matches!(
            SourcePayloadSnapshot::from_builder_outcome(
                &outcome,
                &parent,
                &attributes,
                spec.as_ref(),
                StateRootStatus::Computed,
                PayloadLimits::default()
            ),
            Err(PayloadSourceError::SourceMismatch)
        ));
        let raw = outcome.block_access_list.as_ref().unwrap().as_raw().clone();
        let provider = captured_provider(factory, &outcome, output);
        assert!(matches!(
            SourcePayloadSnapshot::from_reth_historical(
                &provider,
                &NoopBalStore,
                outcome.block.hash(),
                PayloadLimits::default()
            ),
            Err(PayloadSourceError::MissingInput(PayloadSourceInput::BlockAccessList))
        ));
        let bad = InMemoryBalStore::default();
        bad.insert(
            BlockNumHash::new(outcome.block.number(), outcome.block.hash()),
            RawBal::new(Bytes::from_static(&[0xc0])),
        )
        .unwrap();
        assert!(matches!(
            SourcePayloadSnapshot::from_reth_historical(
                &provider,
                &bad,
                outcome.block.hash(),
                PayloadLimits::default()
            ),
            Err(PayloadSourceError::NativeCommitmentMismatch(PayloadSourceInput::BlockAccessList))
        ));
        let good = InMemoryBalStore::default();
        good.insert(
            BlockNumHash::new(outcome.block.number(), outcome.block.hash()),
            RawBal::new(raw.clone()),
        )
        .unwrap();
        for limit in [
            PayloadLimits { bal_bytes: raw.len() - 1, ..Default::default() },
            PayloadLimits { transactions: 0, ..Default::default() },
        ] {
            assert!(matches!(
                SourcePayloadSnapshot::from_reth_historical(
                    &provider,
                    &good,
                    outcome.block.hash(),
                    limit
                ),
                Err(PayloadSourceError::LimitExceeded)
            ));
        }
    }

    #[test]
    fn stored_blocks_without_full_requests_are_unavailable_not_empty() {
        let (factory, outcome, output, _, _, _, _) = build(BuildCase::Transfer);
        let provider = captured_provider(factory.clone(), &outcome, output.clone());
        let view = provider.consistent_provider().unwrap();
        let writer = factory.provider_rw().unwrap();
        writer.insert_block(&outcome.block).unwrap();
        writer
            .write_state(
                &ExecutionOutcome::single(outcome.block.number(), output),
                OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
        writer.commit().unwrap();
        provider.canonical_in_memory_state().remove_persisted_blocks(BlockNumHash::new(
            outcome.block.number(),
            outcome.block.hash(),
        ));
        let stored = provider
            .recovered_block(outcome.block.hash().into(), TransactionVariant::WithHash)
            .unwrap()
            .unwrap();
        assert_eq!(stored.header(), outcome.block.header());
        assert_eq!(stored.hash(), outcome.block.hash());
        assert_eq!(stored.body().transactions, outcome.block.body().transactions);
        assert!(view.execution_source_by_hash(outcome.block.hash()).unwrap().is_some());
        assert!(matches!(
            SourcePayloadSnapshot::from_reth_historical(
                &provider,
                &NoopBalStore,
                outcome.block.hash(),
                PayloadLimits::default()
            ),
            Err(PayloadSourceError::MissingInput(PayloadSourceInput::Requests))
        ));
        let restarted = BlockchainProvider::new(factory).unwrap();
        assert!(matches!(
            SourcePayloadSnapshot::from_reth_historical(
                &restarted,
                &NoopBalStore,
                outcome.block.hash(),
                PayloadLimits::default()
            ),
            Err(PayloadSourceError::MissingInput(PayloadSourceInput::Requests))
        ));
    }

    #[test]
    fn exact_blob_fees_match_alloy_without_saturation_or_narrowing() {
        for params in [BlobParams::cancun(), BlobParams::prague(), BlobParams::osaka()] {
            for excess in [0, 1, 131_072, 10_000_000, 100_000_000] {
                assert_eq!(
                    blob_fee(&params, excess).unwrap(),
                    U256::from(params.calc_blob_fee(excess))
                );
            }
        }
        let mut params = BlobParams::cancun();
        params.update_fraction = 1;
        let wide = blob_fee(&params, 100).unwrap();
        assert!(wide > U256::from(u128::MAX));
        assert_eq!(params.calc_blob_fee(100), u128::MAX);
        assert!(matches!(
            blob_fee(&params, u64::MAX),
            Err(PayloadSourceError::UnsupportedFeeRange)
        ));
        params.update_fraction = 0;
        assert!(matches!(blob_fee(&params, 0), Err(PayloadSourceError::InvalidBlobParameters)));
    }

    #[test]
    fn real_bal_survives_flush_store_reopen_and_pruning() {
        let (_, outcome, _, _, _, _, _) = build(BuildCase::Transfer);
        let raw = outcome.block_access_list.as_ref().unwrap().as_raw().clone();
        let identity = BlockNumHash::new(outcome.block.number(), outcome.block.hash());
        let directory = tempfile::tempdir().unwrap();
        {
            let rocks =
                RocksDBBuilder::new(directory.path()).with_default_tables().build().unwrap();
            let store = RocksDBBalStore::new(rocks);
            store.insert(identity, RawBal::new(raw.clone())).unwrap();
            assert_eq!(store.get_by_hash(identity.hash).unwrap(), Some(raw.clone()));
            store.flush(&[identity]).unwrap();
        }
        let rocks = RocksDBBuilder::new(directory.path()).with_default_tables().build().unwrap();
        let store = RocksDBBalStore::new(rocks);
        assert_eq!(store.get_by_hash(identity.hash).unwrap(), Some(raw));
        assert_eq!(
            store.prune(identity.number + alloy_eip7928::BAL_RETENTION_PERIOD_SLOTS + 1).unwrap(),
            1
        );
        assert!(store.get_by_hash(identity.hash).unwrap().is_none());
    }

    #[test]
    fn amsterdam_paid_gas_and_recovered_authorities_form_a_complete_payload() {
        let (factory, outcome, output, _, _, _, snapshot) = build(BuildCase::SetCode);
        let expected = signer(1).address();
        let stored = &outcome.execution_result.receipts[0];
        let receipt = reth_pureth_receipt::eip6466::receipt_from_transaction(
            &outcome.block.body().transactions[0],
            signer(9).address(),
            stored.cumulative_gas_used,
            stored
                .logs
                .iter()
                .map(reth_pureth_receipt::Log::try_from)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            stored.success,
        )
        .unwrap();
        assert_eq!(receipt.authorities().unwrap(), &[expected, expected, expected, Address::ZERO]);
        assert_eq!(outcome.block.header().gas_used, 55_264);
        assert_eq!(stored.cumulative_gas_used, 90_454);
        let reference =
            Eip6466ReceiptSnapshot::build(reth_pureth_receipt::Receipts::new(vec![receipt]))
                .unwrap();
        assert_eq!(
            reference.root(),
            alloy_primitives::b256!(
                "5fe55da50c43323721d1bd87d8bd7cc22f0ab1a7860714eed8b8323ef78c349a"
            )
        );
        assert_eq!(snapshot.payload().fields().gas_used.regular, 55_264);
        assert_eq!(
            snapshot.root(),
            alloy_primitives::b256!(
                "778a5da0234c90c2ff0af86f57887a4b43fb5100dd65239d8a80825fa0d724f2"
            )
        );
        assert_eq!(snapshot.payload().components().receipts.root(), reference.root());
        let store = InMemoryBalStore::default();
        store
            .insert(
                snapshot.source().native_block,
                RawBal::new(outcome.block_access_list.as_ref().unwrap().as_raw().clone()),
            )
            .unwrap();
        let provider = captured_provider(factory, &outcome, output);
        let received = SourcePayloadSnapshot::from_reth_historical(
            &provider,
            &store,
            outcome.block.hash(),
            PayloadLimits::default(),
        )
        .unwrap();
        assert_eq!(received.payload().serialized(), snapshot.payload().serialized());
    }

    #[test]
    fn genuine_multidimensional_charges_can_exceed_the_block_gas_limit() {
        let (_, outcome, _, _, _, _, snapshot) = build(BuildCase::ResourceGas);
        let paid = outcome.execution_result.receipts.last().unwrap().cumulative_gas_used;
        assert_eq!(outcome.execution_result.receipts.len(), 2);
        assert!(paid > outcome.block.gas_limit());
        assert!(outcome.block.gas_used() <= outcome.block.gas_limit());
        assert_eq!(
            snapshot
                .payload()
                .components()
                .receipts
                .receipts()
                .as_slice()
                .iter()
                .map(|receipt| receipt.gas_used())
                .sum::<u64>(),
            paid
        );
        assert_eq!(snapshot.payload().fields().gas_used.regular, outcome.execution_result.gas_used);
        assert_eq!(
            snapshot.root(),
            alloy_primitives::b256!(
                "377b83f9275e7092066c6c353fbe221c67240635167ca03f68a14967864b5f88"
            )
        );
    }

    #[test]
    fn captured_attached_bal_is_used_without_a_second_store_lookup() {
        let (factory, outcome, output, parent, attributes, spec, _) = build(BuildCase::Transfer);
        let raw = outcome.block_access_list.as_ref().unwrap();
        let prepared = raw
            .clone()
            .map(|bal| Arc::new(revm::state::bal::Bal::try_from_alloy(bal.into()).unwrap()));
        let provider = BlockchainProvider::new(factory).unwrap();
        provider.canonical_in_memory_state().update_chain(NewCanonicalChain::Commit {
            new: vec![ExecutedBlock::new(
                Arc::new(outcome.block.clone()),
                Arc::new(output),
                Default::default(),
            )
            .with_bal(Some(Arc::new(prepared)))],
        });
        let historical = SourcePayloadSnapshot::from_reth_historical(
            &provider,
            &NoopBalStore,
            outcome.block.hash(),
            PayloadLimits::default(),
        )
        .unwrap();
        let fresh = SourcePayloadSnapshot::from_builder_outcome(
            &outcome,
            &parent,
            &attributes,
            spec.as_ref(),
            StateRootStatus::Computed,
            PayloadLimits::default(),
        )
        .unwrap();
        assert_eq!(historical.root(), fresh.root());
    }

    #[test]
    fn source_missing_header_fields_and_unknown_requests_are_typed_failures() {
        let (_, outcome, _, _, _, spec, _) = build(BuildCase::Transfer);
        let raw = outcome.block_access_list.as_ref().unwrap().as_raw().clone();
        for (field, expected) in [
            (0, PayloadSourceInput::Slot),
            (1, PayloadSourceInput::ParentBeaconRoot),
            (2, PayloadSourceInput::BaseFee),
            (3, PayloadSourceInput::BlobGasUsed),
            (4, PayloadSourceInput::ExcessBlobGas),
            (5, PayloadSourceInput::Withdrawals),
        ] {
            let mut native = outcome.block.clone_block();
            match field {
                0 => native.header.slot_number = None,
                1 => native.header.parent_beacon_block_root = None,
                2 => native.header.base_fee_per_gas = None,
                3 => native.header.blob_gas_used = None,
                4 => native.header.excess_blob_gas = None,
                5 => native.body.withdrawals = None,
                _ => unreachable!(),
            }
            let block = RecoveredBlock::new_unhashed(native, outcome.block.senders().to_vec());
            assert!(
                matches!(SourcePayloadSnapshot::build(&block, &outcome.execution_result, raw.clone(), spec.as_ref(), PayloadSourceKind::Historical, CanonicalityStatus::Unknown, PayloadLimits::default()), Err(PayloadSourceError::MissingInput(input)) if input == expected)
            );
        }
        let mut result = outcome.execution_result.clone();
        result.requests = alloy_eips::eip7685::Requests::new(vec![Bytes::from_static(&[3, 1])]);
        assert!(matches!(
            SourcePayloadSnapshot::build(
                &outcome.block,
                &result,
                raw,
                spec.as_ref(),
                PayloadSourceKind::Historical,
                CanonicalityStatus::Unknown,
                PayloadLimits::default()
            ),
            Err(PayloadSourceError::Requests(RequestsError::UnsupportedType(3)))
        ));
    }

    #[test]
    fn full_request_bodies_are_produced_by_the_canonical_system_contract() {
        let (_, outcome, _, _, _, _, snapshot) = build(BuildCase::WithdrawalRequest);
        let requests =
            ExecutionRequestsSnapshot::from_native_groups(&outcome.execution_result.requests)
                .unwrap();
        assert_eq!(requests.requests().withdrawals.len(), 1);
        let request = &requests.requests().withdrawals[0];
        assert_eq!(request.source_address, signer(9).address());
        assert_eq!(request.validator_pubkey.as_slice(), &[0x11; 48]);
        assert_eq!(request.amount, 3);
        assert_eq!(
            requests.root(),
            alloy_primitives::b256!(
                "21e7039cd364059635889c9e92ae80dba81718c4ea07700c5d404b8e6ead552a"
            )
        );
        assert_eq!(
            outcome.block.header().requests_hash,
            Some(outcome.execution_result.requests.requests_hash())
        );
        assert_eq!(outcome.block.header().gas_used, 489_600);
        assert_eq!(outcome.execution_result.receipts[0].cumulative_gas_used, 575_557);
        assert_eq!(snapshot.payload().fields().gas_used.regular, 489_600);
        assert_eq!(
            snapshot.root(),
            alloy_primitives::b256!(
                "06908b870b98d80f2cdfc3d12008e83cda26cbaeb014e5f35f5700204a61e747"
            )
        );
        assert_eq!(snapshot.payload().components().requests.root(), requests.root());
    }

    #[test]
    fn source_bal_storage_errors_remain_distinct_from_missing_data() {
        struct FailingStore;
        impl BalStore for FailingStore {
            fn insert(&self, _: BlockNumHash, _: RawBal) -> reth_provider::ProviderResult<()> {
                Err(ProviderError::other(std::io::Error::other("BAL storage read failed")))
            }
            fn prune(&self, _: u64) -> reth_provider::ProviderResult<usize> {
                Err(ProviderError::other(std::io::Error::other("BAL storage read failed")))
            }
            fn get_by_hashes(
                &self,
                _: &[B256],
            ) -> reth_provider::ProviderResult<Vec<Option<Bytes>>> {
                Err(ProviderError::other(std::io::Error::other("BAL storage read failed")))
            }
        }
        let (factory, outcome, output, _, _, _, _) = build(BuildCase::Transfer);
        let provider = captured_provider(factory, &outcome, output);
        let error = SourcePayloadSnapshot::from_reth_historical(
            &provider,
            &FailingStore,
            outcome.block.hash(),
            PayloadLimits::default(),
        )
        .unwrap_err();
        assert!(matches!(
            &error,
            PayloadSourceError::ProviderRead { input: PayloadSourceInput::BlockAccessList, .. }
        ));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn reorg_keeps_a_captured_view_and_owned_payload_immutable() {
        let (factory, old, old_output, parent, attributes, spec, _) = build(BuildCase::Transfer);
        let (_, new, new_output, new_parent, _, _, _) = build(BuildCase::SetCode);
        assert_eq!(parent.hash(), new_parent.hash());
        let old_executed = ExecutedBlock::new(
            Arc::new(old.block.clone()),
            Arc::new(old_output),
            Default::default(),
        );
        let provider = BlockchainProvider::new(factory).unwrap();
        let memory = provider.canonical_in_memory_state();
        memory.update_chain(NewCanonicalChain::Commit { new: vec![old_executed.clone()] });
        let view = provider.consistent_provider().unwrap();
        let snapshot = SourcePayloadSnapshot::from_builder_outcome(
            &old,
            &parent,
            &attributes,
            spec.as_ref(),
            StateRootStatus::Computed,
            PayloadLimits::default(),
        )
        .unwrap();
        let root = snapshot.root();
        let bytes = snapshot.payload().serialized().to_vec();
        memory.update_chain(NewCanonicalChain::Reorg {
            old: vec![old_executed],
            new: vec![ExecutedBlock::new(
                Arc::new(new.block),
                Arc::new(new_output),
                Default::default(),
            )],
        });
        assert_eq!(
            view.execution_source_by_hash(old.block.hash()).unwrap().unwrap().block_hash,
            old.block.hash()
        );
        assert_eq!(snapshot.root(), root);
        assert_eq!(snapshot.payload().serialized(), bytes);
        assert!(matches!(
            SourcePayloadSnapshot::from_reth_historical(
                &provider,
                &NoopBalStore,
                old.block.hash(),
                PayloadLimits::default()
            ),
            Err(PayloadSourceError::MissingInput(PayloadSourceInput::Block))
        ));
    }

    #[test]
    fn native_finish_rejects_wrong_chain_and_fork_configuration() {
        let (factory, _, _, parent, attributes, spec, _) = build(BuildCase::Transfer);
        for supplied in [
            ChainSpecBuilder::mainnet().chain(2.into()).amsterdam_activated().build(),
            ChainSpecBuilder::mainnet().osaka_activated().build(),
        ] {
            let source = factory.latest().unwrap();
            let evm_source = EvmStateProviderAdapter(source.as_ref());
            let mut state = State::builder()
                .with_database(StateProviderDatabase::new(&evm_source))
                .with_bundle_update()
                .with_bal_builder()
                .build();
            let config = EthEvmConfig::ethereum(spec.clone());
            let builder =
                config.builder_for_next_block(&mut state, &parent, attributes.clone()).unwrap();
            assert!(matches!(
                SourcePayloadSnapshot::finish_builder(
                    builder,
                    source.as_ref(),
                    &parent,
                    &attributes,
                    &supplied,
                    PayloadLimits::default()
                ),
                Err(PayloadSourceError::UnsupportedFork)
            ));
        }
    }

    #[cfg(feature = "native-primitives")]
    #[test]
    fn execution_primitives_retain_the_real_payload_commitment_and_original_source() {
        for case in [
            BuildCase::Transfer,
            BuildCase::SetCode,
            BuildCase::WithdrawalRequest,
            BuildCase::ResourceGas,
        ] {
            let (_, outcome, _, _, _, _, snapshot) = build(case);
            let header = ExecutionHeader::from_payload(snapshot.payload()).unwrap();
            let body = ExecutionBody::from_header(
                &header,
                alloy_consensus::BlockBody {
                    transactions: outcome.block.body().transactions.clone(),
                    ommers: Vec::new(),
                    withdrawals: outcome.block.body().withdrawals.clone(),
                },
            )
            .unwrap();
            let block = ExecutionBlock::try_new(header, body).unwrap();
            assert_eq!(block.header().hash_slow(), snapshot.root());
            assert_ne!(block.header().hash_slow(), outcome.block.hash());
            assert_eq!(snapshot.source().native_block.hash, outcome.block.hash());
            let bytes = alloy_rlp::encode(&block);
            assert_eq!(ExecutionBlock::decode(&mut bytes.as_slice()).unwrap(), block);
            let value = serde_json::to_value(&block).unwrap();
            assert_eq!(serde_json::from_value::<ExecutionBlock>(value).unwrap(), block);
        }
    }
}
