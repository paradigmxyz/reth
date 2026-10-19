use super::*;
use crate::{
    received_source, ExecutionConsensus, ExecutionEngineTypes, ExecutionEngineValidator,
    ExecutionEvmConfig, ExecutionPayloadTypes, ExecutionStorage,
};
use alloy_consensus::{BlockHeader, SignableTransaction, TxLegacy};
use alloy_eips::{
    eip2718::Encodable2718,
    eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
    eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
    eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    eip7251::{CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, CONSOLIDATION_REQUEST_PREDEPLOY_CODE},
    eip7685::RequestsOrHash,
};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ClientCode, ClientVersionV1, ExecutionData, ExecutionPayload,
    ExecutionPayloadEnvelopeV6, ExecutionPayloadSidecar, ForkchoiceState, ForkchoiceUpdated,
    PayloadAttributes, PayloadStatus, PayloadStatusEnum, PraguePayloadFields,
};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use reth_basic_payload_builder::{
    BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig, BuildArguments, BuildOutcome,
    MissingPayloadBehaviour, PayloadBuilder, PayloadConfig,
};
use reth_chain_state::CanonicalInMemoryState;
use reth_chainspec::{ChainSpecBuilder, EthChainSpec, EthereumHardfork, ForkCondition};
use reth_consensus::{Consensus, FullConsensus, HeaderValidator};
use reth_db_common::init::init_genesis;
use reth_engine_primitives::{
    BeaconEngineMessage, ConsensusEngineHandle, EngineApiValidator, NoopInvalidBlockHook,
    OnForkChoiceUpdated, PayloadValidator, TreeConfig,
};
use reth_engine_tree::{
    engine::EngineApiKind,
    tree::{payload_validator::TreeCtx, BasicEngineValidator, EngineApiTreeState, EngineValidator},
};
use reth_ethereum_payload_builder::{
    EthereumBuilderConfig, EthereumExecutionPayloadValidator, EthereumPayloadBuilder,
};
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::{
    execute::{BlockBuilder, Executor},
    ConfigureEngineEvm, ConfigureEvm, NextBlockEnvAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_network_api::noop::NoopNetwork;
use reth_node_types::AnyNodeTypesWithEngine;
use reth_payload_builder::{
    BuildNewPayload, EthBuiltPayload, PayloadBuilderLease, PayloadBuilderResources,
    PayloadBuilderService, PayloadJob, PayloadJobGenerator, PayloadStore,
};
use reth_payload_primitives::{
    BuiltPayload, EngineApiMessageVersion, EngineObjectValidationError, PayloadAttributes as _,
    PayloadKind, PayloadOrAttributes, PayloadTypes,
};
use reth_primitives_traits::{BlockBody, Recovered, SealedHeader};
use reth_provider::{
    providers::BlockchainProvider,
    test_utils::{
        create_test_provider_factory_with_chain_spec, create_test_provider_factory_with_node_types,
        MockEthProvider, MockNodeTypesWithDB,
    },
};
use reth_pureth_block::{
    ExecutionForkConfig, ExecutionHeader, ExecutionPrimitives, ExecutionSource, PayloadLimits,
    PayloadSourceError, PayloadSourceInput,
};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_rpc_api::EngineApiServer;
use reth_rpc_engine_api::{capabilities::EngineCapabilities, EngineApi, EngineApiError};
use reth_storage_api::{StateProvider, StateProviderFactory};
use reth_storage_overlay::OverlayManager;
use reth_tasks::{CancelOnDrop, Runtime};
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, noop::MockTransactionValidator, CoinbaseTipOrdering,
    EthPooledTransaction, Pool, TransactionOrigin, TransactionPool,
};
use reth_trie_parallel::state_root_task::{
    PayloadStateRootHandle, StateRootComputeOutcome, StateRootHandle, StateRootTaskCancelGuard,
};
use std::{
    sync::{atomic::AtomicBool, mpsc, Arc},
    time::Duration,
};

type TestPool = Pool<
    MockTransactionValidator<EthPooledTransaction>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    InMemoryBlobStore,
>;
type TestBuilder =
    ExecutionPayloadBuilder<TestPool, BlockchainProvider<MockNodeTypesWithDB>, EthEvmConfig>;

struct Fixture {
    builder: TestBuilder,
    config: PayloadConfig<PayloadAttributes, ExecutionHeader>,
}

