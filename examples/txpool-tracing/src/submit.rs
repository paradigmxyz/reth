//! Transaction submission functionality for the txpool tracing example
#![allow(unused)]
#![allow(clippy::too_many_arguments)]

use alloy_network::{Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder};
use alloy_primitives::{Address, TxHash, U256};
use futures_util::StreamExt;
use reth_ethereum::{
    node::api::{FullNodeComponents, NodeTypes},
    pool::{PoolTransaction, TransactionEvent, TransactionOrigin, TransactionPool},
    primitives::SignerRecoverable,
    rpc::eth::primitives::TransactionRequest,
    EthPrimitives, TransactionSigned,
};

/// Submit a transaction to the transaction pool
///
/// This function demonstrates how to create, sign, and submit a transaction
/// to the reth transaction pool.
pub async fn submit_transaction<FC>(
    node: &FC,
    wallet: &EthereumWallet,
    to: Address,
    data: Vec<u8>,
    nonce: u64,
    chain_id: u64,
    gas_limit: u64,
    max_priority_fee_per_gas: u128,
    max_fee_per_gas: u128,
) -> eyre::Result<TxHash>
where
    // This enforces `EthPrimitives` types for this node, this unlocks the proper conversions when
    FC: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    // Create the transaction request
    let request = TransactionRequest::default()
        .with_to(to)
        .with_input(data)
        .with_nonce(nonce)
        .with_chain_id(chain_id)
        .with_gas_limit(gas_limit)
        .with_max_priority_fee_per_gas(max_priority_fee_per_gas)
        .with_max_fee_per_gas(max_fee_per_gas);

    // Sign the transaction
    let transaction: TransactionSigned =
        NetworkWallet::<Ethereum>::sign_request(wallet, request).await?.into();

    // Recover the transaction and prepare the pool
    let (tx_hash, pool_transaction) = prepare_pool_transaction::<FC>(transaction)?;

    // Submit the transaction to the pool and get event stream
    let mut tx_events = node
        .pool()
        .add_transaction_and_subscribe(TransactionOrigin::Local, pool_transaction)
        .await?;

    // Wait for the transaction to be added to the pool
    while let Some(event) = tx_events.next().await {
        match event {
            TransactionEvent::Mined(_) => {
                println!("Transaction was mined: {:?}", tx_events.hash());
                break;
            }
            TransactionEvent::Pending => {
                println!("Transaction added to pending pool: {:?}", tx_events.hash());
                break;
            }
            TransactionEvent::Discarded => {
                return Err(eyre::eyre!("Transaction discarded: {:?}", tx_events.hash(),));
            }
            _ => {
                // Continue waiting for added or rejected event
            }
        }
    }

    Ok(tx_hash)
}

/// Helper function to submit a simple ETH transfer transaction
///
/// This will first populate a tx request, sign it then submit to the pool in the required format.
pub async fn submit_eth_transfer<FC>(
    node: &FC,
    wallet: &EthereumWallet,
    to: Address,
    value: U256,
    nonce: u64,
    chain_id: u64,
    gas_limit: u64,
    max_priority_fee_per_gas: u128,
    max_fee_per_gas: u128,
) -> eyre::Result<TxHash>
where
    FC: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    // Create the transaction request for ETH transfer
    let request = TransactionRequest::default()
        .with_to(to)
        .with_value(value)
        .with_nonce(nonce)
        .with_chain_id(chain_id)
        .with_gas_limit(gas_limit)
        .with_max_priority_fee_per_gas(max_priority_fee_per_gas)
        .with_max_fee_per_gas(max_fee_per_gas);

    // Sign the transaction
    let transaction: TransactionSigned =
        NetworkWallet::<Ethereum>::sign_request(wallet, request).await?.into();

    // Recover the transaction and prepare the pool
    let (tx_hash, pool_transaction) = prepare_pool_transaction::<FC>(transaction)?;

    // Submit the transaction to the pool
    node.pool().add_transaction(TransactionOrigin::Local, pool_transaction).await?;

    Ok(tx_hash)
}

/// Helper function to convert a signed transaction to pool transaction
///
/// It is needed before submitting to the pool.
fn prepare_pool_transaction<FC>(
    transaction: TransactionSigned,
) -> eyre::Result<(TxHash, <FC::Pool as TransactionPool>::Transaction)>
where
    FC: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    // Get the transaction hash
    let tx_hash = *transaction.hash();

    // Recover the tx
    let recovered_tx = transaction.try_into_recovered()?;

    // Convert to pool transaction type
    let pool_transaction =
        <FC::Pool as TransactionPool>::Transaction::try_from_consensus(recovered_tx)
            .map_err(|e| eyre::eyre!("Failed to convert to pool transaction: {e}"))?;

    Ok((tx_hash, pool_transaction))
}
