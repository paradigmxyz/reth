use alloy_consensus::{transaction::Recovered, SignableTransaction, TxEip1559};
use alloy_network::TxSignerSync;
use alloy_primitives::{Address, ChainId, TxKind};
use reth_e2e_test_utils::wallet::Wallet;
use reth_optimism_node::txpool::OpPooledTransaction;
use reth_optimism_payload_builder::builder::OpPayloadTransactions;
use reth_payload_util::{
    BestPayloadTransactions, PayloadTransactions, PayloadTransactionsChain,
    PayloadTransactionsFixed,
};
use reth_transaction_pool::PoolTransaction;

#[derive(Clone, Debug)]
pub(crate) struct CustomTxPriority {
    chain_id: ChainId,
}

impl CustomTxPriority {
    pub(crate) fn new(chain_id: ChainId) -> Self {
        Self { chain_id }
    }
}

impl OpPayloadTransactions<OpPooledTransaction> for CustomTxPriority {
    fn best_transactions<Pool>(
        &self,
        pool: Pool,
        attr: reth_transaction_pool::BestTransactionsAttributes,
    ) -> impl PayloadTransactions<Transaction = OpPooledTransaction>
    where
        Pool: reth_transaction_pool::TransactionPool<Transaction = OpPooledTransaction>,
    {
        // Block composition:
        // 1. Best transactions from the pool (up to 250k gas)
        // 2. End-of-block transaction created by the node (up to 100k gas)

        // End of block transaction should send a 0-value transfer to a random address.
        let sender = Wallet::default().inner;
        let mut end_of_block_tx = TxEip1559 {
            chain_id: self.chain_id,
            nonce: 1, // it will be 2nd tx after L1 block info tx that uses the same sender
            gas_limit: 21000,
            max_fee_per_gas: 20e9 as u128,
            to: TxKind::Call(Address::random()),
            value: 0.try_into().unwrap(),
            ..Default::default()
        };
        let signature = sender.sign_transaction_sync(&mut end_of_block_tx).unwrap();
        let end_of_block_tx = OpPooledTransaction::from_pooled(Recovered::new_unchecked(
            op_alloy_consensus::OpPooledTransaction::Eip1559(
                end_of_block_tx.into_signed(signature),
            ),
            sender.address(),
        ));

        PayloadTransactionsChain::new(
            BestPayloadTransactions::new(pool.best_transactions_with_attributes(attr)),
            // Allow 250k gas for the transactions from the pool
            Some(250_000),
            PayloadTransactionsFixed::single(end_of_block_tx),
            // Allow 100k gas for the end-of-block transaction
            Some(100_000),
        )
    }
}