impl Fixture {
    async fn new(amsterdam: bool) -> Self {
        let sender: PrivateKeySigner = B256::repeat_byte(9).to_string().parse().unwrap();
        let mut genesis = Genesis {
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1_000_000_000),
            ..Default::default()
        };
        genesis.config.amsterdam_time = amsterdam.then_some(1);
        genesis.alloc.insert(
            sender.address(),
            GenesisAccount { balance: U256::from(10u64.pow(18)), ..Default::default() },
        );
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
        let spec = ChainSpecBuilder::mainnet().genesis(genesis);
        let spec = Arc::new(if amsterdam {
            spec.amsterdam_activated()
                .with_fork(EthereumHardfork::Amsterdam, ForkCondition::Timestamp(1))
                .build()
        } else {
            spec.paris_activated().build()
        });
        let factory = create_test_provider_factory_with_chain_spec(spec.clone());
        init_genesis(&factory).unwrap();
        let pool = TestPool::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        );
        let tx = TxLegacy {
            chain_id: Some(1),
            gas_limit: 100_000,
            gas_price: 2_000_000_000,
            to: TxKind::Call(Address::repeat_byte(0x44)),
            value: U256::from(1),
            ..Default::default()
        };
        let signature = sender.sign_hash_sync(&tx.signature_hash()).unwrap();
        let tx: TransactionSigned = tx.into_signed(signature).into();
        let encoded_length = tx.encode_2718_len();
        pool.add_transaction(
            TransactionOrigin::Local,
            EthPooledTransaction::new(
                Recovered::new_unchecked(tx, sender.address()),
                encoded_length,
            ),
        )
        .await
        .unwrap();
        let parent = Arc::new(SealedHeader::seal_slow(ExecutionHeader::legacy(
            spec.genesis_header().clone(),
        )));
        let attributes = PayloadAttributes {
            timestamp: 12,
            suggested_fee_recipient: Address::repeat_byte(0x66),
            prev_randao: B256::repeat_byte(0x77),
            withdrawals: amsterdam.then(Vec::new),
            parent_beacon_block_root: amsterdam.then_some(B256::repeat_byte(0x88)),
            slot_number: amsterdam.then_some(1),
            target_gas_limit: amsterdam.then_some(30_000_000),
        };
        let config = PayloadConfig::new(
            parent.clone(),
            attributes.clone(),
            attributes.payload_id(&parent.hash()),
        );
        Self {
            builder: ExecutionPayloadBuilder::new(
                BlockchainProvider::new(factory).unwrap(),
                pool,
                EthEvmConfig::ethereum(spec),
                EthereumBuilderConfig::default(),
                ExecutionForkConfig::new(1),
                PayloadLimits::default(),
            ),
            config,
        }
    }

    fn args(
        &self,
        handle: Option<PayloadStateRootHandle>,
        best: Option<ExecutionBuiltPayload>,
    ) -> BuildArguments<PayloadAttributes, ExecutionBuiltPayload> {
        BuildArguments::new(
            Default::default(),
            None,
            handle,
            self.config.clone(),
            Default::default(),
            best,
        )
    }

    fn build(&self) -> ExecutionBuiltPayload {
        self.builder.try_build(self.args(None, None)).unwrap().into_payload().unwrap()
    }
}

fn root_handle(root: Option<B256>) -> PayloadStateRootHandle {
    let (updates, _updates_rx) = crossbeam_channel::unbounded();
    let (guard, _guard_rx) = StateRootTaskCancelGuard::channel();
    let (root_tx, root_rx) = mpsc::channel();
    let (_hashed_tx, hashed_rx) = mpsc::channel();
    if let Some(state_root) = root {
        root_tx
            .send(Ok(StateRootComputeOutcome {
                state_root,
                trie_updates: Default::default(),
                hashed_state: Default::default(),
            }))
            .unwrap();
    }
    StateRootHandle::new(B256::ZERO, updates, guard, root_rx, hashed_rx)
        .into_payload_state_root_handle()
}

#[tokio::test]
async fn ordinary_builder_retains_source_across_root_paths_and_empty_builds() {
    let fixture = Fixture::new(true).await;
    let sender: PrivateKeySigner = B256::repeat_byte(9).to_string().parse().unwrap();
    let mut input = vec![0x11; 48];
    input.extend_from_slice(&3u64.to_be_bytes());
    let tx = TxLegacy {
        chain_id: Some(1),
        nonce: 1,
        gas_limit: 1_000_000,
        gas_price: 2_000_000_000,
        to: TxKind::Call(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS),
        input: input.into(),
        value: U256::from(1),
    };
    let signature = sender.sign_hash_sync(&tx.signature_hash()).unwrap();
    let tx: TransactionSigned = tx.into_signed(signature).into();
    let encoded_length = tx.encode_2718_len();
    fixture
        .builder
        .pool
        .add_transaction(
            TransactionOrigin::Local,
            EthPooledTransaction::new(
                Recovered::new_unchecked(tx, sender.address()),
                encoded_length,
            ),
        )
        .await
        .unwrap();
    let BuildOutcome::Better { payload: reference, cached_reads } =
        fixture.builder.try_build(fixture.args(None, None)).unwrap()
    else {
        panic!("expected a better candidate");
    };
    assert!(!cached_reads.accounts.is_empty());
    let source = reference.source().unwrap();
    assert_eq!(source.native_receipts().len(), 2);
    assert_ne!(reference.block().state_root(), fixture.config.parent_header.state_root());
    assert!(reference.fees() > U256::ZERO);
    assert!(Arc::ptr_eq(reference.inner().block_arc(), source.block_arc()));
    assert_eq!(source.payload().unwrap().root(), reference.block().hash());
    assert_eq!(source.payload().unwrap().components().requests.requests().withdrawals.len(), 1);
    let validator = ExecutionEngineValidator::new(
        fixture.builder.client.chain_spec(),
        PayloadLimits::default(),
    )
    .unwrap();
    let envelope = ExecutionPayloadEnvelopeV6::try_from(reference.clone()).unwrap();
    assert_eq!(envelope.execution_requests, reference.requests().unwrap());
    assert_eq!(
        &envelope.execution_payload.block_access_list,
        reference.block_access_list().unwrap()
    );
    let wire = ExecutionData::from(reference.clone());
    let received = validator.convert_payload_to_block(wire).unwrap();
    assert_eq!(received.hash(), reference.block().hash());
    assert_eq!(&received, reference.block());
    assert!(Arc::ptr_eq(reference.inner().block_arc(), source.block_arc()));
    for handle in [Some(root_handle(Some(reference.block().state_root()))), Some(root_handle(None))]
    {
        let mut args = fixture.args(handle, None);
        args.cached_reads = cached_reads.clone();
        let payload = fixture.builder.try_build(args).unwrap().into_payload().unwrap();
        assert_eq!(payload.block().hash(), reference.block().hash());
        assert_eq!(
            payload.source().unwrap().payload().unwrap().serialized(),
            source.payload().unwrap().serialized()
        );
        assert_eq!(payload.requests(), reference.requests());
        assert_eq!(payload.block_access_list(), reference.block_access_list());
    }
    assert!(matches!(
        fixture.builder.try_build(fixture.args(None, Some(reference))).unwrap(),
        BuildOutcome::Aborted { .. }
    ));
    let empty = fixture.builder.build_empty_payload(fixture.config.clone()).unwrap();
    assert!(empty.source().unwrap().native_receipts().is_empty());
    assert_eq!(empty.requests(), Some(Default::default()));
    assert!(empty.block_access_list().is_some());
    assert!(empty.executed_block().is_none());
    assert_eq!(
        validator.convert_payload_to_block(ExecutionData::from(empty.clone())).unwrap().hash(),
        empty.block().hash()
    );
}

