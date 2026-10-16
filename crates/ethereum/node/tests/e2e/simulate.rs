use alloy_primitives::{address, bytes, Address, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::{
    simulate::{SimBlock, SimulatePayload, SimulatedBlock},
    state::{AccountOverride, StateOverride, StateOverridesBuilder},
    BlockOverrides, TransactionRequest, TransactionTrait as _,
};
use reth_chainspec::{EthereumHardfork, ForkCondition};
use reth_e2e_test_utils::{test_chain_spec_builder, E2ETestSetupExt};
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

#[tokio::test]
async fn test_simulate_v1_transfer_logs_across_amsterdam() -> eyre::Result<()> {
    let chain_spec =
        Arc::new(test_chain_spec_builder().osaka_activated().with_amsterdam_at(24).build());
    let (node, _) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    let provider = node.rpc_provider();
    let from = Address::with_last_byte(0x41);
    let to = Address::with_last_byte(0x42);
    let reverting = Address::with_last_byte(0x43);
    let call_block = SimBlock::default()
        .with_state_overrides(StateOverride::from_iter([
            (from, AccountOverride::default().with_balance(U256::from(100)).with_nonce(0)),
            (reverting, AccountOverride::default().with_code(bytes!("5f5ffd"))),
        ]))
        .call(TransactionRequest::default().from(from).to(to).value(U256::ONE))
        .call(TransactionRequest::default().from(from).to(reverting).value(U256::ONE));
    let mut untraced: Option<Vec<SimulatedBlock>> = None;
    for trace_transfers in [false, true] {
        let mut payload = SimulatePayload::default()
            .extend(call_block.clone())
            .extend(call_block.clone())
            .extend(call_block.clone());
        payload.trace_transfers = trace_transfers;
        let result: Vec<SimulatedBlock> =
            provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;
        assert_eq!(result.len(), 3);
        for (index, block) in result.iter().enumerate() {
            assert_eq!(block.inner.header.timestamp, (index as u64 + 1) * 12);
            assert!(block.calls[0].status);
            let logs = &block.calls[0].logs;
            if index == 0 && !trace_transfers {
                assert!(logs.is_empty());
            } else {
                assert_eq!(logs.len(), 1);
                let emitter = if index == 0 {
                    Address::repeat_byte(0xee)
                } else {
                    revm::primitives::eip7708::ETH_TRANSFER_LOG_ADDRESS
                };
                assert_eq!(logs[0].address(), emitter);
            }
            assert!(!block.calls[1].status);
            assert!(block.calls[1].logs.is_empty());
            if index > 0 {
                assert!(!block.inner.header.logs_bloom.is_zero());
                if let Some(untraced) = &untraced {
                    assert_eq!(logs[0].inner, untraced[index].calls[0].logs[0].inner);
                    assert_eq!(
                        block.inner.header.logs_bloom,
                        untraced[index].inner.header.logs_bloom
                    );
                    assert_eq!(
                        block.inner.header.receipts_root,
                        untraced[index].inner.header.receipts_root
                    );
                }
            }
        }
        untraced = Some(result);
    }
    Ok(())
}

#[tokio::test]
async fn test_simulate_v1_block_access_list_hash_across_amsterdam() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let chain_spec =
        Arc::new(test_chain_spec_builder().osaka_activated().with_amsterdam_at(24).build());
    let (node, _) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    let provider = node.rpc_provider();
    let from = Address::with_last_byte(0x41);
    let contract = Address::with_last_byte(0x42);
    let state_overrides = StateOverride::from_iter([
        (from, AccountOverride::default().with_nonce(0)),
        (contract, AccountOverride::default().with_code(bytes!("0x5f545f5260205ff3"))),
    ]);
    let call_block = SimBlock::default()
        .with_state_overrides(state_overrides)
        .call(TransactionRequest::default().from(from).to(contract));

    for trace_transfers in [false, true] {
        let mut payload = SimulatePayload::default()
            .extend(SimBlock::default())
            .extend(call_block.clone())
            .extend(call_block.clone())
            .extend(SimBlock::default());
        payload.trace_transfers = trace_transfers;
        let result: Vec<SimulatedBlock> =
            provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].inner.header.block_access_list_hash, None);
        let bal_hash = result[1].inner.header.block_access_list_hash.expect("Amsterdam BAL hash");
        assert!(result[1].calls[0].status);
        assert!(result[2].calls[0].status);
        // Identical calls with the same initial state must reset their BAL indices each block.
        assert_eq!(result[2].inner.header.block_access_list_hash, Some(bal_hash));
        let empty_hash =
            result[3].inner.header.block_access_list_hash.expect("empty block BAL hash");
        assert_ne!(bal_hash, empty_hash);
        for pair in result.windows(2) {
            assert_eq!(pair[1].inner.header.parent_hash, pair[0].inner.header.hash);
        }
        for block in &result {
            assert_eq!(block.inner.header.hash, block.inner.header.inner.hash_slow());
        }
    }
    Ok(())
}

