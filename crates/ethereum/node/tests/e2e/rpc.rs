use crate::utils::eth_payload_attributes;
use alloy_eips::{calc_next_block_base_fee, eip2718::Encodable2718};
use alloy_primitives::{Address, B256, U256};
use alloy_provider::{network::EthereumWallet, Provider, ProviderBuilder, SendableTx};
use alloy_rpc_types_beacon::relay::{
    BidTrace, BuilderBlockValidationRequestV3, BuilderBlockValidationRequestV4,
    SignedBidSubmissionV3, SignedBidSubmissionV4,
};
use alloy_rpc_types_engine::BlobsBundleV1;
use alloy_rpc_types_eth::TransactionRequest;
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::setup_engine;
use reth_node_core::rpc::compat::engine::payload::block_to_payload_v3;
use reth_node_ethereum::EthereumNode;
use reth_payload_primitives::BuiltPayload;
use std::sync::Arc;

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
async fn test_fee_history() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::thread_rng().gen();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {:?}", seed);

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, _tasks, wallet) =
        setup_engine::<EthereumNode>(1, chain_spec.clone(), false, eth_payload_attributes).await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(EthereumWallet::new(wallet.gen().swap_remove(0)))
        .on_http(node.rpc_url());

    let fee_history = provider.get_fee_history(10, 0_u64.into(), &[]).await?;

    let genesis_base_fee = chain_spec.initial_base_fee().unwrap() as u128;
    let expected_first_base_fee = genesis_base_fee -
        genesis_base_fee / chain_spec.base_fee_params_at_block(0).max_change_denominator;
    assert_eq!(fee_history.base_fee_per_gas[0], genesis_base_fee);
    assert_eq!(fee_history.base_fee_per_gas[1], expected_first_base_fee,);

    // Spend some gas
    let builder = GasWaster::deploy_builder(&provider, U256::from(500)).send().await?;
    node.advance_block().await?;
    let receipt = builder.get_receipt().await?;
    assert!(receipt.status());

    let block = provider.get_block_by_number(1.into(), false.into()).await?.unwrap();
    assert_eq!(block.header.gas_used as u128, receipt.gas_used,);
    assert_eq!(block.header.base_fee_per_gas.unwrap(), expected_first_base_fee as u64);

    for _ in 0..100 {
        let _ =
            GasWaster::deploy_builder(&provider, U256::from(rng.gen_range(0..1000))).send().await?;

        node.advance_block().await?;
    }

    let latest_block = provider.get_block_number().await?;

    for _ in 0..100 {
        let latest_block = rng.gen_range(0..=latest_block);
        let block_count = rng.gen_range(1..=(latest_block + 1));

        let fee_history = provider.get_fee_history(block_count, latest_block.into(), &[]).await?;

        let mut prev_header = provider
            .get_block_by_number((latest_block + 1 - block_count).into(), false.into())
            .await?
            .unwrap()
            .header;
        for block in (latest_block + 2 - block_count)..=latest_block {
            let expected_base_fee = calc_next_block_base_fee(
                prev_header.gas_used,
                prev_header.gas_limit,
                prev_header.base_fee_per_gas.unwrap(),
                chain_spec.base_fee_params_at_block(block),
            );

            let header =
                provider.get_block_by_number(block.into(), false.into()).await?.unwrap().header;

            assert_eq!(header.base_fee_per_gas.unwrap(), expected_base_fee as u64);
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
async fn test_flashbots_validate_v3() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, _tasks, wallet) =
        setup_engine::<EthereumNode>(1, chain_spec.clone(), false, eth_payload_attributes).await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(EthereumWallet::new(wallet.gen().swap_remove(0)))
        .on_http(node.rpc_url());

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
    let (payload, attrs) = node.new_payload().await?;

    let mut request = BuilderBlockValidationRequestV3 {
        request: SignedBidSubmissionV3 {
            message: BidTrace {
                parent_hash: payload.block().parent_hash,
                block_hash: payload.block().hash(),
                gas_used: payload.block().gas_used,
                gas_limit: payload.block().gas_limit,
                ..Default::default()
            },
            execution_payload: block_to_payload_v3(payload.block().clone()),
            blobs_bundle: BlobsBundleV1::new([]),
            signature: Default::default(),
        },
        parent_beacon_block_root: attrs.parent_beacon_block_root.unwrap(),
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

    let (mut nodes, _tasks, wallet) =
        setup_engine::<EthereumNode>(1, chain_spec.clone(), false, eth_payload_attributes).await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(EthereumWallet::new(wallet.gen().swap_remove(0)))
        .on_http(node.rpc_url());

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
    let (payload, attrs) = node.new_payload().await?;

    let mut request = BuilderBlockValidationRequestV4 {
        request: SignedBidSubmissionV4 {
            message: BidTrace {
                parent_hash: payload.block().parent_hash,
                block_hash: payload.block().hash(),
                gas_used: payload.block().gas_used,
                gas_limit: payload.block().gas_limit,
                ..Default::default()
            },
            execution_payload: block_to_payload_v3(payload.block().clone()),
            blobs_bundle: BlobsBundleV1::new([]),
            execution_requests: payload.requests().unwrap_or_default().to_vec(),
            signature: Default::default(),
        },
        parent_beacon_block_root: attrs.parent_beacon_block_root.unwrap(),
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
