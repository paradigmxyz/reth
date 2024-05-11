use crate::{L2Output, L2OutputOracle, L2OutputOracle::L2OutputOracleInstance};
use alloy_network::Network;
use alloy_provider::Provider;
use alloy_transport::Transport;
use reth_primitives::U256;
use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
    sync::Arc,
};
pub struct TxManager {
    // NOTE: add a comment what the u64 is
    pub pending_transactions: HashSet<u64>,
}

impl TxManager {
    pub fn new() -> Self {
        Self { pending_transactions: HashSet::new() }
    }

    pub async fn propose_l2_output<T, P, N>(
        &mut self,
        l2_output_oracle: &L2OutputOracleInstance<T, P, N>,
        l2_output: L2Output,
    ) -> eyre::Result<()>
    where
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    {
        self.pending_transactions.insert(l2_output.l2_block_number);

        // Submit a transaction to propose the L2Output to the L2OutputOracle contract
        //TODO: transaction management
        let pending_transaction = l2_output_oracle
            .proposeL2Output(
                l2_output.output_root,
                U256::from(l2_output.l2_block_number),
                l2_output.l1_block_hash,
                U256::from(l2_output.l1_block_number),
            )
            .send()
            .await?;

        // info!(
        //     output_root = ?l2_output.output_root,
        //     l2_block_number = ?current_l2_block,
        //     l1_block_hash = ?l2_output.l1_block_hash,
        //     l1_block_number = ?l2_output.l1_block_number,
        //     "Successfully Proposed L2Output"
        // );

        Ok(())
    }
}
