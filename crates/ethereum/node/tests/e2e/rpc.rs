use crate::utils::{eth_payload_attributes, eth_payload_attributes_amsterdam};
use alloy_eips::{eip2718::Encodable2718, eip7910::EthConfig, BlockNumberOrTag};
use alloy_genesis::Genesis;
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_provider::{
    ext::DebugApi,
    network::{EthereumWallet, TransactionBuilder},
    Provider, ProviderBuilder, SendableTx,
};
use alloy_rpc_types_beacon::relay::{
    BidTrace, BuilderBlockValidationRequestV3, BuilderBlockValidationRequestV4,
    BuilderBlockValidationRequestV6, SignedBidSubmissionV3, SignedBidSubmissionV4,
    SignedBidSubmissionV6,
};
use alloy_rpc_types_engine::{
    BlobsBundleV1, CancunPayloadFields, ExecutionPayload, ExecutionPayloadSidecar,
    ExecutionPayloadV3, PraguePayloadFields,
};
use alloy_rpc_types_eth::{
    error::EthRpcErrorCode,
    state::{AccountOverride, StateOverride},
    TransactionRequest,
};
use alloy_rpc_types_trace::geth::{
    CallConfig, ChainBlockTraceResult, GethDebugTracingOptions, GethTrace,
};
use jsonrpsee::core::client::{ClientT, Subscription, SubscriptionClientT};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_chainspec::{ChainSpecBuilder, EthChainSpec, MAINNET};
use reth_e2e_test_utils::{setup_engine, wallet::Wallet, E2ETestSetupBuilder};
use reth_network::{types::NatResolver, PeersInfo};
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{
    args::{NetworkArgs, RpcServerArgs},
    node_config::NodeConfig,
};
use reth_node_ethereum::EthereumNode;
use reth_payload_primitives::BuiltPayload;
use reth_primitives_traits::Block as _;
use reth_rpc_api::servers::AdminApiServer;
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::Runtime;
use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

alloy_sol_types::sol! {
    #[sol(rpc, bytecode = "6080604052348015600f57600080fd5b5060405160db38038060db833981016040819052602a91607a565b60005b818110156074576040805143602082015290810182905260009060600160408051601f19818403018152919052805160209091012080555080606d816092565b915050602d565b505060b8565b600060208284031215608b57600080fd5b5051919050565b60006001820160b157634e487b7160e01b600052601160045260246000fd5b5060010190565b60168060c56000396000f3fe6080604052600080fdfea164736f6c6343000810000a")]
    contract GasWaster {
        constructor(uint256 iterations) {
            for (uint256 i = 0; i < iterations; i++) {
                bytes32 slot = keccak256(abi.encode(block.number, i));
                assembly {
                    sstore(slot, slot)
                }
            }
        }
    }
}

#[tokio::test]
async fn test_block_access_list_lookup_semantics() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine::<EthereumNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let node = nodes.pop().unwrap();
    let client = node.rpc_client().unwrap();

    let pending: Option<serde_json::Value> =
        client.request("eth_getBlockAccessList", (BlockNumberOrTag::Pending,)).await?;
    assert_eq!(pending, None);

    for method in ["eth_getBlockAccessList", "debug_getRawBlockAccessList"] {
        let error = client
            .request::<serde_json::Value, _>(method, (BlockNumberOrTag::Latest,))
            .await
            .unwrap_err();
        let jsonrpsee::core::client::Error::Call(error) = error else {
            panic!("expected a resource not found error, got {error:?}")
        };
        assert_eq!(error.code(), EthRpcErrorCode::ResourceNotFound.code());
    }

    Ok(())
}