#[tokio::test]
async fn carrier_rejects_incoherent_or_incomplete_sources() {
    let fixture = Fixture::new(true).await;
    let payload = fixture.build();
    let source = payload.source().unwrap();
    let fork = fixture.builder.fork;
    assert!(matches!(
        ExecutionBuiltPayload::try_new(payload.inner().clone(), None, &fork),
        Err(PayloadSourceError::MissingInput(PayloadSourceInput::FullPayload))
    ));
    let cancel = CancelOnDrop::default();
    let acquire_narrow = || {
        ExecutionSource::from_receipts(
            source.block_arc().clone(),
            source.native_receipts_arc().clone(),
            *source.metadata(),
            fixture.builder.client.chain_spec().as_ref(),
            &fork,
            &AtomicBool::new(false),
        )
    };
    let narrow = cancel.scope(|| {
        cancel.request_finalization();
        let narrow = Arc::new(acquire_narrow().unwrap());
        drop(cancel.clone());
        assert!(matches!(acquire_narrow(), Err(PayloadSourceError::Cancelled)));
        narrow
    });
    assert!(matches!(
        ExecutionBuiltPayload::try_new(payload.inner().clone(), Some(narrow), &fork),
        Err(PayloadSourceError::MissingInput(PayloadSourceInput::FullPayload))
    ));
    for (block, requests, bal) in [
        (
            Arc::new(source.block().clone()),
            payload.requests(),
            payload.block_access_list().cloned(),
        ),
        (source.block_arc().clone(), None, payload.block_access_list().cloned()),
        (source.block_arc().clone(), payload.requests(), None),
        (source.block_arc().clone(), payload.requests(), Some(Bytes::new())),
        (
            source.block_arc().clone(),
            Some(vec![Bytes::from_static(&[1, 1])].into()),
            payload.block_access_list().cloned(),
        ),
    ] {
        let inner = EthBuiltPayload::new(block, payload.fees(), requests, bal);
        assert!(ExecutionBuiltPayload::try_new(inner, Some(source.clone()), &fork).is_err());
    }
    assert!(ExecutionBuiltPayload::try_new(
        payload.inner().clone(),
        Some(source.clone()),
        &ExecutionForkConfig::new(13)
    )
    .is_err());
    let cloned = payload.clone();
    assert!(Arc::ptr_eq(cloned.source().unwrap(), source));
}