#[tokio::test]
async fn test_simulate_v1_no_fields_call_defaults_to_remaining_block_gas() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let provider = node.rpc_provider_with_wallet(wallet.signer(0));

    let first_tx_gas_limit = 21_000;
    let sim_block = SimBlock::default()
        .call(TransactionRequest::default().to(Address::ZERO).gas_limit(first_tx_gas_limit))
        .call(TransactionRequest::default());
    let payload = SimulatePayload::default().with_full_transactions().extend(sim_block);

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].calls.len(), 2);
    assert!(result[0].calls.iter().all(|call| call.status));

    let block = &result[0].inner;
    let txs = block.transactions.as_transactions().expect("expected full transactions");
    assert_eq!(txs.len(), 2);
    assert_eq!(txs[0].gas_limit(), first_tx_gas_limit);

    let expected_remaining_gas = block.header.inner.gas_limit - result[0].calls[0].gas_used;
    assert_eq!(txs[1].gas_limit(), expected_remaining_gas);

    Ok(())
}

async fn assert_validation_uses_remaining_gas(fork: EthereumHardfork) -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, wallet) = EthereumNode::test_setup_for(fork).build_single().await?;
    let provider = node.rpc_provider_with_wallet(wallet.signer(0));

    let from = address!("0xc000000000000000000000000000000000000000");
    let state_overrides =
        StateOverridesBuilder::default().with_balance(from, U256::from(1_000_000_000u64)).build();
    let tx = TransactionRequest::default()
        .from(from)
        .max_fee_per_gas(0)
        .max_priority_fee_per_gas(0)
        .nonce(0)
        // Create an empty contract so Amsterdam exercises the state-gas reservoir.
        .input(bytes!("0x60006000f3").into());
    let sim_block = SimBlock::default()
        .with_block_overrides(BlockOverrides {
            gas_limit: Some(30_000_000),
            base_fee: Some(U256::ZERO),
            ..Default::default()
        })
        .with_state_overrides(state_overrides)
        .call(tx);
    let payload =
        SimulatePayload::default().with_validation().with_full_transactions().extend(sim_block);

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].calls.len(), 1);
    assert!(result[0].calls[0].status);
    let txs = result[0].inner.transactions.as_transactions().expect("expected full transactions");
    assert_eq!(txs.len(), 1);
    assert_eq!(txs[0].gas_limit(), 30_000_000);
    assert!(txs[0].gas_limit() > (1 << 24));

    Ok(())
}

#[tokio::test]
async fn test_simulate_v1_validation_omitted_gas_uses_remaining_gas_osaka() -> eyre::Result<()> {
    assert_validation_uses_remaining_gas(EthereumHardfork::Osaka).await
}

#[tokio::test]
async fn test_simulate_v1_validation_omitted_gas_uses_remaining_gas_amsterdam() -> eyre::Result<()>
{
    assert_validation_uses_remaining_gas(EthereumHardfork::Amsterdam).await
}