#[tokio::test]
async fn test_fee_history() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    let fee_history = provider.get_fee_history(10, 0_u64.into(), &[]).await?;

    let genesis_base_fee = chain_spec.initial_base_fee().unwrap() as u128;
    let expected_first_base_fee = genesis_base_fee -
        genesis_base_fee /
            chain_spec
                .base_fee_params_at_timestamp(chain_spec.genesis_timestamp())
                .max_change_denominator;
    assert_eq!(fee_history.base_fee_per_gas[0], genesis_base_fee);
    assert_eq!(fee_history.base_fee_per_gas[1], expected_first_base_fee,);
    // Spend some gas
    let builder = GasWaster::deploy_builder(&provider, U256::from(500)).send().await?;
    node.advance_block().await?;
    let receipt = builder.get_receipt().await?;
    assert!(receipt.status());

    let block = provider.get_block_by_number(1.into()).await?.unwrap();
    assert_eq!(block.header.gas_used, receipt.gas_used,);
    assert_eq!(block.header.base_fee_per_gas.unwrap(), expected_first_base_fee as u64);

    for _ in 0..20 {
        let _ = GasWaster::deploy_builder(&provider, U256::from(rng.random_range(0..100)))
            .send()
            .await?;

        node.advance_block().await?;
    }

    let latest_block = provider.get_block_number().await?;

    for _ in 0..20 {
        let latest_block = rng.random_range(0..=latest_block);
        let block_count = rng.random_range(1..=(latest_block + 1));

        let fee_history = provider.get_fee_history(block_count, latest_block.into(), &[]).await?;

        let mut prev_header = provider
            .get_block_by_number((latest_block + 1 - block_count).into())
            .await?
            .unwrap()
            .header;
        for block in (latest_block + 2 - block_count)..=latest_block {
            let header = provider.get_block_by_number(block.into()).await?.unwrap().header;
            let expected_base_fee =
                chain_spec.next_block_base_fee(&prev_header, header.timestamp).unwrap();

            assert_eq!(header.base_fee_per_gas.unwrap(), expected_base_fee);
            assert_eq!(
                header.base_fee_per_gas.unwrap(),
                fee_history.base_fee_per_gas[(block + block_count - 1 - latest_block) as usize]
                    as u64
            );

            prev_header = header;
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_debug_trace_chain_subscription() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );
    let (mut nodes, wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, eth_payload_attributes)
            .with_node_config_modifier(|config| {
                config.with_rpc(
                    RpcServerArgs::default()
                        .with_unused_ports()
                        .with_http()
                        .with_http_api(RpcModuleSelection::All)
                        .with_ws()
                        .with_ws_api(RpcModuleSelection::All),
                )
            })
            .build()
            .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    // Geth suppresses empty intermediate blocks but always emits the end block.
    node.advance_block().await?;
    let _ = GasWaster::deploy_builder(&provider, U256::from(5)).send().await?;
    let _ = GasWaster::deploy_builder(&provider, U256::from(7)).send().await?;
    node.advance_block().await?;
    node.advance_block().await?;

    let client = node.inner.rpc_server_handle().ws_client().await.unwrap();
    let invalid: Result<Subscription<ChainBlockTraceResult>, _> = client
        .subscribe(
            "debug_subscribe",
            jsonrpsee::rpc_params!["traceChain", "0x3", "0x3"],
            "debug_unsubscribe",
        )
        .await;
    assert!(invalid.is_err(), "equal endpoints must be rejected");

    let mut subscription: Subscription<ChainBlockTraceResult> = client
        .subscribe(
            "debug_subscribe",
            jsonrpsee::rpc_params![
                "traceChain",
                BlockNumberOrTag::Number(0),
                BlockNumberOrTag::Number(3),
                GethDebugTracingOptions::default()
            ],
            "debug_unsubscribe",
        )
        .await?;

    let traced = subscription.next().await.unwrap()?;
    assert_eq!(traced.block, U256::from(2));
    let expected = provider
        .debug_trace_block_by_number(
            BlockNumberOrTag::Number(2),
            GethDebugTracingOptions::default(),
        )
        .await?;
    assert_eq!(expected.len(), 2, "both transactions must be traced");
    assert_eq!(traced.traces, expected.into_iter().map(Some).collect::<Vec<_>>());

    let terminal = subscription.next().await.unwrap()?;
    assert_eq!(terminal.block, U256::from(3));
    assert!(terminal.traces.is_empty());
    subscription.unsubscribe().await?;

    Ok(())
}