#[tokio::test]
async fn fork_cancellation_and_errors_preserve_ordinary_behavior() {
    let mut legacy = Fixture::new(false).await;
    legacy.builder.fork = ExecutionForkConfig::new(13);
    let payload = legacy.build();
    assert!(payload.source().is_none());
    assert!(!payload.block().header().is_ssz());
    assert!(payload.requests().is_none());
    assert!(payload.block_access_list().is_none());
    let native = EthereumPayloadBuilder::new(
        legacy.builder.client.clone(),
        legacy.builder.pool.clone(),
        legacy.builder.evm_config.clone(),
        legacy.builder.builder_config.clone(),
    );
    let parent = Arc::new(SealedHeader::new(
        legacy.config.parent_header.as_native().clone(),
        legacy.config.parent_header.hash(),
    ));
    let native_config =
        PayloadConfig::new(parent, legacy.config.attributes.clone(), legacy.config.payload_id);
    let native_payload = native
        .try_build(BuildArguments::new(
            Default::default(),
            None,
            None,
            native_config,
            Default::default(),
            None,
        ))
        .unwrap()
        .into_payload()
        .unwrap();
    assert_eq!(payload.block().hash(), native_payload.block().hash());
    assert_eq!(payload.fees(), native_payload.fees());
    legacy.builder.builder_config.skip_state_root = true;
    assert!(legacy.build().source().is_none());
    let mut fixture = Fixture::new(true).await;
    let mut args = fixture.args(None, None);
    drop(args.cancel.clone());
    assert!(matches!(fixture.builder.try_build(args).unwrap(), BuildOutcome::Cancelled));
    args = fixture.args(None, None);
    args.cancel.request_finalization();
    assert!(fixture.builder.try_build(args).unwrap().into_payload().is_some());
    fixture.builder.builder_config.skip_state_root = true;
    assert!(fixture.builder.try_build(fixture.args(None, None)).is_err());
    assert!(fixture.builder.build_empty_payload(fixture.config.clone()).is_err());
    fixture.builder.builder_config.skip_state_root = false;
    fixture.builder.limits.transactions = 0;
    let Err(PayloadBuilderError::Other(error)) =
        fixture.builder.try_build(fixture.args(None, None))
    else {
        panic!("expected typed source failure");
    };
    assert!(matches!(
        error.downcast_ref::<PayloadSourceError>(),
        Some(PayloadSourceError::LimitExceeded)
    ));
    fixture.builder.limits = PayloadLimits::default();
    fixture.builder.evm_config = EthEvmConfig::ethereum(Arc::new(
        ChainSpecBuilder::mainnet()
            .genesis(fixture.builder.client.chain_spec().genesis().clone())
            .paris_activated()
            .build(),
    ));
    let Err(PayloadBuilderError::Other(error)) =
        fixture.builder.try_build(fixture.args(None, None))
    else {
        panic!("expected execution configuration failure");
    };
    assert!(matches!(
        error.downcast_ref::<PayloadSourceError>(),
        Some(PayloadSourceError::UnsupportedFork)
    ));
    fixture.builder.builder_config.await_payload_on_missing = true;
    assert!(matches!(
        fixture.builder.on_missing_payload(fixture.args(None, None)),
        MissingPayloadBehaviour::AwaitInProgress
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn basic_job_returns_selected_identity_to_its_completion_callback() {
    let fixture = Fixture::new(true).await;
    let reference = fixture.build();
    let provider = MockEthProvider::<ExecutionPrimitives>::new();
    provider.add_header(
        fixture.config.parent_header.hash(),
        fixture.config.parent_header.clone_header(),
    );
    let generator = BasicPayloadJobGenerator::with_builder(
        provider,
        Runtime::test(),
        BasicPayloadJobGeneratorConfig::default(),
        fixture.builder.clone(),
    );
    let (callback_tx, callback_rx) = mpsc::channel();
    let handle = root_handle(Some(reference.block().state_root())).with_on_payload_built(
        move |hash, root| {
            callback_tx.send((hash, root)).unwrap();
        },
    );
    let lease = Arc::new(());
    let resources = PayloadBuilderResources::new(None, Some(handle))
        .with_lease(PayloadBuilderLease::new(lease.clone()));
    let input = BuildNewPayload {
        attributes: fixture.config.attributes.clone(),
        parent_hash: fixture.config.parent_header.hash(),
        resources,
    };
    let mut job = generator.new_payload_job(input, fixture.config.payload_id).unwrap();
    let (future, _) = job.resolve_kind(PayloadKind::WaitForPending);
    let payload = tokio::time::timeout(Duration::from_secs(30), future).await.unwrap().unwrap();
    assert_eq!(
        callback_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (payload.block().hash(), payload.block().state_root())
    );
    assert_eq!(payload.block().hash(), reference.block().hash());
    assert!(Arc::ptr_eq(payload.inner().block_arc(), payload.source().unwrap().block_arc()));
    drop(job);
    assert_eq!(Arc::strong_count(&lease), 1);
}

#[tokio::test]
async fn engine_rejects_bad_commitments_inputs_and_versions() {
    let fixture = Fixture::new(true).await;
    let validator = ExecutionEngineValidator::new(
        fixture.builder.client.chain_spec(),
        PayloadLimits::default(),
    )
    .unwrap();
    let built = fixture.build();
    let invalid_bundle = built.inner().clone().with_sidecars(
        reth_ethereum_engine_primitives::BlobSidecars::Eip7594(vec![
            alloy_eips::eip7594::BlobTransactionSidecarEip7594::new(
                Vec::new(),
                Vec::new(),
                vec![alloy_consensus::Bytes48::ZERO],
            ),
        ]),
    );
    let invalid_bundle = ExecutionBuiltPayload::try_new(
        invalid_bundle,
        built.source().cloned(),
        &validator.fork_config(),
    )
    .unwrap();
    assert!(ExecutionPayloadEnvelopeV6::try_from(invalid_bundle).is_err());
    let original = ExecutionData::from(built.clone());
    let mut malformed = original.clone();
    if let ExecutionPayload::V4(payload) = &mut malformed.payload {
        payload.block_access_list = Bytes::from_static(&[0xc0, 0]);
    }
    assert!(
        matches!(validator.convert_payload_to_block(malformed), Err(error) if error.is_invalid_block_access_list())
    );
    let mut wrong_hash = original.clone();
    if let ExecutionPayload::V4(payload) = &mut wrong_hash.payload {
        payload.payload_inner.payload_inner.payload_inner.block_hash[0] ^= 1;
    }
    assert!(
        matches!(validator.convert_payload_to_block(wrong_hash), Err(error) if error.is_block_hash_mismatch())
    );
    let mut bad_blobs = original.clone();
    bad_blobs.sidecar = ExecutionPayloadSidecar::v4(
        CancunPayloadFields {
            parent_beacon_block_root: fixture.config.attributes.parent_beacon_block_root.unwrap(),
            versioned_hashes: vec![B256::ZERO],
        },
        PraguePayloadFields::new(built.requests().unwrap()),
    );
    assert!(
        matches!(validator.convert_payload_to_block(bad_blobs), Err(error) if error.is_invalid_versioned_hashes())
    );
    let mut hash_only = original.clone();
    hash_only.sidecar = ExecutionPayloadSidecar::v4(
        original.sidecar.cancun().unwrap().clone(),
        PraguePayloadFields::new(B256::ZERO),
    );
    assert!(validator.convert_payload_to_block(hash_only.clone()).is_err());
    assert!(matches!(
        validator.validate_version_specific_fields(
            EngineApiMessageVersion::V5,
            PayloadOrAttributes::from_execution_payload(&hash_only)
        ),
        Err(EngineObjectValidationError::InvalidParams(_))
    ));
    let mut invalid_requests = original.clone();
    invalid_requests.sidecar = ExecutionPayloadSidecar::v4(
        original.sidecar.cancun().unwrap().clone(),
        PraguePayloadFields::new(alloy_eips::eip7685::Requests::new(vec![Bytes::from_static(&[
            3, 1,
        ])])),
    );
    assert!(validator.convert_payload_to_block(invalid_requests).is_err());
    for version in [
        EngineApiMessageVersion::V1,
        EngineApiMessageVersion::V2,
        EngineApiMessageVersion::V3,
        EngineApiMessageVersion::V4,
        EngineApiMessageVersion::V5,
        EngineApiMessageVersion::V6,
    ] {
        assert_eq!(
            validator
                .validate_version_specific_fields(
                    version,
                    PayloadOrAttributes::from_execution_payload(&original)
                )
                .is_ok(),
            version == EngineApiMessageVersion::V5
        );
        assert_eq!(
            validator.ensure_well_formed_attributes(version, &fixture.config.attributes).is_ok(),
            version == EngineApiMessageVersion::V4
        );
        assert_eq!(
            validator.validate_get_payload_version(version, 12).is_ok(),
            version == EngineApiMessageVersion::V6
        );
    }
    let mut attributes = fixture.config.attributes.clone();
    attributes.target_gas_limit = None;
    let error = validator
        .ensure_well_formed_attributes(EngineApiMessageVersion::V4, &attributes)
        .unwrap_err();
    let error: jsonrpsee_types::ErrorObjectOwned = EngineApiError::from(error).into();
    assert_eq!(error.code(), -38003);
    let limited = ExecutionEngineValidator::new(
        fixture.builder.client.chain_spec(),
        PayloadLimits { transactions: 0, ..Default::default() },
    )
    .unwrap();
    assert!(
        matches!(limited.convert_payload_to_block(original.clone()), Err(error) if error.is_unavailable())
    );
    let cancelled = CancelOnDrop::default();
    cancelled.scope(|| {
        drop(cancelled.clone());
        assert!(matches!(validator.convert_payload_to_block(original), Err(error) if error.is_unavailable()));
    });
    assert!(ExecutionPayloadTypes::block_to_payload(
        built.block().clone(),
        built.block_access_list().cloned()
    )
    .is_err());
}

#[tokio::test]
async fn engine_configuration_and_legacy_identity_are_checked() {
    let fixture = Fixture::new(true).await;
    let spec = fixture.builder.client.chain_spec();
    let validator = ExecutionEngineValidator::new(spec.clone(), PayloadLimits::default()).unwrap();
    assert_eq!(validator.fork_config().activation_timestamp(), 1);
    let mut missing = spec.as_ref().clone();
    missing.genesis.config.amsterdam_time = None;
    assert!(ExecutionEngineValidator::new(Arc::new(missing), PayloadLimits::default()).is_err());
    let mut mismatch = spec.as_ref().clone();
    mismatch.genesis.config.amsterdam_time = Some(2);
    assert!(ExecutionEngineValidator::new(Arc::new(mismatch), PayloadLimits::default()).is_err());
    let mut invalid_params = spec.as_ref().clone();
    invalid_params.blob_params.osaka.update_fraction = 0;
    assert!(
        ExecutionEngineValidator::new(Arc::new(invalid_params), PayloadLimits::default()).is_err()
    );
    let bogota =
        ChainSpecBuilder::mainnet().genesis(spec.genesis().clone()).bogota_activated().build();
    assert!(ExecutionEngineValidator::new(Arc::new(bogota), PayloadLimits::default()).is_err());
    let mut legacy = Fixture::new(false).await;
    legacy.builder.fork = ExecutionForkConfig::new(100);
    let payload = legacy.build();
    let wire = ExecutionData::from(payload);
    let native = EthereumExecutionPayloadValidator::new(legacy.builder.client.chain_spec())
        .ensure_well_formed_payload::<TransactionSigned>(wire.clone())
        .unwrap();
    let mut future = legacy.builder.client.chain_spec().as_ref().clone();
    future.genesis.config.amsterdam_time = Some(100);
    for (fork, timestamp) in [
        (EthereumHardfork::Shanghai, 20),
        (EthereumHardfork::Cancun, 30),
        (EthereumHardfork::Prague, 40),
        (EthereumHardfork::Osaka, 50),
        (EthereumHardfork::Amsterdam, 100),
    ] {
        future.hardforks.insert(fork, ForkCondition::Timestamp(timestamp));
    }
    let validator =
        ExecutionEngineValidator::new(Arc::new(future), PayloadLimits::default()).unwrap();
    let received = validator.convert_payload_to_block(wire).unwrap();
    assert_eq!(received.hash(), native.hash());
    assert!(!received.header().is_ssz());
    assert_eq!(
        ExecutionPayloadTypes::block_to_payload(received.clone(), None)
            .unwrap()
            .payload
            .block_hash(),
        native.hash()
    );
    let evm = ExecutionEvmConfig::new(
        EthEvmConfig::new(validator.chain_spec().clone()),
        validator.clone(),
    )
    .unwrap();
    assert!(!evm.evm_env(received.header()).unwrap().cfg_env.enable_amsterdam_eip8037);
    let recovered = received.try_recover().unwrap();
    let result = evm
        .executor(StateProviderDatabase::new(
            legacy.builder.client.latest().unwrap().into_evm_state_provider(),
        ))
        .execute(&recovered)
        .unwrap();
    ExecutionConsensus::new(validator)
        .validate_block_post_execution(&recovered, &result.result, None, None)
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_rpc_uses_real_payload_service_and_enforces_request_policy() {
    let fixture = Fixture::new(true).await;
    let validator = ExecutionEngineValidator::new(
        fixture.builder.client.chain_spec(),
        PayloadLimits::default(),
    )
    .unwrap();
    let provider = MockEthProvider::<ExecutionPrimitives>::new();
    provider.add_header(
        fixture.config.parent_header.hash(),
        fixture.config.parent_header.clone_header(),
    );
    let generator = BasicPayloadJobGenerator::with_builder(
        provider.clone(),
        Runtime::test(),
        BasicPayloadJobGeneratorConfig::default(),
        fixture.builder.clone(),
    );
    let (service, handle) = PayloadBuilderService::<_, _, ExecutionEngineTypes>::new(
        generator,
        futures_util::stream::empty::<reth_provider::CanonStateNotification<ExecutionPrimitives>>(),
    );
    let service = tokio::spawn(service);
    let id = handle
        .send_new_payload(BuildNewPayload {
            attributes: fixture.config.attributes.clone(),
            parent_hash: fixture.config.parent_header.hash(),
            resources: PayloadBuilderResources::default(),
        })
        .await
        .unwrap()
        .unwrap();
    let (to_engine, mut engine_rx) = tokio::sync::mpsc::unbounded_channel();
    let network = NoopNetwork::default();
    let configured = validator.clone().into_rpc(
        provider.clone(),
        ConsensusEngineHandle::new(to_engine.clone()),
        PayloadStore::new(handle.clone()),
        fixture.builder.pool.clone(),
        Runtime::test(),
        ClientVersionV1 {
            code: ClientCode::RH,
            name: "Reth".into(),
            version: "test".into(),
            commit: "test".into(),
        },
        network.clone(),
    );
    assert!(!configured
        .method_names()
        .any(|name| name == "engine_newPayloadV6" || name == "engine_getPayloadBodiesByHashV2"));
    let api = EngineApi::new(
        provider,
        validator.chain_spec().clone(),
        ConsensusEngineHandle::new(to_engine),
        PayloadStore::new(handle),
        fixture.builder.pool.clone(),
        Runtime::test(),
        ClientVersionV1 {
            code: ClientCode::RH,
            name: "Reth".into(),
            version: "test".into(),
            commit: "test".into(),
        },
        EngineCapabilities::default(),
        validator.clone(),
        true,
        network,
    );
    let legacy_flag_module = api.into_rpc();
    let module = configured;
    let wrong_version =
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"engine_getPayloadV1","params":[id]});
    let (response, _) = module.raw_json_request(&wrong_version.to_string(), 1).await.unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(response.get()).unwrap()["error"]["code"],
        -38005
    );
    let envelope: ExecutionPayloadEnvelopeV6 =
        module.call("engine_getPayloadV6", [id]).await.unwrap();
    let hash = envelope.execution_payload.payload_inner.payload_inner.payload_inner.block_hash;
    let state = ForkchoiceState {
        head_block_hash: hash,
        safe_block_hash: B256::ZERO,
        finalized_block_hash: B256::ZERO,
    };
    let engine = tokio::spawn(async move {
        let Some(BeaconEngineMessage::ForkchoiceUpdated { payload_attrs, tx, .. }) =
            engine_rx.recv().await
        else {
            panic!("expected forkchoice update");
        };
        assert!(payload_attrs.is_none());
        tx.send(Ok(OnForkChoiceUpdated::syncing())).unwrap();
        let Some(BeaconEngineMessage::NewPayload { payload, tx, .. }) = engine_rx.recv().await
        else {
            panic!("expected new payload");
        };
        assert_eq!(validator.convert_payload_to_block(payload).unwrap().hash(), hash);
        tx.send(Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))).unwrap();
    });
    let response: ForkchoiceUpdated = module
        .call(
            "engine_forkchoiceUpdatedV4",
            (state, Option::<PayloadAttributes>::None, Option::<alloy_primitives::B128>::None),
        )
        .await
        .unwrap();
    assert_eq!(response.payload_status.status, PayloadStatusEnum::Syncing);
    let parent = fixture.config.attributes.parent_beacon_block_root.unwrap();
    let bad = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"engine_newPayloadV5",
        "params":[envelope.execution_payload, [], parent, B256::ZERO]});
    let (response, _) = legacy_flag_module.raw_json_request(&bad.to_string(), 1).await.unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(response.get()).unwrap()["error"]["code"],
        -32602
    );
    let status: PayloadStatus = module
        .call(
            "engine_newPayloadV5",
            (
                envelope.execution_payload,
                Vec::<B256>::new(),
                parent,
                RequestsOrHash::Requests(envelope.execution_requests),
            ),
        )
        .await
        .unwrap();
    assert_eq!(status.status, PayloadStatusEnum::Syncing);
    engine.await.unwrap();
    service.abort();
}

