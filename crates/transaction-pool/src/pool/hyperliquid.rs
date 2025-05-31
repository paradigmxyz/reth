use crate::pool::{PoolTransaction, PooledTransaction};
use reth_primitives::transaction::HyperliquidTransaction;
use std::collections::HashMap;

/// Hyperliquid-specific transaction pool that handles both standard transactions
/// and deposit pseudo-transactions
pub struct HyperliquidTransactionPool {
    /// Standard transaction pool
    standard_pool: HashMap<TxHash, PooledTransaction>,
    /// Deposit pseudo-transactions (for indexing purposes)
    deposit_transactions: HashMap<TxHash, DepositTransaction>,
}

impl HyperliquidTransactionPool {
    pub fn new() -> Self {
        Self {
            standard_pool: HashMap::new(),
            deposit_transactions: HashMap::new(),
        }
    }
    
    /// Add transaction to appropriate pool
    pub fn add_transaction(&mut self, tx: HyperliquidTransaction) -> Result<(), PoolError> {
        match tx {
            HyperliquidTransaction::Standard(std_tx) => {
                // Process as normal transaction
                self.add_standard_transaction(std_tx)
            }
            HyperliquidTransaction::Deposit(deposit_tx) => {
                // Store as pseudo-transaction for block explorer indexing
                let hash = deposit_tx.pseudo_hash();
                self.deposit_transactions.insert(hash, deposit_tx);
                Ok(())
            }
        }
    }
    
    /// Get all transactions for block building
    pub fn get_transactions_for_block(&self) -> Vec<HyperliquidTransaction> {
        let mut transactions = Vec::new();
        
        // Add standard transactions
        for tx in self.standard_pool.values() {
            transactions.push(HyperliquidTransaction::Standard(tx.transaction().clone()));
        }
        
        // Add deposit pseudo-transactions
        for deposit in self.deposit_transactions.values() {
            transactions.push(HyperliquidTransaction::Deposit(deposit.clone()));
        }
        
        transactions
    }
}