#[tokio::test]
async fn test_debug_trace_eip8037_gas() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // The test payload clock starts at 1_710_338_135, so this activates Amsterdam for the second
    // block and lets the same node exercise both sides of the fork.
    const AMSTERDAM_TIMESTAMP: u64 = 1_710_338_137;
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .osaka_activated()
            .with_amsterdam_at(AMSTERDAM_TIMESTAMP)
            .build(),
    );
    let payload_attributes = |timestamp| {
        if timestamp >= AMSTERDAM_TIMESTAMP {
            eth_payload_attributes_amsterdam(timestamp)
        } else {
            eth_payload_attributes(timestamp)
        }
    };
    let (mut nodes, wallet) =
        setup_engine::<EthereumNode>(1, chain_spec, false, Default::default(), payload_attributes)
            .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    let pre_amsterdam = GasWaster::deploy_builder(&provider, U256::from(1)).send().await?;
    let pre_amsterdam_hash = *pre_amsterdam.tx_hash();
    node.advance_block().await?;
    assert!(pre_amsterdam.get_receipt().await?.status());

    let GethTrace::Default(frame) = provider
        .debug_trace_transaction(pre_amsterdam_hash, GethDebugTracingOptions::default())
        .await?
    else {
        panic!("expected default trace")
    };
    assert_eq!(frame.execution_gas_used, None);
    assert_eq!(frame.state_gas_used, None);
    assert_eq!(frame.gas_refund, None);
    assert!(frame
        .struct_logs
        .iter()
        .all(|log| log.state_gas_cost.is_none() && log.state_gas_reservoir.is_none()));

    let GethTrace::CallTracer(frame) = provider
        .debug_trace_transaction(
            pre_amsterdam_hash,
            GethDebugTracingOptions::call_tracer(CallConfig::default()),
        )
        .await?
    else {
        panic!("expected call trace")
    };
    assert_eq!(frame.execution_gas_used, None);
    assert_eq!(frame.state_gas_used, None);
    assert_eq!(frame.gas_refund, None);

    let amsterdam = GasWaster::deploy_builder(&provider, U256::from(1)).send().await?;
    let amsterdam_hash = *amsterdam.tx_hash();
    node.advance_block().await?;
    assert!(amsterdam.get_receipt().await?.status());

    let GethTrace::StateGasTracer(state_gas) = provider
        .debug_trace_transaction(amsterdam_hash, GethDebugTracingOptions::state_gas_tracer())
        .await?
    else {
        panic!("expected state gas trace")
    };
    assert!(state_gas.state_gas_used > 0);

    let GethTrace::Default(frame) = provider
        .debug_trace_transaction(amsterdam_hash, GethDebugTracingOptions::default())
        .await?
    else {
        panic!("expected default trace")
    };
    assert_eq!(frame.gas, state_gas.gas_used);
    assert_eq!(frame.execution_gas_used, Some(state_gas.execution_gas_used));
    assert_eq!(frame.state_gas_used, Some(state_gas.state_gas_used));
    assert_eq!(frame.gas_refund, Some(state_gas.gas_refund));
    assert!(frame.struct_logs.iter().any(|log| log.state_gas_cost.is_some()));
    assert!(frame.struct_logs.iter().all(|log| log.state_gas_reservoir.is_some()));

    let GethTrace::CallTracer(frame) = provider
        .debug_trace_transaction(
            amsterdam_hash,
            GethDebugTracingOptions::call_tracer(CallConfig::default()),
        )
        .await?
    else {
        panic!("expected call trace")
    };
    assert_eq!(frame.gas_used, U256::from(state_gas.gas_used));
    assert_eq!(frame.execution_gas_used, Some(U256::from(state_gas.execution_gas_used)));
    assert_eq!(frame.state_gas_used, Some(U256::from(state_gas.state_gas_used)));
    assert_eq!(frame.gas_refund, Some(U256::from(state_gas.gas_refund)));

    Ok(())
}

