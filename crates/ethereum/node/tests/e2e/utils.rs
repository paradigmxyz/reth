use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{bytes, Address, B256};
use alloy_provider::{
    network::{
        Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder, TransactionBuilder7702,
    },
    Provider, ProviderBuilder, SendableTx,
};
use alloy_rpc_types_engine::PayloadAttributes;
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer::SignerSync;
use rand::{seq::SliceRandom, Rng};
use reth_e2e_test_utils::{wallet::Wallet, NodeHelperType, TmpDB};
use reth_node_api::NodeTypesWithDBAdapter;
use reth_node_ethereum::EthereumNode;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::TxType;
use reth_provider::FullProvider;
use revm::primitives::{AccessListItem, Authorization};

/// Helper function to create a new eth payload attributes
pub(crate) fn eth_payload_attributes(timestamp: u64) -> EthPayloadBuilderAttributes {
    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
        target_blobs_per_block: None,
        max_blobs_per_block: None,
    };
    EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
}

/// Advances node by producing blocks with random transactions.
pub(crate) async fn advance_with_random_transactions<Provider>(
    node: &mut NodeHelperType<EthereumNode, Provider>,
    num_blocks: usize,
    rng: &mut impl Rng,
    finalize: bool,
) -> eyre::Result<()>
where
    Provider: FullProvider<NodeTypesWithDBAdapter<EthereumNode, TmpDB>>,
{
    let provider = ProviderBuilder::new().with_recommended_fillers().on_http(node.rpc_url());
    let signers = Wallet::new(1).with_chain_id(provider.get_chain_id().await?).gen();

    // simple contract which writes to storage on any call
    let dummy_bytecode = bytes!("6080604052348015600f57600080fd5b50602880601d6000396000f3fe4360a09081523360c0526040608081905260e08152902080805500fea164736f6c6343000810000a");
    let mut call_destinations = signers.iter().map(|s| s.address()).collect::<Vec<_>>();

    for _ in 0..num_blocks {
        let tx_count = rng.gen_range(1..20);

        let mut pending = vec![];
        for _ in 0..tx_count {
            let signer = signers.choose(rng).unwrap();
            let tx_type = TxType::try_from(rng.gen_range(0..=4) as u64).unwrap();

            let nonce = provider
                .get_transaction_count(signer.address())
                .block_id(BlockId::Number(BlockNumberOrTag::Pending))
                .await?;

            let mut tx =
                TransactionRequest::default().with_from(signer.address()).with_nonce(nonce);

            let should_create =
                rng.gen::<bool>() && tx_type != TxType::Eip4844 && tx_type != TxType::Eip7702;
            if should_create {
                tx = tx.into_create().with_input(dummy_bytecode.clone());
            } else {
                tx = tx.with_to(*call_destinations.choose(rng).unwrap()).with_input(
                    (0..rng.gen_range(0..10000)).map(|_| rng.gen()).collect::<Vec<u8>>(),
                );
            }

            if matches!(tx_type, TxType::Legacy | TxType::Eip2930) {
                tx = tx.with_gas_price(provider.get_gas_price().await?);
            }

            if rng.gen::<bool>() || tx_type == TxType::Eip2930 {
                tx = tx.with_access_list(
                    vec![AccessListItem {
                        address: *call_destinations.choose(rng).unwrap(),
                        storage_keys: (0..rng.gen_range(0..100)).map(|_| rng.gen()).collect(),
                    }]
                    .into(),
                );
            }

            if tx_type == TxType::Eip7702 {
                let signer = signers.choose(rng).unwrap();
                let auth = Authorization {
                    chain_id: provider.get_chain_id().await?,
                    address: *call_destinations.choose(rng).unwrap(),
                    nonce: provider
                        .get_transaction_count(signer.address())
                        .block_id(BlockId::Number(BlockNumberOrTag::Pending))
                        .await?,
                };
                let sig = signer.sign_hash_sync(&auth.signature_hash())?;
                tx = tx.with_authorization_list(vec![auth.into_signed(sig)])
            }

            let gas = provider
                .estimate_gas(&tx)
                .block(BlockId::Number(BlockNumberOrTag::Pending))
                .await
                .unwrap_or(1_000_000);

            tx.set_gas_limit(gas);

            let SendableTx::Builder(tx) = provider.fill(tx).await? else { unreachable!() };
            let tx =
                NetworkWallet::<Ethereum>::sign_request(&EthereumWallet::new(signer.clone()), tx)
                    .await?;

            pending.push(provider.send_tx_envelope(tx).await?);
        }

        let (payload, _) = node.build_and_submit_payload().await?;
        if finalize {
            node.engine_api
                .update_forkchoice(payload.block().hash(), payload.block().hash())
                .await?;
        } else {
            let last_safe = provider
                .get_block_by_number(BlockNumberOrTag::Safe, false.into())
                .await?
                .unwrap()
                .header
                .hash;
            node.engine_api.update_forkchoice(last_safe, payload.block().hash()).await?;
        }

        for pending in pending {
            let receipt = pending.get_receipt().await?;
            if let Some(address) = receipt.contract_address {
                call_destinations.push(address);
            }
        }
    }

    Ok(())
}
