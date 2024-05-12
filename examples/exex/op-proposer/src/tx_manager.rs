use crate::{L2Output, L2OutputOracle, L2OutputOracle::L2OutputOracleInstance};
use alloy_network::Network;
use alloy_provider::{PendingTransaction, Provider};
use alloy_transport::{Transport, TransportResult};
use futures::{channel::mpsc::Receiver, stream::FuturesUnordered};
use reth_primitives::U256;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::{
    sync::{mpsc::Sender, Mutex},
    task::JoinHandle,
};
pub struct TxManager<'a, T, N, P>
where
    T: Transport + Clone,
    N: Network,
    P: Provider<T, N>,
{
    // NOTE: add a comment what the u64 is
    pub pending_transactions: Arc<Mutex<HashSet<u64>>>,
    pub pending_transaction_tx: Sender<(u64, PendingTransaction)>,
    pub l2_output_oracle: &'a L2OutputOracleInstance<T, Arc<P>, N>,
}

impl<'a, T, N, P> TxManager<'a, T, N, P>
where
    T: Transport + Clone,
    N: Network,
    P: Provider<T, N>,
{
    pub fn new(l2_output: &'a L2OutputOracleInstance<T, Arc<P>, N>) -> Self {
        let (pending_transaction_tx, _) =
            tokio::sync::mpsc::channel::<(u64, PendingTransaction)>(1);

        Self {
            pending_transactions: Arc::new(Mutex::new(HashSet::new())),
            l2_output_oracle: l2_output,
            pending_transaction_tx,
        }
    }

    pub async fn propose_l2_output(
        &mut self,
        l2_output_oracle: &L2OutputOracleInstance<T, Arc<P>, N>,
        l2_output: L2Output,
    ) -> eyre::Result<()> {
        self.pending_transactions.lock().await.insert(l2_output.l2_block_number);

        // Submit a transaction to propose the L2Output to the L2OutputOracle contract
        let transport_result = l2_output_oracle
            .proposeL2Output(
                l2_output.output_root,
                U256::from(l2_output.l2_block_number),
                l2_output.l1_block_hash,
                U256::from(l2_output.l1_block_number),
            )
            .send()
            .await?
            .register()
            .await?;

        self.pending_transaction_tx.send((l2_output.l2_block_number, transport_result)).await?;

        //send through channel

        // info!(
        //     output_root = ?l2_output.output_root,
        //     l2_block_number = ?current_l2_block,
        //     l1_block_hash = ?l2_output.l1_block_hash,
        //     l1_block_number = ?l2_output.l1_block_number,
        //     "Successfully Proposed L2Output"
        // );

        Ok(())
    }

    fn spawn(&mut self) -> JoinHandle<eyre::Result<()>> {
        let (pending_tx, mut pending_rx) =
            tokio::sync::mpsc::channel::<(u64, PendingTransaction)>(100);

        self.pending_transaction_tx = pending_tx;

        let pending_transactions = self.pending_transactions.clone();

        tokio::spawn(async move {
            loop {
                if let Some((l2_block_number, pending_transaction)) = pending_rx.recv().await {
                    match pending_transaction.await {
                        Ok(_) => {
                            // TODO: logging
                            // remove from pending transactions
                            pending_transactions.lock().await.remove(&l2_block_number);
                        }
                        Err(e) => {
                            // TODO: logging
                        }
                    };
                }
            }
        })
    }
}