#[tokio::test]
async fn test_flashbots_validate_v3() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    node.advance(100, |_| {
        let provider = provider.clone();
        Box::pin(async move {
            let SendableTx::Envelope(tx) =
                provider.fill(TransactionRequest::default().to(Address::ZERO)).await.unwrap()
            else {
                unreachable!()
            };

            tx.encoded_2718().into()
        })
    })
    .await?;

    let _ = provider.send_transaction(TransactionRequest::default().to(Address::ZERO)).await?;
    let payload = node.new_payload().await?;

    let mut request = BuilderBlockValidationRequestV3 {
        request: SignedBidSubmissionV3 {
            message: BidTrace {
                parent_hash: payload.block().parent_hash,
                block_hash: payload.block().hash(),
                gas_used: payload.block().gas_used,
                gas_limit: payload.block().gas_limit,
                ..Default::default()
            },
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                payload.block().hash(),
                &payload.block().clone().into_block(),
            ),
            blobs_bundle: BlobsBundleV1::new([]),
            signature: Default::default(),
        },
        parent_beacon_block_root: payload.block().parent_beacon_block_root.unwrap(),
        registered_gas_limit: payload.block().gas_limit,
    };

    assert!(provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV3".into(), (&request,))
        .await
        .is_ok());

    request.registered_gas_limit -= 1;
    assert!(provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV3".into(), (&request,))
        .await
        .is_err());
    request.registered_gas_limit += 1;

    request.request.execution_payload.payload_inner.payload_inner.state_root = B256::ZERO;
    assert!(provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV3".into(), (&request,))
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn test_flashbots_validate_v4() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .prague_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    node.advance(100, |_| {
        let provider = provider.clone();
        Box::pin(async move {
            let SendableTx::Envelope(tx) =
                provider.fill(TransactionRequest::default().to(Address::ZERO)).await.unwrap()
            else {
                unreachable!()
            };

            tx.encoded_2718().into()
        })
    })
    .await?;

    let _ = provider.send_transaction(TransactionRequest::default().to(Address::ZERO)).await?;
    let payload = node.new_payload().await?;

    let mut request = BuilderBlockValidationRequestV4 {
        request: SignedBidSubmissionV4 {
            message: BidTrace {
                parent_hash: payload.block().parent_hash,
                block_hash: payload.block().hash(),
                gas_used: payload.block().gas_used,
                gas_limit: payload.block().gas_limit,
                ..Default::default()
            },
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                payload.block().hash(),
                &payload.block().clone().into_block(),
            ),
            blobs_bundle: BlobsBundleV1::new([]),
            execution_requests: payload.requests().unwrap().try_into().unwrap(),
            signature: Default::default(),
        },
        parent_beacon_block_root: payload.block().parent_beacon_block_root.unwrap(),
        registered_gas_limit: payload.block().gas_limit,
    };

    provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV4".into(), (&request,))
        .await
        .expect("request should validate");

    request.registered_gas_limit -= 1;
    assert!(provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV4".into(), (&request,))
        .await
        .is_err());
    request.registered_gas_limit += 1;

    request.request.execution_payload.payload_inner.payload_inner.state_root = B256::ZERO;
    assert!(provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV4".into(), (&request,))
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn test_flashbots_validate_v6() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .amsterdam_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    for nonce in 0..3 {
        let _ = provider
            .send_transaction(TransactionRequest::default().to(Address::ZERO).nonce(nonce))
            .await?;
    }

    let payload = node.new_payload().await?;
    assert!(payload.block_access_list().is_some());
    assert!(payload.block().body().transactions().count() >= 3);

    let envelope = payload.clone().try_into_v6()?;
    let mut request = BuilderBlockValidationRequestV6 {
        request: SignedBidSubmissionV6 {
            message: BidTrace {
                parent_hash: payload.block().parent_hash,
                block_hash: payload.block().hash(),
                gas_used: payload.block().gas_used,
                gas_limit: payload.block().gas_limit,
                ..Default::default()
            },
            execution_payload: envelope.execution_payload,
            blobs_bundle: envelope.blobs_bundle,
            execution_requests: envelope.execution_requests.try_into().unwrap(),
            signature: Default::default(),
        },
        parent_beacon_block_root: payload.block().parent_beacon_block_root.unwrap(),
        registered_gas_limit: payload.block().gas_limit,
    };

    provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV6".into(), (&request,))
        .await
        .expect("request should validate");

    request.registered_gas_limit -= 1;
    assert!(provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV6".into(), (&request,))
        .await
        .is_err());
    request.registered_gas_limit += 1;

    let mut invalid_bal_request = request.clone();
    invalid_bal_request.request.execution_payload.block_access_list = Bytes::from_static(&[0xc0]);
    assert!(provider
        .raw_request::<_, ()>(
            "flashbots_validateBuilderSubmissionV6".into(),
            (&invalid_bal_request,)
        )
        .await
        .is_err());

    // undecodable block access list bytes are rejected before the payload is processed
    let mut undecodable_bal_request = request.clone();
    undecodable_bal_request.request.execution_payload.block_access_list =
        Bytes::from_static(&[0x80]);
    let err = provider
        .raw_request::<_, ()>(
            "flashbots_validateBuilderSubmissionV6".into(),
            (&undecodable_bal_request,),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid block access list"), "{err}");

    // an empty block access list with a consistent block hash is rejected because the access
    // list rebuilt during execution doesn't match the submitted one
    let mut mismatched_bal_request = request.clone();
    mismatched_bal_request.request.execution_payload.block_access_list =
        Bytes::from_static(&[0xc0]);
    update_block_hash_v6(&mut mismatched_bal_request)?;
    let err = provider
        .raw_request::<_, ()>(
            "flashbots_validateBuilderSubmissionV6".into(),
            (&mismatched_bal_request,),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("block access list hash mismatch"), "{err}");

    request.request.execution_payload.payload_inner.payload_inner.payload_inner.state_root =
        B256::ZERO;
    assert!(provider
        .raw_request::<_, ()>("flashbots_validateBuilderSubmissionV6".into(), (&request,))
        .await
        .is_err());

    Ok(())
}

#[tokio::test]
async fn test_estimate_gas_basic_transfers_post_amsterdam() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .amsterdam_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine::<EthereumNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new().connect_http(node.rpc_url());

    let mut signers = Wallet::new(2).wallet_gen();
    let from = signers.remove(0).address();
    let existing_recipient = signers.remove(0).address();

    let self_send_gas = provider
        .estimate_gas(
            TransactionRequest::default().with_from(from).with_to(from).with_value(U256::from(1)),
        )
        .await?;
    assert_eq!(self_send_gas, 12_000);

    let zero_value_existing_gas = provider
        .estimate_gas(TransactionRequest::default().with_from(from).with_to(existing_recipient))
        .await?;
    assert_eq!(zero_value_existing_gas, 15_000);

    let value_existing_gas = provider
        .estimate_gas(
            TransactionRequest::default()
                .with_from(from)
                .with_to(existing_recipient)
                .with_value(U256::from(1)),
        )
        .await?;
    assert_eq!(value_existing_gas, 21_000);

    let fresh_recipient = Address::repeat_byte(0xfe);
    let zero_value_fresh_gas = provider
        .estimate_gas(TransactionRequest::default().with_from(from).with_to(fresh_recipient))
        .await?;
    assert_eq!(zero_value_fresh_gas, 15_000);

    // Creating the recipient requires additional state gas, so the 21_000-gas run fails and
    // estimation falls back to binary search.
    let value_fresh_gas = provider
        .estimate_gas(
            TransactionRequest::default()
                .with_from(from)
                .with_to(fresh_recipient)
                .with_value(U256::from(1)),
        )
        .await?;
    assert!(value_fresh_gas > 21_000);

    // The override installs code that needs more than 4_000 gas at entry. If the estimator
    // mistakes this for a basic transfer, it returns too little gas for the code to run.
    let gas_gate = Address::repeat_byte(0xaa);
    let mut overrides = StateOverride::default();
    overrides.insert(
        gas_gate,
        AccountOverride {
            code: Some("0x5a610fa010600957fe5b00".parse::<Bytes>()?),
            ..Default::default()
        },
    );
    let gated_tx = TransactionRequest::default().with_from(from).with_to(gas_gate);
    let gated_gas = provider.estimate_gas(gated_tx.clone()).overrides(overrides.clone()).await?;
    provider.call(gated_tx.with_gas_limit(gated_gas)).overrides(overrides).await?;

    Ok(())
}