#[tokio::test]
async fn test_simulate_v1_blockhash_reads_prior_simulated_block() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let provider = node.rpc_provider_with_wallet(wallet.signer(0));

    let contract = Address::with_last_byte(0x42);
    let mut state_overrides = StateOverride::default();
    state_overrides.insert(
        contract,
        AccountOverride::default().with_code(bytes!("0x6004354303405f5260205ff3")),
    );
    let input =
        bytes!("0xee82ac5e000000000000000000000000000000000000000000000000000000000000000f").into();

    let payload = SimulatePayload::default().extend(SimBlock::default()).extend(
        SimBlock::default()
            .with_block_overrides(BlockOverrides {
                number: Some(U256::from(16)),
                ..Default::default()
            })
            .with_state_overrides(state_overrides)
            .call(TransactionRequest::default().to(contract).input(input)),
    );

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    assert_eq!(result.len(), 16);
    assert_eq!(result[15].calls.len(), 1);
    assert!(result[15].calls[0].status);
    assert_eq!(result[15].calls[0].return_data.as_ref(), result[0].inner.header.hash.as_slice());

    Ok(())
}

#[tokio::test]
async fn test_simulate_v1_explicit_gas_over_remaining_block_gas_errors() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let provider = node.rpc_provider_with_wallet(wallet.signer(0));

    let sim_block = SimBlock::default()
        .with_block_overrides(BlockOverrides { gas_limit: Some(50_000), ..Default::default() })
        .call(TransactionRequest::default().to(Address::ZERO))
        .call(TransactionRequest::default().to(Address::ZERO).gas_limit(30_000));
    let payload = SimulatePayload::default().extend(sim_block);

    let err = provider
        .raw_request::<_, Vec<SimulatedBlock>>("eth_simulateV1".into(), (&payload, "latest"))
        .await
        .unwrap_err();
    let err = err.as_error_resp().expect("expected JSON-RPC error response");

    assert_eq!(err.code, -38015);
    assert_eq!(err.message, "Block gas limit exceeded by the block's transactions");

    Ok(())
}

/// Tests that `eth_simulateV1` handles a transaction with `maxFeePerBlobGas` set but no
/// `blob_versioned_hashes` or sidecar. The transaction should be treated as EIP-1559, not
/// EIP-4844.
///
/// Reproduces <https://github.com/paradigmxyz/reth/issues/21809>
#[tokio::test]
async fn test_simulate_v1_with_max_fee_per_blob_gas_only() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let provider = node.rpc_provider_with_wallet(wallet.signer(0));

    let _ = provider.send_transaction(TransactionRequest::default().to(Address::ZERO)).await?;
    node.advance_block().await?;

    let from = address!("0xc000000000000000000000000000000000000000");
    let to = address!("0xc100000000000000000000000000000000000000");

    let tx = TransactionRequest::default()
        .from(from)
        .to(to)
        .gas_limit(0x52080)
        .max_fee_per_gas(0)
        .max_priority_fee_per_gas(0)
        .max_fee_per_blob_gas(0)
        .value(U256::ZERO)
        .nonce(0)
        .input(Default::default());

    let state_overrides =
        StateOverridesBuilder::default().with_balance(from, U256::from(0x334500u64)).build();

    let sim_block = SimBlock::default()
        .with_block_overrides(BlockOverrides { base_fee: Some(U256::ZERO), ..Default::default() })
        .with_state_overrides(state_overrides)
        .call(tx);

    let payload = SimulatePayload::default().with_validation().extend(sim_block);

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    assert_eq!(result.len(), 1, "expected exactly 1 simulated block");
    assert_eq!(result[0].calls.len(), 1, "expected exactly 1 call result");
    let call = &result[0].calls[0];
    assert!(call.status, "expected call to succeed");
    assert!(call.error.is_none(), "expected no error");
    assert_eq!(call.max_used_gas, Some(call.gas_used), "expected maxUsedGas in call result");

    Ok(())
}

#[tokio::test]
async fn test_simulate_v1_too_many_blocks_error() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let provider = node.rpc_provider_with_wallet(wallet.signer(0));

    let payload: SimulatePayload<TransactionRequest> =
        (0..257).fold(SimulatePayload::default(), |payload, _| payload.extend(SimBlock::default()));

    let err = provider
        .raw_request::<_, Vec<SimulatedBlock>>("eth_simulateV1".into(), (&payload, "latest"))
        .await
        .unwrap_err();
    let err = err.as_error_resp().expect("expected JSON-RPC error response");

    assert_eq!(err.code, -38026);
    assert_eq!(err.message, "too many blocks");

    Ok(())
}

