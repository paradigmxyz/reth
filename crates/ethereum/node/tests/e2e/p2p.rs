use crate::utils::eth_payload_attributes;
use alloy_consensus::TxType;
use alloy_primitives::bytes;
use alloy_provider::{
    network::{
        Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder, TransactionBuilder7702,
    },
    Provider, ProviderBuilder, SendableTx,
};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer::SignerSync;
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{setup, setup_engine, transaction::TransactionTestContext};
use reth_node_ethereum::EthereumNode;
use revm::primitives::{AccessListItem, Authorization};
use std::sync::Arc;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _tasks, wallet) = setup::<EthereumNode>(
        2,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .cancun_activated()
                .build(),
        ),
        false,
        eth_payload_attributes,
    )
    .await?;

    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let mut second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

    // Make the first node advance
    let tx_hash = first_node.rpc.inject_tx(raw_tx).await?;

    // make the node advance
    let (payload, _) = first_node.advance_block().await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // assert the block has been committed to the blockchain
    first_node.assert_new_block(tx_hash, block_hash, block_number).await?;

    // only send forkchoice update to second node
    second_node.engine_api.update_forkchoice(block_hash, block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash, 1).await?;

    Ok(())
}

#[tokio::test]
async fn e2e_test_send_transactions() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::thread_rng().gen();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {:?}", seed);

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _tasks, wallet) =
        setup_engine::<EthereumNode>(2, chain_spec.clone(), false, eth_payload_attributes).await?;
    let mut node = nodes.pop().unwrap();
    let signers = wallet.gen();
    let provider = ProviderBuilder::new().with_recommended_fillers().on_http(node.rpc_url());

    // simple contract which writes to storage on any call
    let dummy_bytecode = bytes!("6080604052348015600f57600080fd5b50602880601d6000396000f3fe4360a09081523360c0526040608081905260e08152902080805500fea164736f6c6343000810000a");
    let mut call_destinations = signers.iter().map(|s| s.address()).collect::<Vec<_>>();

    // Produce 100 random blocks with random transactions
    for _ in 0..100 {
        let tx_count = rng.gen_range(1..20);

        let mut pending = vec![];
        for _ in 0..tx_count {
            let signer = signers.choose(&mut rng).unwrap();
            let tx_type = TxType::try_from(rng.gen_range(0..=4)).unwrap();

            let mut tx = TransactionRequest::default().with_from(signer.address());

            let should_create =
                rng.gen::<bool>() && tx_type != TxType::Eip4844 && tx_type != TxType::Eip7702;
            if should_create {
                tx = tx.into_create().with_input(dummy_bytecode.clone());
            } else {
                tx = tx.with_to(*call_destinations.choose(&mut rng).unwrap()).with_input(
                    (0..rng.gen_range(0..10000)).map(|_| rng.gen()).collect::<Vec<u8>>(),
                );
            }

            if matches!(tx_type, TxType::Legacy | TxType::Eip2930) {
                tx = tx.with_gas_price(provider.get_gas_price().await?);
            }

            if rng.gen::<bool>() || tx_type == TxType::Eip2930 {
                tx = tx.with_access_list(
                    vec![AccessListItem {
                        address: *call_destinations.choose(&mut rng).unwrap(),
                        storage_keys: (0..rng.gen_range(0..100)).map(|_| rng.gen()).collect(),
                    }]
                    .into(),
                );
            }

            if tx_type == TxType::Eip7702 {
                let signer = signers.choose(&mut rng).unwrap();
                let auth = Authorization {
                    chain_id: provider.get_chain_id().await?,
                    address: *call_destinations.choose(&mut rng).unwrap(),
                    nonce: provider.get_transaction_count(signer.address()).await?,
                };
                let sig = signer.sign_hash_sync(&auth.signature_hash())?;
                tx = tx.with_authorization_list(vec![auth.into_signed(sig)])
            }

            let SendableTx::Builder(tx) = provider.fill(tx).await? else { unreachable!() };
            let tx =
                NetworkWallet::<Ethereum>::sign_request(&EthereumWallet::new(signer.clone()), tx)
                    .await?;

            pending.push(provider.send_tx_envelope(tx).await?);
        }

        let (payload, _) = node.advance_block().await?;
        assert!(payload.block().raw_transactions().len() == tx_count);

        for pending in pending {
            let receipt = pending.get_receipt().await?;
            if let Some(address) = receipt.contract_address {
                call_destinations.push(address);
            }
        }
    }

    let second_node = nodes.pop().unwrap();
    let second_provider =
        ProviderBuilder::new().with_recommended_fillers().on_http(second_node.rpc_url());

    assert_eq!(second_provider.get_block_number().await?, 0);

    let head =
        provider.get_block_by_number(Default::default(), false.into()).await?.unwrap().header.hash;
    second_node.engine_api.update_forkchoice(head, head).await?;

    let start = std::time::Instant::now();

    while provider.get_block_number().await? != second_provider.get_block_number().await? {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(start.elapsed() <= std::time::Duration::from_secs(10), "timed out");
    }

    Ok(())
}