/// Recomputes the block hash of the request after its execution payload has been modified and
/// updates it in the payload and the bid trace.
fn update_block_hash_v6(request: &mut BuilderBlockValidationRequestV6) -> eyre::Result<()> {
    let block_hash = ExecutionPayload::V4(request.request.execution_payload.clone())
        .try_into_block_with_sidecar::<reth_ethereum_primitives::TransactionSigned>(
            &ExecutionPayloadSidecar::v4(
                CancunPayloadFields {
                    parent_beacon_block_root: request.parent_beacon_block_root,
                    versioned_hashes: request.request.blobs_bundle.versioned_hashes(),
                },
                PraguePayloadFields::new(request.request.execution_requests.to_requests()),
            ),
        )?
        .seal_slow()
        .hash();
    request.request.execution_payload.payload_inner.payload_inner.payload_inner.block_hash =
        block_hash;
    request.request.message.block_hash = block_hash;
    Ok(())
}

#[tokio::test]
async fn test_eth_config() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let prague_timestamp = 10;
    let osaka_timestamp = timestamp + 10000000;

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .with_prague_at(prague_timestamp)
            .with_osaka_at(osaka_timestamp)
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    let _ = provider.send_transaction(TransactionRequest::default().to(Address::ZERO)).await?;
    node.advance_block().await?;

    let config = provider.client().request_noparams::<EthConfig>("eth_config").await?;

    assert_eq!(config.last.unwrap().activation_time, osaka_timestamp);
    assert_eq!(config.current.activation_time, prague_timestamp);
    assert_eq!(config.next.unwrap().activation_time, osaka_timestamp);

    Ok(())
}