/// A `blockOverrides.time` that crosses the Cancun activation on a pre-Cancun parent must run the
/// simulated block under Cancun rules, instead of failing with `EIP-4788 parent beacon block root
/// missing for active Cancun block`.
///
/// Regression test for <https://github.com/paradigmxyz/reth/issues/27809>.
#[tokio::test]
async fn test_simulate_v1_block_override_time_crosses_cancun() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let cancun_time = 1000;
    let chain_spec = Arc::new(
        test_chain_spec_builder()
            .shanghai_activated()
            .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(cancun_time))
            .build(),
    );
    let (node, _) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    let provider = node.rpc_provider();

    let contract = Address::with_last_byte(0x33);
    // PUSH1 0 TLOAD POP PUSH1 0 PUSH1 0 RETURN; TLOAD (EIP-1153) only exists from Cancun.
    let state_overrides = StateOverride::from_iter([(
        contract,
        AccountOverride::default().with_code(bytes!("0x60005c5060006000f3")),
    )]);
    let sim_block = SimBlock::default()
        .with_block_overrides(BlockOverrides { time: Some(1500), ..Default::default() })
        .with_state_overrides(state_overrides)
        .call(TransactionRequest::default().to(contract));
    let payload = SimulatePayload::default().extend(sim_block);

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].calls.len(), 1);
    assert!(result[0].calls[0].status, "call failed: {:?}", result[0].calls[0].error);
    assert_eq!(result[0].inner.header.timestamp, 1500);

    Ok(())
}

/// Multi-block probe of #27809 Part 2: a four-block request whose times cross Cancun (1000) and
/// Prague (2000) must switch opcodes and precompiles at the right blocks.
#[tokio::test]
async fn test_simulate_v1_multi_block_crosses_cancun_and_prague() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        test_chain_spec_builder()
            .shanghai_activated()
            .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(1000))
            .with_fork(EthereumHardfork::Prague, ForkCondition::Timestamp(2000))
            .build(),
    );
    let (node, _) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    let provider = node.rpc_provider();

    // TLOAD (EIP-1153) exists only from Cancun; 0x0a is the Cancun KZG precompile and 0x0b the
    // Prague BLS12-381 precompile, so a call with empty input errors once each is a precompile.
    let tload = Address::with_last_byte(0x33);
    let kzg = Address::with_last_byte(0x0a);
    let bls = Address::with_last_byte(0x0b);
    let state_overrides = StateOverride::from_iter([(
        tload,
        AccountOverride::default().with_code(bytes!("0x60005c5060006000f3")),
    )]);

    let block = |time: u64| {
        SimBlock::default()
            .with_block_overrides(BlockOverrides { time: Some(time), ..Default::default() })
            .with_state_overrides(state_overrides.clone())
            .call(TransactionRequest::default().to(tload).gas_limit(100_000))
            .call(TransactionRequest::default().to(kzg).gas_limit(100_000))
            .call(TransactionRequest::default().to(bls).gas_limit(100_000))
    };
    let payload = SimulatePayload::default()
        .extend(block(500))
        .extend(block(1500))
        .extend(block(1501))
        .extend(block(2500));

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    let timestamps: Vec<u64> = result.iter().map(|b| b.inner.header.timestamp).collect();
    assert_eq!(timestamps, vec![500, 1500, 1501, 2500]);

    let statuses: Vec<Vec<bool>> =
        result.iter().map(|b| b.calls.iter().map(|c| c.status).collect()).collect();
    assert_eq!(statuses[0], vec![false, true, true], "pre-Cancun");
    assert_eq!(statuses[1], vec![true, false, true], "Cancun");
    assert_eq!(statuses[2], vec![true, false, true], "Cancun+1");
    assert_eq!(statuses[3], vec![true, false, false], "Prague");

    Ok(())
}