type ReceiverNodeTypes = AnyNodeTypesWithEngine<
    ExecutionPrimitives,
    ExecutionEngineTypes,
    reth_chainspec::ChainSpec<ExecutionHeader>,
    ExecutionStorage,
    ExecutionEngineTypes,
>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_executes_and_retains_source_before_accepting_commitments() {
    let fixture = Fixture::new(true).await;
    tokio::task::block_in_place(|| {
        let built = fixture.build();
        let spec = fixture.builder.client.chain_spec();
        let validator =
            ExecutionEngineValidator::new(spec.clone(), PayloadLimits::default()).unwrap();
        let consensus = Arc::new(ExecutionConsensus::new(validator.clone()));
        let evm =
            ExecutionEvmConfig::new(fixture.builder.evm_config.clone(), validator.clone()).unwrap();
        let wire = ExecutionData::from(built.clone());
        assert!(evm.evm_env_for_payload(&wire).unwrap().cfg_env.enable_amsterdam_eip8037);
        for (parallel_bal, fallback) in [(false, false), (true, false), (false, true)] {
            let factory = create_test_provider_factory_with_node_types::<ReceiverNodeTypes>(
                Arc::new(spec.as_ref().clone().map_header(ExecutionHeader::legacy)),
            );
            init_genesis(&factory).unwrap();
            let provider = BlockchainProvider::new(factory).unwrap();
            let runtime = Runtime::test();
            let overlay = OverlayManager::new(runtime.state_trie_overlay_worker_pool());
            let config = TreeConfig::default()
                .with_has_enough_parallelism(!fallback)
                .with_state_root_fallback(fallback)
                .without_bal_parallel_execution(!parallel_bal);
            let mut receiver = BasicEngineValidator::new(
                provider.clone(),
                consensus.clone(),
                evm.clone(),
                validator.clone(),
                config.clone(),
                Box::new(NoopInvalidBlockHook::default()),
                overlay.clone(),
                runtime,
            );
            let mut state = EngineApiTreeState::new(
                10,
                10,
                config.invalid_header_hit_eviction_threshold(),
                fixture.config.parent_header.num_hash(),
                EngineApiKind::Ethereum,
                overlay,
            );
            let canonical = CanonicalInMemoryState::with_head(
                fixture.config.parent_header.as_ref().clone(),
                None,
                None,
            );
            let result = receiver
                .validate_payload(wire.clone(), TreeCtx::new(&mut state, &canonical))
                .unwrap();
            let source = received_source(&result.executed_block).unwrap();
            assert!(Arc::ptr_eq(source.block_arc(), &result.executed_block.recovered_block));
            assert!(!Arc::ptr_eq(&source, built.source().unwrap()));
            assert_eq!(
                source.payload().unwrap().serialized(),
                built.source().unwrap().payload().unwrap().serialized()
            );
            assert_eq!(
                source.native_receipts(),
                result.executed_block.execution_output.result.receipts
            );
            let empty = fixture.builder.build_empty_payload(fixture.config.clone()).unwrap();
            let attributes = NextBlockEnvAttributes {
                timestamp: empty.block().timestamp(),
                suggested_fee_recipient: empty.block().beneficiary(),
                prev_randao: empty.block().mix_hash().unwrap(),
                gas_limit: empty.block().gas_limit(),
                parent_beacon_block_root: empty.block().parent_beacon_block_root(),
                withdrawals: empty.block().body().withdrawals().cloned(),
                extra_data: empty.block().extra_data().clone(),
                slot_number: empty.block().slot_number(),
            };
            let mut db = State::builder()
                .with_database(StateProviderDatabase::new(
                    provider.latest().unwrap().into_evm_state_provider(),
                ))
                .with_bundle_update()
                .with_bal_builder_if(true)
                .build();
            let mut builder = evm
                .builder_for_next_block(&mut db, &fixture.config.parent_header, attributes)
                .unwrap();
            builder.apply_pre_execution_changes().unwrap();
            let assembled = builder.finish(provider.latest().unwrap(), None).unwrap();
            assert_eq!(assembled.block.hash(), empty.block().hash());
            let received_empty = receiver
                .validate_payload(
                    ExecutionData::from(empty.clone()),
                    TreeCtx::new(&mut state, &canonical),
                )
                .unwrap();
            assert_eq!(received_empty.executed_block.sealed_block().hash(), empty.block().hash());
            assert!(received_source(&received_empty.executed_block)
                .unwrap()
                .native_receipts()
                .is_empty());
            let bal = alloy_eip7928::bal::DecodedBal::from_rlp_bytes(
                built.block_access_list().unwrap().clone(),
            )
            .unwrap();
            for hint in [None, Some((B256::repeat_byte(0x55), alloy_primitives::Bloom::ZERO))] {
                let data = consensus
                    .validate_block_post_execution_with_data(
                        &result.executed_block.recovered_block,
                        &result.executed_block.execution_output.result,
                        hint,
                        Some(bal.hash()),
                        Some(bal.as_bal()),
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    data.downcast_ref::<ExecutionSource>().unwrap().payload().unwrap().root(),
                    built.block().hash()
                );
            }
            let limited = ExecutionConsensus::new(
                ExecutionEngineValidator::new(
                    spec.clone(),
                    PayloadLimits { transactions: 0, ..Default::default() },
                )
                .unwrap(),
            );
            assert!(matches!(
                limited.validate_block_post_execution_with_data(
                    &result.executed_block.recovered_block,
                    &result.executed_block.execution_output.result,
                    None,
                    Some(bal.hash()),
                    Some(bal.as_bal())
                ),
                Err(reth_consensus::ConsensusError::Unavailable(_))
            ));
            let cancelled = CancelOnDrop::default();
            cancelled.scope(|| {
                drop(cancelled.clone());
                assert!(matches!(
                    consensus.validate_block_post_execution_with_data(
                        &result.executed_block.recovered_block,
                        &result.executed_block.execution_output.result,
                        None,
                        Some(bal.hash()),
                        Some(bal.as_bal())
                    ),
                    Err(reth_consensus::ConsensusError::Unavailable(_))
                ));
            });
            let mut detached = result.executed_block.clone();
            detached.recovered_block =
                Arc::new(result.executed_block.recovered_block.as_ref().clone());
            assert!(matches!(received_source(&detached), Err(PayloadSourceError::SourceMismatch)));
            assert!(consensus
                .validate_block_post_execution(
                    source.block(),
                    &result.executed_block.execution_output.result,
                    None,
                    Some(bal.hash())
                )
                .is_err());
            consensus.validate_header(built.block().sealed_header()).unwrap();
            consensus
                .validate_header_against_parent(
                    built.block().sealed_header(),
                    &fixture.config.parent_header,
                )
                .unwrap();
            consensus.validate_block_pre_execution(built.block()).unwrap();
            let wrong_mode = SealedHeader::seal_slow(ExecutionHeader::legacy(
                built.block().header().as_native().clone(),
            ));
            assert!(consensus.validate_header(&wrong_mode).is_err());
            for field in ["state", "receipts", "requests"] {
                let mut altered = wire.clone();
                if let ExecutionPayload::V4(payload) = &mut altered.payload {
                    let inner = &mut payload.payload_inner.payload_inner.payload_inner;
                    if field == "state" {
                        inner.state_root = B256::repeat_byte(0xab);
                    } else if field == "receipts" {
                        inner.receipts_root = B256::repeat_byte(0xcd);
                    }
                    inner.block_hash = B256::ZERO;
                }
                let mut summary =
                    built.source().unwrap().payload().unwrap().summary().summary().clone();
                if field == "state" {
                    summary.state_root = B256::repeat_byte(0xab);
                } else if field == "receipts" {
                    summary.receipts_root = B256::repeat_byte(0xcd);
                } else {
                    let mut raw = vec![0; 77];
                    raw[0] = 1;
                    let requests = alloy_eips::eip7685::Requests::new(vec![raw.into()]);
                    summary.requests_root = reth_pureth_block::ExecutionRequestsSnapshot::build(
                        reth_pureth_block::ExecutionRequests::from_native_groups(&requests)
                            .unwrap(),
                    )
                    .unwrap()
                    .root();
                    altered.sidecar = ExecutionPayloadSidecar::v4(
                        wire.sidecar.cancun().unwrap().clone(),
                        PraguePayloadFields::new(requests),
                    );
                }
                let hash = ExecutionHeader::from_summary(summary).unwrap().hash_slow();
                if let ExecutionPayload::V4(payload) = &mut altered.payload {
                    payload.payload_inner.payload_inner.payload_inner.block_hash = hash;
                }
                assert!(validator.convert_payload_to_block(altered.clone()).is_ok());
                assert!(receiver
                    .validate_payload(altered, TreeCtx::new(&mut state, &canonical))
                    .is_err());
            }
            let skipped = TreeConfig::default().with_skip_state_root(true);
            let mut receiver = BasicEngineValidator::new(
                provider,
                consensus.clone(),
                evm.clone(),
                validator.clone(),
                skipped,
                Box::new(NoopInvalidBlockHook::default()),
                OverlayManager::default(),
                Runtime::test(),
            );
            assert!(
                matches!(receiver.validate_payload(wire.clone(), TreeCtx::new(&mut state, &canonical)), Err(reth_engine_tree::tree::error::InsertPayloadError::Payload(error)) if error.is_unavailable())
            );
        }
    });
}