// <https://github.com/paradigmxyz/reth/issues/19765>
#[tokio::test]
async fn test_admin_external_ip() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let runtime = Runtime::test();

    // Chain spec with test allocs
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec =
        Arc::new(ChainSpecBuilder::default().chain(MAINNET.chain).genesis(genesis).build());

    let external_ip = "10.64.128.71".parse().unwrap();
    // Node setup
    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_network(
            NetworkArgs::default().with_nat_resolver(NatResolver::ExternalIp(external_ip)),
        )
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .node(EthereumNode::default())
        .launch()
        .await?;

    let api = node.add_ons_handle.admin_api();

    let info = api.node_info().await.unwrap();

    assert_eq!(info.ip, external_ip);

    Ok(())
}

#[tokio::test]
async fn test_admin_node_info_uses_discv5_port_when_discv4_is_disabled() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let runtime = Runtime::test();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec =
        Arc::new(ChainSpecBuilder::default().chain(MAINNET.chain).genesis(genesis).build());

    let mut network = NetworkArgs::default().with_unused_ports();
    network.bootnodes = Some(Vec::new());
    network.discovery.disable_dns_discovery = true;
    network.discovery.disable_discv4_discovery = true;
    network = network.with_nat_resolver(NatResolver::ExternalIp("127.0.0.1".parse().unwrap()));

    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_network(network)
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .node(EthereumNode::default())
        .launch()
        .await?;

    assert!(node.network.discv4().is_none());
    let discv5_port = node.network.discv5().expect("discv5 should be enabled").local_port();

    let local_record = node.network.local_node_record();
    let local_enr = node.network.local_enr();
    let info = node.add_ons_handle.admin_api().node_info().await.unwrap();

    assert_eq!(local_record.udp_port, discv5_port);
    assert_eq!(local_enr.udp4(), Some(discv5_port));
    assert_eq!(info.ports.discovery, discv5_port);
    assert_eq!(info.ports.listener, local_record.tcp_port);
    assert_eq!(info.enode, local_record.to_string());
    assert!(info.enode.contains(&format!("?discport={discv5_port}")));

    Ok(())
}

#[tokio::test]
async fn test_admin_node_info_discv5_enr_uses_nat_extip_when_discv4_is_disabled() -> eyre::Result<()>
{
    reth_tracing::init_test_tracing();

    let runtime = Runtime::test();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec =
        Arc::new(ChainSpecBuilder::default().chain(MAINNET.chain).genesis(genesis).build());

    let mut network = NetworkArgs::default().with_unused_ports();
    network.bootnodes = Some(Vec::new());
    network.discovery.disable_dns_discovery = true;
    network.discovery.disable_discv4_discovery = true;
    let external_ip = Ipv4Addr::new(203, 0, 113, 7);
    network = network.with_nat_resolver(NatResolver::ExternalIp(IpAddr::V4(external_ip)));

    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_network(network)
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .node(EthereumNode::default())
        .launch()
        .await?;

    assert!(node.network.discv4().is_none());
    let discv5 = node.network.discv5().expect("discv5 should be enabled");
    let discv5_port = discv5.local_port();
    let info = node.add_ons_handle.admin_api().node_info().await.unwrap();
    let admin_enr: enr::Enr<enr::secp256k1::SecretKey> =
        info.enr.parse().map_err(|err| eyre::eyre!("failed to parse admin ENR: {err}"))?;

    assert_eq!(discv5.local_enr().ip4(), Some(external_ip));
    assert_eq!(discv5.local_enr().udp4(), Some(discv5_port));
    assert_eq!(admin_enr.ip4(), Some(external_ip));
    assert_eq!(admin_enr.udp4(), Some(discv5_port));
    assert_eq!(info.ip, IpAddr::V4(external_ip));
    assert_eq!(info.ports.discovery, discv5_port);

    Ok(())
}
