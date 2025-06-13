use crate::types::{RollkitPayloadAttributes, PayloadAttributesError};
use reth_primitives::{SealedBlock, Header, Block, BlockBody};
use alloy_consensus::Transaction as ConsensusTx;
use reth_provider::StateProviderFactory;
use std::sync::Arc;

/// Payload builder for Rollkit Reth node
pub struct RollkitPayloadBuilder<Client> {
    /// The client for state access
    pub client: Arc<Client>,
}

impl<Client> RollkitPayloadBuilder<Client>
where
    Client: StateProviderFactory + Send + Sync + 'static,
{
    /// Creates a new instance of RollkitPayloadBuilder
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }

    /// Builds a payload using the provided attributes
    pub async fn build_payload(
        &self,
        attributes: RollkitPayloadAttributes,
    ) -> Result<SealedBlock, PayloadAttributesError> {
        // Validate attributes
        attributes.validate()?;

        // Execute transactions
        let mut cumulative_gas_used = 0;
        let mut executed_transactions = Vec::new();

        for tx in attributes.transactions {
            let tx_gas_limit = tx.gas_limit();
            
            // Check gas limit if specified
            if let Some(gas_limit) = attributes.gas_limit {
                if cumulative_gas_used + tx_gas_limit > gas_limit {
                    break;
                }
            }

            // Execute transaction
            // Note: In a real implementation, you would need to properly execute the transaction
            // using the client's state provider and handle the execution result
            executed_transactions.push(tx);
            cumulative_gas_used += tx_gas_limit;
        }

        // Transactions are already signed and in the correct envelope format
        let envelope_transactions = executed_transactions;

        // Create a basic block using the transactions
        let block = Block::new(
            Header {
                timestamp: attributes.timestamp,
                mix_hash: attributes.prev_randao,
                beneficiary: attributes.suggested_fee_recipient,
                ..Default::default()
            },
            BlockBody {
                transactions: envelope_transactions,
                ommers: vec![],
                withdrawals: None,
            },
        );

        // Seal the block using the seal_slow method
        Ok(SealedBlock::seal_slow(block))
    }
}

/// Creates a new payload builder service
pub fn create_payload_builder_service<Client>(
    client: Arc<Client>,
) -> Option<RollkitPayloadBuilder<Client>>
where
    Client: StateProviderFactory + Send + Sync + 'static,
{
    Some(RollkitPayloadBuilder::new(client))
} 