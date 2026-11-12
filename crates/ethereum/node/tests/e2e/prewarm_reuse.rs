//! E2E tests for block validation that reuses speculative prewarm results.

use alloy_primitives::{bytes, Address, Bytes, U256};
use alloy_provider::Provider;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_node_ethereum::EthereumNode;

/// Init code of a contract whose runtime code increments storage slot 0 on every call.
///
/// Runtime: `PUSH0 SLOAD PUSH1 1 ADD PUSH0 SSTORE STOP`.
const COUNTER_INIT_CODE: Bytes = bytes!("675f546001015f55005f5260086018f3");

/// A follower that reuses speculative results validates blocks whose transactions depend on each
/// other through nonces, a shared recipient's balance and a shared storage slot, and ends with the
/// producer's state.
#[tokio::test]
async fn validates_dependent_transactions_with_reuse() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, wallet) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_num_nodes(2)
        .with_tree_config_modifier(|config| config.with_prewarm_reuse(true))
        .build()
        .await?;
    let follower = nodes.pop().unwrap();
    let mut producer = nodes.pop().unwrap();

    let mut deployer = wallet.account(0);
    let deploy = deployer.deploy(COUNTER_INIT_CODE).await;
    let mined = producer.mine([deploy]).await?.ensure_success()?;
    let counter = mined.receipts[0].contract_address.expect("deployment receipt has an address");
    follower.import_payload(mined.payload).await?;

    let recipient = Address::with_last_byte(0x77);
    let mut accounts = (1..20).map(|i| wallet.account(i)).collect::<Vec<_>>();
    for round in 0..3u64 {
        let mut txs = Vec::new();
        for account in &mut accounts {
            txs.push(account.transfer(recipient, U256::from(round + 1)).await);
            txs.push(account.call(counter, Bytes::new()).await);
            txs.push(account.transfer(recipient, U256::from(round + 2)).await);
            txs.push(account.call(counter, Bytes::new()).await);
        }
        let mined = producer.mine(txs).await?.ensure_success()?;
        let hash = mined.block().hash();
        follower.import_payload(mined.payload).await?;
        assert_eq!(follower.current_forkchoice_state()?.head_block_hash, hash);
    }

    let calls = accounts.len() as u64 * 2 * 3;
    let follower_provider = follower.rpc_provider();
    assert_eq!(follower_provider.get_storage_at(counter, U256::ZERO).await?, U256::from(calls));
    let transferred = accounts.len() as u64 * (1 + 2 + 2 + 3 + 3 + 4);
    assert_eq!(follower_provider.get_balance(recipient).await?, U256::from(transferred));
    Ok(())
}
